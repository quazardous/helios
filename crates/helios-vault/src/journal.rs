//! A workflow journal kept in the vault: its rows are documents of a
//! namespace of their own (`journal/<name>`), so a journal outlives the
//! process that runs the workflow, on whatever driver keeps the vault.
//!
//! The journal's storage interface cannot fail (`helios_journal::Store`):
//! a vault that cannot read or write stops the workflow with a panic that
//! names what failed, rather than letting it run on rows it did not keep.
//!
//! A journal transaction holds its writes in memory and applies them in one
//! vault transaction when it commits: a process that dies before the commit
//! leaves the vault as it was. The last token handed out is a document of
//! `journal/<name>/meta`, written in the same transaction.

use std::sync::Arc;

use helios_journal::{Pending, Row};

use crate::{Store, Vault};

/// Separates an item from a node in a document key.
const SEP: char = '\u{1f}';

/// The key of the last token handed out, in the meta namespace.
const TOKEN: &str = "last_token";

/// The namespaces of the journal named `name`: its rows, then its meta.
pub(crate) fn namespaces(name: &str) -> [String; 2] {
    [format!("journal/{name}"), format!("journal/{name}/meta")]
}

pub struct VaultJournal<S: Store> {
    vault: Arc<Vault<S>>,
    ns: String,
    meta: String,
    /// The writes of the open transaction.
    pending: Option<Pending>,
}

impl<S: Store> VaultJournal<S> {
    /// The journal named `name` in `vault`.
    pub fn new(vault: Arc<Vault<S>>, name: &str) -> Self {
        let [ns, meta] = namespaces(name);
        VaultJournal {
            vault,
            ns,
            meta,
            pending: None,
        }
    }

    fn key(item: &str, node: &str) -> String {
        format!("{item}{SEP}{node}")
    }

    fn store(&self) -> std::sync::MutexGuard<'_, S> {
        self.vault.store.lock().expect("store lock")
    }
}

impl<S: Store> helios_journal::Store for VaultJournal<S> {
    fn get(&self, item: &str, node: &str) -> Option<Row> {
        if let Some(written) = self.pending.as_ref().and_then(|p| p.get(item, node)) {
            return written;
        }
        let doc = self
            .store()
            .document(&self.ns, &Self::key(item, node))
            .unwrap_or_else(|e| panic!("journal {}: {e}", self.ns));
        doc.map(|d| {
            serde_json::from_value(d).unwrap_or_else(|e| panic!("journal {}: {e}", self.ns))
        })
    }

    fn put(&mut self, item: &str, node: &str, row: Row) {
        if let Some(p) = &mut self.pending {
            p.put(item, node, row);
            return;
        }
        let doc = serde_json::to_value(&row).expect("rows serialize");
        self.store()
            .set_document(&self.ns, &Self::key(item, node), Some(&doc))
            .unwrap_or_else(|e| panic!("journal {}: {e}", self.ns));
    }

    fn remove(&mut self, item: &str, node: &str) {
        if let Some(p) = &mut self.pending {
            p.remove(item, node);
            return;
        }
        self.store()
            .set_document(&self.ns, &Self::key(item, node), None)
            .unwrap_or_else(|e| panic!("journal {}: {e}", self.ns));
    }

    fn rows(&self) -> Vec<(String, String, Row)> {
        let kept = self
            .store()
            .documents(&self.ns)
            .unwrap_or_else(|e| panic!("journal {}: {e}", self.ns))
            .into_iter()
            .map(|(key, doc)| {
                let (item, node) = key
                    .split_once(SEP)
                    .unwrap_or_else(|| panic!("journal {}: a key without a node", self.ns));
                let row = serde_json::from_value(doc)
                    .unwrap_or_else(|e| panic!("journal {}: {e}", self.ns));
                (item.to_string(), node.to_string(), row)
            })
            .collect();
        match &self.pending {
            Some(p) => p.over(kept),
            None => kept,
        }
    }

    fn begin(&mut self) {
        assert!(
            self.pending.is_none(),
            "journal {}: a transaction is already open",
            self.ns
        );
        self.pending = Some(Pending::default());
    }

    fn commit(&mut self) {
        let pending = self
            .pending
            .take()
            .unwrap_or_else(|| panic!("journal {}: no transaction to commit", self.ns));
        if pending.is_empty() {
            return;
        }
        let (rows, token) = pending.into_writes();
        self.vault
            .transaction(|store| {
                for (item, node, row) in &rows {
                    let doc = row
                        .as_ref()
                        .map(|r| serde_json::to_value(r).expect("rows serialize"));
                    store.set_document(&self.ns, &Self::key(item, node), doc.as_ref())?;
                }
                if let Some(t) = token {
                    store.set_document(&self.meta, TOKEN, Some(&serde_json::json!(t)))?;
                }
                Ok(())
            })
            .unwrap_or_else(|e| panic!("journal {}: {e}", self.ns));
    }

    fn last_token(&self) -> u64 {
        if let Some(t) = self.pending.as_ref().and_then(Pending::last_token) {
            return t;
        }
        self.store()
            .document(&self.meta, TOKEN)
            .unwrap_or_else(|e| panic!("journal {}: {e}", self.ns))
            .map_or(0, |d| {
                d.as_u64()
                    .unwrap_or_else(|| panic!("journal {}: a token that is not a number", self.ns))
            })
    }

    fn set_last_token(&mut self, token: u64) {
        if let Some(p) = &mut self.pending {
            p.set_last_token(token);
            return;
        }
        self.store()
            .set_document(&self.meta, TOKEN, Some(&serde_json::json!(token)))
            .unwrap_or_else(|e| panic!("journal {}: {e}", self.ns));
    }
}
