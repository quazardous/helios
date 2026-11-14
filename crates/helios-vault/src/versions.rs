//! Edits as versions (ROADMAP section 2.3): an image's edits form a graph
//! of versions, each pointing at the versions it continues. Two people (or
//! a person and an agent) editing at once make two versions with the same
//! parent: both are kept, as two heads, and nothing is overwritten. A
//! version with two parents merges them; which values it keeps is the
//! decision of whoever merges, not the vault's.
//!
//! A version's id is the BLAKE3 of its image, its parents and its edit in
//! canonical JSON (keys sorted): the same edit on the same parents is the
//! same version, so writing it twice changes nothing. Who made it, when
//! and its label are not part of the id.

use std::collections::{HashMap, HashSet};

use helios_core::ContentHash;
use serde::Serialize;
use serde_json::Value;

use crate::{ImageId, parse_hash};

/// A version, identified by its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct VersionId(pub ContentHash);

impl VersionId {
    pub fn hex(&self) -> String {
        self.0.to_string()
    }

    pub fn from_hex(hex: &str) -> Result<Self, String> {
        parse_hash(hex).map(VersionId)
    }
}

/// One version of an image's edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Version {
    pub id: VersionId,
    pub image: ImageId,
    /// The versions this one continues: none for a first edit, two or more
    /// for a merge. Sorted, so a merge's id does not depend on their order.
    pub parents: Vec<VersionId>,
    /// The edit: a network of named parameters, decision answers included
    /// (the shape of a `helios-node` network description).
    pub edit: Value,
    /// Who made it: `user`, `agent:<session>`, `import:xmp`…
    pub origin: String,
    pub at: u64,
    pub label: Option<String>,
    /// What else the step that made it carries, not hashed: the workshop
    /// session it was made in (`{"workshop_session": "<id>"}`), so a
    /// session can be rebuilt from the versions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

impl Version {
    /// A version, its id computed from its content.
    pub fn new(
        image: ImageId,
        mut parents: Vec<VersionId>,
        edit: Value,
        origin: &str,
        at: u64,
    ) -> Self {
        parents.sort();
        parents.dedup();
        let mut h = blake3::Hasher::new();
        h.update(b"helios.version.v1");
        h.update(&image.0.0);
        h.update(&(parents.len() as u64).to_le_bytes());
        for p in &parents {
            h.update(&p.0.0);
        }
        // serde_json's maps are sorted by key: this is canonical.
        h.update(edit.to_string().as_bytes());
        Version {
            id: VersionId(ContentHash(*h.finalize().as_bytes())),
            image,
            parents,
            edit,
            origin: origin.into(),
            at,
            label: None,
            meta: None,
        }
    }

    pub fn labelled(mut self, label: &str) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The version with its metadata (not part of its id).
    pub fn with_meta(mut self, meta: Option<Value>) -> Self {
        self.meta = meta;
        self
    }
}

/// The versions no other version continues: what an image's edit is now.
/// More than one head means edits diverged and wait for a merge (or not:
/// variants are allowed to live side by side). Oldest first.
pub fn heads(versions: &[Version]) -> Vec<&Version> {
    // One pass over the parents, not one per version: an image's history
    // grows with every save.
    let continued: HashSet<&VersionId> = versions.iter().flat_map(|v| &v.parents).collect();
    let mut out: Vec<&Version> = versions
        .iter()
        .filter(|v| !continued.contains(&v.id))
        .collect();
    out.sort_by_key(|v| (v.at, v.id));
    out
}

/// How long the line leading to each of `versions` is (what
/// [`crate::Vault::history`] returns for it, counted): each version's
/// first parent, back to a first edit. Worked out from the versions given,
/// which hold every parent of an image's versions, so no store read is
/// made per version; an error names a parent missing from them.
pub fn depths(versions: &[Version]) -> Result<HashMap<VersionId, usize>, String> {
    let by_id: HashMap<VersionId, &Version> = versions.iter().map(|v| (v.id, v)).collect();
    let mut depths: HashMap<VersionId, usize> = HashMap::with_capacity(versions.len());
    for version in versions {
        // Down the line to a version already counted (or a first edit),
        // then back up it, each counted once.
        let mut line = Vec::new();
        let mut next = Some(version.id);
        let mut below = 0;
        while let Some(id) = next {
            if let Some(&depth) = depths.get(&id) {
                below = depth;
                break;
            }
            let v = by_id
                .get(&id)
                .ok_or_else(|| format!("no version {}", id.hex()))?;
            line.push(id);
            next = v.parents.first().copied();
        }
        for id in line.into_iter().rev() {
            below += 1;
            depths.insert(id, below);
        }
    }
    Ok(depths)
}

/// A version was kept.
#[derive(Serialize)]
pub struct VersionAdded {
    pub image: String,
    pub version: String,
    pub parents: Vec<String>,
    pub origin: String,
    /// How many heads the image has now: more than one means edits diverged.
    pub heads: usize,
}

impl helios_core::Event for VersionAdded {
    const KIND: &'static str = "vault.version_added";
}

impl<S: crate::Store> crate::Vault<S> {
    /// Keeps a version of an image's edit. Its image must be known and its
    /// parents must be versions of that image. Returns false when the same
    /// version was already kept: nothing changes, nothing is announced.
    pub fn add_version(&self, version: &Version) -> Result<bool, String> {
        // The checks and the write in one transaction: another process
        // cannot remove what was checked before it is written.
        let added = self.transaction(|store| {
            if store.image(&version.image)?.is_none() {
                return Err(format!("no image {}", version.image.hex()));
            }
            for p in &version.parents {
                match store.version(p)? {
                    Some(parent) if parent.image == version.image => {}
                    Some(_) => return Err(format!("version {} is of another image", p.hex())),
                    None => return Err(format!("no version {}", p.hex())),
                }
            }
            store.put_version(version)
        })?;
        if !added {
            return Ok(false);
        }
        if let Some(bus) = &self.bus {
            let all = self.read(|s| s.versions(&version.image))?;
            bus.publish(
                "vault",
                None,
                &VersionAdded {
                    image: version.image.hex(),
                    version: version.id.hex(),
                    parents: version.parents.iter().map(VersionId::hex).collect(),
                    origin: version.origin.clone(),
                    heads: heads(&all).len(),
                },
            );
        }
        Ok(true)
    }

    pub fn version(&self, id: &VersionId) -> Result<Option<Version>, String> {
        self.read(|s| s.version(id))
    }

    /// An image's versions, oldest first.
    pub fn versions(&self, image: &ImageId) -> Result<Vec<Version>, String> {
        let mut all = self.read(|s| s.versions(image))?;
        all.sort_by_key(|v| (v.at, v.id));
        Ok(all)
    }

    /// The line of versions that leads to `head`, oldest first: each
    /// version's first parent, back to a first edit. Times are in seconds
    /// and steps come faster: the line, not the time, orders a history.
    pub fn history(&self, head: &VersionId) -> Result<Vec<Version>, String> {
        self.read(|store| {
            let mut line = Vec::new();
            let mut next = Some(*head);
            while let Some(id) = next {
                let version = store
                    .version(&id)?
                    .ok_or_else(|| format!("no version {}", id.hex()))?;
                next = version.parents.first().copied();
                line.push(version);
            }
            line.reverse();
            Ok(line)
        })
    }

    /// An image's heads: its current edit, or its diverged edits.
    pub fn heads(&self, image: &ImageId) -> Result<Vec<Version>, String> {
        let all = self.versions(image)?;
        Ok(heads(&all).into_iter().cloned().collect())
    }
}
