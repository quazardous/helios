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
//! - **leases** expire: `expire` gives back what a dead worker held.
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

/// Where rows are kept. A store only stores: every rule is the journal's.
pub trait Store: Send {
    fn get(&self, item: &str, node: &str) -> Option<Row>;
    fn put(&mut self, item: &str, node: &str, row: Row);
    fn remove(&mut self, item: &str, node: &str);
    /// Every row, as (item, node, row).
    fn rows(&self) -> Vec<(String, String, Row)>;
}

/// Rows in memory: the reference other stores are checked against.
#[derive(Default)]
pub struct MemoryStore(BTreeMap<(String, String), Row>);

impl Store for MemoryStore {
    fn get(&self, item: &str, node: &str) -> Option<Row> {
        self.0.get(&(item.to_string(), node.to_string())).cloned()
    }

    fn put(&mut self, item: &str, node: &str, row: Row) {
        self.0.insert((item.to_string(), node.to_string()), row);
    }

    fn remove(&mut self, item: &str, node: &str) {
        self.0.remove(&(item.to_string(), node.to_string()));
    }

    fn rows(&self) -> Vec<(String, String, Row)> {
        self.0
            .iter()
            .map(|((i, n), r)| (i.clone(), n.clone(), r.clone()))
            .collect()
    }
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

struct Inner<S> {
    store: S,
    next_token: u64,
}

/// The journal of a workflow, over a store.
pub struct Journal<S: Store> {
    workflow: Workflow,
    inner: Mutex<Inner<S>>,
}

impl<S: Store> Journal<S> {
    pub fn new(workflow: Workflow, store: S) -> Self {
        Self {
            workflow,
            inner: Mutex::new(Inner {
                store,
                next_token: 0,
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

    fn claimable(store: &S, item: &str, node: &Node, now: u64) -> bool {
        match store.get(item, &node.name) {
            None => Self::joined(store, item, node),
            Some(r) => r.status == Status::Scheduled && r.due.is_none_or(|d| d <= now),
        }
    }

    /// Claims `node` for at most `max` of the `candidates` that are
    /// claimable now. The claim is atomic: two workers never get the same item.
    pub fn claim(
        &self,
        node: &str,
        max: usize,
        candidates: &[&str],
        now: u64,
    ) -> Result<Lease, Error> {
        let node = self.node(node)?;
        let mut inner = self.inner.lock().expect("journal lock");
        inner.next_token += 1;
        let token = inner.next_token;
        let mut items = Vec::new();
        for item in candidates {
            if items.len() == max {
                break;
            }
            if Self::claimable(&inner.store, item, node, now) {
                let attempts = inner.store.get(item, &node.name).map_or(0, |r| r.attempts);
                let row = Row {
                    status: Status::Running,
                    attempts: attempts + 1,
                    token: Some(token),
                    since: now,
                    due: Some(now + node.lease_ms),
                    error: None,
                };
                inner.store.put(item, &node.name, row);
                items.push(item.to_string());
            }
        }
        Ok(Lease {
            node: node.name.clone(),
            token,
            items,
        })
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
        for item in &lease.items {
            let held = inner
                .store
                .get(item, &node.name)
                .is_some_and(|r| r.status == Status::Running && r.token == Some(lease.token));
            if !held {
                return Err(Error::StaleLease {
                    node: node.name.clone(),
                    item: item.clone(),
                });
            }
        }
        for item in &lease.items {
            let previous = inner.store.get(item, &node.name).expect("checked above");
            let mut row = Row {
                token: None,
                since: now,
                due: None,
                ..previous
            };
            match &outcome {
                Outcome::Done | Outcome::Branch(_) => row.status = Status::Done,
                Outcome::Skipped => row.status = Status::Skipped,
                Outcome::Failed(error) => {
                    row.error = Some(error.clone());
                    if row.attempts < node.retry.limit {
                        let factor = if node.retry.exponential {
                            1u64 << (row.attempts - 1).min(32)
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
            inner.store.put(item, &node.name, row);
            if let Outcome::Branch(branch) = &outcome {
                self.omit_other_branches(&mut inner.store, item, node, branch, now);
            }
        }
        Ok(())
    }

    /// The other children of a choice are omitted, then every node whose
    /// parents are all omitted: what only the other branches lead to.
    fn omit_other_branches(
        &self,
        store: &mut S,
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
            if child.name != branch && store.get(item, &child.name).is_none() {
                store.put(item, &child.name, omitted.clone());
            }
        }
        for node in &self.workflow.nodes {
            let all_omitted = !node.parents.is_empty()
                && node.parents.iter().all(|p| {
                    store
                        .get(item, p)
                        .is_some_and(|r| r.status == Status::Omitted)
                });
            if all_omitted && store.get(item, &node.name).is_none() {
                store.put(item, &node.name, omitted.clone());
            }
        }
    }

    /// Gives back every lease held past its expiry; returns how many rows.
    /// An expired lease does not count as an attempt.
    pub fn expire(&self, now: u64) -> usize {
        let mut inner = self.inner.lock().expect("journal lock");
        let expired: Vec<_> = inner
            .store
            .rows()
            .into_iter()
            .filter(|(_, _, r)| r.status == Status::Running && r.due.is_some_and(|d| d <= now))
            .collect();
        for (item, node, row) in &expired {
            if row.attempts <= 1 {
                inner.store.remove(item, node);
            } else {
                let back = Row {
                    status: Status::Scheduled,
                    attempts: row.attempts - 1,
                    token: None,
                    since: now,
                    due: Some(now),
                    ..row.clone()
                };
                inner.store.put(item, node, back);
            }
        }
        expired.len()
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

    /// Per node: rows by status, the oldest running since, retries due by `now`.
    pub fn monitor(&self, now: u64) -> serde_json::Value {
        let inner = self.inner.lock().expect("journal lock");
        let rows = inner.store.rows();
        let nodes: serde_json::Map<_, _> = self
            .workflow
            .nodes
            .iter()
            .map(|n| {
                let mine: Vec<&Row> = rows
                    .iter()
                    .filter(|(_, node, _)| node == &n.name)
                    .map(|(_, _, r)| r)
                    .collect();
                let count = |s: Status| mine.iter().filter(|r| r.status == s).count();
                let oldest_running = mine
                    .iter()
                    .filter(|r| r.status == Status::Running)
                    .map(|r| r.since)
                    .min();
                let retries_due = mine
                    .iter()
                    .filter(|r| r.status == Status::Scheduled && r.due.is_none_or(|d| d <= now))
                    .count();
                (
                    n.name.clone(),
                    json!({
                        "running": count(Status::Running), "scheduled": count(Status::Scheduled),
                        "done": count(Status::Done), "skipped": count(Status::Skipped),
                        "failed": count(Status::Failed), "omitted": count(Status::Omitted),
                        "oldest_running_since": oldest_running, "retries_due": retries_due,
                    }),
                )
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
