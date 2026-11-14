//! A photo's copies (`docs/design/library.md`, L10): its heads, which one
//! is current, and removing one.
//!
//! The current version is the head an edit opens: the head the photo's
//! [`CURRENT_VERSION`] field names (darktable's duplicate manager and
//! Lightroom's virtual copies let the user pick it), else its newest head.
//! Once the picked head is continued (an edit saved on it), it is no head
//! any more and the newest one, the continuation, is current again.
//!
//! [`Vault::switch_version`] and [`Vault::remove_head`] change the
//! catalogue in one transaction, with an audit row, and return the step
//! ([`Changes`]) that [`Vault::revert`] undoes: a copy removed comes back
//! with its whole line.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::Value;

use crate::AuditEntry;
use crate::catalog::FieldChanged;
use crate::undo::{Change, Changes, rows_of};
use crate::versions::{Version, VersionId, depths, heads};
use crate::{ImageId, Store, Vault};

/// The field naming the head the user picked as a photo's current
/// version (its id, in hex); none when the newest head is current.
pub const CURRENT_VERSION: &str = "current_version";

/// The head an edit opens among `versions` (an image's): the one `picked`
/// names when it is still a head, else the newest (the latest time, then
/// the longest history, then the greatest id).
///
/// # Errors
/// A parent missing from `versions`.
pub fn current<'a>(
    versions: &'a [Version],
    picked: Option<&Value>,
) -> Result<Option<&'a Version>, String> {
    let all = heads(versions);
    let picked = picked
        .and_then(Value::as_str)
        .and_then(|hex| VersionId::from_hex(hex).ok());
    if let Some(head) = picked.and_then(|p| all.iter().find(|v| v.id == p)) {
        return Ok(Some(head));
    }
    let depths = depths(versions)?;
    Ok(all.into_iter().max_by_key(|v| (v.at, depths[&v.id], v.id)))
}

/// One head of a photo, as the versions strip shows it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Head {
    pub version: VersionId,
    /// Its place among the photo's heads, oldest first, from 1.
    pub number: usize,
    pub label: Option<String>,
    pub at: u64,
    /// The head an edit opens.
    pub current: bool,
}

/// The versions only `head` leads to: its line back to the first version
/// another head also continues. Removing them leaves the other heads
/// whole.
fn own_line(versions: &[Version], head: &VersionId) -> Vec<VersionId> {
    let by_id: HashMap<VersionId, &Version> = versions.iter().map(|v| (v.id, v)).collect();
    let ancestry = |from: &VersionId| {
        let mut seen = HashSet::new();
        let mut todo = vec![*from];
        while let Some(id) = todo.pop() {
            if seen.insert(id)
                && let Some(v) = by_id.get(&id)
            {
                todo.extend(v.parents.iter().copied());
            }
        }
        seen
    };
    let others: HashSet<VersionId> = heads(versions)
        .into_iter()
        .filter(|v| v.id != *head)
        .flat_map(|v| ancestry(&v.id))
        .collect();
    ancestry(head)
        .into_iter()
        .filter(|id| !others.contains(id))
        .collect()
}

impl<S: Store> Vault<S> {
    /// A photo's current version: the head an edit opens ([`current`]).
    pub fn current_head(&self, image: &ImageId) -> Result<Option<Version>, String> {
        let all = self.versions(image)?;
        let picked = self.read(|s| s.field(image, CURRENT_VERSION))?;
        Ok(current(&all, picked.as_ref())?.cloned())
    }

    /// A photo's heads, oldest first, numbered, the current one marked.
    pub fn heads_of(&self, image: &ImageId) -> Result<Vec<Head>, String> {
        if self.image(image)?.is_none() {
            return Err(format!("no image {}", image.hex()));
        }
        let all = self.versions(image)?;
        let picked = self.read(|s| s.field(image, CURRENT_VERSION))?;
        let now = current(&all, picked.as_ref())?.map(|v| v.id);
        Ok(heads(&all)
            .into_iter()
            .enumerate()
            .map(|(i, v)| Head {
                version: v.id,
                number: i + 1,
                label: v.label.clone(),
                at: v.at,
                current: Some(v.id) == now,
            })
            .collect())
    }

