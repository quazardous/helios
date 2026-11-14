//! Pair and Unpair (`docs/design/raw-jpeg-pairs.md`, sections 3 and 4):
//! two photos of one shot (a RAW and the camera's JPEG or HEIF) made one
//! photo with two files, and back.
//!
//! **Pair** keeps the RAW as the photo (its id is the RAW's content id) and
//! attaches the JPEG as its `camera` file, by the merge rules ([`merge`],
//! a pure function over both photos' rows):
//!
//! | What | Rule |
//! |---|---|
//! | Single-valued marks (rating, flag, rotation, title, description) | The RAW's when set, else the JPEG's; two set values that differ keep the RAW's and are reported ([`Conflict`]) |
//! | Set-valued marks (labels, tags) | Union, the RAW's first |
//! | Albums and the quick collection | The JPEG replaced by the RAW where the RAW is not, else taken out |
//! | Stacks | The same: the RAW keeps its stack, a JPEG's place goes to the RAW when the RAW is in none, else the JPEG leaves; a stack left with one photo is gone |
//! | Edits | The RAW's versions are the photo's; the JPEG's stay under its id, hidden with it, and come back on Unpair (counted in [`Paired::hidden_versions`]) |
//! | Library flag | The RAW's set, with the earlier date; the JPEG's cleared (`by: pair`) |
//!
//! **Unpair** makes a photo's files photos of their own again: each file
//! back in the Library with a copy of the photo's marks, put in the
//! photo's albums beside it, its own old versions shown again.
//!
//! Each runs in one transaction and answers one step ([`Changes`]) the
//! vault's [`Vault::revert`] undoes and [`Vault::replay`] redoes; every
//! row it writes is audited, on both images.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Value, json};

use crate::collections::{ALBUM, COLLECTIONS, QUICK};
use crate::fields::{FIELDS, FieldKind, PICKED, REJECTED};
use crate::files::PHOTO_FILE;
use crate::stacks::{STACKS, Stack};
use crate::undo::{Change, DocumentChange, ImageRows, rows_of};
use crate::{AuditEntry, Changes, FileRole, ImageId, LibraryFlag, PhotoFile, Store, Vault};

/// Who a JPEG's Library flag says took it out when paired.
pub const PAIR: &str = "pair";

/// The marks the user gives a photo: every field a command may change.
pub fn mark_names() -> impl Iterator<Item = &'static str> {
    FIELDS.iter().filter(|f| !f.read_only).map(|f| f.name)
}

/// What a photo brings to a pairing: its id, its marks, its Library flag
/// and how many versions it has.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Side {
    pub id: Option<ImageId>,
    /// Its marks by field name ([`mark_names`]).
    pub marks: BTreeMap<String, Value>,
    pub library: LibraryFlag,
    pub versions: usize,
}

/// A mark both photos set differently: the RAW's value is kept.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Conflict {
    /// The field, or `flag` for picked against rejected.
    pub mark: String,
    pub kept: Value,
    pub dropped: Value,
}

/// A collection (an album, the quick collection) or a stack, by its key,
/// and its members as ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Members {
    pub key: String,
    pub members: Vec<String>,
    /// A stack's top; none for a collection.
    pub top: Option<String>,
}

/// The merged photo [`merge`] computes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merged {
    /// The photo's marks after: every mark it holds.
    pub marks: BTreeMap<String, Value>,
    /// Its Library flag after.
    pub library: LibraryFlag,
    /// The collections changed, with their members after.
    pub collections: Vec<Members>,
    /// The stacks changed, with their members after (none: gone).
    pub stacks: Vec<(String, Option<Members>)>,
    pub conflicts: Vec<Conflict>,
    /// The JPEG's versions, kept hidden under its id.
    pub hidden_versions: usize,
}

/// Whether a stored value counts as no value: nothing, false, 0, an
/// empty text or set. A photo with no stars has none to keep.
fn unset(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => !b,
        Some(Value::Number(n)) => n.as_f64() == Some(0.0),
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
    }
}

