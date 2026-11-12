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

use std::sync::mpsc::{self, RecvTimeoutError};
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
