//! A group edit of photos' metadata (`docs/design/library.md`, L13): the
//! fields a metadata panel or an agent sets on a selection (title,
//! description, creator, copyright, rating, labels, flag) and the
//! keywords added or taken off, as **one step**.
//!
//! [`Vault::set_meta`] applies the commands in one transaction, with an
//! audit row per field changed, and returns the step ([`Changes`]) that
//! [`Vault::revert`] undoes and [`Vault::replay`] does again. A field a
//! command does not name keeps each photo's own value: that is how a
//! "‹mixed›" field left untouched stays mixed.
//!
//! Metadata templates (Photo Mechanic's stamps) are a library document
//! ([`TEMPLATES_NS`], [`TEMPLATES_KEY`]): named sets of field values and
//! keywords, applied through the same group edit.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::catalog::FieldChanged;
use crate::fields::{self, Command, Op};
use crate::undo::{Change, Changes, rows_of};
use crate::{AuditEntry, ImageId, Store, Vault};

pub use crate::fields::{COPYRIGHT, CREATOR, DESCRIPTION, TITLE};

/// The library document namespace of the metadata templates.
pub const TEMPLATES_NS: &str = "meta";
/// Their document's key: `{ "templates": [{ name, fields, keywords }] }`.
pub const TEMPLATES_KEY: &str = "templates";

/// The text fields a group edit sets, in the panel's order.
pub const TEXT_FIELDS: [&str; 4] = [TITLE, DESCRIPTION, CREATOR, COPYRIGHT];

impl<S: Store> Vault<S> {
    /// Applies `commands` (fields set, names added or removed, over any
    /// number of photos) by `who` at `now`, in one transaction: each
    /// field changed gets its audit row, and the whole is one step,
    /// returned for an undo. Photos a command leaves as they were are
    /// left out of the step. A bad command or an unknown image fails it
    /// all, nothing written.
    pub fn set_meta(&self, commands: &[Command], who: &str, now: u64) -> Result<Changes, String> {
        let mut ops: BTreeMap<ImageId, BTreeMap<String, Vec<Op>>> = BTreeMap::new();
        for c in commands {
            let op = c.checked()?;
            ops.entry(c.image)
                .or_default()
                .entry(c.field.clone())
                .or_default()
                .push(op);
        }
        let (changes, events) = self.transaction(|store| {
            let mut images = Vec::new();
            let mut events = Vec::new();
            for (id, by_field) in &ops {
                let mut before =
                    rows_of(&*store, id)?.ok_or_else(|| format!("no image {}", id.hex()))?;
                let mut changed = false;
                for (field, list) in by_field {
                    let old = store.field(id, field)?;
                    // A value no command wrote (an import's) has no audit
                    // row yet: the step keeps it all the same.
                    if let Some(old) = &old {
                        before.fields.insert(field.clone(), old.clone());
                    }
                    let new = fields::fold(field, old.clone(), list)?;
                    if old == new {
                        continue;
                    }
                    store.set_field(id, field, new.as_ref())?;
                    let audit = store.add_audit(&AuditEntry {
                        seq: 0,
                        at: now,
                        who: who.into(),
                        job: None,
                        image: *id,
                        field: field.clone(),
                        old: old.clone(),
                        new: new.clone(),
                    })?;
                    events.push(FieldChanged {
                        image: id.hex(),
                        field: field.clone(),
                        old,
                        new,
                        who: who.into(),
                        job: None,
                        audit,
                    });
                    changed = true;
                }
                if changed {
                    images.push(Change {
                        image: *id,
                        before: Some(before),
                        after: rows_of(&*store, id)?,
                    });
                }
            }
            Ok((
                Changes {
                    images,
                    ..Changes::default()
                },
                events,
            ))
        })?;
        if let Some(bus) = &self.bus {
            for event in &events {
                bus.publish("vault", None, event);
            }
        }
        if !changes.is_empty() {
            self.catalog.lock().expect("catalog lock").version += 1;
        }
        Ok(changes)
    }
}

