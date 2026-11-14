//! Promises as the node's commands (see `docs/protocol.md`, "Promises"):
//! a long command answers at once with a promise, a handle to its result
//! to come ([`helios_vault::promises`]), and its work runs on a thread of
//! the node's, reporting through a [`Handle`].
//!
//! - `promises.get` and `promises.list` read them; `promises.wait`
//!   answers once one is settled or at its timeout (at most ten minutes:
//!   wait again rather than longer), so an agent has one way to wait on
//!   anything long.
//! - `promises.cancel` asks its runner to stop (its job is cancelled too):
//!   the runner settles it `cancelled`. One with no runner in this process
//!   is settled `cancelled` at once.
//! - `promises.retry` runs a failed promise again when its failure may be
//!   retried and this node knows its kind.
//!
//! **After a stop or a crash** ([`Promises::recover`], as the node
//! starts): a promise left pending or running with no runner is taken up
//! again when its kind says it can resume (a resumable import skips what
//! it already did), else settled `failed` with the reason
//! [`INTERRUPTED`], retryable when this node knows its kind. Nothing is
//! left running in people's eyes with nobody working on it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use helios_core::jobs::Jobs;
use helios_inspect::rpc::{self, Call, Error};
use helios_vault::promises::{Filter, INTERRUPTED, Promise, State};
use helios_vault::{Store, Vault};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, Need, object};

/// The ids of the promise commands.
pub const IDS: &[&str] = &[
    "promises.get",
    "promises.list",
    "promises.cancel",
    "promises.retry",
    "promises.wait",
];

/// The longest `promises.wait`, in milliseconds: the MCP wait cap.
pub const MAX_WAIT_MS: u64 = 600_000;

/// The most promises one list answers.
pub const MAX_LIST: u64 = 1_000;

