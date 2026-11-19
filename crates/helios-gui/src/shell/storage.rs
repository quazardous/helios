//! Storage: what the shell keeps of the volumes the vault knows
//! (`helios_storage::registry`), the library's backup strategy
//! (`helios_storage::strategy`), each photo's backups and the cards
//! waiting for theirs.
//!
//! The shell reaches no catalogue itself: the host gives a
//! [`StorageLink`] ([`NodeStorage`] over the desktop's node and the
//! machine's volumes, read and written off the frame). The same link names the volumes of offline photos in the
//! Library ([`crate::library::Library::set_volumes`]).
//!
//! The Storage page is the Settings app's Storage and Backup scopes
//! (`crate::config::manifest`), read and written through the commands of
//! [`super::storage_admin`]. `library.storage` opens it at Storage ›
//! Volumes; `{"volume": id}` marks that volume and scrolls to it (the
//! health check's free-space fix does).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};

use egui_pop::api::toasts::Toast;
use egui_pop::core::icon::Icon;
use helios_core::Message;
use helios_inspect::client::Client;
use helios_library::import::Card;
use helios_storage::strategy::{self, BackupStatus, Destination, Strategy};
use helios_storage::{Role, Volume, VolumeId, VolumeKind, Volumes, registry};
use serde_json::{Value, json};

use super::Shell;
use crate::backup_end::{self, CardList};
use crate::import_end;
use crate::storage_read;
use egui_pop::api::bus::bus_contract::Bus;

/// Opens the Settings app at Storage › Volumes; `volume` (an id) marks
/// that volume and scrolls to it.
pub const COMMAND: &str = "library.storage";

/// Backs up now what is below the library's strategy.
pub const BACKUP_NOW: &str = "library.backupNow";

/// What the Storage page reads and writes.
pub trait StorageLink: Send + Sync {
    /// Every volume known: those mounted now (with their mount point and
    /// fresh free space), then the others, offline.
    fn volumes(&self) -> Result<Vec<Volume>, String>;
    /// How many copies of photos the library holds on each volume.
    fn photos(&self) -> Result<HashMap<VolumeId, usize>, String>;
    /// Saves what the user set (name, roles, off site, RAID).
    fn save(&self, volume: &Volume) -> Result<Volume, String>;
    /// Writes the identity file on a mounted volume (the user asked).
    fn mark(&self, volume: &Volume) -> Result<Volume, String>;
    /// The library's backup strategy; `None` when there is none.
    fn strategy(&self) -> Result<Option<Strategy>, String> {
        Ok(None)
    }
    /// Keeps (or with `None` removes) the library's backup strategy.
    fn set_strategy(&self, _strategy: Option<&Strategy>) -> Result<(), String> {
        Err("no backup strategy here".into())
    }
    /// Every image's backup status against the library's strategy, by id
    /// (`helios_storage::strategy::statuses`): one read for the whole
    /// Library.
    fn statuses(&self) -> Result<HashMap<String, BackupStatus>, String> {
        Ok(HashMap::new())
    }
    /// Whether a card an import read is safe to format now
    /// (`helios_library::import::card_safety`, from its files and their
    /// images).
    fn card_safety(&self, card: &Card) -> Result<Card, String> {
        Ok(card.clone())
    }
    /// Reads everything again, off the caller's thread when the link
    /// reads through a node; [`StorageLink::seq`] moves once it is read.
    fn refresh(&self) {}
    /// Moves each time what the link answers changed: the shell shows it
    /// again at its next frame.
    fn seq(&self) -> u64 {
        0
    }
}

/// The link over the desktop's node and the volumes mounted
/// (`library.storage.get` and the `storage.*` commands). The volumes
/// mounted are read on the link's thread too, with the node's answers:
/// asking each for its free space may take seconds (a network drive).
///
/// Nothing here waits for the node on the caller's thread: the reads
/// answer what was last read, and a thread of its own runs the writes in
/// the order made, then reads the volumes, the strategy and the backups
/// again ([`StorageLink::refresh`] asks the same). Each read that comes
/// back moves [`StorageLink::seq`], so the shell shows it at a coming
/// frame.
pub struct NodeStorage {
    /// What a marker names as the library that wrote it.
    library: String,
    node: Weak<Client>,
    cached: Arc<Cached>,
    jobs: Sender<Job>,
}