    /// Makes `version`, one of the photo's heads, its current version,
    /// by `who` at `now`. Returns the step (empty when it already was).
    pub fn switch_version(
        &self,
        image: &ImageId,
        version: &VersionId,
        who: &str,
        now: u64,
    ) -> Result<Changes, String> {
        self.change_heads(image, who, now, |_, all| {
            if !heads(all).iter().any(|v| v.id == *version) {
                return Err(format!(
                    "{} is not a head of {}",
                    version.hex(),
                    image.hex()
                ));
            }
            Ok(Some(Value::from(version.hex())))
        })
        .map(|(changes, _)| changes)
    }

    /// Removes a copy: `version`, one of the photo's heads, with the
    /// versions only it leads to. The photo's last head is refused (a
    /// photo keeps one version). Returns the step and how many versions
    /// went.
    pub fn remove_head(
        &self,
        image: &ImageId,
        version: &VersionId,
        who: &str,
        now: u64,
    ) -> Result<(Changes, usize), String> {
        self.change_heads(image, who, now, |store, all| {
            let all_heads = heads(all);
            if !all_heads.iter().any(|v| v.id == *version) {
                return Err(format!(
                    "{} is not a head of {}",
                    version.hex(),
                    image.hex()
                ));
            }
            if all_heads.len() < 2 {
                return Err("the photo's only version cannot be removed".into());
            }
            // Children first: a version goes once nothing names it.
            let by_id: HashMap<VersionId, &Version> = all.iter().map(|v| (v.id, v)).collect();
            let mut left: BTreeSet<VersionId> = own_line(all, version).into_iter().collect();
            while !left.is_empty() {
                let named: HashSet<VersionId> = left
                    .iter()
                    .filter_map(|id| by_id.get(id))
                    .flat_map(|v| v.parents.iter().copied())
                    .collect();
                let free: Vec<VersionId> = left
                    .iter()
                    .filter(|id| !named.contains(id))
                    .copied()
                    .collect();
                if free.is_empty() {
                    return Err("versions that name each other".into());
                }
                for id in free {
                    store.remove_version(&id)?;
                    left.remove(&id);
                }
            }
            // The pick goes with the copy it named.
            let picked = store.field(image, CURRENT_VERSION)?;
            let keep = picked.filter(|p| p.as_str() != Some(version.hex().as_str()));
            Ok(keep)
        })
    }

    /// Runs `step` on a photo's versions in one transaction; it returns
    /// the [`CURRENT_VERSION`] value to leave, set with its audit row when
    /// it changes. Returns the step and how many versions went.
    fn change_heads(
        &self,
        image: &ImageId,
        who: &str,
        now: u64,
        step: impl FnOnce(&mut S, &[Version]) -> Result<Option<Value>, String>,
    ) -> Result<(Changes, usize), String> {
        let (changes, removed, event) = self.transaction(|store| {
            let before =
                rows_of(&*store, image)?.ok_or_else(|| format!("no image {}", image.hex()))?;
            let all = before.versions.clone();
            let old = store.field(image, CURRENT_VERSION)?;
            let new = step(store, &all)?;
            let mut event = None;
            if old != new {
                store.set_field(image, CURRENT_VERSION, new.as_ref())?;
                let audit = store.add_audit(&AuditEntry {
                    seq: 0,
                    at: now,
                    who: who.into(),
                    job: None,
                    image: *image,
                    field: CURRENT_VERSION.into(),
                    old: old.clone(),
                    new: new.clone(),
                })?;
                event = Some(FieldChanged {
                    image: image.hex(),
                    field: CURRENT_VERSION.into(),
                    old,
                    new,
                    who: who.into(),
                    job: None,
                    audit,
                });
            }
            let after = rows_of(&*store, image)?;
            let removed = all.len() - after.as_ref().map_or(0, |a| a.versions.len());
            let change = Change {
                image: *image,
                before: Some(before),
                after,
            };
            let images = if change.changed() {
                vec![change]
            } else {
                Vec::new()
            };
            Ok((
                Changes {
                    images,
                    ..Changes::default()
                },
                removed,
                event,
            ))
        })?;
        if let (Some(bus), Some(event)) = (&self.bus, &event) {
            bus.publish("vault", None, event);
        }
        if !changes.is_empty() {
            self.catalog.lock().expect("catalog lock").version += 1;
            if removed > 0 {
                self.library_changed(1);
            }
        }
        Ok((changes, removed))
    }
}