/// A photo's flag: `pick`, `reject` or none.
fn flag(marks: &BTreeMap<String, Value>) -> Option<&'static str> {
    if !unset(marks.get(REJECTED)) {
        Some("reject")
    } else if !unset(marks.get(PICKED)) {
        Some("pick")
    } else {
        None
    }
}

/// The union of two sets of names, `first`'s in its order, then
/// `second`'s new ones.
fn union(first: Option<&Value>, second: Option<&Value>) -> Option<Value> {
    let mut items: Vec<Value> = Vec::new();
    for v in [first, second].into_iter().flatten() {
        for item in v.as_array().into_iter().flatten() {
            if !items.contains(item) {
                items.push(item.clone());
            }
        }
    }
    (!items.is_empty()).then_some(Value::Array(items))
}

/// The marks of the merged photo, and the conflicts: single-valued marks
/// the RAW's when set, else the JPEG's; sets united; the flag (picked,
/// rejected) as one mark.
pub fn merge_marks(
    raw: &BTreeMap<String, Value>,
    jpeg: &BTreeMap<String, Value>,
) -> (BTreeMap<String, Value>, Vec<Conflict>) {
    let mut out = BTreeMap::new();
    let mut conflicts = Vec::new();
    for spec in FIELDS.iter().filter(|f| !f.read_only) {
        let name = spec.name;
        if name == PICKED || name == REJECTED {
            continue;
        }
        let (r, j) = (raw.get(name), jpeg.get(name));
        let value = match spec.kind {
            FieldKind::Labels(_) | FieldKind::Tags => union(r, j),
            _ => {
                if !unset(r) {
                    if !unset(j) && r != j {
                        conflicts.push(Conflict {
                            mark: name.into(),
                            kept: r.cloned().unwrap_or(Value::Null),
                            dropped: j.cloned().unwrap_or(Value::Null),
                        });
                    }
                    r.cloned()
                } else if !unset(j) {
                    j.cloned()
                } else {
                    r.cloned()
                }
            }
        };
        if let Some(v) = value {
            out.insert(name.to_string(), v);
        }
    }
    // The flag: the RAW's when it has one, else the JPEG's.
    let from = match (flag(raw), flag(jpeg)) {
        (Some(r), Some(j)) => {
            if r != j {
                conflicts.push(Conflict {
                    mark: "flag".into(),
                    kept: json!(r),
                    dropped: json!(j),
                });
            }
            raw
        }
        (Some(_), None) => raw,
        (None, Some(_)) => jpeg,
        (None, None) => raw,
    };
    for name in [PICKED, REJECTED] {
        if let Some(v) = from.get(name) {
            out.insert(name.to_string(), v.clone());
        }
    }
    (out, conflicts)
}

/// `members` with `jpeg` replaced by `raw` where `raw` is not among them,
/// else taken out; none when `jpeg` is not a member.
pub fn rekey(members: &[String], raw: &str, jpeg: &str) -> Option<Vec<String>> {
    rekey_unless(members, raw, jpeg, false)
}

/// [`rekey`], the JPEG taken out also when `raw_elsewhere` (the RAW in a
/// group of the same kind already: an image is in one stack at most).
fn rekey_unless(
    members: &[String],
    raw: &str,
    jpeg: &str,
    raw_elsewhere: bool,
) -> Option<Vec<String>> {
    if !members.iter().any(|m| m == jpeg) {
        return None;
    }
    let has_raw = raw_elsewhere || members.iter().any(|m| m == raw);
    Some(
        members
            .iter()
            .filter_map(|m| {
                if m != jpeg {
                    Some(m.clone())
                } else if has_raw {
                    None
                } else {
                    Some(raw.to_string())
                }
            })
            .collect(),
    )
}

