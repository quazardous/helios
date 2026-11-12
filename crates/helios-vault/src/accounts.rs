//! The import connectors' accounts (`docs/design/import-connectors.md`,
//! section 2.4), kept as documents in the namespace `import/accounts`,
//! one per account id, so a second machine opening the library sees the
//! account and asks to sign in there.
//!
//! An account document holds public facts only (its connector, its name
//! as shown, its volume, its connection state, the form values that are
//! not secret): passwords and tokens live in the system keyring, never
//! here. What an account is belongs to `helios-connect`; the vault only
//! keeps the documents, each stamped with a `format` ([`upgrade`]).

use serde_json::Value;

use crate::{Store, Vault};

/// The documents' namespace.
pub const ACCOUNTS: &str = "import/accounts";

/// The format this Helios writes.
pub const ACCOUNT_FORMAT: u64 = 1;

/// A stored account document brought to [`ACCOUNT_FORMAT`]; one written
/// by a newer Helios is refused.
pub fn upgrade(key: &str, mut doc: Value) -> Result<Value, String> {
    let format = doc.get("format").and_then(Value::as_u64).unwrap_or(1);
    if format > ACCOUNT_FORMAT {
        return Err(format!(
            "account {key} has format {format}, written by a newer Helios (this one reads {ACCOUNT_FORMAT})"
        ));
    }
    if let Some(object) = doc.as_object_mut() {
        object.insert("format".into(), ACCOUNT_FORMAT.into());
    }
    Ok(doc)
}

impl<S: Store> Vault<S> {
    /// Every account document, by id, upgraded.
    pub fn account_documents(&self) -> Result<Vec<(String, Value)>, String> {
        self.transaction(|store| {
            store
                .documents(ACCOUNTS)?
                .into_iter()
                .map(|(k, d)| upgrade(&k, d).map(|d| (k, d)))
                .collect()
        })
    }

    /// Keeps (or with `None` forgets) an account's document, stamped with
    /// [`ACCOUNT_FORMAT`].
    pub fn set_account_document(&self, id: &str, doc: Option<&Value>) -> Result<(), String> {
        let doc = doc.map(|d| upgrade(id, d.clone())).transpose()?;
        self.transaction(|store| store.set_document(ACCOUNTS, id, doc.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::MemoryStore;

    #[test]
    fn account_documents_are_stamped_forgotten_and_newer_ones_refused() {
        let vault = Vault::new(MemoryStore::default());
        vault
            .set_account_document("nas-1", Some(&json!({ "connector": "nas" })))
            .unwrap();
        assert_eq!(
            vault.account_documents().unwrap(),
            [(
                "nas-1".to_string(),
                json!({ "connector": "nas", "format": 1 })
            )]
        );
        vault.set_account_document("nas-1", None).unwrap();
        assert!(vault.account_documents().unwrap().is_empty());
        assert!(upgrade("nas-2", json!({ "format": 99 })).is_err());
    }
}
