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
//!
//! The window goes first: the frame that sees the close request draws
//! nothing and only writes what the user changed (the library's last
//! changes, the edit's last step), and the window closes after it. What
//! is left (closing the vault) runs once the window is gone, on threads
//! of their own, each with a deadline ([`Finisher`]): what is not done by
//! then is logged and left, and the process exits anyway. Nothing the
//! user saved is at stake there: each edit step, rating and label is in
//! the vault before the window closes (see docs/persistence.md).

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

/// How long closing the vault (its clean-shutdown marker, SQLite's last
/// checkpoint) may take once the window is gone.
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

#[cfg(test)]
mod tests {
    use super::*;

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