/// The merged photo of a RAW and a JPEG: its marks
/// ([`merge_marks`]), its Library flag (the RAW's, set, from the earlier
/// date), the collections and stacks of `collections` and `stacks` that
/// change ([`rekey`]; a stack left with one photo is gone, its top
/// following the RAW), the conflicts and the JPEG's versions kept
/// hidden. Pure: the vault writes what it says.
pub fn merge(raw: &Side, jpeg: &Side, collections: &[Members], stacks: &[Members]) -> Merged {
    let (marks, conflicts) = merge_marks(&raw.marks, &jpeg.marks);
    let at = match (raw.library.at, jpeg.library.at) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let library = LibraryFlag {
        in_library: true,
        at,
        by: raw.library.by.clone().or_else(|| jpeg.library.by.clone()),
    };
    let (r, j) = (
        raw.id.map(|i| i.hex()).unwrap_or_default(),
        jpeg.id.map(|i| i.hex()).unwrap_or_default(),
    );
    let collections = collections
        .iter()
        .filter_map(|c| {
            rekey(&c.members, &r, &j).map(|members| Members {
                key: c.key.clone(),
                members,
                top: None,
            })
        })
        .collect();
    let raw_stacked = stacks.iter().any(|s| s.members.contains(&r));
    let stacks = stacks
        .iter()
        .filter_map(|s| {
            let members = rekey_unless(&s.members, &r, &j, raw_stacked)?;
            if members.len() < 2 {
                return Some((s.key.clone(), None));
            }
            let top = match s.top.as_deref() {
                Some(t) if t == j => {
                    if members.contains(&r) {
                        r.clone()
                    } else {
                        members[0].clone()
                    }
                }
                Some(t) => t.to_string(),
                None => members[0].clone(),
            };
            Some((
                s.key.clone(),
                Some(Members {
                    key: s.key.clone(),
                    members,
                    top: Some(top),
                }),
            ))
        })
        .collect();
    Merged {
        marks,
        library,
        collections,
        stacks,
        conflicts,
        hidden_versions: jpeg.versions,
    }
}

/// The collections a file joins when unpaired: each holding `photo`, with
/// `file` put right after it when not there yet.
pub fn split_collections(collections: &[Members], photo: &str, file: &str) -> Vec<Members> {
    collections
        .iter()
        .filter(|c| c.members.iter().any(|m| m == photo) && !c.members.iter().any(|m| m == file))
        .map(|c| {
            let mut members = Vec::with_capacity(c.members.len() + 1);
            for m in &c.members {
                members.push(m.clone());
                if m == photo {
                    members.push(file.to_string());
                }
            }
            Members {
                key: c.key.clone(),
                members,
                top: None,
            }
        })
        .collect()
}

/// A pair made.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Paired {
    /// The photo: the RAW's id.
    pub photo: String,
    /// The JPEG, now its `camera` file.
    pub file: String,
    pub conflicts: Vec<Conflict>,
    /// The JPEG's versions, kept hidden until Unpair.
    pub hidden_versions: usize,
}

/// A file made a photo again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Unpaired {
    /// The photo it was a file of.
    pub photo: String,
    /// The file, a photo of its own now.
    pub file: String,
    /// Its own versions, shown again.
    pub versions: usize,
}

/// An image's rows with every mark it holds (a mark set before any
/// audit, by an import, is taken too, so an undo sets it back).
fn rows_with_marks<S: Store + ?Sized>(
    store: &S,
    id: &ImageId,
) -> Result<Option<ImageRows>, String> {
    let mut rows = rows_of(store, id)?;
    if let Some(rows) = &mut rows {
        for name in mark_names() {
            if let Some(v) = store.field(id, name)? {
                rows.fields.insert(name.to_string(), v);
            }
        }
    }
    Ok(rows)
}

