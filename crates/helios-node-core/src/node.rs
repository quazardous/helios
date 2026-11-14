//! The local node: the vault's owner. The desktop embeds it in its
//! process (docs/design/node-client-split.md, choice A); `helios-node
//! --vault` runs it headless.
//!
//! The node opens the vault ([`LocalNode::open`]), saves it on its own
//! schedule (a flush every [`FLUSH_EVERY`] on a thread of its own, with
//! or without a window), and closes it when it stops. The windows are its
//! clients: the desktop connects one ([`LocalNode::connect`]) as it
//! starts, and that client disconnects when the GUI ends (eframe's
//! `on_exit`). Quitting is asked of the node ([`Link::quit`], by the
//! shell's lifetime policy); the node stops once it is asked to and its
//! last client is gone: a last flush, then the vault is shut down (marked
//! closed cleanly, later writes refused).
//!
//! The node runs the work that outlives a window ([`NodeJobs`], adopted
//! with [`Link::adopt`]): the exports, the backups and the health check
//! ([`crate::jobs`]). A window starts such work and
//! watches it through the jobs API (the Queue panel, the jobs centre);
//! closing it leaves the work running. The node's stop waits for that
//! work, or cancels it first when the quit was confirmed as cancelling
//! the running jobs ([`Link::quit_cancelling_jobs`]), before its last
//! flush.
//!
//! The node watches the machine's volumes ([`LocalNode::watch_volumes`],
//! [`crate::volumes`]): cards and disks coming and going are
//! published on its bus, with or without a window, for its clients and
//! its jobs (a backup disk plugged in) to hear.
//!
//! The node keeps the person's and the libraries' settings layers
//! ([`LocalNode::settings`], [`crate::settings`]): its clients read and
//! write them through it, and hear their changes on its bus.
//!
//! A remote node's life is its own: its clients quitting does not stop
//! it. Only the embedded node follows the desktop's quit.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use helios_core::{Bus, Message};
use helios_storage::Volumes;
use helios_vault::{Store, Vault};

use crate::settings::NodeSettings;
use crate::volumes::VolumeWatch;

/// How often the node flushes the vault, whatever the windows do.
pub const FLUSH_EVERY: Duration = Duration::from_secs(5);

