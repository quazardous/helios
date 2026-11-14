//! How long quitting takes, step by step, and the work left for after
//! the window.
//!
//! The marks live in the node's crate ([`helios_node_core::quit`]), which
//! marks its own steps too. The quit starts when the window first sees its close request
//! ([`requested`]); each step after it is marked ([`mark`]) with the time
//! since that request and since the step before, in the log at debug
//! level (target `helios::quit`). With `HELIOS_QUIT_TRACE` set in the
//! environment the same lines go to standard error, with the wall clock
//! in microseconds, so a script driving the window can line them up with
//! when it asked for the close and when the window went away:
//!
//! ```text
//! helios-quit <epoch µs> <ms since the request> <ms since the last mark> <step>
//! ```
//!
//! Nothing is marked before the request: a mark outside a quit costs a
//! lock and nothing else.
//!
//! The window goes first: the frame that sees the close request draws
//! nothing and only sends what the user changed (the library's last
//! changes, the edit's last step) to the vault, and the window closes
//! after it; the GUI client then disconnects from the local node
//! ([`crate::node`]), which owns the vault: asked to quit, it flushes and
//! closes it once its last client is gone. The host waits for the node's
//! stop on a thread of its own, with a deadline ([`Finisher`]): what is
//! not done by then is logged and left, and the process exits anyway.
//! Nothing the user saved is at stake there: each edit step, rating and
//! label is in the vault before the window closes (see
//! docs/persistence.md).
//!
//! eframe 0.37 tears the application down as the main window's close is
//! seen (`on_exit`, then the application dropped), but leaves its event
//! loop only at the next event of a window: when none comes (nothing
//! moves over the window, the compositor sends nothing), the window stays
//! on screen, no longer drawn, a black one, and the process with it. The
//! quit's work being done by then, the host does not wait for eframe:
//! [`Released`] tells it when eframe let go of the application, and the
//! work left for after the window ([`AfterWindow`]) runs once, from the
//! host's thread when eframe returns, or from a watch past [`GRACE`],
//! which then ends the process.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub use helios_node_core::quit::{MarkOnDrop, TRACE_ENV, mark, requested, started};

/// How long the node's stop (its last flush, the vault's clean-shutdown
/// marker) may take once the window is gone.
pub const VAULT_DEADLINE: Duration = Duration::from_secs(3);

/// Work left for after the window: each piece on a thread of its own,
/// waited for until its deadline.
#[derive(Default)]
pub struct Finisher {
    pending: Vec<Pending>,
}

struct Pending {
    name: String,
    until: Instant,
    done: mpsc::Receiver<()>,
}

impl Finisher {
    /// Starts `work` now, on a thread named after it; [`Finisher::finish`]
    /// waits for it until `deadline` from now.
    pub fn spawn(&mut self, name: &str, deadline: Duration, work: impl FnOnce() + Send + 'static) {
        let (tx, done) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name(format!("helios-quit-{name}"))
            .spawn(move || {
                work();
                let _ = tx.send(());
            });
        if let Err(e) = started {
            log::warn!(target: "helios::quit", "{name}: no thread to run it ({e}), left");
            return;
        }
        self.pending.push(Pending {
            name: name.to_string(),
            until: Instant::now() + deadline,
            done,
        });
    }

    /// Waits for each piece until its deadline; returns the ones left
    /// unfinished (logged; the quit goes on without them).
    pub fn finish(self) -> Vec<String> {
        let mut left = Vec::new();
        for p in self.pending {
            let wait = p.until.saturating_duration_since(Instant::now());
            match p.done.recv_timeout(wait) {
                Ok(()) => mark(&p.name),
                Err(RecvTimeoutError::Timeout) => {
                    log::warn!(target: "helios::quit", "{}: not done by its deadline, left", p.name);
                    mark(&format!("{} (left at its deadline)", p.name));
                    left.push(p.name);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    log::warn!(target: "helios::quit", "{}: its thread stopped (a panic)", p.name);
                    mark(&format!("{} (stopped)", p.name));
                    left.push(p.name);
                }
            }
        }
        left
    }
}

/// How long eframe may take to return once it let go of the
/// application, before the quit goes on without it.
pub const GRACE: Duration = Duration::from_millis(200);

/// The work left for after the window: run once, by whoever comes first
/// (the host as eframe returns, or the watch past [`GRACE`]).
#[derive(Clone)]
pub struct AfterWindow(Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>);

impl AfterWindow {
    pub fn new(work: impl FnOnce() + Send + 'static) -> Self {
        AfterWindow(Arc::new(Mutex::new(Some(Box::new(work)))))
    }

    /// Runs the work unless it ran already: whether it ran now.
    pub fn run(&self) -> bool {
        let work = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        work.map(|w| w()).is_some()
    }