/// What [`NodeStorage`] last read, and the jobs not yet done.
#[derive(Default)]
struct Cached {
    read: Mutex<Read>,
    /// Jobs sent and not yet done, and the condition
    /// [`NodeStorage::settle`] waits on.
    pending: Mutex<usize>,
    done: Condvar,
}

#[derive(Default)]
struct Read {
    known: Vec<Volume>,
    photos: HashMap<VolumeId, usize>,
    strategy: Option<Strategy>,
    statuses: HashMap<String, BackupStatus>,
    /// The volumes mounted on this machine, as last read (their free
    /// space asked of each: slow on a network drive).
    mounted: Vec<Volume>,
    seq: u64,
}

enum Job {
    Read,
    Save(Volume),
    Mark(Volume, String),
    Strategy(Option<Strategy>),
}

impl NodeStorage {
    /// The link over `node`, read once at once (off the caller's thread).
    pub fn new(node: &Arc<Client>, mounted: Arc<dyn Volumes>, library: impl Into<String>) -> Self {
        let (jobs, queue) = std::sync::mpsc::channel();
        let cached = Arc::new(Cached::default());
        let (weak, c, m) = (Arc::downgrade(node), cached.clone(), mounted.clone());
        let spawned = std::thread::Builder::new()
            .name("storage-node".into())
            .spawn(move || work(&weak, &*m, &c, &queue));
        if let Err(e) = spawned {
            log::warn!("storage: no thread to reach the node: {e}");
        }
        let link = Self {
            library: library.into(),
            node: Arc::downgrade(node),
            cached,
            jobs,
        };
        link.send(Job::Read);
        link
    }

    fn send(&self, job: Job) {
        *self
            .cached
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += 1;
        if self.jobs.send(job).is_err() {
            self.cached.finished(1);
        }
    }

    fn read(&self) -> std::sync::MutexGuard<'_, Read> {
        self.cached
            .read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until every job sent is done, `timeout` at most: whether it
    /// was.
    pub fn settle(&self, timeout: std::time::Duration) -> bool {
        let pending = self
            .cached
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (left, _) = self
            .cached
            .done
            .wait_timeout_while(pending, timeout, |n| *n > 0)
            .unwrap_or_else(PoisonError::into_inner);
        *left == 0
    }

    /// Whether a job sent is not done yet.
    pub fn busy(&self) -> bool {
        *self
            .cached
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            > 0
    }
}

impl Cached {
    fn finished(&self, n: usize) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        *pending = pending.saturating_sub(n);
        if *pending == 0 {
            self.done.notify_all();
        }
    }
}

/// The link's thread: each batch of jobs waiting, the writes in order,
/// then one read of all the page shows.
fn work(node: &Weak<Client>, mounted: &dyn Volumes, cached: &Cached, queue: &Receiver<Job>) {
    while let Ok(first) = queue.recv() {
        let batch: Vec<Job> = std::iter::once(first).chain(queue.try_iter()).collect();
        let n = batch.len();
        if let Some(node) = node.upgrade() {
            for job in batch {
                let written = match job {
                    Job::Read => Ok(Value::Null),
                    Job::Save(v) => node.request("storage.volume.save", json!({ "volume": v })),
                    Job::Mark(v, library) => node.request(
                        "storage.volume.mark",
                        json!({ "volume": v, "mount": v.mount, "library": library }),
                    ),
                    Job::Strategy(s) => {
                        node.request("storage.strategy.set", json!({ "strategy": s }))
                    }
                };
                if let Err(e) = written {
                    log::warn!("storage: {}", e.message);
                }
            }
            match read_all(&node) {
                Ok(mut read) => {
                    read.mounted = mounted.volumes();
                    let mut kept = cached.read.lock().unwrap_or_else(PoisonError::into_inner);
                    read.seq = kept.seq + 1;
                    *kept = read;
                }
                Err(e) => log::warn!("storage: {e}"),
            }
        }
        cached.finished(n);
    }
}

