//! Promises: the durable handle to a result to come (see
//! `docs/protocol.md`, "Promises").
//!
//! A long command answers at once with a promise instead of its result;
//! the caller (a window, the CLI, an agent) holds the id, may go away and
//! come back, and reads the result once the promise is settled. A promise
//! is a document in the namespace [`PROMISES`], so it survives a restart
//! and every client of the node sees it.
//!
//! Its life: `pending`, then `running` with its progress, then settled
//! once: `done` with its result, `failed` with a reason (and whether it can
//! be retried) or `cancelled`. A settled promise never changes again,
//! except a failed one retried, which goes back to `pending`.
//!
//! Each change is announced on the vault's bus: [`PROGRESS`] while it is
//! unsettled (created, progressing, retried), [`DONE`] when it settles;
//! the payload is the promise itself. Time is passed in, never read.
//!
//! The vault also keeps, in memory only, which promises have a runner in
//! this process ([`Vault::set_promise_live`]): a promise left unsettled
//! without one was cut by a stop or a crash.

use std::collections::BTreeSet;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Store, Vault};

/// The documents' namespace, one per promise id.
pub const PROMISES: &str = "node/promises";

/// The namespace of the id sequence.
const SEQUENCE: &str = "node/promise-sequence";

/// The bus event of a promise created, progressing or retried.
pub const PROGRESS: &str = "promise.progress";

/// The bus event of a promise settled: done, failed or cancelled.
pub const DONE: &str = "promise.done";

/// The reason a promise cut by a stop or a crash fails with.
pub const INTERRUPTED: &str = "interrupted";

/// Where a promise stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// Created; its work has not started.
    Pending,
    /// Its work is under way: `done` of `total` (unknown yet: `None`),
    /// with a short note (the file at hand).
    Running {
        done: u64,
        total: Option<u64>,
        note: Option<String>,
    },
    /// Settled with its result.
    Done { result: Value },
    /// Settled without a result: why, and whether running it again may
    /// work.
    Failed { reason: String, retryable: bool },
    /// Settled: stopped on request.
    Cancelled,
}

impl State {
    /// Its name: `pending`, `running`, `done`, `failed` or `cancelled`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running { .. } => "running",
            Self::Done { .. } => "done",
            Self::Failed { .. } => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether it is final: done, failed or cancelled.
    pub fn is_settled(&self) -> bool {
        matches!(
            self,
            Self::Done { .. } | Self::Failed { .. } | Self::Cancelled
        )
    }
}

/// A result to come, as the vault keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Promise {
    /// `promise-<n>`, unique in the vault.
    pub id: String,
    /// What fulfils it: the job id its work reports under (`import-3`).
    pub owner: String,
    /// What kind of work it stands for (`import`): the runner that can
    /// resume or retry it.
    pub kind: String,
    /// What it stands for, for people: "Import 2,000 photos from Google".
    pub label: String,
    /// What was asked, kept to run it again (a retry, a resume).
    #[serde(default)]
    pub request: Value,
    #[serde(flatten)]
    pub state: State,
    /// A cancel was asked; its runner settles it `cancelled`.
    #[serde(default)]
    pub cancel_asked: bool,
    /// Runs so far: 1, then one more per retry or resume.
    #[serde(default)]
    pub attempts: u32,
    /// When it was created and last changed, in seconds since the epoch.
    pub created: u64,
    pub updated: u64,
}

/// Which promises a list keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    /// State names (`running`, `failed`...); empty: every state.
    pub states: Vec<String>,
    /// Only those of this owner.
    pub owner: Option<String>,
    /// Only those of this kind.
    pub kind: Option<String>,
}

impl Filter {
    fn keeps(&self, p: &Promise) -> bool {
        (self.states.is_empty() || self.states.iter().any(|s| s == p.state.name()))
            && self.owner.as_ref().is_none_or(|o| *o == p.owner)
            && self.kind.as_ref().is_none_or(|k| *k == p.kind)
    }
}

/// What the vault keeps of its promises in memory: which have a runner in
/// this process, and a signal each change wakes the waiters with.
#[derive(Default)]
pub(crate) struct Runtime {
    live: Mutex<BTreeSet<String>>,
    changed: Condvar,
}