/// Opens the library (vault file) at `path` for a node to own, its
/// folder made first (the default library's, on the first launch). A
/// catalogue snapshot whose restore was asked for
/// (`library.catalogue.restore`) is put in place first, the catalogue it
/// replaces kept aside; a restore that fails leaves the catalogue as it
/// was, and says why in the log.
pub fn open_library(path: &std::path::Path) -> Result<helios_vault_sqlite::SqliteStore, String> {
    let _step = crate::phases::boot().begin(Message::new("node-boot-vault", "Opening the library"));
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    match helios_vault_sqlite::snapshot::apply_pending(path, now) {
        Ok(Some(r)) => log::info!(
            "catalogue restored from {}; the one it replaced is kept as {}",
            r.from.display(),
            r.aside.display()
        ),
        Ok(None) => {}
        Err(e) => log::error!("catalogue restore refused: {e}"),
    }
    helios_vault_sqlite::SqliteStore::open(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Work the node runs for its clients, which outlives their windows
/// (exports, backups, the health check): what a quit asks
/// about, and what the node's stop cancels or waits for.
pub trait NodeJobs: Send + Sync {
    /// The jobs running that a quit would cancel, by id.
    fn running(&self) -> Vec<String>;
    /// Cancels every job running.
    fn cancel_all(&self);
    /// Waits until every job ended, at most `deadline`; whether they did.
    fn wait(&self, deadline: Duration) -> bool;
}

/// What the node's stop does with its running jobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AtQuit {
    /// Waits for them to end.
    Wait,
    /// Cancels them (the quit was confirmed as cancelling them).
    Cancel,
}

/// Where the node's life stands, shared with its clients' links.
#[derive(Debug, Default)]
struct State {
    /// Clients connected (the desktop's GUI is one).
    clients: usize,
    /// Quit was asked: the node stops once no client is left, doing
    /// this with its running jobs.
    quit: Option<AtQuit>,
    /// Stop now, clients or not (the host ending the process).
    force: bool,
    /// The vault is shut down.
    stopped: bool,
    /// Flushes done, the last one at the stop included.
    flushes: u64,
}

impl State {
    fn stopping(&self) -> bool {
        self.force || (self.quit.is_some() && self.clients == 0)
    }
}

#[derive(Default)]
struct Life {
    state: Mutex<State>,
    changed: Condvar,
    /// The work the node runs for its clients.
    jobs: Mutex<Vec<Arc<dyn NodeJobs>>>,
    /// The volume watches the node runs, stopped as it stops.
    watches: Mutex<Vec<VolumeWatch>>,
}

impl std::fmt::Debug for Life {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Life")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Life {
    fn update<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let out = f(&mut state);
        self.changed.notify_all();
        out
    }

    fn read<T>(&self, f: impl FnOnce(&State) -> T) -> T {
        f(&self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn jobs(&self) -> Vec<Arc<dyn NodeJobs>> {
        self.jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Before the last flush: the jobs cancelled when the quit said so,
    /// waited for otherwise. Cancelled exports are not waited for: they
    /// write nothing to the vault, and their renders under way end on
    /// their own. A job whose last step writes to the vault (a backup
    /// recording its copy) waits in its own `cancel_all`.
    fn settle_jobs(&self) {
        let jobs = self.jobs();
        // Leaving them is safe: what they wrote is in the vault, the rest
        // is done again (an export asked again, a backup run again).
        let step = crate::phases::end()
            .begin(Message::new("node-end-jobs", "Stopping background jobs"))
            .can_skip();
        if self.read(|s| s.quit == Some(AtQuit::Cancel)) {
            for j in &jobs {
                j.cancel_all();
            }
            return;
        }
        for j in &jobs {
            let running = j.running().len();
            if running > 0 {
                crate::quit::mark("the node waits for its running jobs");
                step.why(
                    Message::new(
                        "node-end-jobs-waiting",
                        "Waiting for { $count } running jobs to finish",
                    )
                    .with("count", running as f64),
                );
            }
            while !j.wait(Duration::from_mins(1)) {
                log::info!(target: "helios::node", "still waiting for running jobs");
            }
        }
    }
}

/// A client's way to the node's life, without its vault's type: what the
/// shell holds to ask the node to quit.
#[derive(Clone, Debug)]
pub struct Link(Arc<Life>);

impl Link {
    /// Asks the node to quit: it stops once its last client is gone.
    pub fn quit(&self) {
        self.0
            .update(|s| s.quit = Some(s.quit.unwrap_or(AtQuit::Wait)));
    }

    /// Asks the node to quit, cancelling its running jobs (the quit
    /// question was answered): it stops once its last client is gone.
    pub fn quit_cancelling_jobs(&self) {
        self.0.update(|s| s.quit = Some(AtQuit::Cancel));
    }

    /// The node runs `jobs` from now on: they outlive the window that
    /// started them, and the node's stop settles them.
    pub fn adopt(&self, jobs: Arc<dyn NodeJobs>) {
        self.0
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(jobs);
    }

    /// The node's jobs running, by id: what a quit would cancel.
    pub fn running_jobs(&self) -> Vec<String> {
        self.0.jobs().iter().flat_map(|j| j.running()).collect()
    }

    /// Whether the node was asked to quit.
    pub fn quitting(&self) -> bool {
        self.0.read(|s| s.quit.is_some())
    }

    /// Whether the node stopped (its vault shut down).
    pub fn stopped(&self) -> bool {
        self.0.read(|s| s.stopped)
    }

    /// The node's flushes so far.
    pub fn flushes(&self) -> u64 {
        self.0.read(|s| s.flushes)
    }

    /// Waits until the node stopped, at most `deadline`; whether it did.
    pub fn wait_stopped(&self, deadline: Duration) -> bool {
        let state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, _) = self
            .0
            .changed
            .wait_timeout_while(state, deadline, |s| !s.stopped)
            .unwrap_or_else(PoisonError::into_inner);
        state.stopped
    }
}

/// A connected client; dropping it disconnects.
#[derive(Debug)]
pub struct Client(Link);

impl Client {
    /// The node this client is connected to.
    pub fn link(&self) -> &Link {
        &self.0
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        (self.0)
            .0
            .update(|s| s.clients = s.clients.saturating_sub(1));
    }
}

/// The local node, the vault's owner: embedded in the desktop's process,
/// or headless in `helios-node`.
pub struct LocalNode<S: Store + 'static> {
    vault: Arc<Vault<S>>,
    bus: Arc<Bus>,
    link: Link,
    runner: Option<JoinHandle<()>>,
    promises: Arc<crate::promises::Promises<S>>,
    import_watch: crate::import_promise::Watch,
}

impl<S: Store + 'static> LocalNode<S> {
    /// Opens the vault on `store`, announcing its changes on `bus`, and
    /// starts the node: it flushes every `every` until it stops.
    pub fn open(store: S, bus: Arc<Bus>, every: Duration) -> Self {
        Self::own(Vault::new(store), bus, every)
    }

    /// Takes `vault`, already open (read before the bus existed: the
    /// settings it keeps size the bus), announcing its changes on `bus`
    /// from now on, and starts the node as [`LocalNode::open`] does.
    pub fn own(vault: Vault<S>, bus: Arc<Bus>, every: Duration) -> Self {
        let vault = Arc::new(vault.with_bus(bus.clone()));
        // The node's promises, with the kinds it runs (imports by rule);
        // those a stop or a crash left unsettled are taken up again when
        // their kind resumes, else failed `interrupted` (crate::promises).
        let import_watch = crate::import_promise::Watch::default();
        let promises = Arc::new(crate::import_promise::register_watched(
            crate::promises::Promises::new(
                vault.clone(),
                helios_core::jobs::Jobs::new(bus.clone()),
            ),
            bus.clone(),
            import_watch.clone(),
        ));
        if let Err(e) = promises.recover() {
            log::warn!(target: "helios::node", "promises not recovered: {e}");
        }
        let link = Link(Arc::default());
        let runner = {
            let (vault, life) = (vault.clone(), link.0.clone());
            std::thread::Builder::new()
                .name("helios-node".into())
                .spawn(move || run(&vault, &life, every))
                .map_err(|e| log::warn!(target: "helios::node", "no thread to flush on: {e}"))
                .ok()
        };
        Self {
            vault,
            bus,
            link,
            runner,
            promises,
            import_watch,
        }
    }

    /// Where the watch of the node's imports by rule is set: what it
    /// holds from then on hears them (the desktop's undo step, its
    /// panels). An import resumed as the node started is not heard.
    pub fn import_watch(&self) -> crate::import_promise::Watch {
        self.import_watch.clone()
    }

    /// The node's promises: what its endpoints answer `promises.*` and
    /// an import by rule with.
    pub fn promises(&self) -> &Arc<crate::promises::Promises<S>> {
        &self.promises
    }

    /// Watches `volumes` (the machine's: `helios_storage::system()`) on a
    /// thread of the node's, with or without a window: their comings and
    /// goings are published on the node's bus
    /// ([`crate::volumes`]). The watch stops as the node stops.
    pub fn watch_volumes(&self, volumes: Arc<dyn Volumes>) -> VolumeWatch {
        let watch = VolumeWatch::start(volumes, self.bus.clone(), crate::volumes::POLL);
        self.link
            .0
            .watches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(watch.clone());
        watch
    }

    /// The vault the node owns, for the clients in its process.
    pub fn vault(&self) -> &Arc<Vault<S>> {
        &self.vault
    }

    /// The settings layers the node keeps (the person's and the
    /// libraries'), for a client to read and write.
    pub fn settings(&self) -> NodeSettings<S> {
        NodeSettings::new(self.vault.clone(), self.bus.clone())
    }

    /// The node's life, for the shell to ask it to quit.
    pub fn link(&self) -> Link {
        self.link.clone()
    }

    /// Connects a client (a GUI); it disconnects when dropped.
    pub fn connect(&self) -> Client {
        self.link.0.update(|s| s.clients += 1);
        Client(self.link.clone())
    }

    /// Stops the node now, clients or not (the host's last word, as the
    /// process ends): the last flush, the vault shut down. Waits for it.
    pub fn stop(mut self) {
        self.link.0.update(|s| {
            if s.clients > 0 && !s.stopped {
                log::warn!(target: "helios::node", "stopped with {} clients connected", s.clients);
            }
            s.force = true;
        });
        match self.runner.take() {
            Some(runner) => {
                if runner.join().is_err() {
                    log::warn!(target: "helios::node", "its thread stopped (a panic)");
                }
            }
            None => close(&self.vault, &self.link.0),
        }
    }
}

