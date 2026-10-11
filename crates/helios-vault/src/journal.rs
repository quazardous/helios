//! A workflow journal kept in the vault: its rows are documents of a
//! namespace of their own (`journal/<name>`), so a journal outlives the
//! process that runs the workflow, on whatever driver keeps the vault.
//!
//! The journal's storage interface cannot fail (`helios_journal::Store`):
//! a vault that cannot read or write stops the workflow with a panic that
//! names what failed, rather than letting it run on rows it did not keep.

use std::sync::Arc;

use helios_journal::Row;

use crate::{Store, Vault};

/// Separates an item from a node in a document key.
const SEP: char = '\u{1f}';

pub struct VaultJournal<S: Store> {
    vault: Arc<Vault<S>>,
    ns: String,
}

impl<S: Store> VaultJournal<S> {
    /// The journal named `name` in `vault`.
    pub fn new(vault: Arc<Vault<S>>, name: &str) -> Self {
        VaultJournal {
            vault,
            ns: format!("journal/{name}"),
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
        let doc = self
            .store()
            .document(&self.ns, &Self::key(item, node))
            .unwrap_or_else(|e| panic!("journal {}: {e}", self.ns));
        doc.map(|d| {
            serde_json::from_value(d).unwrap_or_else(|e| panic!("journal {}: {e}", self.ns))
        })
    }

    fn put(&mut self, item: &str, node: &str, row: Row) {
        let doc = serde_json::to_value(&row).expect("rows serialize");
        self.store()
            .set_document(&self.ns, &Self::key(item, node), Some(&doc))
            .unwrap_or_else(|e| panic!("journal {}: {e}", self.ns));
    }

    fn remove(&mut self, item: &str, node: &str) {
        self.store()
            .set_document(&self.ns, &Self::key(item, node), None)
            .unwrap_or_else(|e| panic!("journal {}: {e}", self.ns));
    }

    fn rows(&self) -> Vec<(String, String, Row)> {
        self.store()
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
            .collect()
    }
}
