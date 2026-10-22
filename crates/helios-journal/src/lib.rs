//! The workflow level: one network applied to many items over time.
//!
//! A render is pure and needs no queue (see `docs/architecture.md`, "Two
//! levels of execution"); applying a workflow to the 3,200 images of a
//! wedding does. This crate keeps a **node journal** per (item, node), after
//! [grampy](https://github.com/quazardous/grampy)'s model:
//!
//! - an item is anything with a stable id (an image most of the time);
//! - a node is **claimable** for an item when it has no row and is
//!   **joined**: `need` parents (all by default) are done, skipped or
//!   omitted; a failed parent satisfies nobody;
//! - workers **claim** a node for some items, getting a lease with a token,
//!   and **conclude** it; a worker whose lease was given back writes nothing;
//! - an **exclusive choice** concludes with a branch: the other branches, and
//!   every node only they lead to, are omitted in the same write;
//! - a failure is **retried** when the node declares it, after a delay that
//!   can grow; past the limit it stands;
//! - **leases** expire: `expire` gives back what a dead worker held, and
//!   a worker renews the lease of a long job; `recover` gives back every
//!   running row when a process opens the journal again;
//! - every write runs in one store **transaction**: kept whole or not at
//!   all.
//!
//! Time is always passed in (milliseconds), never read from a clock, so the
//! journal is deterministic and every worker agrees on what is due.
//!
//! Storage is a [`Store`]: [`MemoryStore`] here, the vault's
//! (`helios_vault::journal::VaultJournal`) for a journal that lasts. Every
//! store must pass the same [`contract`].
//!
//! Not yet: groups, lanes, waits and signals, rate limits, graph versions,
//! history of what forget and retries take away.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use helios_core::Describe;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub mod contract;
mod store;

pub use store::{MemoryStore, Pending, Store, Write};

/// Where an item stands at a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    /// Failed, to be tried again when due.
    Scheduled,
    Done,
    Skipped,
    Failed,
    /// Not on the branch a choice took.
    Omitted,
}

impl Status {
    /// Every status, in the order of their declaration.
    pub const ALL: [Status; 6] = [
        Status::Running,
        Status::Scheduled,
        Status::Done,
        Status::Skipped,
        Status::Failed,
        Status::Omitted,
    ];

    /// Its name, as rows and `monitor` spell it.
    pub fn name(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Scheduled => "scheduled",
            Status::Done => "done",
            Status::Skipped => "skipped",
            Status::Failed => "failed",
            Status::Omitted => "omitted",
        }
    }

    /// Whether a child may count this parent as concluded.
    fn accepted(self) -> bool {
        matches!(self, Status::Done | Status::Skipped | Status::Omitted)
    }
}

/// The journal row of an item at a node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub status: Status,
    /// Attempts started so far.
    pub attempts: u32,
    /// The token of the lease that holds it, while running.
    pub token: Option<u64>,
    /// When the row got its status.
    pub since: u64,
    /// When a running lease expires, or when a scheduled retry is due.
    pub due: Option<u64>,
    pub error: Option<String>,
}

/// How a failure is retried: at most `limit` attempts in all, the first
/// retry after `delay_ms`, doubling each time when `exponential`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retry {
    pub limit: u32,
    pub delay_ms: u64,
    pub exponential: bool,
}

/// A step of a workflow.
#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub parents: Vec<String>,
    /// How many parents must be concluded (all when `None`).
    pub need: Option<usize>,
    /// Concludes with a branch: one of its children.
    pub choice: bool,
    /// How long a claim holds before `expire` gives it back.
    pub lease_ms: u64,
    pub retry: Retry,
}

impl Node {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            parents: Vec::new(),
            need: None,
            choice: false,
            lease_ms: 60_000,
            retry: Retry {
                limit: 1,
                delay_ms: 0,
                exponential: false,
            },
        }
    }

    pub fn after(mut self, parents: &[&str]) -> Self {
        self.parents = parents
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        self
    }

    pub fn need(mut self, k: usize) -> Self {
        self.need = Some(k);
        self
    }

    pub fn choice(mut self) -> Self {
        self.choice = true;
        self
    }

    pub fn lease(mut self, ms: u64) -> Self {
        self.lease_ms = ms;
        self
    }

    pub fn retry(mut self, limit: u32, delay_ms: u64, exponential: bool) -> Self {
        self.retry = Retry {
            limit,
            delay_ms,
            exponential,
        };
        self
    }
}

/// A DAG of nodes, each declared after its parents.
#[derive(Clone, Debug)]
pub struct Workflow {
    nodes: Vec<Node>,
}