fn from<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// The volumes known, the photos on each, the strategy and the backups,
/// read through `node`.
fn read_all(node: &Client) -> Result<Read, String> {
    let ask = |method: &str| node.request(method, json!({})).map_err(|e| e.message);
    let stored = ask("library.storage.get")?;
    let volumes = ask("storage.volumes")?;
    let statuses = ask("storage.statuses")?;
    let field = |v: &Value, what: &str| -> Result<Value, String> {
        v.get(what)
            .cloned()
            .ok_or_else(|| format!("no `{what}` read"))
    };
    let photos: HashMap<String, usize> = from(field(&volumes, "photos")?)?;
    Ok(Read {
        known: from(field(&stored, "volumes")?)?,
        strategy: from(field(&stored, "strategy")?)?,
        photos: photos
            .into_iter()
            .map(|(id, n)| (VolumeId(id), n))
            .collect(),
        statuses: from(field(&statuses, "statuses")?)?,
        mounted: Vec::new(),
        seq: 0,
    })
}

impl StorageLink for NodeStorage {
    fn volumes(&self) -> Result<Vec<Volume>, String> {
        let read = self.read();
        Ok(registry::merge(read.known.clone(), &read.mounted))
    }

    fn photos(&self) -> Result<HashMap<VolumeId, usize>, String> {
        Ok(self.read().photos.clone())
    }

    /// Kept as `registry::save` keeps it and shown at once; written in
    /// turn.
    fn save(&self, volume: &Volume) -> Result<Volume, String> {
        let saved = {
            let mut read = self.read();
            let record = read
                .known
                .iter_mut()
                .find(|v| v.id == volume.id)
                .ok_or_else(|| format!("no volume {}", volume.id))?;
            let name = volume.name.trim();
            record.name = if name.is_empty() {
                record.label.clone().unwrap_or_else(|| record.name.clone())
            } else {
                name.to_string()
            };
            record.roles.clone_from(&volume.roles);
            record.off_site = volume.off_site;
            record.raid = volume.raid;
            let saved = Volume {
                mount: volume.mount.clone(),
                ..record.clone()
            };
            read.seq += 1;
            saved
        };
        self.send(Job::Save(saved.clone()));
        Ok(saved)
    }

    /// Written in turn by the node: the volume answered is the one asked,
    /// its new id read back with the others.
    fn mark(&self, volume: &Volume) -> Result<Volume, String> {
        self.send(Job::Mark(volume.clone(), self.library.clone()));
        Ok(volume.clone())
    }

    fn strategy(&self) -> Result<Option<Strategy>, String> {
        Ok(self.read().strategy.clone())
    }

    fn set_strategy(&self, strategy: Option<&Strategy>) -> Result<(), String> {
        {
            let mut read = self.read();
            read.strategy = strategy.cloned();
            read.seq += 1;
        }
        self.send(Job::Strategy(strategy.cloned()));
        Ok(())
    }

    fn statuses(&self) -> Result<HashMap<String, BackupStatus>, String> {
        Ok(self.read().statuses.clone())
    }

    /// Asks the node and waits for it: the shell calls it off its frame.
    fn card_safety(&self, card: &Card) -> Result<Card, String> {
        let node = self.node.upgrade().ok_or("the node is gone")?;
        let files: Vec<Value> = card
            .files
            .iter()
            .map(|(path, id)| json!([path, id.as_ref().map(helios_vault::ImageId::hex)]))
            .collect();
        let answer = node
            .request(
                "storage.card.safety",
                json!({ "root": card.root, "path": card.path, "label": card.label, "files": files }),
            )
            .map_err(|e| e.message)?;
        let mut now = import_end::card_of(&answer).ok_or("no card answered")?;
        now.files.clone_from(&card.files);
        Ok(now)
    }

    fn refresh(&self) {
        self.send(Job::Read);
    }

    fn seq(&self) -> u64 {
        self.read().seq
    }
}

/// The waiting cards checked again: each one's path and what it is now.
type Checked = Vec<(std::path::PathBuf, Result<Card, String>)>;

/// What names the owners of remote volumes, for the Storage page's rows:
/// the import accounts and the saved sources using them, once the
/// accounts are kept (`Shell::set_import_accounts`).
pub(crate) type Owners = Arc<
    Mutex<
        Option<(
            Arc<dyn helios_connect::account::AccountBook>,
            super::import_sources::SharedSources,
        )>,
    >,
>;

/// The volumes and the strategy as the link last answered them: shared
/// (`Rc`) between the shell and `library.changed`'s local listener
/// `library.volumes`, which reads them again inside the pass
/// ([`refresh_volumes`]). What is read is passed through
/// `storage.read` ([`crate::storage_read`]) to whoever shows it.
#[derive(Default)]
pub(crate) struct StorageRead {
    link: Option<Arc<dyn StorageLink>>,
    list: Vec<Volume>,
    /// The library's backup strategy, as last read.
    strategy: Option<Strategy>,
    error: Option<String>,
}