/// Whether `id` is a promise command.
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// The promise commands' specs.
pub fn specs() -> Vec<CommandSpec> {
    let id = || {
        (
            "id",
            json!({ "type": "string", "description": "the promise's id, `promise-3`" }),
            true,
        )
    };
    let spec = |id: &str, label: &str| CommandSpec::new(id, label, "Jobs").needs(Need::Vault);
    let states = json!({ "type": "array", "items": { "type": "string",
        "enum": ["pending", "running", "done", "failed", "cancelled"] } });
    vec![
        spec("promises.get", "Read a promise")
            .params(object(&[id()]))
            .read_only(),
        spec("promises.list", "List the promises, newest first")
            .params(object(&[
                ("states", states, false),
                ("owner", json!({ "type": "string" }), false),
                ("kind", json!({ "type": "string" }), false),
                (
                    "limit",
                    json!({ "type": "integer", "minimum": 1, "maximum": MAX_LIST }),
                    false,
                ),
            ]))
            .read_only(),
        spec("promises.cancel", "Cancel a promise's work").params(object(&[id()])),
        spec("promises.retry", "Run a failed promise again").params(object(&[id()])),
        spec("promises.wait", "Wait until a promise is settled")
            .params(object(&[
                id(),
                (
                    "timeout_ms",
                    json!({ "type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS,
                            "description": "answer by then, settled or not" }),
                    true,
                ),
            ]))
            .read_only(),
    ]
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What runs a promise's work: called on a thread of its own with the
/// promise's handle, it reports through it and settles it.
pub type Run<S> = Arc<dyn Fn(Handle<S>) + Send + Sync>;

/// A kind of work promises stand for, as a node runs it.
pub struct Kind<S: Store> {
    run: Run<S>,
    resumable: bool,
}

/// The way a runner reports on its promise.
pub struct Handle<S: Store> {
    vault: Arc<Vault<S>>,
    promise: Promise,
    resumed: bool,
}

impl<S: Store> Handle<S> {
    /// The promise as it was when its run began.
    pub fn promise(&self) -> &Promise {
        &self.promise
    }

    pub fn id(&self) -> &str {
        &self.promise.id
    }

    /// The job id its work reports under.
    pub fn owner(&self) -> &str {
        &self.promise.owner
    }

    /// What was asked.
    pub fn request(&self) -> &Value {
        &self.promise.request
    }

    /// Whether this run takes up a promise cut by a stop.
    pub fn resumed(&self) -> bool {
        self.resumed
    }

    /// Whether a cancel was asked: the runner stops and says
    /// [`Handle::cancelled`].
    pub fn cancel_asked(&self) -> bool {
        self.vault
            .promise(&self.promise.id)
            .ok()
            .flatten()
            .is_none_or(|p| p.cancel_asked)
    }

    /// `done` of `total` so far (`None`: not known yet), with a note.
    pub fn progress(&self, done: u64, total: Option<u64>, note: Option<&str>) {
        self.report(|v, id| v.progress_promise(id, done, total, note, now()));
    }

    /// Settles it with its result.
    pub fn done(&self, result: Value) {
        self.report(|v, id| v.settle_promise(id, State::Done { result }, now()));
    }

    /// Settles it failed: why, and whether running it again may work.
    pub fn failed(&self, reason: &str, retryable: bool) {
        self.report(|v, id| {
            v.settle_promise(
                id,
                State::Failed {
                    reason: reason.into(),
                    retryable,
                },
                now(),
            )
        });
    }

    /// Settles it cancelled.
    pub fn cancelled(&self) {
        self.report(|v, id| v.settle_promise(id, State::Cancelled, now()));
    }

    fn report(&self, f: impl FnOnce(&Vault<S>, &str) -> Result<Promise, String>) {
        if let Err(e) = f(&self.vault, &self.promise.id) {
            log::warn!(target: "helios::promises", "{e}");
        }
    }
}

/// The node's promises: the vault keeps them, the kinds registered run
/// them.
pub struct Promises<S: Store> {
    vault: Arc<Vault<S>>,
    jobs: Jobs,
    /// Other jobs a promise's job may run on (the desktop's, mirrored by
    /// [`crate::job_promises`]): a cancel reaches them too.
    through: std::sync::Mutex<Vec<Jobs>>,
    kinds: BTreeMap<String, Kind<S>>,
}

impl<S: Store> Promises<S> {
    /// The promises of `vault`, their jobs cancelled on `jobs`; no kind
    /// runs yet.
    pub fn new(vault: Arc<Vault<S>>, jobs: Jobs) -> Self {
        Self {
            vault,
            jobs,
            through: std::sync::Mutex::default(),
            kinds: BTreeMap::new(),
        }
    }

    /// A cancel of a promise whose job runs on `jobs` reaches it: the
    /// jobs [`crate::job_promises`] mirrors.
    pub fn cancel_through(&self, jobs: Jobs) {
        self.through
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(jobs);
    }

    /// Runs the promises of kind `name` with `run`; `resumable`: one cut
    /// by a stop is taken up again rather than failed.
    #[must_use]
    pub fn kind(
        mut self,
        name: &str,
        resumable: bool,
        run: impl Fn(Handle<S>) + Send + Sync + 'static,
    ) -> Self {
        self.kinds.insert(
            name.into(),
            Kind {
                run: Arc::new(run),
                resumable,
            },
        );
        self
    }

    pub fn vault(&self) -> &Arc<Vault<S>> {
        &self.vault
    }

    /// The jobs its work reports under.
    pub fn jobs(&self) -> &Jobs {
        &self.jobs
    }
}

impl<S: Store + Send + 'static> Promises<S> {
    /// Starts a promise of kind `kind` for `label` on `request`: it is
    /// answered at once, pending, its work started on a thread. Its owner
    /// is the job `<kind>-<promise id>`.
    pub fn start(&self, kind: &str, label: &str, request: Value) -> Result<Promise, String> {
        if !self.kinds.contains_key(kind) {
            return Err(format!("no runner for promises of kind `{kind}`"));
        }
        let p =
            self.vault
                .create_promise(kind, &format!("{kind}-{{id}}"), label, request, now())?;
        self.spawn(&p, false);
        Ok(p)
    }

    /// Runs `p` on a thread, its runner noted live until it returns. A
    /// runner that returns without settling it fails it.
    fn spawn(&self, p: &Promise, resumed: bool) {
        let Some(kind) = self.kinds.get(&p.kind) else {
            return;
        };
        let run = kind.run.clone();
        let vault = self.vault.clone();
        let promise = p.clone();
        vault.set_promise_live(&p.id, true);
        let id = p.id.clone();
        let spawned = std::thread::Builder::new()
            .name("helios-promise".into())
            .spawn(move || {
                let handle = Handle {
                    vault: vault.clone(),
                    promise,
                    resumed,
                };
                let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(handle)));
                let open = vault
                    .promise(&id)
                    .ok()
                    .flatten()
                    .is_some_and(|p| !p.state.is_settled());
                if open {
                    let reason = if ran.is_err() {
                        "its work stopped on an internal error"
                    } else {
                        "its work ended without an outcome"
                    };
                    let failed = State::Failed {
                        reason: reason.into(),
                        retryable: true,
                    };
                    let _ = vault.settle_promise(&id, failed, now());
                }
                vault.set_promise_live(&id, false);
            });
        if let Err(e) = spawned {
            let failed = State::Failed {
                reason: format!("no thread to run on: {e}"),
                retryable: true,
            };
            let _ = self.vault.settle_promise(&p.id, failed, now());
            self.vault.set_promise_live(&p.id, false);
        }
    }

    /// Takes up the promises a stop or a crash left pending or running
    /// with no runner: those of a resumable kind run again, the others are
    /// settled failed ([`INTERRUPTED`]), retryable when their kind is
    /// known here. The ids taken up.
    pub fn recover(&self) -> Result<Vec<String>, String> {
        let open = Filter {
            states: vec!["pending".into(), "running".into()],
            ..Filter::default()
        };
        let mut resumed = Vec::new();
        for p in self.vault.promises(&open)? {
            if self.vault.promise_live(&p.id) {
                continue;
            }
            match self.kinds.get(&p.kind) {
                Some(kind) if kind.resumable && !p.cancel_asked => {
                    let p = self.vault.resume_promise(&p.id, now())?;
                    self.spawn(&p, true);
                    resumed.push(p.id);
                }
                known => {
                    let outcome = if p.cancel_asked {
                        State::Cancelled
                    } else {
                        State::Failed {
                            reason: INTERRUPTED.into(),
                            retryable: known.is_some(),
                        }
                    };
                    self.vault.settle_promise(&p.id, outcome, now())?;
                }
            }
        }
        Ok(resumed)
    }

    /// Whether some promise's work runs in this process now.
    pub fn working(&self) -> bool {
        let open = Filter {
            states: vec!["pending".into(), "running".into()],
            ..Filter::default()
        };
        self.vault
            .promises(&open)
            .is_ok_and(|open| open.iter().any(|p| self.vault.promise_live(&p.id)))
    }

    /// Asks promise `id`'s work to stop: its runner settles it cancelled
    /// (its job is cancelled too). One with no runner here is settled at
    /// once.
    pub fn cancel(&self, id: &str) -> Result<Promise, String> {
        if !self.vault.promise_live(id) {
            return self.vault.settle_promise(id, State::Cancelled, now());
        }
        let p = self.vault.ask_cancel_promise(id, now())?;
        // Its job may have ended or never started: the runner reads the
        // cancel all the same.
        let through = self
            .through
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for jobs in std::iter::once(&self.jobs).chain(&through) {
            if jobs.cancellable().contains(&p.owner) {
                let _ = jobs.cancel(&p.owner);
            }
        }
        Ok(p)
    }

    /// Runs failed promise `id` again, when it may be retried and its kind
    /// runs here.
    pub fn retry(&self, id: &str) -> Result<Promise, String> {
        let p = self
            .vault
            .promise(id)?
            .ok_or_else(|| format!("no promise `{id}`"))?;
        if !self.kinds.contains_key(&p.kind) {
            return Err(format!(
                "no runner for promises of kind `{}` on this node",
                p.kind
            ));
        }
        let p = self.vault.retry_promise(id, now())?;
        self.spawn(&p, false);
        Ok(p)
    }

    /// Answers promise command `method` on params its schema checked.
    pub fn run(&self, method: &str, params: &Value, call: &Call) -> Result<Value, Error> {
        let refused = |e: String| Error::new(rpc::code::REFUSED, e);
        let id = params["id"].as_str().unwrap_or("");
        let one = |p: Promise| json!({ "promise": p });
        match method {
            "promises.get" => match self.vault.promise(id).map_err(refused)? {
                Some(p) => Ok(one(p)),
                None => Err(refused(format!("no promise `{id}`"))),
            },
            "promises.list" => {
                let filter = Filter {
                    states: params["states"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(Into::into)
                        .collect(),
                    owner: params["owner"].as_str().map(Into::into),
                    kind: params["kind"].as_str().map(Into::into),
                };
                let limit = params["limit"].as_u64().unwrap_or(MAX_LIST);
                let mut all = self.vault.promises(&filter).map_err(refused)?;
                let total = all.len();
                all.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
                Ok(json!({ "promises": all, "total": total }))
            }
            "promises.cancel" => self.cancel(id).map(one).map_err(refused),
            "promises.retry" => self.retry(id).map(one).map_err(refused),
            "promises.wait" => {
                let ms = params["timeout_ms"].as_u64().unwrap_or(0).min(MAX_WAIT_MS);
                let p = self
                    .vault
                    .wait_promise(id, Duration::from_millis(ms), &|| call.is_cancelled())
                    .map_err(refused)?;
                let settled = p.state.is_settled();
                Ok(json!({ "promise": p, "settled": settled }))
            }
            _ => Err(Error::method_not_found(method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::commands::validate;

    use super::*;

    /// Params each command takes, its optional fields given.
    fn examples() -> Vec<(&'static str, Value)> {
        vec![
            ("promises.get", json!({ "id": "promise-3" })),
            (
                "promises.list",
                json!({ "states": ["running", "failed"], "owner": "import-promise-3",
                        "kind": "import", "limit": 50 }),
            ),
            ("promises.cancel", json!({ "id": "promise-3" })),
            ("promises.retry", json!({ "id": "promise-3" })),
            (
                "promises.wait",
                json!({ "id": "promise-3", "timeout_ms": 60_000 }),
            ),
        ]
    }

    #[test]
    fn every_promise_command_has_an_example_its_schema_takes() {
        let specs = specs();
        let ids: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, IDS, "one spec per id");
        let examples = examples();
        let named: Vec<&str> = examples.iter().map(|(id, _)| *id).collect();
        assert_eq!(named, IDS, "one example per command");
        for (id, params) in examples {
            let spec = specs.iter().find(|s| s.id == id).unwrap();
            validate(&spec.params, &params).unwrap_or_else(|e| panic!("{id}: {e}"));
        }
        let wait = specs.iter().find(|s| s.id == "promises.wait").unwrap();
        assert!(
            validate(
                &wait.params,
                &json!({ "id": "p", "timeout_ms": MAX_WAIT_MS + 1 })
            )
            .is_err(),
            "over the wait cap"
        );
    }
}