impl Workflow {
    pub fn new(nodes: Vec<Node>) -> Result<Self, String> {
        for (i, n) in nodes.iter().enumerate() {
            if nodes[..i].iter().any(|m| m.name == n.name) {
                return Err(format!("two nodes named `{}`", n.name));
            }
            for p in &n.parents {
                if !nodes[..i].iter().any(|m| &m.name == p) {
                    return Err(format!(
                        "`{}`: parent `{p}` must be declared before it",
                        n.name
                    ));
                }
            }
            if n.need.is_some_and(|k| k == 0 || k > n.parents.len()) {
                return Err(format!(
                    "`{}`: need must be between 1 and its number of parents",
                    n.name
                ));
            }
            if n.retry.limit == 0 {
                return Err(format!(
                    "`{}`: a retry limit counts attempts, at least 1",
                    n.name
                ));
            }
        }
        Ok(Self { nodes })
    }

    pub fn node(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.name == name)
    }

    fn children(&self, name: &str) -> impl Iterator<Item = &Node> {
        self.nodes
            .iter()
            .filter(move |n| n.parents.iter().any(|p| p == name))
    }
}

/// What a claim hands a worker.
#[derive(Clone, Debug, PartialEq)]
pub struct Lease {
    pub node: String,
    pub token: u64,
    pub items: Vec<String>,
}

/// How a worker concludes its lease.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Done,
    Skipped,
    Failed(String),
    /// For a choice node: the child it takes.
    Branch(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    UnknownNode(String),
    /// The lease was given back (expired) or concluded: nothing was written.
    StaleLease {
        node: String,
        item: String,
    },
    NotAChoice(String),
    UnknownBranch {
        node: String,
        branch: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnknownNode(n) => write!(f, "no node `{n}`"),
            Error::StaleLease { node, item } => write!(
                f,
                "`{node}` for `{item}`: the lease is no longer held, nothing written"
            ),
            Error::NotAChoice(n) => write!(f, "`{n}` is not a choice node"),
            Error::UnknownBranch { node, branch } => {
                write!(f, "`{branch}` is not a branch of `{node}`")
            }
        }
    }
}

impl std::error::Error for Error {}

/// What the journal keeps in memory of a node's rows, so watching it reads
/// no row: counts by status, and the rows that time moves on (running ones
/// for their expiry, scheduled ones for their retry).
#[derive(Default)]
struct Tally {
    counts: [usize; Status::ALL.len()],
    /// Running items: since, due.
    running: BTreeMap<String, (u64, Option<u64>)>,
    /// Scheduled items: due.
    scheduled: BTreeMap<String, Option<u64>>,
}

impl Tally {
    fn add(&mut self, item: &str, row: &Row) {
        self.counts[row.status as usize] += 1;
        match row.status {
            Status::Running => {
                self.running.insert(item.into(), (row.since, row.due));
            }
            Status::Scheduled => {
                self.scheduled.insert(item.into(), row.due);
            }
            _ => {}
        }
    }

    fn take(&mut self, item: &str, row: &Row) {
        let count = &mut self.counts[row.status as usize];
        *count = count.saturating_sub(1);
        self.running.remove(item);
        self.scheduled.remove(item);
    }
}

struct Inner<S> {
    store: S,
    /// The last token handed out.
    last_token: u64,
    /// Per node, built when the journal opens and kept by its writes.
    tallies: BTreeMap<String, Tally>,
}

impl<S: Store> Inner<S> {
    /// Runs `f` in one store transaction: its writes are kept whole or not
    /// at all. `f` checks before it writes: an error leaves nothing written.
    fn atomically<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.store.begin();
        let value = f(self);
        self.store.commit();
        value
    }

    /// Writes (or with `None` removes) the row of `item` at `node`, which
    /// was `before`, keeping the tallies.
    fn write(&mut self, item: &str, node: &str, before: Option<&Row>, after: Option<Row>) {
        let tally = self.tallies.entry(node.to_string()).or_default();
        if let Some(b) = before {
            tally.take(item, b);
        }
        match after {
            Some(row) => {
                tally.add(item, &row);
                self.store.put(item, node, row);
            }
            None => self.store.remove(item, node),
        }
    }
}

/// The journal of a workflow, over a store.
pub struct Journal<S: Store> {
    workflow: Workflow,
    inner: Mutex<Inner<S>>,
}

impl<S: Store> Journal<S> {
    /// Opens the journal over `store`, reading its rows once. Tokens start
    /// above every token the store kept or a row carries, so a journal
    /// opened again over rows a dead process left never hands out a token
    /// they hold.
    pub fn new(workflow: Workflow, store: S) -> Self {
        let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
        let mut last_token = store.last_token();
        for (item, node, row) in store.rows() {
            last_token = last_token.max(row.token.unwrap_or(0));
            tallies.entry(node).or_default().add(&item, &row);
        }
        Self {
            workflow,
            inner: Mutex::new(Inner {
                store,
                last_token,
                tallies,
            }),
        }
    }