/// Reads the volumes again: the page's list, and the volumes passed
/// through `storage.read` on `bus` (the Library names them: a scan ran, a
/// disk came or went). What the link reads off the frame shows at a
/// coming one.
pub(crate) fn refresh_volumes(read: &RefCell<StorageRead>, bus: &dyn Bus) {
    let Some(link) = read.borrow().link.clone() else {
        return;
    };
    link.refresh();
    show_volumes(read, bus);
}

/// The volumes and the strategy as the link answers them now; the
/// volumes passed through `storage.read` on `bus`.
fn show_volumes(read: &RefCell<StorageRead>, bus: &dyn Bus) {
    let Some(link) = read.borrow().link.clone() else {
        return;
    };
    let volumes = link.volumes();
    // The import window's strategy line reads it too.
    let strategy = link.strategy();
    let mut read = read.borrow_mut();
    let mut readout = storage_read::Readout::default();
    match volumes {
        Ok(volumes) => {
            readout.volumes = Some(volumes.clone());
            read.list = volumes;
            read.error = None;
        }
        Err(e) => read.error = Some(e),
    }
    match strategy {
        Ok(s) => read.strategy = s,
        Err(e) => read.error = Some(e),
    }
    if let Some(e) = &read.error {
        log::warn!("storage: {e}");
    }
    drop(read);
    if readout.volumes.is_some() {
        storage_read::pass(bus, readout);
    }
}

/// What the shell keeps of storage.
#[derive(Default)]
pub(crate) struct StoragePage {
    /// The volumes and the strategy as last read.
    pub(crate) read: Rc<RefCell<StorageRead>>,
    /// The accounts and sources behind remote volumes, read by
    /// `library.volumes` to show one row per account.
    pub(crate) owners: Owners,
    /// The cards an import left short of their backups, while inserted:
    /// the one list every reader reads ([`CardList`]).
    pub(crate) cards: CardList,
    /// The link's [`StorageLink::seq`] last shown.
    seen: u64,
    /// The node facts the read kernel follows ([`super::reads`]).
    pub(super) reads: Option<crate::node_events::Listener>,
    /// The waiting cards checked again off the frame, once a backup
    /// ended (`cards.safety`): each one's path and what it is now.
    checking: Rc<RefCell<Option<Receiver<Checked>>>>,
}

impl Shell {
    /// Reads and writes volumes through `link`: the Storage page, and the
    /// names of offline photos' volumes in the Library.
    pub fn set_storage(&mut self, link: Arc<dyn StorageLink>) {
        self.register_storage_commands(&link);
        self.core.storage.read.borrow_mut().link = Some(link);
        self.refresh_volumes();
    }

    /// Reads the volumes again: the page's list, and the volumes the
    /// Library names (a scan ran, a disk came or went). What the link
    /// reads off the frame shows at a coming one.
    pub fn refresh_volumes(&mut self) {
        refresh_volumes(&self.core.storage.read, &*self.core.app.handle().bus());
    }

    /// The link the volumes are read through, once set.
    fn storage_link(&self) -> Option<Arc<dyn StorageLink>> {
        self.core.storage.read.borrow().link.clone()
    }

    /// Each photo's backups read again (one read for the Library), for
    /// its badges and its `backup:` filters; what the link reads off the
    /// frame shows at a coming one.
    pub fn refresh_backups(&mut self) {
        refresh_backups(&self.core.storage.read, &*self.core.app.handle().bus());
    }

    /// `storage.cards-waiting`, the shell's listener of `import.ended`:
    /// the cards an import read, from its report (`cards`), those short
    /// of their backups waiting for a backup job to end.
    pub(super) fn listen_cards_waiting(&self) -> Result<(), String> {
        let cards = self.core.storage.cards.clone();
        import_end::listen_local(
            &*self.core.app.handle().bus(),
            import_end::CARDS,
            move |end, _| {
                if let Ok(report) = &end.outcome {
                    cards.wait_for_backups(&report.cards);
                }
            },
        )
    }

    /// The one card list: the cards an import left short of their
    /// backups, while inserted (the health check's thread reads it).
    pub fn waiting_cards(&self) -> CardList {
        self.core.storage.cards.clone()
    }

