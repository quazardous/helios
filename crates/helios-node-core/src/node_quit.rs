//! The node's own quit, as one request on its bus: `helios.node.quit`.
//!
//! When the node stops (asked to quit and its last client gone, or the
//! host's last word), it asks [`NodeQuit`] once on the application's
//! bus, handed to it as it starts (`LocalNode::quit_on`). Its listeners are what the node must settle before the
//! process may end, named, in a declared order ([`ORDER`]), each with its
//! own deadline:
//!
//! - one per adopter of [`crate::node::NodeJobs`], named after it
//!   ([`listener_of`]): the backups, the image map, the health check, the
//!   exports, the job mirror, then any other jobs; each cancels its jobs
//!   and waits for them up to its [`crate::node::NodeJobs::stop_within`];
//! - [`VAULT`], the library closed (its last flush, the clean-shutdown
//!   marker): always last, run on the node's thread, so it is never cut.
//!
//! Like the desktop's `helios.app.quit`, the request is **fail-open**: a
//! jobs listener that misses its deadline is cut (the request stops
//! waiting for it, its thread runs on, a write it tries once the vault
//! closed is refused), reported, and the quit goes on to the next one.
//!
//! The node says how its quit goes as facts on its fact bus, which reach
//! its clients over their link: [`ListenerBegan`] as each listener
//! begins (the desktop's end splash names the one it waits on), then
//! [`QuitEnded`], what each did, the cut ones named (the desktop puts it
//! in its quit report, `last-quit.json`).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bus_contract::PointId;
use bus_contract::{Bus as HookBus, ListenOpts, Point, PointDecl, PointKind, Refused};
use helios_core::{Bus, Event, Message};
use serde::{Deserialize, Serialize};

use crate::node::NodeJobs;

/// The request the node's quit is asked on.
pub const QUIT: &str = "helios.node.quit";

/// The owner of the node's quit listeners.
pub const OWNER: &str = "helios.node";

/// The library closed: the last listener, never cut.
pub const VAULT: &str = "helios.node.quit.vault";

/// The listener of jobs that name no place of their own.
pub const JOBS: &str = "helios.node.quit.jobs";

/// The order the node's listeners run in: the ones named here first, in
/// this order, any other after them, the library's close last.
pub const ORDER: &[&str] = &[
    "helios.node.quit.backups",
    "helios.node.quit.map",
    "helios.node.quit.health",
    "helios.node.quit.exports",
    "helios.node.quit.mirror",
    JOBS,
];

/// The deadline of a listener that sets none.
pub const DEADLINE: Duration = crate::node::JOB_STOP;

/// The node's quit, as its listeners read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeQuitAsk {
    /// The running jobs are cancelled (a confirmed quit, or the host's
    /// last word), not waited for. They were waited for already when not.
    pub cancelling: bool,
}

/// The node quits (`helios.node.quit`): each listener settles its part
/// and says its id once done.
#[derive(Debug)]
pub struct NodeQuit;

impl Point for NodeQuit {
    const KIND: PointKind = PointKind::Request;
    const ID: PointId = PointId::new(QUIT);
    type Input = NodeQuitAsk;
    type Value = Vec<String>;
}

/// The node's quit point. Fail-open by design (the module's doc): never
/// `.fail_closed()`.
pub const POINTS: &[PointDecl] = &[PointDecl::of::<NodeQuit>(
    "The node stops: its background jobs settled one listener at a time, each within its deadline, then the library closed, last; fail-open, a listener past its deadline is cut, reported and the quit goes on",
)
.deadline(DEADLINE)];

/// Declares [`POINTS`] on `bus` (once).
pub fn declare(bus: &dyn HookBus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios::node", "quit: {refused}");
        }
    }
}

/// A listener of the node's quit began: the one the quit waits on now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListenerBegan {
    /// Its id (`helios.node.quit.exports`).
    pub listener: String,
    /// What it does, for the end splash ("Stopping the exports").
    pub label: Message,
    /// Its deadline, in milliseconds.
    pub deadline_ms: u64,
}

impl Event for ListenerBegan {
    const KIND: &'static str = "helios.node.quit.listenerBegan";
}

