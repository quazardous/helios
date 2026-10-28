//! The documents of the workflow level besides its journal rows: one per
//! run (`workflow/runs`) and one per item of a run (`workflow/<run>/items`),
//! kept by the node process that works the run. The vault only keeps them;
//! what they hold is the node's (see `docs/workflow-level.md`, "The store").
//!
//! Every namespace given here is under `workflow/`, so these calls reach
//! nothing else the vault keeps.

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

#[cfg(test)]
mod tests {
    use serde_json::json;

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
}