    /// The cards still waiting for their backups.
    pub fn cards_waiting_for_backups(&self) -> Vec<String> {
        self.core
            .storage
            .cards
            .waiting()
            .into_iter()
            .map(|c| c.label)
            .collect()
    }

    /// Storage's listeners of `backup.ended` and `card.safe`
    /// ([`crate::backup_end`]), each in its declared place: the badges,
    /// the cards checked again, then the toast and the Storage page's
    /// reads once some are safe (the health check listens on its own:
    /// [`Shell::listen_cards_safe`]).
    pub(super) fn listen_backup_points(&mut self) {
        self.import.get_mut().card_list = self.core.storage.cards.clone();
        let bus = self.core.app.handle().bus();
        let (read, on) = (self.core.storage.read.clone(), Arc::downgrade(&bus));
        let mut listened = vec![backup_end::listen_ended(
            &*bus,
            backup_end::BACKUPS,
            move |_, _| {
                if let Some(bus) = on.upgrade() {
                    refresh_backups(&read, &*bus);
                }
            },
        )];
        let (read, cards, checking) = (
            self.core.storage.read.clone(),
            self.core.storage.cards.clone(),
            self.core.storage.checking.clone(),
        );
        let app = self.core.app.handle().clone();
        listened.push(backup_end::listen_ended(
            &*bus,
            backup_end::SAFETY,
            move |_, _| {
                let link = read.borrow().link.clone();
                if let Some(link) = link
                    && let Some(r) = check_cards(link, cards.waiting(), app.egui())
                {
                    *checking.borrow_mut() = Some(r);
                }
            },
        ));
        let (teller, log) = (self.teller(), self.core.log.clone());
        listened.push(backup_end::listen_local(
            &*bus,
            backup_end::TOAST,
            move |safe, _| {
                for card in &safe.cards {
                    let message = Message::new(
                        "gui-backup-card-safe",
                        "{ $card } is safe to format: every photo has its backups",
                    )
                    .with("card", card.label.clone());
                    let text = teller.text(message.clone());
                    teller.toast(Toast::success(&text));
                    log.borrow_mut().push(crate::dialogs::Level::Info, message);
                    teller.note(text);
                }
            },
        ));
        let app = self.core.app.handle().clone();
        listened.push(backup_end::listen_local(
            &*bus,
            backup_end::STORAGE,
            // The Storage page's card rows read again, from the list.
            move |_, _| {
                if let Some(k) = app
                    .service(&egui_pop::api::admin::ADMIN)
                    .and_then(|a| a.reads())
                {
                    k.event(super::reads::VOLUMES_CHANGED);
                }
            },
        ));
        for e in listened.into_iter().filter_map(Result::err) {
            self.notes.push(format!("{}: {e}", backup_end::ENDED));
        }
    }

    /// A backup job ended: passed through `backup.ended` (the badges read
    /// again, each card still inserted checked again off the frame:
    /// [`Shell::storage_frame`] passes those found safe through
    /// `card.safe`), then said on the journal.
    pub(super) fn backup_ended(&mut self) {
        let bus = self.core.app.handle().bus();
        let reactions = backup_end::pass_ended(&*bus, backup_end::BackupEnd::default());
        self.core.app.handle().publish(
            "helios",
            None,
            backup_end::ENDED,
            backup_end::said(&reactions.ran, json!({})),
        );
    }

    /// Each frame: what the link read since shows, and the cards checked
    /// since a backup ended say whether they are safe to format.
    pub(super) fn storage_frame(&mut self) {
        self.reads_frame();
        if let Some(link) = self.storage_link() {
            let seq = link.seq();
            if seq != self.core.storage.seen {
                self.core.storage.seen = seq;
                let bus = self.core.app.handle().bus();
                show_volumes(&self.core.storage.read, &*bus);
                show_backups(&self.core.storage.read, &*bus);
                // The Storage page's reads follow, their values kept
                // while read again.
                self.read_fact(super::reads::VOLUMES_CHANGED);
            }
        }
        let checked = {
            let mut checking = self.core.storage.checking.borrow_mut();
            let checked = match &*checking {
                Some(r) => match r.try_recv() {
                    Ok(checked) => checked,
                    Err(std::sync::mpsc::TryRecvError::Empty) => return,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Vec::new(),
                },
                None => return,
            };
            *checking = None;
            checked
        };
        // The list marked first: whoever reads it from now on (the
        // listeners below, the import window, the Storage page, the
        // health check) says the same.
        let cards = &self.core.storage.cards;
        let mut safe = Vec::new();
        for (path, now) in checked {
            match now {
                Ok(now) => {
                    if cards.checked(&path, now.clone()) {
                        safe.push(now);
                    }
                }
                Err(e) => log::warn!("{}: {e}", path.display()),
            }
        }
        if safe.is_empty() {
            return;
        }
        let labels: Vec<String> = safe.iter().map(|c| c.label.clone()).collect();
        let bus = self.core.app.handle().bus();
        let reactions = backup_end::pass_safe(&*bus, backup_end::SafeCards { cards: safe });
        self.absorb_said();
        self.core.app.handle().publish(
            "helios",
            None,
            backup_end::SAFE,
            backup_end::said(&reactions.ran, json!({ "cards": labels })),
        );
    }

