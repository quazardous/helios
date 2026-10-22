//! Where rows are kept: the [`Store`] interface, the writes a transaction
//! holds until it commits ([`Pending`]), and the in-memory store.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::Row;

/// Where rows are kept. A store only stores: every rule is the journal's.
///
/// The journal runs each of its writes (a claim, a conclusion with its
/// omissions, an expiry) between [`Store::begin`] and [`Store::commit`]: a
/// store that keeps rows beyond the process keeps them whole or, when it is
/// dropped before the commit (a crash, a panic), not at all.
pub trait Store: Send {
    fn get(&self, item: &str, node: &str) -> Option<Row>;
    fn put(&mut self, item: &str, node: &str, row: Row);
    fn remove(&mut self, item: &str, node: &str);
    /// Every row, as (item, node, row).
    fn rows(&self) -> Vec<(String, String, Row)>;

    /// Starts a transaction: the writes until [`Store::commit`] are kept
    /// together, and reads in it see them. Transactions do not nest. A store
    /// without transactions keeps each write at once.
    fn begin(&mut self) {}
    fn commit(&mut self) {}

    /// The last lease token handed out over these rows (0 when none), so a
    /// journal opened again starts above it.
    fn last_token(&self) -> u64 {
        0
    }
    /// Kept with the writes of the open transaction.
    fn set_last_token(&mut self, _token: u64) {}
}

/// A row written by a transaction: (item, node, row; `None` to remove).
pub type Write = (String, String, Option<Row>);

/// The writes an open transaction holds until it commits, for a store that
/// keeps its rows elsewhere (a shared map, a vault): reads look here first.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    rows: BTreeMap<(String, String), Option<Row>>,
    last_token: Option<u64>,
}

impl Pending {
    pub fn put(&mut self, item: &str, node: &str, row: Row) {
        self.rows.insert((item.into(), node.into()), Some(row));
    }

    pub fn remove(&mut self, item: &str, node: &str) {
        self.rows.insert((item.into(), node.into()), None);
    }

    /// `Some` when the transaction wrote the row: `Some(None)` when it
    /// removed it.
    pub fn get(&self, item: &str, node: &str) -> Option<Option<Row>> {
        self.rows.get(&(item.into(), node.into())).cloned()
    }

    pub fn set_last_token(&mut self, token: u64) {
        self.last_token = Some(token);
    }

    pub fn last_token(&self) -> Option<u64> {
        self.last_token
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.last_token.is_none()
    }

    /// `kept` as it reads with these writes over it.
    pub fn over(&self, kept: Vec<(String, String, Row)>) -> Vec<(String, String, Row)> {
        let mut all: BTreeMap<(String, String), Row> =
            kept.into_iter().map(|(i, n, r)| ((i, n), r)).collect();
        for (key, row) in &self.rows {
            match row {
                Some(r) => all.insert(key.clone(), r.clone()),
                None => all.remove(key),
            };
        }
        all.into_iter().map(|((i, n), r)| (i, n, r)).collect()
    }

    /// The writes, and the last token when it was set.
    pub fn into_writes(self) -> (Vec<Write>, Option<u64>) {
        let rows = self.rows.into_iter().map(|((i, n), r)| (i, n, r)).collect();
        (rows, self.last_token)
    }
}

#[derive(Default)]
struct Kept {
    rows: BTreeMap<(String, String), Row>,
    last_token: u64,
}

/// Rows in memory: the reference other stores are checked against.
///
/// [`MemoryStore::share`] opens the same rows again, as a second process
/// opens one file: it sees what was committed, never what another handle
/// holds in an open transaction.
#[derive(Default)]
pub struct MemoryStore {
    kept: Arc<Mutex<Kept>>,
    pending: Option<Pending>,
}

impl MemoryStore {
    /// Another handle on the same committed rows.
    pub fn share(&self) -> Self {
        Self {
            kept: Arc::clone(&self.kept),
            pending: None,
        }
    }

    fn kept(&self) -> std::sync::MutexGuard<'_, Kept> {
        self.kept.lock().expect("memory store lock")
    }
}

impl Store for MemoryStore {
    fn get(&self, item: &str, node: &str) -> Option<Row> {
        if let Some(written) = self.pending.as_ref().and_then(|p| p.get(item, node)) {
            return written;
        }
        self.kept()
            .rows
            .get(&(item.to_string(), node.to_string()))
            .cloned()
    }

    fn put(&mut self, item: &str, node: &str, row: Row) {
        match &mut self.pending {
            Some(p) => p.put(item, node, row),
            None => {
                self.kept()
                    .rows
                    .insert((item.to_string(), node.to_string()), row);
            }
        }
    }

    fn remove(&mut self, item: &str, node: &str) {
        match &mut self.pending {
            Some(p) => p.remove(item, node),
            None => {
                self.kept()
                    .rows
                    .remove(&(item.to_string(), node.to_string()));
            }
        }
    }

    fn rows(&self) -> Vec<(String, String, Row)> {
        let kept: Vec<_> = self
            .kept()
            .rows
            .iter()
            .map(|((i, n), r)| (i.clone(), n.clone(), r.clone()))
            .collect();
        match &self.pending {
            Some(p) => p.over(kept),
            None => kept,
        }
    }

    fn begin(&mut self) {
        assert!(self.pending.is_none(), "a transaction is already open");
        self.pending = Some(Pending::default());
    }

    fn commit(&mut self) {
        let pending = self.pending.take().expect("no transaction to commit");
        let (rows, token) = pending.into_writes();
        let mut kept = self.kept();
        for (item, node, row) in rows {
            match row {
                Some(r) => kept.rows.insert((item, node), r),
                None => kept.rows.remove(&(item, node)),
            };
        }
        if let Some(t) = token {
            kept.last_token = t;
        }
    }

    fn last_token(&self) -> u64 {
        self.pending
            .as_ref()
            .and_then(Pending::last_token)
            .unwrap_or_else(|| self.kept().last_token)
    }

    fn set_last_token(&mut self, token: u64) {
        match &mut self.pending {
            Some(p) => p.set_last_token(token),
            None => self.kept().last_token = token,
        }
    }
}
