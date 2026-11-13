//! What an operation changed in the catalogue, as a value an undo replays.
//!
//! An import records, for each image it touched, the image's rows before
//! it ([`ImageRows`]; an image it recorded, off the Library and with only
//! its locations, which are facts an undo keeps) and after it: its
//! Library flag, locations, fields, signals and versions. [`Vault::revert`] brings the
//! rows back to before, [`Vault::replay`] to after, each in one
//! transaction. Only catalogue rows move: no file is ever touched, so a
//! copy an import made stays on disk after an undo (a later import finds
//! it again as a duplicate, by its hash).
//!
//! Rows changed since are kept, never overwritten:
//!
//! - an image to forget (it was new) that has gained a version or a
//!   location since is left as it is, and counted as kept;
//! - a field or the signals are set back only where they still hold the
//!   value the step left; a location only where its path is still the
//!   image's (or free); a version only when no later version names it as
//!   a parent.
//!
//! A step may also change documents (the Library's collections): each is
//! set back only where it still holds what the step left.
//!
//! Audit rows stay: they are a record, not state.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::{Image, ImageId, LibraryFlag, Location, Store, Vault, Version};

/// The rows of one image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImageRows {
    pub image: Image,
    /// Whether it is in the Library.
    pub library: LibraryFlag,
    pub locations: Vec<Location>,
    /// Every field it holds, by name.
    pub fields: BTreeMap<String, Value>,
    /// Its near-duplicate signals ([`Vault::signals`]).
    pub signals: Option<Value>,
    pub versions: Vec<Version>,
}

impl ImageRows {
    /// The same rows, a location's last check left out: a file seen again
    /// is not a change.
    fn settled(&self) -> Self {
        let mut rows = self.clone();
        for l in &mut rows.locations {
            l.last_verified = 0;
        }
        rows.locations.sort_by(|a, b| a.path.cmp(&b.path));
        rows.versions.sort_by_key(|v| v.id);
        rows
    }
}

/// One image's rows before and after a step (`None`: not in the vault).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    pub image: ImageId,
    pub before: Option<ImageRows>,
    pub after: Option<ImageRows>,
}

impl Change {
    /// Whether it changed anything but when a file was last seen.
    pub fn changed(&self) -> bool {
        self.before.as_ref().map(ImageRows::settled) != self.after.as_ref().map(ImageRows::settled)
    }
}

/// A document before and after a step (`None`: not there), as the
/// Library's collections change ([`crate::collections`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DocumentChange {
    pub ns: String,
    pub key: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

/// What a step changed, image by image, and the documents it changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Changes {
    pub images: Vec<Change>,
    pub documents: Vec<DocumentChange>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.images.is_empty() && self.documents.is_empty()
    }

    /// Images the step brought into the Library (new to the vault, or
    /// known off the Library and flagged).
    pub fn added(&self) -> usize {
        let on = |r: &Option<ImageRows>| r.as_ref().is_some_and(|r| r.library.in_library);
        self.images
            .iter()
            .filter(|c| !on(&c.before) && on(&c.after))
            .count()
    }
}

/// What a revert or a replay did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Replayed {
    /// Images taken out of the vault.
    pub removed: u64,
    /// Images put back in.
    pub restored: u64,
    /// Images whose rows moved.
    pub changed: u64,
    /// Images left as they are: changed since (an edit saved, a new
    /// location) or gone.
    pub kept: u64,
}

impl<S: Store> Vault<S> {
    /// An image's rows, as [`Changes`] records them.
    pub fn rows(&self, id: &ImageId) -> Result<Option<ImageRows>, String> {
        self.read(|store| rows_of(store, id))
    }

    /// Brings the images of `changes` back to their rows before it.
    pub fn revert(&self, changes: &Changes) -> Result<Replayed, String> {
        self.walk(changes, false)
    }

    /// Brings the images of `changes` to their rows after it again.
    pub fn replay(&self, changes: &Changes) -> Result<Replayed, String> {
        self.walk(changes, true)
    }

