//! The start's and the end's steps, as the parts that run them declare
//! them: what the boot and end splashes list (`helios_gui::splash`).
//!
//! A part opening or closing something declares its step on the phase it
//! belongs to ([`boot`] or [`end`]) as it begins it ([`Phase::begin`]), or
//! ahead of time ([`Phase::declare`], then [`Running::start`]); the step
//! is done when its handle is dropped ([`Running::done`]). While it runs,
//! the part may say why it lasts ([`Running::why`]: "12 files left") and
//! whether stopping early is safe ([`Running::can_skip`]). A new part adds
//! its line by declaring it: the splash draws whatever the phase holds.
//!
//! The person may ask to stop waiting ([`Phase::quit_now`]): the host
//! reads it ([`Phase::quitting_now`]) and goes on without the steps left.
//! A phase nobody draws (`helios-node` without a window) only keeps its
//! list: a lock per step.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use helios_core::Message;

/// One step as the splash shows it.
#[derive(Clone, Debug)]
pub struct Step {
    /// What it does ("Opening the library").
    pub label: Message,
    /// When it began; `None` while it waits its turn.
    pub started: Option<Instant>,
    /// How long it took, once done.
    pub took: Option<Duration>,
    /// Why it lasts, said by the part running it.
    pub why: Option<Message>,
    /// Stopping before it ends is safe.
    pub can_skip: bool,
}

impl Step {
    /// Whether it is done.
    pub fn done(&self) -> bool {
        self.took.is_some()
    }

    /// Whether it began and is not done.
    pub fn running(&self) -> bool {
        self.started.is_some() && !self.done()
    }

    /// How long it has run at `now` (zero while it waits its turn).
    pub fn running_for(&self, now: Instant) -> Duration {
        match (self.started, self.took) {
            (_, Some(took)) => took,
            (Some(at), None) => now.saturating_duration_since(at),
            (None, None) => Duration::ZERO,
        }
    }
}

#[derive(Debug, Default)]
struct Inner {
    steps: Mutex<Vec<Step>>,
    quit_now: AtomicBool,
}

/// The steps of one phase (the start, or the end), shared by the parts
/// declaring them and the splash drawing them.
#[derive(Clone, Debug, Default)]
pub struct Phase(Arc<Inner>);

impl Phase {
    pub fn new() -> Self {
        Self::default()
    }

    fn update<T>(&self, f: impl FnOnce(&mut Vec<Step>) -> T) -> T {
        f(&mut self.0.steps.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Declares a step that waits its turn: listed now, begun with
    /// [`Running::start`].
    pub fn declare(&self, label: Message) -> Running {
        let index = self.update(|steps| {
            steps.push(Step {
                label,
                started: None,
                took: None,
                why: None,
                can_skip: false,
            });
            steps.len() - 1
        });
        Running {
            phase: self.clone(),
            index,
        }
    }

    /// Declares a step and begins it.
    pub fn begin(&self, label: Message) -> Running {
        let step = self.declare(label);
        step.start();
        step
    }

    /// The steps as they stand.
    pub fn steps(&self) -> Vec<Step> {
        self.update(|steps| steps.clone())
    }

    /// Whether a step is begun or waits its turn.
    pub fn busy(&self) -> bool {
        self.update(|steps| steps.iter().any(|s| !s.done()))
    }

    /// The person asked to stop waiting.
    pub fn quit_now(&self) {
        self.0.quit_now.store(true, Ordering::SeqCst);
    }

    /// Whether the person asked to stop waiting.
    pub fn quitting_now(&self) -> bool {
        self.0.quit_now.load(Ordering::SeqCst)
    }
}

/// A step declared, done when dropped.
#[derive(Debug)]
pub struct Running {
    phase: Phase,
    index: usize,
}

impl Running {
    fn update(&self, f: impl FnOnce(&mut Step)) {
        self.phase.update(|steps| {
            if let Some(step) = steps.get_mut(self.index) {
                f(step);
            }
        });
    }

    /// Begins it (once; again changes nothing).
    pub fn start(&self) {
        self.update(|s| {
            s.started.get_or_insert_with(Instant::now);
        });
    }

    /// Says why it lasts (replacing what it said before).
    pub fn why(&self, why: Message) {
        self.update(|s| s.why = Some(why));
    }

    /// Stopping before it ends is safe: the splash offers "Quit now" once
    /// it lasts.
    #[must_use]
    pub fn can_skip(self) -> Self {
        self.update(|s| s.can_skip = true);
        self
    }

    /// It is done (as dropping it).
    pub fn done(self) {}
}

impl Drop for Running {
    fn drop(&mut self) {
        self.update(|s| {
            let now = Instant::now();
            let at = *s.started.get_or_insert(now);
            s.took.get_or_insert(now - at);
        });
    }
}

/// The start's steps: from the process's start until the main window is
/// ready.
pub fn boot() -> &'static Phase {
    static BOOT: OnceLock<Phase> = OnceLock::new();
    BOOT.get_or_init(Phase::new)
}

/// The end's steps: from the quit until the process ends.
pub fn end() -> &'static Phase {
    static END: OnceLock<Phase> = OnceLock::new();
    END.get_or_init(Phase::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_listed_as_declared_and_done_as_their_handles_go() {
        let phase = Phase::new();
        let library = phase.begin(Message::new("test-a", "Opening the library"));
        let later = phase.declare(Message::new("test-b", "Reading the library"));
        let steps = phase.steps();
        assert_eq!(steps.len(), 2);
        assert!(steps[0].running() && !steps[1].running() && !steps[1].done());
        assert!(phase.busy());
        library.why(Message::new("test-c", "Migrating"));
        drop(library);
        later.start();
        let later = later.can_skip();
        let steps = phase.steps();
        assert!(steps[0].done() && steps[0].why.is_some());
        assert!(steps[1].running() && steps[1].can_skip);
        later.done();
        assert!(!phase.busy());
        assert!(!phase.quitting_now());
        phase.quit_now();
        assert!(phase.quitting_now());
    }

    #[test]
    fn a_step_done_without_starting_took_nothing() {
        let phase = Phase::new();
        phase.declare(Message::new("test-a", "Skipped")).done();
        assert_eq!(phase.steps()[0].took, Some(Duration::ZERO));
    }
}