    fn node(&self, name: &str) -> Result<&Node, Error> {
        self.workflow
            .node(name)
            .ok_or_else(|| Error::UnknownNode(name.into()))
    }

    fn joined(store: &S, item: &str, node: &Node) -> bool {
        let accepted = node
            .parents
            .iter()
            .filter(|p| store.get(item, p).is_some_and(|r| r.status.accepted()))
            .count();
        accepted >= node.need.unwrap_or(node.parents.len())
    }

    /// Whether `row`, the row of `item` at `node`, may be claimed now.
    fn claimable(store: &S, item: &str, node: &Node, row: Option<&Row>, now: u64) -> bool {
        match row {
            None => Self::joined(store, item, node),
            Some(r) => r.status == Status::Scheduled && r.due.is_none_or(|d| d <= now),
        }
    }

    /// Claims `node` for at most `max` of the `candidates` that are
    /// claimable now. The claim is atomic: two workers never get the same
    /// item. It reads the rows of its candidates (and their parents) only,
    /// and stops at the `max`-th.
    pub fn claim(
        &self,
        node: &str,
        max: usize,
        candidates: &[&str],
        now: u64,
    ) -> Result<Lease, Error> {
        let node = self.node(node)?;
        let mut inner = self.inner.lock().expect("journal lock");
        inner.last_token += 1;
        let token = inner.last_token;
        let items = inner.atomically(|inner| {
            let mut items = Vec::new();
            for item in candidates {
                if items.len() == max {
                    break;
                }
                let before = inner.store.get(item, &node.name);
                if Self::claimable(&inner.store, item, node, before.as_ref(), now) {
                    let row = Row {
                        status: Status::Running,
                        attempts: before.as_ref().map_or(0, |r| r.attempts) + 1,
                        token: Some(token),
                        since: now,
                        due: Some(now + node.lease_ms),
                        error: None,
                    };
                    inner.write(item, &node.name, before.as_ref(), Some(row));
                    items.push(item.to_string());
                }
            }
            // An empty lease marks no row: its token need not be kept.
            if !items.is_empty() {
                inner.store.set_last_token(token);
            }
            items
        });
        Ok(Lease {
            node: node.name.clone(),
            token,
            items,
        })
    }

    /// The rows `lease` holds, or the first item it no longer holds.
    fn held(inner: &Inner<S>, lease: &Lease) -> Result<Vec<Row>, Error> {
        lease
            .items
            .iter()
            .map(|item| {
                inner
                    .store
                    .get(item, &lease.node)
                    .filter(|r| r.status == Status::Running && r.token == Some(lease.token))
                    .ok_or_else(|| Error::StaleLease {
                        node: lease.node.clone(),
                        item: item.clone(),
                    })
            })
            .collect()
    }

    /// Concludes a lease for all its items. If any item is no longer held by
    /// this lease, nothing at all is written.
    pub fn conclude(&self, lease: &Lease, outcome: Outcome, now: u64) -> Result<(), Error> {
        let node = self.node(&lease.node)?;
        if let Outcome::Branch(branch) = &outcome {
            if !node.choice {
                return Err(Error::NotAChoice(node.name.clone()));
            }
            if !self
                .workflow
                .children(&node.name)
                .any(|c| &c.name == branch)
            {
                return Err(Error::UnknownBranch {
                    node: node.name.clone(),
                    branch: branch.clone(),
                });
            }
        }
        let mut inner = self.inner.lock().expect("journal lock");
        let rows = Self::held(&inner, lease)?;
        inner.atomically(|inner| {
            for (item, previous) in lease.items.iter().zip(rows) {
                let mut row = Row {
                    token: None,
                    since: now,
                    due: None,
                    ..previous.clone()
                };
                match &outcome {
                    Outcome::Done | Outcome::Branch(_) => row.status = Status::Done,
                    Outcome::Skipped => row.status = Status::Skipped,
                    Outcome::Failed(error) => {
                        row.error = Some(error.clone());
                        if row.attempts < node.retry.limit {
                            let factor = if node.retry.exponential {
                                1u64 << row.attempts.saturating_sub(1).min(32)
                            } else {
                                1
                            };
                            row.status = Status::Scheduled;
                            row.due = Some(now + node.retry.delay_ms.saturating_mul(factor));
                        } else {
                            row.status = Status::Failed;
                        }
                    }
                }
                inner.write(item, &node.name, Some(&previous), Some(row));
                if let Outcome::Branch(branch) = &outcome {
                    self.omit_other_branches(inner, item, node, branch, now);
                }
            }
        });
        Ok(())
    }

