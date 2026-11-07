//! The volumes Helios knows (`helios-storage`'s registry), kept as
//! documents in the namespace `storage/volumes`, one per volume id, so
//! they travel with the library and its snapshots and every node opening
//! the vault sees the same volumes (`docs/storage-backup-research.md`,
//! section 4.1).
//!
//! The vault only keeps the documents: what a volume is belongs to
//! `helios-storage`. Each document carries a `format`; [`upgrade`] brings
//! an older one to [`VOLUME_FORMAT`] when it is read (the documents table
//! needs no schema step), and refuses one written by a newer Helios.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Store, Vault};

/// A volume's id: a UUID, a share's address, a marker's id or a weak
/// label-and-size, prefixed by how it was found (`uuid:…`, `share:…`,
/// `marker:…`, `weak:…`). It lives here, below `helios-storage`, because
/// a [`Location`](crate::Location) names the volume it is on.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VolumeId(pub String);

impl VolumeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for VolumeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The documents' namespace.
pub const VOLUMES: &str = "storage/volumes";

/// The format this Helios writes.
pub const VOLUME_FORMAT: u64 = 1;

/// A stored volume document brought to [`VOLUME_FORMAT`]. Format 1 is
/// the first; a later one adds its step here.
pub fn upgrade(key: &str, mut doc: Value) -> Result<Value, String> {
    let format = doc.get("format").and_then(Value::as_u64).unwrap_or(1);
    if format > VOLUME_FORMAT {
        return Err(format!(
            "volume {key} has format {format}, written by a newer Helios (this one reads {VOLUME_FORMAT})"
        ));
    }
    if let Some(object) = doc.as_object_mut() {
        object.insert("format".into(), VOLUME_FORMAT.into());
    }
    Ok(doc)
}

/// The volume documents, inside one transaction (see
/// [`Vault::storage_transaction`]).
pub struct StorageTx<'a, S: Store> {
    store: &'a mut S,
}

impl<S: Store> StorageTx<'_, S> {
    /// A volume's document, upgraded.
    pub fn volume(&self, id: &str) -> Result<Option<Value>, String> {
        self.store
            .document(VOLUMES, id)?
            .map(|d| upgrade(id, d))
            .transpose()
    }

    /// Every volume document, by id, upgraded.
    pub fn volumes(&self) -> Result<Vec<(String, Value)>, String> {
        self.store
            .documents(VOLUMES)?
            .into_iter()
            .map(|(k, d)| upgrade(&k, d).map(|d| (k, d)))
            .collect()
    }

    /// Keeps (or with `None` forgets) a volume's document, stamped with
    /// [`VOLUME_FORMAT`].
    pub fn set_volume(&mut self, id: &str, doc: Option<&Value>) -> Result<(), String> {
        let doc = doc.map(|d| upgrade(id, d.clone())).transpose()?;
        self.store.set_document(VOLUMES, id, doc.as_ref())
    }

    /// A backup strategy's document by its scope's key (`library` for the
    /// library default).
    pub fn strategy(&self, key: &str) -> Result<Option<Value>, String> {
        self.store
            .document(STRATEGIES, key)?
            .map(|d| upgrade_strategy(key, d))
            .transpose()
    }

    /// Keeps (or with `None` forgets) a strategy's document, stamped with
    /// [`STRATEGY_FORMAT`].
    pub fn set_strategy(&mut self, key: &str, doc: Option<&Value>) -> Result<(), String> {
        let doc = doc.map(|d| upgrade_strategy(key, d.clone())).transpose()?;
        self.store.set_document(STRATEGIES, key, doc.as_ref())
    }

    /// The copies of one image verified by hash (an image's hex id as the
    /// key): `{"<volume id>\t<path inside it>": seconds}`.
    pub fn verified(&self, image: &str) -> Result<Option<Value>, String> {
        self.store.document(VERIFIED, image)
    }

    /// Every image's verified copies, by image.
    pub fn all_verified(&self) -> Result<Vec<(String, Value)>, String> {
        self.store.documents(VERIFIED)
    }

    /// Keeps (or with `None` forgets) an image's verified copies.
    pub fn set_verified(&mut self, image: &str, doc: Option<&Value>) -> Result<(), String> {
        self.store.set_document(VERIFIED, image, doc)
    }
}

/// The backup strategies' namespace: one document per scope (`library`
/// in v1; shoots, imports and cards later).
pub const STRATEGIES: &str = "storage/strategies";

/// The namespace of the copies verified by hash, one document per image.
pub const VERIFIED: &str = "storage/verified";

/// The strategy format this Helios writes.
pub const STRATEGY_FORMAT: u64 = 1;

/// A stored strategy document brought to [`STRATEGY_FORMAT`].
pub fn upgrade_strategy(key: &str, mut doc: Value) -> Result<Value, String> {
    let format = doc.get("format").and_then(Value::as_u64).unwrap_or(1);
    if format > STRATEGY_FORMAT {
        return Err(format!(
            "strategy {key} has format {format}, written by a newer Helios (this one reads {STRATEGY_FORMAT})"
        ));
    }
    if let Some(object) = doc.as_object_mut() {
        object.insert("format".into(), STRATEGY_FORMAT.into());
    }
    Ok(doc)
}

impl<S: Store> Vault<S> {
    /// Runs `f` over the volume documents in one transaction: kept whole
    /// when it succeeds, not at all when it fails.
    pub fn storage_transaction<T>(
        &self,
        f: impl FnOnce(&mut StorageTx<'_, S>) -> Result<T, String>,
    ) -> Result<T, String> {
        self.transaction(|store| f(&mut StorageTx { store }))
    }

    /// Every volume document, by id (see [`StorageTx::volumes`]).
    pub fn volume_documents(&self) -> Result<Vec<(String, Value)>, String> {
        self.storage_transaction(|tx| tx.volumes())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::MemoryStore;

    #[test]
    fn volume_documents_are_stamped_and_newer_ones_refused() {
        let vault = Vault::new(MemoryStore::default());
        vault
            .storage_transaction(|tx| tx.set_volume("uuid:1", Some(&json!({ "name": "A" }))))
            .unwrap();
        assert_eq!(
            vault.volume_documents().unwrap(),
            [("uuid:1".to_string(), json!({ "name": "A", "format": 1 }))]
        );
        assert!(upgrade("uuid:2", json!({ "format": 99 })).is_err());
    }
}