    fn walk(&self, changes: &Changes, forward: bool) -> Result<Replayed, String> {
        let done = self.transaction(|store| {
            let mut done = Replayed::default();
            for c in &changes.images {
                let (from, to) = if forward {
                    (c.before.as_ref(), c.after.as_ref())
                } else {
                    (c.after.as_ref(), c.before.as_ref())
                };
                move_rows(store, &c.image, from, to, &mut done)?;
            }
            // A document is set back only where it still holds the step's.
            for d in &changes.documents {
                let (from, to) = if forward {
                    (&d.before, &d.after)
                } else {
                    (&d.after, &d.before)
                };
                if store.document(&d.ns, &d.key)? == *from {
                    store.set_document(&d.ns, &d.key, to.as_ref())?;
                    done.changed += 1;
                } else {
                    done.kept += 1;
                }
            }
            Ok(done)
        })?;
        if done.removed + done.restored + done.changed > 0 {
            self.placed(changes.images.iter().map(|c| c.image));
        }
        Ok(done)
    }
}

pub(crate) fn rows_of<S: Store + ?Sized>(
    store: &S,
    id: &ImageId,
) -> Result<Option<ImageRows>, String> {
    let Some(image) = store.image(id)? else {
        return Ok(None);
    };
    // The fields a command changed, and the capture fields (read from the
    // file, never audited).
    let mut names: BTreeSet<String> = store.audit(id)?.into_iter().map(|e| e.field).collect();
    names.extend(crate::capture::NAMES.iter().map(|n| (*n).to_string()));
    let mut fields = BTreeMap::new();
    for name in names {
        if let Some(v) = store.field(id, &name)? {
            fields.insert(name, v);
        }
    }
    Ok(Some(ImageRows {
        image,
        library: store.library_flag(id)?.unwrap_or_default(),
        locations: store.locations(id)?,
        fields,
        signals: store.document(crate::stacks::SIGNALS, &id.hex())?,
        versions: store.versions(id)?,
    }))
}

/// Moves one image's rows from `from` to `to`, keeping what changed since.
fn move_rows<S: Store>(
    store: &mut S,
    id: &ImageId,
    from: Option<&ImageRows>,
    to: Option<&ImageRows>,
    done: &mut Replayed,
) -> Result<(), String> {
    let now = rows_of(&*store, id)?;
    let (Some(now), Some(to)) = (now.as_ref(), to) else {
        return match (now, to) {
            // Gone since, or never there: nothing to take out.
            (None, None) => Ok(()),
            (None, Some(to)) => {
                if from.is_some() {
                    // It was there after the step and is gone since.
                    done.kept += 1;
                    return Ok(());
                }
                store.add_image(&to.image)?;
                put_rows(store, id, None, to)?;
                done.restored += 1;
                Ok(())
            }
            (Some(now), None) => forget(store, id, &now, from, done),
            (Some(_), Some(_)) => unreachable!("matched above"),
        };
    };
    if put_rows(store, id, from.or(Some(now)), to)? {
        done.changed += 1;
    }
    Ok(())
}

/// Takes an image out, unless it gained a location or a version since.
fn forget<S: Store>(
    store: &mut S,
    id: &ImageId,
    now: &ImageRows,
    from: Option<&ImageRows>,
    done: &mut Replayed,
) -> Result<(), String> {
    let known_path =
        |l: &Location| from.is_some_and(|f| f.locations.iter().any(|k| k.path == l.path));
    let known_version = |v: &Version| from.is_some_and(|f| f.versions.iter().any(|k| k.id == v.id));
    if !now.locations.iter().all(known_path) || !now.versions.iter().all(known_version) {
        done.kept += 1;
        return Ok(());
    }
    for l in &now.locations {
        store.remove_location(&l.path)?;
    }
    // Children first: a version is removed once nothing names it.
    let mut left: Vec<&Version> = now.versions.iter().collect();
    while !left.is_empty() {
        let before = left.len();
        let parents: BTreeSet<_> = left
            .iter()
            .flat_map(|v| v.parents.iter().copied())
            .collect();
        let mut next = Vec::new();
        for v in left {
            if parents.contains(&v.id) {
                next.push(v);
            } else {
                store.remove_version(&v.id)?;
            }
        }
        if next.len() == before {
            return Err("versions that name each other".into());
        }
        left = next;
    }
    store.set_document(crate::stacks::SIGNALS, &id.hex(), None)?;
    store.remove_image(id)?;
    done.removed += 1;
    Ok(())
}