    /// The import window's strategy line, and whether it is the one
    /// without a strategy ("No backup set up:", the Storage page offered):
    /// "2 copies: Backup disk (plugged in), NAS (not seen for 3 days)".
    pub fn import_backup_line(&self) -> (String, bool) {
        let read = self.core.storage.read.borrow();
        backup_line(&|m| self.text(m), read.strategy.as_ref(), &read.list)
    }

    /// Whether the Settings app is open on its Storage or Backup scope.
    pub fn storage_page_open(&self) -> bool {
        let app = self.core.app.handle();
        app.service(&egui_pop::api::admin::ADMIN).is_some_and(|a| {
            a.is_open() && matches!(a.shown().0.as_deref(), Some("storage" | "backup"))
        })
    }

    /// The library's backup strategy, as last read.
    pub fn storage_strategy(&self) -> Option<Strategy> {
        self.core.storage.read.borrow().strategy.clone()
    }

    /// The volumes known, online ones first.
    pub fn storage_volumes(&self) -> Vec<Volume> {
        self.core.storage.read.borrow().list.clone()
    }

    /// `library.storage`: opens the Settings app at Storage › Volumes,
    /// on `volume` when given.
    pub(super) fn open_storage(&mut self, args: &Value) -> Result<Value, String> {
        self.refresh_volumes();
        let path = match args["volume"].as_str() {
            Some(v) => format!("storage/volumes/{v}"),
            None => "storage/volumes".into(),
        };
        self.open_settings_page(&path)
    }
}

/// A destination's words: any disk of a role, or one disk by name.
pub(super) fn destination_text(
    t: &dyn Fn(Message) -> String,
    d: &Destination,
    volumes: &[Volume],
) -> String {
    match d {
        Destination::Role(Role::Archive) => {
            t(Message::new("gui-storage-dest-archive", "Any archive disk"))
        }
        Destination::Role(_) => t(Message::new("gui-storage-dest-backup", "Any backup disk")),
        Destination::Volume(id) => volumes
            .iter()
            .find(|v| &v.id == id)
            .map_or_else(|| id.to_string(), |v| v.name.clone()),
    }
}

/// A kind's icon and words.
pub(super) fn kind_of(kind: VolumeKind) -> (Icon, Message) {
    match kind {
        VolumeKind::Internal => (
            Icon::Home,
            Message::new("gui-storage-kind-internal", "Internal disk"),
        ),
        VolumeKind::External => (
            Icon::Save,
            Message::new("gui-storage-kind-external", "External disk"),
        ),
        VolumeKind::Removable => (
            Icon::File,
            Message::new("gui-storage-kind-removable", "Card or stick"),
        ),
        VolumeKind::Share => (
            Icon::Folder,
            Message::new("gui-storage-kind-share", "Network share"),
        ),
        VolumeKind::Remote => (
            Icon::Up,
            Message::new("gui-storage-kind-remote", "Remote store"),
        ),
    }
}

/// A role's words.
pub fn role_text(role: Role) -> Message {
    match role {
        Role::Library => Message::new("gui-storage-role-library", "Library"),
        Role::Archive => Message::new("gui-storage-role-archive", "Archive"),
        Role::Backup => Message::new("gui-storage-role-backup", "Backup"),
        Role::Ingest => Message::new("gui-storage-role-ingest", "Ingest"),
        Role::Scratch => Message::new("gui-storage-role-scratch", "Scratch"),
    }
}

