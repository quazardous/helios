//! The documents of the workflow level besides its journal rows: one per
//! run (`workflow/runs`) and one per item of a run (`workflow/<run>/items`),
//! kept by the node process that works the run. The vault only keeps them;
//! what they hold is the node's (see `docs/workflow-level.md`, "The store").
//!
//! Every namespace given here is under `workflow/`, so these calls reach
//! nothing else the vault keeps, except [`WorkflowTx::remove_run`], which
//! also removes the run's journal rows (`journal/<run>`).
//!
//! [`Vault::workflow_transaction`] reads and writes in one transaction,
//! which on SQLite holds the write lock from its first statement: two node
//! processes over one vault cannot both see a run free and both take it.

use serde_json::Value;

use crate::{Store, Vault};

/// The prefix of every namespace these calls reach.
const PREFIX: &str = "workflow/";

fn namespace(ns: &str) -> String {
    format!("{PREFIX}{ns}")
}

/// A document written by [`Vault::set_workflow_documents`]: namespace
/// (under `workflow/`), key, and the document, or `None` to remove it.
pub type WorkflowWrite = (String, String, Option<Value>);

impl<S: Store> Vault<S> {
    /// A workflow document, by namespace (under `workflow/`) and key.
    pub fn workflow_document(&self, ns: &str, key: &str) -> Result<Option<Value>, String> {
        self.store
            .lock()
            .expect("store lock")
            .document(&namespace(ns), key)
    }

    /// Every document of a workflow namespace (under `workflow/`), by key.
    pub fn workflow_documents(&self, ns: &str) -> Result<Vec<(String, Value)>, String> {
        self.store
            .lock()
            .expect("store lock")
            .documents(&namespace(ns))
    }

    /// Writes workflow documents in one transaction: all of them, or none.
    pub fn set_workflow_documents(&self, writes: &[WorkflowWrite]) -> Result<(), String> {
        self.transaction(|store| {
            for (ns, key, doc) in writes {
                store.set_document(&namespace(ns), key, doc.as_ref())?;
            }
            Ok(())
        })
    }
}

/// The workflow documents, inside one transaction (see
/// [`Vault::workflow_transaction`]).
pub struct WorkflowTx<'a, S: Store> {
    store: &'a mut S,
}

impl<S: Store> WorkflowTx<'_, S> {
    /// A workflow document, by namespace (under `workflow/`) and key.
    pub fn document(&self, ns: &str, key: &str) -> Result<Option<Value>, String> {
        self.store.document(&namespace(ns), key)
    }

    /// Every document of a workflow namespace (under `workflow/`), by key.
    pub fn documents(&self, ns: &str) -> Result<Vec<(String, Value)>, String> {
        self.store.documents(&namespace(ns))
    }

    /// Sets (or with `None` removes) a workflow document.
    pub fn set(&mut self, ns: &str, key: &str, doc: Option<&Value>) -> Result<(), String> {
        self.store.set_document(&namespace(ns), key, doc)
    }

    /// Removes a run whole: its documents in `runs` and `owners`, its
    /// item documents, and its journal's rows and meta.
    pub fn remove_run(&mut self, run: &str) -> Result<(), String> {
        let [rows, meta] = crate::journal::namespaces(run);
        for ns in [namespace(&format!("{run}/items")), rows, meta] {
            for (key, _) in self.store.documents(&ns)? {
                self.store.set_document(&ns, &key, None)?;
            }
        }
        self.set("runs", run, None)?;
        self.set("owners", run, None)
    }
}

impl<S: Store> Vault<S> {
    /// Runs `f` over the workflow documents in one transaction: what it
    /// wrote is kept when it succeeds, and nothing when it fails.
    pub fn workflow_transaction<T>(
        &self,
        f: impl FnOnce(&mut WorkflowTx<'_, S>) -> Result<T, String>,
    ) -> Result<T, String> {
        self.transaction(|store| f(&mut WorkflowTx { store }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use helios_journal::{Row, Status, Store as _};
    use serde_json::json;

    use crate::journal::VaultJournal;

    use crate::{MemoryStore, Vault};

    #[test]
    fn workflow_documents_round_trip_under_their_prefix() {
        let vault = Vault::new(MemoryStore::default());
        vault
            .set_workflow_documents(&[
                ("runs".into(), "r1".into(), Some(json!({ "title": "one" }))),
                ("r1/items".into(), "a".into(), Some(json!({ "order": 0 }))),
                ("r1/items".into(), "b".into(), Some(json!({ "order": 1 }))),
            ])
            .unwrap();
        assert_eq!(
            vault.workflow_document("runs", "r1").unwrap(),
            Some(json!({ "title": "one" }))
        );
        let items = vault.workflow_documents("r1/items").unwrap();
        assert_eq!(items.len(), 2);
        // Under `workflow/`: the same names elsewhere are not reached.
        assert_eq!(vault.config_layer("runs").unwrap(), None);
        vault
            .set_workflow_documents(&[("r1/items".into(), "a".into(), None)])
            .unwrap();
        assert_eq!(vault.workflow_documents("r1/items").unwrap().len(), 1);
    }

    #[test]
    fn a_transaction_keeps_all_or_nothing_and_a_run_is_removed_whole() {
        let vault = Arc::new(Vault::new(MemoryStore::default()));
        let failed: Result<(), String> = vault.workflow_transaction(|tx| {
            tx.set("runs", "r1", Some(&json!({ "title": "one" })))?;
            Err("refused".into())
        });
        assert!(failed.is_err());
        assert_eq!(vault.workflow_document("runs", "r1").unwrap(), None);

        vault
            .workflow_transaction(|tx| {
                assert_eq!(tx.document("owners", "r1")?, None);
                tx.set("runs", "r1", Some(&json!({ "title": "one" })))?;
                tx.set("owners", "r1", Some(&json!({ "instance": "a" })))?;
                tx.set("r1/items", "a", Some(&json!({ "order": 0 })))?;
                tx.set("runs", "r2", Some(&json!({ "title": "two" })))?;
                tx.set("r2/items", "a", Some(&json!({ "order": 0 })))
            })
            .unwrap();
        let mut rows = VaultJournal::new(vault.clone(), "r1");
        let done = Row {
            status: Status::Done,
            attempts: 1,
            token: None,
            since: 1,
            due: None,
            error: None,
        };
        rows.put("a", "submit", done);
        rows.set_last_token(3);
        let mut other = VaultJournal::new(vault.clone(), "r2");
        other.set_last_token(5);

        vault
            .workflow_transaction(|tx| tx.remove_run("r1"))
            .unwrap();
        assert_eq!(vault.workflow_document("runs", "r1").unwrap(), None);
        assert_eq!(vault.workflow_document("owners", "r1").unwrap(), None);
        assert!(vault.workflow_documents("r1/items").unwrap().is_empty());
        let rows = VaultJournal::new(vault.clone(), "r1");
        assert!(rows.rows().is_empty());
        assert_eq!(rows.last_token(), 0);
        // Another run is left as it was.
        assert!(vault.workflow_document("runs", "r2").unwrap().is_some());
        assert_eq!(vault.workflow_documents("r2/items").unwrap().len(), 1);
        assert_eq!(VaultJournal::new(vault, "r2").last_token(), 5);
    }
}