    /// Starts the watch: past `grace`, the work not run yet runs on the
    /// watch's thread, then `end` (the process ends there).
    pub fn watch(&self, grace: Duration, end: impl FnOnce() + Send + 'static) {
        let me = self.clone();
        let started = std::thread::Builder::new()
            .name("helios-quit-watch".into())
            .spawn(move || {
                std::thread::sleep(grace);
                if me.run() {
                    end();
                }
            });
        if let Err(e) = started {
            log::warn!(target: "helios::quit", "no thread to watch eframe's return: {e}");
        }
    }
}

/// The application as eframe holds it: every call goes through to it;
/// dropped (eframe's teardown), it drops the application, then runs its
/// `released` hook.
pub struct Released<A: eframe::App> {
    app: Option<A>,
    released: Option<Box<dyn FnOnce()>>,
}

impl<A: eframe::App> Released<A> {
    /// `app` for eframe; `released` runs once eframe dropped it.
    pub fn new(app: A, released: impl FnOnce() + 'static) -> Self {
        Released {
            app: Some(app),
            released: Some(Box::new(released)),
        }
    }

    fn app(&self) -> &A {
        self.app.as_ref().expect("taken only as it drops")
    }

    fn app_mut(&mut self) -> &mut A {
        self.app.as_mut().expect("taken only as it drops")
    }
}

impl<A: eframe::App> Drop for Released<A> {
    fn drop(&mut self) {
        drop(self.app.take());
        mark("application dropped");
        if let Some(released) = self.released.take() {
            released();
        }
    }
}

impl<A: eframe::App> eframe::App for Released<A> {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app_mut().logic(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app_mut().ui(ui, frame);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.app_mut().save(storage);
    }

    fn on_exit(&mut self) {
        self.app_mut().on_exit();
    }

    fn auto_save_interval(&self) -> Duration {
        self.app().auto_save_interval()
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.app().clear_color(visuals)
    }

    fn persist_egui_memory(&self) -> bool {
        self.app().persist_egui_memory()
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.app_mut().raw_input_hook(ctx, raw_input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Quiet(Arc<std::sync::atomic::AtomicBool>);

    impl eframe::App for Quiet {
        fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
        fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
            [0.5; 4]
        }
    }

    impl Drop for Quiet {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn released_drops_the_application_then_runs_its_hook() {
        use eframe::App as _;
        use std::sync::atomic::{AtomicBool, Ordering};
        let dropped = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(AtomicBool::new(false));
        let (d, s) = (dropped.clone(), seen.clone());
        let app = Released::new(Quiet(dropped.clone()), move || {
            s.store(d.load(Ordering::SeqCst), Ordering::SeqCst);
        });
        // Calls go through.
        assert_eq!(app.clear_color(&egui::Visuals::dark()), [0.5; 4]);
        drop(app);
        assert!(seen.load(Ordering::SeqCst), "the hook ran after the drop");
    }

    /// eframe returns: the host runs the work, the watch finds it done.
    #[test]
    fn the_work_after_the_window_runs_once_from_the_host() {
        let (tx, rx) = mpsc::channel();
        let after = AfterWindow::new(move || tx.send("work").unwrap());
        let (ended_tx, ended) = mpsc::channel();
        after.watch(Duration::from_millis(100), move || {
            ended_tx.send(()).unwrap()
        });
        assert!(after.run(), "the host runs it");
        assert!(!after.run(), "once");
        assert_eq!(rx.recv().unwrap(), "work");
        // The watch, past its grace, finds nothing to do and ends nothing.
        assert!(ended.recv_timeout(Duration::from_secs(5)).is_err());
    }

    /// eframe does not return: past the grace, the watch runs the work,
    /// then ends.
    #[test]
    fn the_watch_runs_the_work_when_eframe_does_not_return() {
        let (tx, rx) = mpsc::channel();
        let after = AfterWindow::new(move || tx.send("work").unwrap());
        let (ended_tx, ended) = mpsc::channel();
        let started = Instant::now();
        after.watch(Duration::from_millis(50), move || {
            ended_tx.send(()).unwrap()
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), "work");
        ended.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(50));
        assert!(!after.run(), "done by the watch");
    }

    #[test]
    fn finish_waits_for_what_ends_and_leaves_what_passes_its_deadline() {
        let mut f = Finisher::default();
        f.spawn("quick", Duration::from_secs(30), || {});
        // Held until the test lets it go, after the finish.
        let (release, held) = mpsc::channel::<()>();
        f.spawn("stuck", Duration::from_millis(50), move || {
            let _ = held.recv();
        });
        f.spawn("panics", Duration::from_secs(30), || panic!("on purpose"));
        let started = Instant::now();
        let left = f.finish();
        assert_eq!(left, ["stuck", "panics"]);
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
        release.send(()).unwrap();
    }
}