/// The node's thread: a flush every `every`, until it stops; then the
/// last flush and the vault shut down.
fn run<S: Store>(vault: &Vault<S>, life: &Life, every: Duration) {
    loop {
        let state = life.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, _) = life
            .changed
            .wait_timeout_while(state, every, |s| !s.stopping())
            .unwrap_or_else(PoisonError::into_inner);
        if state.stopping() {
            break;
        }
        drop(state);
        match vault.flush() {
            Ok(()) => life.update(|s| s.flushes += 1),
            Err(e) => log::warn!(target: "helios::node", "cannot flush the vault: {e}"),
        }
    }
    close(vault, life);
}

/// The node's jobs settled, then the vault shut down (flushed, marked
/// clean, closed to writes).
fn close<S: Store>(vault: &Vault<S>, life: &Life) {
    for watch in life
        .watches
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain(..)
    {
        watch.stop();
    }
    life.settle_jobs();
    let closing = crate::phases::end().begin(Message::new(
        "node-end-vault",
        "Saving and closing the library",
    ));
    let done = vault.shut_down();
    drop(closing);
    if let Err(e) = &done {
        log::warn!(target: "helios::node", "cannot close the vault cleanly: {e}");
    }
    life.update(|s| {
        if done.is_ok() {
            s.flushes += 1;
        }
        s.stopped = true;
    });
    crate::quit::mark("vault closed by the node");
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_vault_sqlite::SqliteStore;
    use std::time::Instant;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("helios-node-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The database file alone (its log left behind): what a power loss
    /// would keep.
    fn file_alone(dir: &std::path::Path, name: &str) -> SqliteStore {
        let to = dir.join(name);
        std::fs::create_dir_all(&to).unwrap();
        std::fs::copy(dir.join("vault.db"), to.join("vault.db")).unwrap();
        SqliteStore::open(&to.join("vault.db")).unwrap()
    }

    /// With no window at all, the node saves on its own schedule: what a
    /// client wrote reaches the database file at the node's next flush.
    #[test]
    fn the_node_saves_without_any_window() {
        let dir = scratch("schedule");
        let store = SqliteStore::open(&dir.join("vault.db")).unwrap();
        let node = LocalNode::open(store, Arc::new(Bus::new()), Duration::from_millis(20));
        let link = node.link();
        node.vault()
            .set_config_layer("user", Some("a = 1"))
            .unwrap();
        let before = link.flushes();
        let deadline = Instant::now() + Duration::from_secs(10);
        while link.flushes() < before + 2 {
            assert!(Instant::now() < deadline, "no flush in 10 s");
            std::thread::sleep(Duration::from_millis(10));
        }
        let alone = file_alone(&dir, "copy");
        assert_eq!(
            Vault::new(alone).config_layer("user").unwrap().as_deref(),
            Some("a = 1"),
            "in the file, without the log"
        );
        assert!(!link.stopped());
        node.stop();
        assert!(link.stopped());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Quit asked while a client is connected: the vault stays open until
    /// the client disconnects, then the node closes it, cleanly.
    #[test]
    fn the_node_stops_once_quit_and_its_last_client_is_gone() {
        let dir = scratch("quit");
        let store = SqliteStore::open(&dir.join("vault.db")).unwrap();
        // A long schedule: only the stop flushes.
        let node = LocalNode::open(store, Arc::new(Bus::new()), Duration::from_hours(1));
        let link = node.link();
        let client = node.connect();
        link.quit();
        assert!(
            !link.wait_stopped(Duration::from_millis(50)),
            "a client is left"
        );
        node.vault()
            .set_config_layer("user", Some("b = 2"))
            .unwrap();
        drop(client);
        assert!(link.wait_stopped(Duration::from_secs(10)), "stopped");
        assert!(
            node.vault().set_config_layer("user", Some("late")).is_err(),
            "closed to writes"
        );
        node.stop();
        let again = SqliteStore::open(&dir.join("vault.db")).unwrap();
        assert!(!again.report().unclean_shutdown, "closed cleanly");
        assert_eq!(
            Vault::new(again).config_layer("user").unwrap().as_deref(),
            Some("b = 2")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