    /// The other children of a choice are omitted, then every node whose
    /// parents are all omitted: what only the other branches lead to.
    fn omit_other_branches(
        &self,
        inner: &mut Inner<S>,
        item: &str,
        choice: &Node,
        branch: &str,
        now: u64,
    ) {
        let omitted = Row {
            status: Status::Omitted,
            attempts: 0,
            token: None,
            since: now,
            due: None,
            error: None,
        };
        for child in self.workflow.children(&choice.name) {
            if child.name != branch && inner.store.get(item, &child.name).is_none() {
                inner.write(item, &child.name, None, Some(omitted.clone()));
            }
        }
        for node in &self.workflow.nodes {
            let all_omitted = !node.parents.is_empty()
                && node.parents.iter().all(|p| {
                    inner
                        .store
                        .get(item, p)
                        .is_some_and(|r| r.status == Status::Omitted)
                });
            if all_omitted && inner.store.get(item, &node.name).is_none() {
                inner.write(item, &node.name, None, Some(omitted.clone()));
            }
        }
    }

    /// Gives back every lease held past its expiry; returns how many rows.
    /// An expired lease does not count as an attempt. It reads the expired
    /// rows only.
    pub fn expire(&self, now: u64) -> usize {
        self.give_back(now, |due| due.is_some_and(|d| d <= now))
    }

    /// Gives back every running row, as `expire` does, whatever its due
    /// time; returns how many rows. Called when a process opens a journal
    /// others may have left: no lease is alive across a restart.
    pub fn recover(&self, now: u64) -> usize {
        self.give_back(now, |_| true)
    }

    /// Gives back the running rows whose due time `expired` accepts.
    fn give_back(&self, now: u64, expired: impl Fn(Option<u64>) -> bool) -> usize {
        let mut inner = self.inner.lock().expect("journal lock");
        let rows: Vec<(String, String)> = inner
            .tallies
            .iter()
            .flat_map(|(node, tally)| {
                tally
                    .running
                    .iter()
                    .filter(|(_, (_, due))| expired(*due))
                    .map(|(item, _)| (item.clone(), node.clone()))
            })
            .collect();
        if rows.is_empty() {
            return 0;
        }
        inner.atomically(|inner| {
            for (item, node) in &rows {
                let Some(row) = inner.store.get(item, node) else {
                    continue;
                };
                let back = (row.attempts > 1).then(|| Row {
                    status: Status::Scheduled,
                    attempts: row.attempts - 1,
                    token: None,
                    since: now,
                    due: Some(now),
                    ..row.clone()
                });
                inner.write(item, node, Some(&row), back);
            }
        });
        rows.len()
    }

    /// Extends a lease: every item it holds is due `lease_ms` from `now`.
    /// A worker whose job outlasts the lease renews it while it runs. If any
    /// item is no longer held, nothing is written.
    pub fn renew(&self, lease: &Lease, now: u64) -> Result<(), Error> {
        let node = self.node(&lease.node)?;
        let mut inner = self.inner.lock().expect("journal lock");
        let rows = Self::held(&inner, lease)?;
        inner.atomically(|inner| {
            for (item, previous) in lease.items.iter().zip(rows) {
                let row = Row {
                    due: Some(now + node.lease_ms),
                    ..previous.clone()
                };
                inner.write(item, &node.name, Some(&previous), Some(row));
            }
        });
        Ok(())
    }

    pub fn status(&self, item: &str, node: &str) -> Option<Status> {
        self.inner
            .lock()
            .expect("journal lock")
            .store
            .get(item, node)
            .map(|r| r.status)
    }

    pub fn row(&self, item: &str, node: &str) -> Option<Row> {
        self.inner
            .lock()
            .expect("journal lock")
            .store
            .get(item, node)
    }

    /// Per node: rows by status, the oldest running since, retries due by
    /// `now`. Read from what the journal keeps in memory: no row is read.
    pub fn monitor(&self, now: u64) -> serde_json::Value {
        let inner = self.inner.lock().expect("journal lock");
        let empty = Tally::default();
        let nodes: serde_json::Map<_, _> = self
            .workflow
            .nodes
            .iter()
            .map(|n| {
                let tally = inner.tallies.get(&n.name).unwrap_or(&empty);
                let mut out: serde_json::Map<_, _> = Status::ALL
                    .iter()
                    .map(|s| (s.name().to_string(), json!(tally.counts[*s as usize])))
                    .collect();
                let oldest_running = tally.running.values().map(|(since, _)| *since).min();
                let retries_due = tally
                    .scheduled
                    .values()
                    .filter(|due| due.is_none_or(|d| d <= now))
                    .count();
                out.insert("oldest_running_since".into(), json!(oldest_running));
                out.insert("retries_due".into(), json!(retries_due));
                (n.name.clone(), serde_json::Value::Object(out))
            })
            .collect();
        json!({ "nodes": nodes })
    }
}

impl<S: Store> Describe for Journal<S> {
    fn describe(&self) -> serde_json::Value {
        self.monitor(0)
    }
}