/// A promise's document read back.
fn parse(doc: Value) -> Result<Promise, String> {
    serde_json::from_value(doc).map_err(|e| format!("a promise does not read back: {e}"))
}

impl<S: Store> Vault<S> {
    /// Creates a pending promise of `kind`, fulfilled by job `owner`
    /// (`{id}` in it is replaced by the promise's id), for `label`, keeping
    /// `request` to run it again. Announced as [`PROGRESS`].
    pub fn create_promise(
        &self,
        kind: &str,
        owner: &str,
        label: &str,
        request: Value,
        now: u64,
    ) -> Result<Promise, String> {
        let promise = self.transaction(|s| {
            let n = s
                .document(SEQUENCE, "next")?
                .and_then(|d| d["n"].as_u64())
                .unwrap_or(1);
            s.set_document(SEQUENCE, "next", Some(&json!({ "n": n + 1 })))?;
            let id = format!("promise-{n}");
            let promise = Promise {
                owner: owner.replace("{id}", &id),
                id,
                kind: kind.into(),
                label: label.into(),
                request,
                state: State::Pending,
                cancel_asked: false,
                attempts: 1,
                created: now,
                updated: now,
            };
            write(s, &promise)?;
            Ok(promise)
        })?;
        self.promise_changed(&promise);
        Ok(promise)
    }

    /// The promise of id `id`, if the vault keeps one.
    pub fn promise(&self, id: &str) -> Result<Option<Promise>, String> {
        self.read(|s| s.document(PROMISES, id))?
            .map(parse)
            .transpose()
    }

    /// The promises `filter` keeps, newest first.
    pub fn promises(&self, filter: &Filter) -> Result<Vec<Promise>, String> {
        let mut all = self
            .read(|s| s.documents(PROMISES))?
            .into_iter()
            .map(|(_, doc)| parse(doc))
            .collect::<Result<Vec<_>, _>>()?;
        all.retain(|p| filter.keeps(p));
        all.sort_by_key(|p| std::cmp::Reverse((p.created, sequence(&p.id))));
        Ok(all)
    }

    /// Its work goes on: `done` of `total`, with `note`. A pending promise
    /// becomes running. Refused once it is settled.
    pub fn progress_promise(
        &self,
        id: &str,
        done: u64,
        total: Option<u64>,
        note: Option<&str>,
        now: u64,
    ) -> Result<Promise, String> {
        self.change_promise(id, now, |p| {
            if p.state.is_settled() {
                return Err(format!("promise `{id}` is already {}", p.state.name()));
            }
            p.state = State::Running {
                done,
                total,
                note: note.map(Into::into),
            };
            Ok(())
        })
    }

    /// Settles it with `outcome` (done, failed or cancelled), once: a
    /// settled promise refuses, and so does an outcome that is not final.
    /// Announced as [`DONE`].
    pub fn settle_promise(&self, id: &str, outcome: State, now: u64) -> Result<Promise, String> {
        if !outcome.is_settled() {
            return Err(format!(
                "`{}` does not settle a promise: done, failed or cancelled do",
                outcome.name()
            ));
        }
        self.change_promise(id, now, |p| {
            if p.state.is_settled() {
                return Err(format!("promise `{id}` is already {}", p.state.name()));
            }
            p.state = outcome;
            Ok(())
        })
    }

    /// Notes that a cancel was asked: its runner reads it
    /// ([`Promise::cancel_asked`]) and settles it `cancelled`. Refused once
    /// it is settled.
    pub fn ask_cancel_promise(&self, id: &str, now: u64) -> Result<Promise, String> {
        self.change_promise(id, now, |p| {
            if p.state.is_settled() {
                return Err(format!("promise `{id}` is already {}", p.state.name()));
            }
            p.cancel_asked = true;
            Ok(())
        })
    }

    /// A failed promise that can be retried goes back to pending, one
    /// attempt more, its cancel forgotten. Refused for any other.
    pub fn retry_promise(&self, id: &str, now: u64) -> Result<Promise, String> {
        self.change_promise(id, now, |p| match &p.state {
            State::Failed {
                retryable: true, ..
            } => {
                p.state = State::Pending;
                p.attempts += 1;
                p.cancel_asked = false;
                Ok(())
            }
            State::Failed { .. } => Err(format!("promise `{id}` failed for good: no retry")),
            other => Err(format!(
                "promise `{id}` is {}: only a failed one is retried",
                other.name()
            )),
        })
    }