fn side<S: Store + ?Sized>(store: &S, id: &ImageId) -> Result<Side, String> {
    let mut marks = BTreeMap::new();
    for name in mark_names() {
        if let Some(v) = store.field(id, name)? {
            marks.insert(name.to_string(), v);
        }
    }
    Ok(Side {
        id: Some(*id),
        marks,
        library: store.library_flag(id)?.unwrap_or_default(),
        versions: store.versions(id)?.len(),
    })
}

/// The plain albums and the quick collection, with their members.
fn collections<S: Store + ?Sized>(store: &S) -> Result<Vec<Members>, String> {
    let mut out = Vec::new();
    for (key, doc) in store.documents(COLLECTIONS)? {
        let kind = doc.get("kind").and_then(Value::as_str);
        if kind != Some(ALBUM) && !(key == QUICK && kind == Some(QUICK)) {
            continue;
        }
        out.push(Members {
            members: ids_of(&doc),
            key,
            top: None,
        });
    }
    Ok(out)
}

fn ids_of(doc: &Value) -> Vec<String> {
    doc.get("images")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn stacks_of<S: Store + ?Sized>(store: &S) -> Result<Vec<Members>, String> {
    Ok(crate::stacks::all(store)?
        .into_iter()
        .map(|s| Members {
            key: s.id,
            members: s.members,
            top: Some(s.top),
        })
        .collect())
}

/// The documents a step changed, by key: the first value before, the
/// last after.
#[derive(Default)]
struct Documents(Vec<DocumentChange>);

impl Documents {
    fn set<S: Store + ?Sized>(
        &mut self,
        store: &mut S,
        ns: &str,
        key: &str,
        to: Option<Value>,
    ) -> Result<(), String> {
        let before = store.document(ns, key)?;
        if before == to {
            return Ok(());
        }
        store.set_document(ns, key, to.as_ref())?;
        match self.0.iter_mut().find(|d| d.ns == ns && d.key == key) {
            Some(d) => d.after = to,
            None => self.0.push(DocumentChange {
                ns: ns.into(),
                key: key.into(),
                before,
                after: to,
            }),
        }
        Ok(())
    }

    fn collection<S: Store + ?Sized>(&mut self, store: &mut S, c: &Members) -> Result<(), String> {
        let Some(mut doc) = store.document(COLLECTIONS, &c.key)? else {
            return Ok(());
        };
        doc["images"] = json!(c.members);
        self.set(store, COLLECTIONS, &c.key, Some(doc))
    }

    fn stack<S: Store + ?Sized>(
        &mut self,
        store: &mut S,
        key: &str,
        after: Option<&Members>,
    ) -> Result<(), String> {
        let to = match after {
            None => None,
            Some(m) => {
                let Some(doc) = store.document(STACKS, key)? else {
                    return Ok(());
                };
                let mut stack: Stack =
                    serde_json::from_value(doc).map_err(|e| format!("a stored stack: {e}"))?;
                stack.members.clone_from(&m.members);
                if let Some(top) = &m.top {
                    stack.top.clone_from(top);
                }
                Some(serde_json::to_value(&stack).expect("stacks serialize"))
            }
        };
        self.set(store, STACKS, key, to)
    }
}

/// Sets `id`'s marks to `marks` (a mark it lacks cleared), each change
/// audited.
fn write_marks<S: Store + ?Sized>(
    store: &mut S,
    id: &ImageId,
    marks: &BTreeMap<String, Value>,
    who: &str,
    now: u64,
) -> Result<(), String> {
    for name in mark_names() {
        let old = store.field(id, name)?;
        let new = marks.get(name).cloned();
        if old == new {
            continue;
        }
        store.set_field(id, name, new.as_ref())?;
        audit(store, id, name, old, new, who, now)?;
    }
    Ok(())
}

fn audit<S: Store + ?Sized>(
    store: &mut S,
    image: &ImageId,
    field: &str,
    old: Option<Value>,
    new: Option<Value>,
    who: &str,
    now: u64,
) -> Result<(), String> {
    store.add_audit(&AuditEntry {
        seq: 0,
        at: now,
        who: who.into(),
        job: None,
        image: *image,
        field: field.into(),
        old,
        new,
    })?;
    Ok(())
}

fn in_library<S: Store + ?Sized>(store: &S, id: &ImageId) -> Result<bool, String> {
    Ok(store.library_flag(id)?.is_some_and(|f| f.in_library))
}

/// Why `raw` and `jpeg` cannot be paired, if they cannot.
fn refusal<S: Store + ?Sized>(
    store: &S,
    raw: &ImageId,
    jpeg: &ImageId,
) -> Result<Option<String>, String> {
    if raw == jpeg {
        return Ok(Some("a photo cannot be paired with itself".into()));
    }
    for id in [raw, jpeg] {
        if store.image(id)?.is_none() {
            return Ok(Some(format!("no image {}", id.hex())));
        }
        if let Some(row) = store.photo_of(id)? {
            return Ok(Some(format!(
                "{} is already a file of photo {}",
                id.hex(),
                row.photo.hex()
            )));
        }
        if !in_library(store, id)? {
            return Ok(Some(format!("{} is not a photo in the Library", id.hex())));
        }
    }
    if !store.photo_files(jpeg)?.is_empty() {
        return Ok(Some(format!(
            "{} has files of its own: unpair it first",
            jpeg.hex()
        )));
    }
    if store
        .photo_files(raw)?
        .iter()
        .any(|f| f.role == FileRole::Camera)
    {
        return Ok(Some(format!("{} has a camera file already", raw.hex())));
    }
    Ok(None)
}

impl<S: Store> Vault<S> {
    /// Pairs each `(raw, jpeg)`: one photo, the RAW's, with the JPEG as
    /// its `camera` file, by the merge rules ([`merge`]). All in one
    /// transaction and one step; refused as a whole when one pair is
    /// (the same image twice, an image not in the Library, a file of a
    /// photo already, a RAW with a camera file).
    pub fn pair(
        &self,
        pairs: &[(ImageId, ImageId)],
        who: &str,
        now: u64,
    ) -> Result<(Vec<Paired>, Changes), String> {
        if pairs.is_empty() {
            return Err("no pair to make".into());
        }
        let mut seen = BTreeSet::new();
        for (raw, jpeg) in pairs {
            for id in [raw, jpeg] {
                if !seen.insert(*id) {
                    return Err(format!("{} is in two pairs", id.hex()));
                }
            }
        }
        let (paired, changes) = self.transaction(|store| {
            let mut paired = Vec::new();
            let mut images: Vec<(ImageId, Option<ImageRows>)> = Vec::new();
            let mut docs = Documents::default();
            for (raw, jpeg) in pairs {
                if let Some(why) = refusal(&*store, raw, jpeg)? {
                    return Err(why);
                }
                images.push((*raw, rows_with_marks(&*store, raw)?));
                images.push((*jpeg, rows_with_marks(&*store, jpeg)?));
                let merged = merge(
                    &side(&*store, raw)?,
                    &side(&*store, jpeg)?,
                    &collections(&*store)?,
                    &stacks_of(&*store)?,
                );
                write_marks(store, raw, &merged.marks, who, now)?;
                store.set_library_flag(raw, &merged.library)?;
                store.set_library_flag(
                    jpeg,
                    &LibraryFlag {
                        in_library: false,
                        at: Some(now),
                        by: Some(PAIR.into()),
                    },
                )?;
                let row = PhotoFile {
                    image: *jpeg,
                    photo: *raw,
                    role: FileRole::Camera,
                    rank: 0,
                    added_at: now,
                    added_by: PAIR.into(),
                };
                store.put_photo_file(&row)?;
                let attached = json!({ "photo": raw.hex(), "role": "camera", "rank": 0 });
                audit(store, jpeg, PHOTO_FILE, None, Some(attached), who, now)?;
                let file = json!({ "file": jpeg.hex(), "role": "camera" });
                audit(store, raw, PHOTO_FILE, None, Some(file), who, now)?;
                for c in &merged.collections {
                    docs.collection(store, c)?;
                }
                for (key, after) in &merged.stacks {
                    docs.stack(store, key, after.as_ref())?;
                }
                paired.push(Paired {
                    photo: raw.hex(),
                    file: jpeg.hex(),
                    conflicts: merged.conflicts,
                    hidden_versions: merged.hidden_versions,
                });
            }
            let mut changes = Changes {
                documents: docs.0,
                ..Changes::default()
            };
            for (id, before) in images {
                changes.images.push(Change {
                    image: id,
                    before,
                    after: rows_with_marks(&*store, &id)?,
                });
            }
            Ok((paired, changes))
        })?;
        self.stepped(&changes);
        Ok((paired, changes))
    }

    /// Unpairs `images`: for a photo, each of its files; for a file, that
    /// file. Each file is a photo of its own again: in the Library, with
    /// a copy of the photo's marks, beside it in its albums, its own
    /// versions shown again. One transaction and one step; refused when
    /// none of `images` has a file or is one.
    pub fn unpair(
        &self,
        images: &[ImageId],
        who: &str,
        now: u64,
    ) -> Result<(Vec<Unpaired>, Changes), String> {
        let (unpaired, changes) = self.transaction(|store| {
            let mut rows: Vec<PhotoFile> = Vec::new();
            for id in images {
                if store.image(id)?.is_none() {
                    return Err(format!("no image {}", id.hex()));
                }
                let mut found = match store.photo_of(id)? {
                    Some(row) => vec![row],
                    None => store.photo_files(id)?,
                };
                PhotoFile::sort(&mut found);
                for row in found {
                    if !rows.iter().any(|r| r.image == row.image) {
                        rows.push(row);
                    }
                }
            }
            if rows.is_empty() {
                return Err("none of the photos has files to unpair".into());
            }
            let mut before: Vec<(ImageId, Option<ImageRows>)> = Vec::new();
            for row in &rows {
                for id in [row.photo, row.image] {
                    if !before.iter().any(|(i, _)| *i == id) {
                        before.push((id, rows_with_marks(&*store, &id)?));
                    }
                }
            }
            let mut docs = Documents::default();
            let mut unpaired = Vec::new();
            for row in &rows {
                let (photo, file) = (row.photo, row.image);
                store.remove_photo_file(&file)?;
                let was =
                    json!({ "photo": photo.hex(), "role": row.role.as_str(), "rank": row.rank });
                audit(store, &file, PHOTO_FILE, Some(was), None, who, now)?;
                let gone = json!({ "file": file.hex(), "role": row.role.as_str() });
                audit(store, &photo, PHOTO_FILE, Some(gone), None, who, now)?;
                store.set_library_flag(
                    &file,
                    &LibraryFlag {
                        in_library: true,
                        at: Some(now),
                        by: Some(who.into()),
                    },
                )?;
                let marks = side(&*store, &photo)?.marks;
                write_marks(store, &file, &marks, who, now)?;
                for c in split_collections(&collections(&*store)?, &photo.hex(), &file.hex()) {
                    docs.collection(store, &c)?;
                }
                unpaired.push(Unpaired {
                    photo: photo.hex(),
                    file: file.hex(),
                    versions: store.versions(&file)?.len(),
                });
            }
            let mut changes = Changes {
                documents: docs.0,
                ..Changes::default()
            };
            for (id, before) in before {
                changes.images.push(Change {
                    image: id,
                    before,
                    after: rows_with_marks(&*store, &id)?,
                });
            }
            Ok((unpaired, changes))
        })?;
        self.stepped(&changes);
        Ok((unpaired, changes))
    }

    /// Announces a pairing step: the catalog's version moves, the
    /// Library reads itself again.
    fn stepped(&self, changes: &Changes) {
        if changes.is_empty() {
            return;
        }
        self.placed(changes.images.iter().map(|c| c.image));
        self.library_changed(changes.images.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::{LABELS, RATING, ROTATION, TAGS, TITLE};

    fn marks(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    fn id(n: u8) -> ImageId {
        ImageId(helios_core::ContentHash([n; 32]))
    }

    fn side(n: u8, m: &[(&str, Value)], at: Option<u64>, versions: usize) -> Side {
        Side {
            id: Some(id(n)),
            marks: marks(m),
            library: LibraryFlag {
                in_library: true,
                at,
                by: Some(format!("import-{n}")),
            },
            versions,
        }
    }

    fn members(key: &str, ids: &[u8], top: Option<u8>) -> Members {
        Members {
            key: key.into(),
            members: ids.iter().map(|n| id(*n).hex()).collect(),
            top: top.map(|n| id(n).hex()),
        }
    }

    #[test]
    fn a_single_valued_mark_is_the_raw_s_when_set_else_the_jpeg_s() {
        let (out, conflicts) = merge_marks(
            &marks(&[(RATING, json!(0)), (TITLE, json!("Raw"))]),
            &marks(&[
                (RATING, json!(4)),
                ("description", json!("from the JPEG")),
                (ROTATION, json!(1)),
            ]),
        );
        assert_eq!(out[RATING], json!(4), "0 stars is no rating: the JPEG's");
        assert_eq!(out[TITLE], json!("Raw"));
        assert_eq!(out["description"], json!("from the JPEG"));
        assert_eq!(out[ROTATION], json!(1));
        assert!(conflicts.is_empty(), "{conflicts:?}");
    }

    #[test]
    fn two_set_values_keep_the_raw_s_and_are_reported() {
        let (out, conflicts) = merge_marks(
            &marks(&[(RATING, json!(5)), (TITLE, json!("A"))]),
            &marks(&[(RATING, json!(2)), (TITLE, json!("A"))]),
        );
        assert_eq!(out[RATING], json!(5));
        assert_eq!(
            conflicts,
            vec![Conflict {
                mark: RATING.into(),
                kept: json!(5),
                dropped: json!(2),
            }],
            "the same title on both is no conflict"
        );
    }

    #[test]
    fn the_flag_is_one_mark_picked_against_rejected_a_conflict() {
        // The RAW picked, the JPEG rejected: the pick stays, reported.
        let (out, conflicts) = merge_marks(
            &marks(&[(PICKED, json!(true))]),
            &marks(&[(REJECTED, json!(true)), (PICKED, json!(false))]),
        );
        assert_eq!(out.get(PICKED), Some(&json!(true)));
        assert_eq!(out.get(REJECTED), None, "the reject is not merged in");
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].mark, "flag");
        assert_eq!(
            (&conflicts[0].kept, &conflicts[0].dropped),
            (&json!("pick"), &json!("reject"))
        );
        // No flag on the RAW: the JPEG's whole.
        let (out, conflicts) = merge_marks(
            &marks(&[(PICKED, json!(false))]),
            &marks(&[(REJECTED, json!(true))]),
        );
        assert_eq!(out.get(REJECTED), Some(&json!(true)));
        assert!(conflicts.is_empty());
        // The same flag on both: no conflict.
        let both = marks(&[(REJECTED, json!(true))]);
        assert!(merge_marks(&both, &both).1.is_empty());
    }

    #[test]
    fn labels_and_tags_are_united_the_raw_s_first() {
        let (out, conflicts) = merge_marks(
            &marks(&[(LABELS, json!(["red"])), (TAGS, json!(["sea", "boat"]))]),
            &marks(&[
                (LABELS, json!(["blue", "red"])),
                (TAGS, json!(["boat", "sky"])),
            ]),
        );
        assert_eq!(out[LABELS], json!(["red", "blue"]));
        assert_eq!(out[TAGS], json!(["sea", "boat", "sky"]));
        assert!(conflicts.is_empty(), "sets never conflict");
        let (out, _) = merge_marks(&marks(&[]), &marks(&[]));
        assert!(out.is_empty(), "nothing on either: nothing on the photo");
    }

    #[test]
    fn rekeying_replaces_the_jpeg_or_takes_it_out() {
        let (r, j, o) = (id(1).hex(), id(2).hex(), id(3).hex());
        assert_eq!(
            rekey(&[o.clone(), j.clone()], &r, &j),
            Some(vec![o.clone(), r.clone()]),
            "the JPEG's place goes to the RAW"
        );
        assert_eq!(
            rekey(&[r.clone(), o.clone(), j.clone()], &r, &j),
            Some(vec![r.clone(), o.clone()]),
            "the RAW there already: the JPEG leaves"
        );
        assert_eq!(rekey(&[r.clone(), o], &r, &j), None, "no JPEG: no change");
    }

    #[test]
    fn the_merge_takes_the_earlier_date_albums_stacks_and_counts_hidden_versions() {
        let raw = side(1, &[(RATING, json!(3))], Some(200), 1);
        let jpeg = side(2, &[(RATING, json!(1))], Some(100), 3);
        let albums = [
            members("trip", &[2, 9], None),
            members("both", &[1, 2], None),
            members("other", &[9], None),
        ];
        let stacks = [
            // The JPEG on top of a burst: the RAW takes its place and top.
            members("burst", &[2, 7, 8], Some(2)),
        ];
        let m = merge(&raw, &jpeg, &albums, &stacks);
        assert_eq!(m.marks[RATING], json!(3));
        assert_eq!(m.conflicts.len(), 1);
        assert_eq!(m.hidden_versions, 3);
        assert_eq!(
            m.library,
            LibraryFlag {
                in_library: true,
                at: Some(100),
                by: Some("import-1".into()),
            }
        );
        assert_eq!(
            m.collections,
            vec![members("trip", &[1, 9], None), members("both", &[1], None)]
        );
        assert_eq!(
            m.stacks,
            vec![("burst".into(), Some(members("burst", &[1, 7, 8], Some(1))))]
        );
    }

    #[test]
    fn a_stack_of_the_raw_and_its_jpeg_is_gone_and_another_s_jpeg_leaves() {
        let raw = side(1, &[], None, 1);
        let jpeg = side(2, &[], None, 1);
        // Stacked together (the old import's way): the stack goes.
        let m = merge(&raw, &jpeg, &[], &[members("pair", &[1, 2], Some(1))]);
        assert_eq!(m.stacks, vec![("pair".into(), None)]);
        // The RAW in its own stack, the JPEG in another: the JPEG leaves
        // its, whose top moves to its first member.
        let m = merge(
            &raw,
            &jpeg,
            &[],
            &[
                members("raws", &[1, 5], Some(5)),
                members("jpegs", &[2, 6, 7], Some(2)),
            ],
        );
        assert_eq!(
            m.stacks,
            vec![("jpegs".into(), Some(members("jpegs", &[6, 7], Some(6))))]
        );
        // The RAW in its stack only: unchanged.
        let m = merge(&raw, &jpeg, &[], &[members("raws", &[1, 5], Some(1))]);
        assert!(m.stacks.is_empty());
        // No date on either: none.
        assert_eq!(m.library.at, None);
    }

    #[test]
    fn unpairing_puts_the_file_beside_its_photo_in_its_albums() {
        let albums = [
            members("trip", &[9, 1, 8], None),
            members("has", &[1, 2], None),
            members("other", &[9], None),
        ];
        assert_eq!(
            split_collections(&albums, &id(1).hex(), &id(2).hex()),
            vec![members("trip", &[9, 1, 2, 8], None)]
        );
    }
}