/// A listener past its deadline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LateListener {
    pub listener: String,
    /// Its deadline, in milliseconds.
    pub deadline_ms: u64,
    /// The quit stopped waiting for it: its work may be lost.
    pub cut: bool,
}

/// The node's quit ended: what each listener did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuitEnded {
    /// The listeners that did their part in time, in order.
    pub finished: Vec<String>,
    /// The listeners past their deadline.
    pub late: Vec<LateListener>,
    /// The listeners that failed, and how.
    pub failed: Vec<(String, String)>,
}

impl Event for QuitEnded {
    const KIND: &'static str = "helios.node.quit.ended";
}

/// The listener id of an adopter of jobs, from its end step's key
/// (`node-end-exports` gives `helios.node.quit.exports`); [`JOBS`] when
/// it names none.
pub fn listener_of(jobs: &dyn NodeJobs) -> String {
    let name = jobs.name();
    match name.key.strip_prefix("node-end-") {
        Some(part) if !part.is_empty() && part != "jobs" => format!("{QUIT}.{part}"),
        _ => JOBS.to_owned(),
    }
}

/// Where a listener runs among the others: those of [`ORDER`] first, in
/// its order, any other after them.
fn priority(id: &str) -> i32 {
    let base = id.split('#').next().unwrap_or(id);
    ORDER
        .iter()
        .position(|o| *o == base)
        .map_or(0, |at| i32::try_from(ORDER.len() - at).unwrap_or(0) * 10)
}