    /// An unsettled promise taken up again after a stop: running again,
    /// one attempt more.
    pub fn resume_promise(&self, id: &str, now: u64) -> Result<Promise, String> {
        self.change_promise(id, now, |p| {
            if p.state.is_settled() {
                return Err(format!("promise `{id}` is already {}", p.state.name()));
            }
            p.attempts += 1;
            Ok(())
        })
    }

    /// Forgets a settled promise. Whether there was one.
    pub fn forget_promise(&self, id: &str) -> Result<bool, String> {
        self.transaction(|s| {
            let Some(p) = s.document(PROMISES, id)?.map(parse).transpose()? else {
                return Ok(false);
            };
            if !p.state.is_settled() {
                return Err(format!(
                    "promise `{id}` is {}: not forgotten",
                    p.state.name()
                ));
            }
            s.set_document(PROMISES, id, None)?;
            Ok(true)
        })
    }

    /// Notes whether promise `id` has a runner in this process.
    pub fn set_promise_live(&self, id: &str, live: bool) {
        let rt = &self.promises_rt;
        let mut set = rt
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if live {
            set.insert(id.into());
        } else {
            set.remove(id);
        }
        rt.changed.notify_all();
    }

    /// Whether promise `id` has a runner in this process.
    pub fn promise_live(&self, id: &str) -> bool {
        let rt = &self.promises_rt;
        rt.live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(id)
    }

    /// Waits until promise `id` is settled or `timeout` passed, waking on
    /// each of its changes; `stop` ends the wait early (a cancelled call).
    /// The promise as it then stands.
    pub fn wait_promise(
        &self,
        id: &str,
        timeout: Duration,
        stop: &dyn Fn() -> bool,
    ) -> Result<Promise, String> {
        let until = Instant::now() + timeout;
        let rt = &self.promises_rt;
        loop {
            // Read under the lock: a change made after the read wakes the
            // wait below, so none is missed.
            let guard = rt
                .live
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let p = self
                .promise(id)?
                .ok_or_else(|| format!("no promise `{id}`"))?;
            let now = Instant::now();
            if p.state.is_settled() || now >= until || stop() {
                return Ok(p);
            }
            // A slice at most, to look at `stop` again.
            let slice = (until - now).min(Duration::from_millis(200));
            drop(
                rt.changed
                    .wait_timeout(guard, slice)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }
    }

    /// Reads promise `id`, applies `f`, writes it back stamped `now` and
    /// announces it.
    fn change_promise(
        &self,
        id: &str,
        now: u64,
        f: impl FnOnce(&mut Promise) -> Result<(), String>,
    ) -> Result<Promise, String> {
        let promise = self.transaction(|s| {
            let mut p = s
                .document(PROMISES, id)?
                .map(parse)
                .transpose()?
                .ok_or_else(|| format!("no promise `{id}`"))?;
            f(&mut p)?;
            p.updated = now;
            write(s, &p)?;
            Ok(p)
        })?;
        self.promise_changed(&promise);
        Ok(promise)
    }

    /// Wakes the waiters and announces `p` on the bus.
    fn promise_changed(&self, p: &Promise) {
        {
            let rt = &self.promises_rt;
            let _guard = rt
                .live
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            rt.changed.notify_all();
        }
        if let Some(bus) = &self.bus {
            let kind = if p.state.is_settled() { DONE } else { PROGRESS };
            bus.publish_value(
                "vault",
                None,
                kind,
                serde_json::to_value(p).expect("a promise serializes"),
            );
        }
    }
}

fn write<S: Store>(s: &mut S, p: &Promise) -> Result<(), String> {
    let doc = serde_json::to_value(p).expect("a promise serializes");
    s.set_document(PROMISES, &p.id, Some(&doc))
}

/// The number in `promise-<n>`, to order promises created the same second.
fn sequence(id: &str) -> u64 {
    id.rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}