/// When a volume was last seen, as a day.
pub(super) fn day_of(at: u64) -> String {
    i64::try_from(at)
        .ok()
        .and_then(crate::library::day_text)
        .unwrap_or_else(|| "?".into())
}

/// The strategy's line ([`Shell::import_backup_line`]) from a strategy
/// and the volumes known.
pub fn backup_line(
    t: &dyn Fn(Message) -> String,
    strategy: Option<&Strategy>,
    volumes: &[Volume],
) -> (String, bool) {
    let Some(s) = strategy else {
        return (
            t(Message::new("gui-import-backup-none", "No backup set up:")),
            true,
        );
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let online = strategy::destinations_online(s, volumes);
    let disks: Vec<String> = volumes
        .iter()
        .filter(|v| !v.is_card() && s.destinations.iter().any(|d| d.takes(v)))
        .map(|v| {
            let disk = v.name.clone();
            let m = if online.iter().any(|o| o.id == v.id) {
                Message::new("gui-import-backup-plugged", "{ $disk } (plugged in)")
            } else if v.online() {
                Message::new(
                    "gui-import-backup-unusable",
                    "{ $disk } (plugged in, cannot take copies)",
                )
            } else {
                match v
                    .last_seen
                    .as_ref()
                    .map(|l| now.saturating_sub(l.at) / 86_400)
                {
                    None => Message::new("gui-import-backup-never", "{ $disk } (never seen)"),
                    Some(0) => Message::new("gui-import-backup-today", "{ $disk } (seen today)"),
                    Some(1) => Message::new(
                        "gui-import-backup-one-day",
                        "{ $disk } (not seen for 1 day)",
                    ),
                    Some(days) => Message::new(
                        "gui-import-backup-days",
                        "{ $disk } (not seen for { $days } days)",
                    )
                    .with("days", days),
                }
            };
            t(m.with("disk", disk))
        })
        .collect();
    let disks = if disks.is_empty() {
        t(Message::new(
            "gui-import-backup-no-disk",
            "no disk for them yet",
        ))
    } else {
        disks.join(", ")
    };
    let line = if s.copies == 1 {
        Message::new("gui-import-backup-one", "1 copy: { $disks }")
    } else {
        Message::new("gui-import-backup-copies", "{ $copies } copies: { $disks }")
            .with("copies", u64::from(s.copies))
    };
    (t(line.with("disks", disks)), false)
}

/// Each photo's backups read again (one read for the Library), for its
/// badges and its `backup:` filters; what the link reads off the frame
/// shows at a coming one.
fn refresh_backups(read: &RefCell<StorageRead>, bus: &dyn Bus) {
    let link = read.borrow().link.clone();
    if let Some(link) = link {
        link.refresh();
    }
    show_backups(read, bus);
}

/// Each photo's backups as the link answers them now, passed through
/// `storage.read` on `bus`.
fn show_backups(read: &RefCell<StorageRead>, bus: &dyn Bus) {
    let Some(link) = read.borrow().link.clone() else {
        return;
    };
    match link.statuses() {
        Ok(statuses) => {
            let read = read.borrow();
            let badges = read.strategy.is_some();
            let rule = strategy::rule(&read.list, read.strategy.as_ref());
            drop(read);
            let backups = storage_read::BackupsRead {
                statuses,
                badges,
                rule,
            };
            storage_read::pass(
                bus,
                storage_read::Readout {
                    volumes: None,
                    backups: Some(backups),
                },
            );
        }
        Err(e) => log::warn!("backups: {e}"),
    }
}

/// `cards` checked again through `link` on a thread of their own (the
/// check reads the vault), `wake`'s context asked a pass once it is done; where the
/// result comes, or none when there is nothing to check.
fn check_cards(
    link: Arc<dyn StorageLink>,
    cards: Vec<Card>,
    wake: egui::Context,
) -> Option<Receiver<Checked>> {
    if cards.is_empty() {
        return None;
    }
    let (send, checked) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("card-safety".into())
        .spawn(move || {
            let now = cards
                .iter()
                .map(|c| (c.path.clone(), link.card_safety(c)))
                .collect();
            let _ = send.send(now);
            wake.request_repaint();
        });
    match spawned {
        Ok(_) => Some(checked),
        Err(e) => {
            log::warn!("cards: no thread to check them: {e}");
            None
        }
    }
}