/// Writes the rows of `to` that differ from `from` where the image still
/// holds `from`'s; whether anything was written.
fn put_rows<S: Store>(
    store: &mut S,
    id: &ImageId,
    from: Option<&ImageRows>,
    to: &ImageRows,
) -> Result<bool, String> {
    let mut wrote = false;
    let from_locations: Vec<&Location> = from
        .map(|f| f.locations.iter().collect())
        .unwrap_or_default();
    // Locations the step leaves behind: forgotten where still this image's.
    for l in &from_locations {
        if to.locations.iter().any(|t| t.path == l.path) {
            continue;
        }
        if store.at_path(&l.path)?.is_some_and(|(i, _)| i == *id) {
            store.remove_location(&l.path)?;
            wrote = true;
        }
    }
    for l in &to.locations {
        let was = from_locations.iter().find(|f| f.path == l.path);
        if was.is_some_and(|w| **w == *l) {
            continue;
        }
        match store.at_path(&l.path)? {
            Some((i, _)) if i != *id => {}
            _ => {
                store.put_location(id, l)?;
                wrote = true;
            }
        }
    }
    // Versions: the step's own taken out (children first), the target's
    // put back (parents first).
    let target: BTreeSet<_> = to.versions.iter().map(|v| v.id).collect();
    let mut gone: Vec<&Version> = from
        .map(|f| {
            f.versions
                .iter()
                .filter(|v| !target.contains(&v.id))
                .collect()
        })
        .unwrap_or_default();
    gone.sort_by_key(|v| std::cmp::Reverse((v.at, v.id)));
    for v in gone {
        let named = store
            .versions(id)?
            .iter()
            .any(|w| w.parents.contains(&v.id));
        if !named && store.remove_version(&v.id)? {
            wrote = true;
        }
    }
    let mut back: Vec<&Version> = to.versions.iter().collect();
    back.sort_by_key(|v| (v.at, v.id));
    for v in back {
        if store.version(&v.id)?.is_none() {
            store.put_version(v)?;
            wrote = true;
        }
    }
    // The Library flag: set back where it still holds the step's.
    let was = from.map(|f| &f.library);
    if was != Some(&to.library) {
        let now = store.library_flag(id)?.unwrap_or_default();
        if was.is_none_or(|w| *w == now) && now != to.library {
            store.set_library_flag(id, &to.library)?;
            wrote = true;
        }
    }
    // Fields and signals: set back where they still hold the step's value.
    let empty = BTreeMap::new();
    let from_fields = from.map_or(&empty, |f| &f.fields);
    let names: BTreeSet<&String> = from_fields.keys().chain(to.fields.keys()).collect();
    for name in names {
        let (was, will) = (from_fields.get(name), to.fields.get(name));
        if was == will || store.field(id, name)?.as_ref() != was {
            continue;
        }
        store.set_field(id, name, will)?;
        wrote = true;
    }
    let was = from.and_then(|f| f.signals.as_ref());
    let key = id.hex();
    if was != to.signals.as_ref() && store.document(crate::stacks::SIGNALS, &key)?.as_ref() == was {
        store.set_document(crate::stacks::SIGNALS, &key, to.signals.as_ref())?;
        wrote = true;
    }
    Ok(wrote)
}
