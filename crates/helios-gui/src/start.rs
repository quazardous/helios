//! How long starting takes, step by step.
//!
//! The clock starts when `main` begins ([`begin`]); each step after it is
//! marked ([`mark`]) with the time since then and since the step before,
//! in the log at debug level (target `helios::start`). With
//! `HELIOS_START_TRACE` set in the environment the same lines go to
//! standard error, with the wall clock in microseconds, so a script
//! launching the window can line them up with when it launched it, when
//! the window showed and when it answered an input:
//!
//! ```text
//! helios-start <epoch µs> <ms since main> <ms since the last mark> <step>
//! ```
//!
//! The marks a script waits for are [`FIRST_FRAME`] (the first frame
//! presented) and [`INTERACTIVE`] (the first frame that saw a key or the
//! pointer, built).
//! Nothing is marked before [`begin`] nor once the start is over (both
//! marks made): a step that runs again later (a theme installed) marks
//! its first time only. A mark costs a lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The environment variable that sends the marks to standard error.
pub const TRACE_ENV: &str = "HELIOS_START_TRACE";

/// The mark of the first frame presented (eframe shows the window once
/// its first frame is painted).
pub const FIRST_FRAME: &str = "first frame presented";

/// The mark of the first frame that answered an input.
pub const INTERACTIVE: &str = "interactive (an input answered)";

/// When `main` began, and when the last step was marked.
static CLOCK: Mutex<Option<(Instant, Instant)>> = Mutex::new(None);

/// Set once the start is over: marks are no longer made.
static OVER: AtomicBool = AtomicBool::new(false);

/// Set once the first frame is built.
static FIRST_BUILT: AtomicBool = AtomicBool::new(false);

/// `main` began: the start's clock starts (once).
pub fn begin() {
    {
        let mut clock = CLOCK.lock().unwrap_or_else(PoisonError::into_inner);
        if clock.is_some() {
            return;
        }
        let now = Instant::now();
        *clock = Some((now, now));
    }
    mark("main");
}

/// Marks a step of the start (nothing before [`begin`], nothing once it
/// is over).
pub fn mark(step: &str) {
    if OVER.load(Ordering::Relaxed) {
        return;
    }
    let (total, since_last) = {
        let mut clock = CLOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let Some((start, last)) = clock.as_mut() else {
            return;
        };
        let now = Instant::now();
        let times = (now - *start, now - *last);
        *last = now;
        times
    };
    let (total, since_last) = (total.as_secs_f64() * 1e3, since_last.as_secs_f64() * 1e3);
    log::debug!(target: "helios::start", "{step}: {total:.1} ms ({since_last:.1} ms)");
    if std::env::var_os(TRACE_ENV).is_some() {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_micros());
        eprintln!("helios-start {wall} {total:.1} {since_last:.1} {step}");
    }
}

/// Marks a step of the first frame (nothing in the frames after it).
pub fn mark_in_first_frame(step: &str) {
    if !FIRST_BUILT.load(Ordering::Relaxed) {
        mark(step);
    }
}

/// Marks the frames that end the start: the first one built, then
/// presented (when the next one begins), then the first one that answered
/// an input (a key, the pointer). It stops marking once all are.
#[derive(Default)]
pub struct Frames {
    began: bool,
    built: u32,
    presented: bool,
    answered: bool,
}

impl Frames {
    /// A frame begins (eframe's `raw_input_hook`): the one before it was
    /// painted and presented.
    pub fn begin(&mut self) {
        if self.built == 0 && !self.began {
            self.began = true;
            mark("first frame begins");
        }
        if self.built > 0 && !self.presented {
            self.presented = true;
            mark(FIRST_FRAME);
            self.over_when_done();
        }
    }

    /// Once every mark is made, the start is over: later marks are none.
    fn over_when_done(&self) {
        if self.done() {
            OVER.store(true, Ordering::Relaxed);
        }
    }

    /// A frame was built, and whether it saw an input. After the first
    /// one, `next` asks for another at once (it marks the first presented).
    pub fn end(&mut self, saw_input: bool, next: impl FnOnce()) {
        self.built += 1;
        if self.built == 1 {
            FIRST_BUILT.store(true, Ordering::Relaxed);
            mark("first frame built");
            next();
        }
        if saw_input && !self.answered {
            self.answered = true;
            mark(INTERACTIVE);
            self.over_when_done();
        }
    }

    /// Whether every mark was made.
    pub fn done(&self) -> bool {
        self.presented && self.answered
    }
}

/// Whether a frame's input holds something a user did: a key, the
/// pointer moved or pressed, text, a scroll.
pub fn saw_input(input: &egui::InputState) -> bool {
    input.events.iter().any(|e| {
        matches!(
            e,
            egui::Event::Key { .. }
                | egui::Event::Text(_)
                | egui::Event::PointerMoved(_)
                | egui::Event::PointerButton { .. }
                | egui::Event::MouseWheel { .. }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_mark_built_presented_then_the_first_input_once() {
        let mut f = Frames::default();
        f.begin();
        assert!(!f.presented);
        let mut asked = 0;
        f.end(false, || asked += 1);
        assert_eq!((f.built, asked), (1, 1));
        f.begin();
        assert!(f.presented && !f.done());
        f.end(true, || asked += 1);
        assert!(f.done());
        assert_eq!(asked, 1, "one more frame asked for, after the first");
    }

    #[test]
    fn a_pointer_move_is_an_input_a_focus_change_is_not() {
        let mut input = egui::InputState::default();
        input.events.push(egui::Event::WindowFocused(true));
        assert!(!saw_input(&input));
        input
            .events
            .push(egui::Event::PointerMoved(egui::pos2(1.0, 2.0)));
        assert!(saw_input(&input));
    }
}
