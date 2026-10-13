//! How long quitting takes, step by step.
//!
//! The quit starts when the window first sees its close request
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

use std::sync::{Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The environment variable that sends the marks to standard error.
pub const TRACE_ENV: &str = "HELIOS_QUIT_TRACE";

/// When the close was requested, and when the last step was marked.
static CLOCK: Mutex<Option<(Instant, Instant)>> = Mutex::new(None);

/// The close request is seen: the quit's clock starts (once; a second
/// request changes nothing).
pub fn requested() {
    let started = {
        let mut clock = CLOCK.lock().unwrap_or_else(PoisonError::into_inner);
        if clock.is_some() {
            false
        } else {
            let now = Instant::now();
            *clock = Some((now, now));
            true
        }
    };
    if started {
        mark("close requested");
    }
}

/// Whether the quit started.
pub fn started() -> bool {
    CLOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some()
}

/// Marks a step of the quit (nothing before the close request).
pub fn mark(step: &str) {
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
    log::debug!(target: "helios::quit", "{step}: {total:.1} ms ({since_last:.1} ms)");
    if std::env::var_os(TRACE_ENV).is_some() {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_micros());
        eprintln!("helios-quit {wall} {total:.1} {since_last:.1} {step}");
    }
}

/// Marks a step when dropped: a field placed in a struct after the ones
/// whose drop it times.
pub struct MarkOnDrop(pub &'static str);

impl Drop for MarkOnDrop {
    fn drop(&mut self) {
        mark(self.0);
    }
}