/// The commands of a group edit over `images`, from the node's params:
/// `fields` (a value per field: `null` clears it; `flag` as `pick`,
/// `reject` or `none`), keywords (`add`, `remove`: lists of `|` paths; a
/// keyword removed takes those below it too) and colour labels added or
/// removed one by one ([`Labels`]).
///
/// `tags_of` gives a photo's keywords now, for the branches removed.
///
/// # Errors
/// An unknown or read-only field, a value it does not take, a bad flag
/// or keyword.
pub fn commands(
    images: &[ImageId],
    fields_set: &serde_json::Map<String, Value>,
    add: &[String],
    remove: &[String],
    labels: &Labels,
    tags_of: &dyn Fn(&ImageId) -> Vec<String>,
) -> Result<Vec<Command>, String> {
    let add: Vec<String> = add
        .iter()
        .map(|k| crate::keywords::path(k))
        .collect::<Result<_, _>>()?;
    let remove: Vec<String> = remove
        .iter()
        .map(|k| crate::keywords::path(k))
        .collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for id in images.iter().filter(|id| seen.insert(**id)) {
        for (name, value) in fields_set {
            if name == "flag" {
                let (picked, rejected) = match value.as_str() {
                    Some("pick") => (true, false),
                    Some("reject") => (false, true),
                    Some("none") => (false, false),
                    None if value.is_null() => (false, false),
                    _ => return Err(format!("`flag`: {value} is not pick, reject or none")),
                };
                out.push(Command::set(*id, fields::PICKED, Value::from(picked)));
                out.push(Command::set(*id, fields::REJECTED, Value::from(rejected)));
                continue;
            }
            let spec = fields::spec(name)?;
            if spec.read_only {
                return Err(format!(
                    "`{name}` is read from the file: no command changes it"
                ));
            }
            // An empty text is no text.
            let empty = value.is_null() || value.as_str().is_some_and(|t| t.trim().is_empty());
            out.push(if empty {
                Command::clear(*id, name)
            } else {
                Command::set(*id, name, value.clone())
            });
        }
        if !remove.is_empty() {
            for tag in tags_of(id) {
                if remove.iter().any(|r| crate::keywords::in_branch(&tag, r)) {
                    out.push(Command::remove(*id, fields::TAGS, &tag));
                }
            }
        }
        for k in &add {
            out.push(Command::add(*id, fields::TAGS, k));
        }
        for l in &labels.add {
            out.push(Command::add(*id, fields::LABELS, l));
        }
        for l in &labels.remove {
            out.push(Command::remove(*id, fields::LABELS, l));
        }
    }
    Ok(out)
}

/// Colour labels a group edit adds or takes off, one by one, each
/// photo's others kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Labels {
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_edit_sets_named_fields_only() {
        let a = ImageId(helios_core::ContentHash([1; 32]));
        let b = ImageId(helios_core::ContentHash([2; 32]));
        let mut set = serde_json::Map::new();
        set.insert(CREATOR.into(), Value::from("Ann"));
        set.insert(TITLE.into(), Value::from("  "));
        set.insert("flag".into(), Value::from("pick"));
        let tags = |id: &ImageId| {
            if *id == a {
                vec!["Animals|Dog".to_string(), "Places".to_string()]
            } else {
                vec![]
            }
        };
        let c = commands(
            &[a, b, a],
            &set,
            &["Trips|2026".into()],
            &["Animals".into()],
            &Labels {
                add: vec!["red".into()],
                remove: vec![],
            },
            &tags,
        )
        .unwrap();
        let of_a: Vec<_> = c.iter().filter(|c| c.image == a).collect();
        assert!(of_a.iter().any(|c| c.field == CREATOR));
        assert!(
            of_a.iter()
                .any(|c| c.field == TITLE && c.op == Op::Set(None)),
            "an empty text clears the field"
        );
        assert!(of_a.iter().any(|c| c.field == fields::PICKED));
        assert!(
            of_a.iter()
                .any(|c| c.op == Op::Remove("Animals|Dog".into()))
        );
        assert!(!of_a.iter().any(|c| c.op == Op::Remove("Places".into())));
        assert!(of_a.iter().any(|c| c.op == Op::Add("Trips|2026".into())));
        assert!(
            of_a.iter()
                .any(|c| c.field == fields::LABELS && c.op == Op::Add("red".into()))
        );
        assert!(
            !c.iter()
                .any(|c| c.image == b && matches!(c.op, Op::Remove(_)))
        );
        assert_eq!(c.iter().filter(|c| c.image == a).count(), of_a.len());
        assert!(
            commands(
                &[a],
                &[("camera_model".to_string(), Value::from("X"))]
                    .into_iter()
                    .collect(),
                &[],
                &[],
                &Labels::default(),
                &tags
            )
            .is_err(),
            "a field read from the file is refused"
        );
    }
}