/// Says on `facts` that `listener` begins, and marks it in the quit's
/// trace.
fn began(facts: &Bus, listener: &str, label: &Message, deadline: Duration) {
    crate::quit::mark(&format!("{QUIT}: {listener} begins"));
    facts.publish(
        "node",
        None,
        &ListenerBegan {
            listener: listener.to_owned(),
            label: label.clone(),
            deadline_ms: millis(deadline),
        },
    );
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Registers the node's listeners on `bus` for this quit: one per
/// adopter of `jobs`, threaded, each within its own deadline, and the
/// library's close `close`, last, inline. Those of an earlier quit are
/// replaced.
pub fn listen(
    bus: &dyn HookBus,
    facts: &Arc<Bus>,
    jobs: &[Arc<dyn NodeJobs>],
    close: impl Fn() -> Result<(), String> + Send + Sync + 'static,
) {
    declare(bus);
    for l in bus.listeners(QUIT).unwrap_or_default() {
        bus.unlisten(QUIT, &l.id);
    }
    let mut taken: Vec<String> = Vec::new();
    // All the jobs' listeners together stay within the node's jobs stop:
    // what one does not use is not given to the next, but none is given
    // more than what is left.
    let mut left = crate::node::JOBS_STOP;
    for job in jobs {
        let base = listener_of(&**job);
        let mut id = base.clone();
        let mut n = 1;
        while taken.contains(&id) {
            n += 1;
            id = format!("{base}#{n}");
        }
        taken.push(id.clone());
        // Cancelled only (its jobs not waited for): the cancel is given a
        // moment.
        let wait = job.stop_within();
        let deadline = if wait.is_zero() { CANCEL_ONLY } else { wait }
            .min(left)
            .max(LEAST);
        left = left.saturating_sub(deadline);
        let (job, facts, me) = (job.clone(), facts.clone(), id.clone());
        let listened = bus.listen::<NodeQuit>(
            &id,
            ListenOpts::new()
                .owner(OWNER)
                .priority(priority(&id))
                .deadline(deadline)
                .threaded(),
            move |_, done| {
                began(&facts, &me, &job.name(), deadline);
                job.cancel_all();
                if wait.is_zero() || job.wait(deadline) {
                    done.push(me.clone());
                    Ok(())
                } else {
                    Err(bus_contract::HookError::failed(format!(
                        "jobs still running after {} ms",
                        deadline.as_millis()
                    )))
                }
            },
        );
        if let Err(e) = listened {
            log::warn!(target: "helios::node", "{QUIT}: `{id}` not heard: {e}");
        }
    }
    let facts = facts.clone();
    let listened = bus.listen::<NodeQuit>(
        VAULT,
        ListenOpts::new()
            .owner(OWNER)
            .deadline(crate::node::LIBRARY_CLOSE)
            .last(),
        move |_, done| {
            let label = Message::new("node-end-vault", "Saving and closing the library");
            began(&facts, VAULT, &label, crate::node::LIBRARY_CLOSE);
            close().map_err(bus_contract::HookError::failed)?;
            done.push(VAULT.to_owned());
            Ok(())
        },
    );
    if let Err(e) = listened {
        log::warn!(target: "helios::node", "{QUIT}: `{VAULT}` not heard: {e}");
    }
}

/// How long an adopter whose jobs are only cancelled, not waited for
/// ([`NodeJobs::stop_within`] zero), is given for the cancel.
const CANCEL_ONLY: Duration = Duration::from_millis(500);

/// The least a jobs listener is given, the jobs stop's time used up.
const LEAST: Duration = Duration::from_millis(100);

/// Asks the node's quit on `bus`: its listeners run in order, each within
/// its deadline; what they did is said on `facts` ([`QuitEnded`]) and
/// returned. Fail-open: whatever a listener did, the next one runs.
pub fn ask(bus: &dyn HookBus, facts: &Bus, ask: NodeQuitAsk) -> QuitEnded {
    declare(bus);
    let deadlines: Vec<(String, Duration)> = bus
        .listeners(QUIT)
        .unwrap_or_default()
        .into_iter()
        .map(|l| (l.id, l.deadline))
        .collect();
    let out = bus.alter::<NodeQuit>(ask, Vec::new());
    let mut ended = QuitEnded {
        finished: out.value,
        ..QuitEnded::default()
    };
    for failure in out.failures.iter().filter(|f| f.dropped()) {
        log::warn!(target: "helios::node", "{failure}: the node's quit goes on");
        crate::quit::mark(&format!("{QUIT}: {} failed or late", failure.listener));
        match &failure.kind {
            bus_contract::FailureKind::Late { deadline, took } => {
                ended.late.push(LateListener {
                    listener: failure.listener.clone(),
                    deadline_ms: millis(*deadline),
                    cut: took.is_none(),
                });
            }
            // A jobs listener whose wait ended without its jobs: past its
            // deadline, cut.
            bus_contract::FailureKind::Error(why) if why.starts_with("jobs still running") => {
                let deadline = deadlines
                    .iter()
                    .find(|(id, _)| *id == failure.listener)
                    .map_or(DEADLINE, |(_, d)| *d);
                ended.late.push(LateListener {
                    listener: failure.listener.clone(),
                    deadline_ms: millis(deadline),
                    cut: true,
                });
            }
            other => ended
                .failed
                .push((failure.listener.clone(), other.to_string())),
        }
    }
    facts.publish("node", None, &ended);
    ended
}

/// The order the node's quit listeners run in on `bus`.
pub fn order(bus: &dyn HookBus) -> String {
    bus_contract::order_dump(bus, &[QUIT])
}

/// What the node's quit did, kept by the node for its own clients in the
/// same process (the desktop reads it through [`crate::node::Link`]).
#[derive(Debug, Default)]
pub struct Kept(Mutex<Option<QuitEnded>>);

impl Kept {
    pub fn set(&self, ended: QuitEnded) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(ended);
    }

    pub fn get(&self) -> Option<QuitEnded> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::LocalNode;
    use helios_vault_sqlite::SqliteStore;
    use std::sync::Condvar;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("helios-node-quit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    type Gate = Arc<(Mutex<bool>, Condvar)>;

    /// Jobs named after an end step's key; held until `gate` opens when
    /// it is given (a job that does not stop in time).
    struct Named {
        key: &'static str,
        gate: Option<Gate>,
        within: Duration,
    }

    impl Named {
        fn quick(key: &'static str) -> Arc<dyn NodeJobs> {
            Arc::new(Self {
                key,
                gate: None,
                within: Duration::from_secs(1),
            })
        }
    }

    impl NodeJobs for Named {
        fn name(&self) -> Message {
            Message::new(self.key, self.key)
        }
        fn running(&self) -> Vec<String> {
            Vec::new()
        }
        fn cancel_all(&self) {}
        fn wait(&self, deadline: Duration) -> bool {
            let Some(gate) = &self.gate else {
                return true;
            };
            let (open, opened) = &**gate;
            let held = open.lock().unwrap_or_else(PoisonError::into_inner);
            let (held, _) = opened
                .wait_timeout_while(held, deadline * 20, |o| !*o)
                .unwrap_or_else(PoisonError::into_inner);
            *held
        }
        fn stop_within(&self) -> Duration {
            self.within
        }
    }

    /// The node quits with `jobs` adopted: what its quit did, and the
    /// listeners it said began, in order.
    fn quit_with(name: &str, jobs: Vec<Arc<dyn NodeJobs>>) -> (QuitEnded, Vec<String>, bool) {
        let dir = scratch(name);
        let store = SqliteStore::open(&dir.join("vault.db")).unwrap();
        let facts = Arc::new(Bus::new());
        let began = facts.subscribe_kinds(&[ListenerBegan::KIND], None);
        let node = LocalNode::open(store, facts, Duration::from_hours(1));
        node.quit_on(Arc::new(egui_pop_hooks::Kernel::new()));
        let link = node.link();
        for j in jobs {
            link.adopt(j);
        }
        let client = node.connect();
        link.quit_cancelling_jobs();
        drop(client);
        assert!(
            link.wait_stopped(Duration::from_secs(20)),
            "the node stopped"
        );
        let ended = link.quit_ended().expect("the node's quit ended");
        let closed = link.closed();
        node.stop();
        let began = began
            .try_iter()
            .map(|e| e.payload["listener"].as_str().unwrap().to_owned())
            .collect();
        let again = SqliteStore::open(&dir.join("vault.db")).unwrap();
        assert!(!again.report().unclean_shutdown, "closed cleanly");
        let _ = std::fs::remove_dir_all(&dir);
        (ended, began, closed)
    }

    /// Adopted in any order, the node's listeners run in the declared
    /// one, each said as it begins; the library's close is last.
    #[test]
    fn the_node_runs_its_quit_listeners_in_declared_order_the_vault_last() {
        let (ended, began, closed) = quit_with(
            "order",
            vec![
                Named::quick("node-end-mirror"),
                Named::quick("node-end-other"),
                Named::quick("node-end-exports"),
                Named::quick("node-end-health"),
                Named::quick("node-end-backups"),
                Named::quick("node-end-map"),
            ],
        );
        let want = [
            "helios.node.quit.backups",
            "helios.node.quit.map",
            "helios.node.quit.health",
            "helios.node.quit.exports",
            "helios.node.quit.mirror",
            "helios.node.quit.other",
            VAULT,
        ];
        assert_eq!(ended.finished, want, "{ended:?}");
        assert_eq!(began, want, "said as each began");
        assert!(ended.late.is_empty() && ended.failed.is_empty());
        assert!(closed);
    }

    /// A job past its deadline is cut and named; the quit goes on and
    /// the library still closes, last.
    #[test]
    fn a_slow_job_past_its_deadline_is_cut_and_named_and_the_vault_closes_last() {
        let gate: Gate = Arc::default();
        let (ended, began, closed) = quit_with(
            "cut",
            vec![
                Arc::new(Named {
                    key: "node-end-exports",
                    gate: Some(gate.clone()),
                    within: Duration::from_millis(100),
                }),
                Named::quick("node-end-mirror"),
            ],
        );
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        assert_eq!(ended.late.len(), 1, "{ended:?}");
        assert_eq!(ended.late[0].listener, "helios.node.quit.exports");
        assert_eq!(ended.late[0].deadline_ms, 100);
        assert!(ended.late[0].cut, "the quit stopped waiting for it");
        assert_eq!(
            ended.finished,
            ["helios.node.quit.mirror", VAULT],
            "the quit went on"
        );
        assert_eq!(began.last().map(String::as_str), Some(VAULT));
        assert!(closed, "the library closed cleanly");
    }

    #[test]
    fn an_adopter_is_heard_under_its_end_step_s_name() {
        assert_eq!(
            listener_of(&*Named::quick("node-end-backups")),
            "helios.node.quit.backups"
        );
        assert_eq!(listener_of(&*Named::quick("node-end-jobs")), JOBS);
        assert_eq!(listener_of(&*Named::quick("whatever")), JOBS);
    }
}
