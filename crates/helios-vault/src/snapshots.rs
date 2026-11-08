//! Named states of an image's edit: its history's snapshots.
//!
//! A snapshot is taken on a surface (the desktop's History panel, an
//! agent) without saving anything; the surface pins it to the version
//! its next save leaves as the image's edit, and only then keeps it here.
//! A snapshot never pinned (the surface quit or was killed first) is not
//! in the vault, and is lost with the surface.
//!
//! The snapshots of an image are one document of the namespace
//! [`NAMESPACE`], keyed by the image's id:
//!
//! ```json
//! { "snapshots": [{ "id": 1, "name": "Before", "origin": "user", "at": 1730700000,
//!                   "version": "<version id>" }] }
//! ```
//!
//! A snapshot of an image never edited is pinned to no version
//! (`"version": null`): the image as it is with no edit.

use serde_json::{Value, json};

use crate::{ImageId, Store, Vault, VersionId};

/// The namespace of the snapshots' documents.
pub const NAMESPACE: &str = "snapshots";

/// A snapshot as the vault keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Unique among the image's snapshots; never 0.
    pub id: u64,
    pub name: String,
    /// Who took it: `user`, `agent:<id>`…
    pub origin: String,
    /// When it was taken, in seconds since the epoch.
    pub at: u64,
    /// The version it is pinned to; none: the image with no edit.
    pub version: Option<VersionId>,
}

impl Snapshot {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "origin": self.origin,
            "at": self.at,
            "version": self.version.map(|v| v.hex()),
        })
    }

    fn from_json(v: &Value) -> Result<Self, String> {
        let id = v["id"]
            .as_u64()
            .filter(|id| *id != 0)
            .ok_or_else(|| format!("a snapshot without an id: {v}"))?;
        Ok(Self {
            id,
            name: v["name"].as_str().unwrap_or_default().to_string(),
            origin: v["origin"].as_str().unwrap_or_default().to_string(),
            at: v["at"].as_u64().unwrap_or(0),
            version: match v["version"].as_str() {
                Some(hex) => Some(VersionId::from_hex(hex)?),
                None => None,
            },
        })
    }
}

impl<S: Store> Vault<S> {
    /// An image's snapshots, in the order they were kept (oldest first).
    pub fn snapshots(&self, image: &ImageId) -> Result<Vec<Snapshot>, String> {
        let doc = self
            .store
            .lock()
            .expect("store lock")
            .document(NAMESPACE, &image.hex())?;
        doc.as_ref()
            .and_then(|d| d["snapshots"].as_array())
            .into_iter()
            .flatten()
            .map(Snapshot::from_json)
            .collect()
    }

    /// Keeps an image's snapshots, replacing those it had (none: the
    /// document goes), in one transaction. Each must be pinned to a
    /// version of that image, and ids are unique.
    pub fn set_snapshots(&self, image: &ImageId, snapshots: &[Snapshot]) -> Result<(), String> {
        let mut ids: Vec<u64> = snapshots.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        if ids.windows(2).any(|w| w[0] == w[1]) || ids.first() == Some(&0) {
            return Err("snapshots need unique ids, never 0".into());
        }
        self.transaction(|store| {
            if store.image(image)?.is_none() {
                return Err(format!("no image {}", image.hex()));
            }
            for s in snapshots {
                let Some(v) = &s.version else { continue };
                match store.version(v)? {
                    Some(version) if version.image == *image => {}
                    Some(_) => return Err(format!("version {} is of another image", v.hex())),
                    None => return Err(format!("no version {}", v.hex())),
                }
            }
            let doc = (!snapshots.is_empty()).then(|| {
                json!({ "snapshots": snapshots.iter().map(Snapshot::to_json).collect::<Vec<_>>() })
            });
            store.set_document(NAMESPACE, &image.hex(), doc.as_ref())
        })
    }
}

#[cfg(test)]
mod tests {
    use helios_core::ContentHash;

    use super::*;
    use crate::{Image, MemoryStore, Version};

    fn vault() -> (Vault<MemoryStore>, ImageId, ImageId) {
        let vault = Vault::new(MemoryStore::default());
        let (a, b) = (ImageId(ContentHash([1; 32])), ImageId(ContentHash([2; 32])));
        for id in [a, b] {
            vault
                .transaction(|s| {
                    s.add_to_library(&Image {
                        id,
                        size: 1,
                        partial: id.0,
                    })
                })
                .unwrap();
        }
        (vault, a, b)
    }

    fn snap(id: u64, version: Option<VersionId>) -> Snapshot {
        Snapshot {
            id,
            name: format!("Snapshot {id}"),
            origin: "user".into(),
            at: 10,
            version,
        }
    }

    #[test]
    fn snapshots_round_trip_per_image_and_go_when_emptied() {
        let (vault, a, b) = vault();
        let v = Version::new(a, Vec::new(), json!({ "modules": [] }), "user", 1);
        vault.add_version(&v).unwrap();
        assert!(vault.snapshots(&a).unwrap().is_empty());
        let kept = [snap(1, None), snap(3, Some(v.id))];
        vault.set_snapshots(&a, &kept).unwrap();
        assert_eq!(vault.snapshots(&a).unwrap(), kept);
        assert!(vault.snapshots(&b).unwrap().is_empty());
        vault.set_snapshots(&a, &[]).unwrap();
        assert!(vault.snapshots(&a).unwrap().is_empty());
    }

    #[test]
    fn a_snapshot_is_pinned_to_a_version_of_its_own_image() {
        let (vault, a, b) = vault();
        let v = Version::new(a, Vec::new(), json!({ "modules": [] }), "user", 1);
        vault.add_version(&v).unwrap();
        let err = vault.set_snapshots(&b, &[snap(1, Some(v.id))]).unwrap_err();
        assert!(err.contains("another image"), "{err}");
        let other = Version::new(a, Vec::new(), json!({ "other": 1 }), "user", 1);
        let err = vault
            .set_snapshots(&a, &[snap(1, Some(other.id))])
            .unwrap_err();
        assert!(err.contains("no version"), "{err}");
        let err = vault
            .set_snapshots(&a, &[snap(2, None), snap(2, None)])
            .unwrap_err();
        assert!(err.contains("unique"), "{err}");
        assert!(vault.snapshots(&a).unwrap().is_empty(), "nothing kept");
    }
}
