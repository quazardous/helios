//! Hierarchical keywords (`docs/design/library.md`, L12 and L-C7 A): a
//! photo's [`TAGS`] written as paths with [`SEPARATOR`] (`Animals|Dog`,
//! as darktable writes them and as imports read them). A branch is a
//! path and every keyword below it.
//!
//! [`Vault::rename_keyword`] and [`Vault::merge_keyword`] move a branch
//! in one transaction: every photo of the Library carrying a keyword of
//! it has that keyword moved, with an audit row, and the step
//! ([`Changes`]) that [`Vault::revert`] undoes. The photos are found by
//! the stores' own query ([`crate::Query::keywords`]), not tested apart.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::AuditEntry;
use crate::catalog::FieldChanged;
use crate::fields::{self, Query};
use crate::undo::{Change, Changes, rows_of};
use crate::{Store, Vault};

pub use crate::fields::TAGS;

/// The separator of a keyword's path.
pub const SEPARATOR: char = '|';

/// A keyword's path in its canonical form: each level trimmed, joined
/// with [`SEPARATOR`].
///
/// # Errors
/// An empty path, or an empty level (`Animals||Dog`, `|Dog`).
pub fn path(text: &str) -> Result<String, String> {
    let levels: Vec<&str> = text.split(SEPARATOR).map(str::trim).collect();
    if levels.iter().any(|l| l.is_empty()) {
        return Err(format!("`{text}` is not a keyword path (an empty level)"));
    }
    Ok(levels.join(&SEPARATOR.to_string()))
}

/// Whether `keyword` is in `branch`: the branch itself or below it.
pub fn in_branch(keyword: &str, branch: &str) -> bool {
    keyword
        .strip_prefix(branch)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(SEPARATOR))
}

/// `keyword` with the branch `from` moved to `to` (`Animals|Dog` from
/// `Animals` to `Pets` is `Pets|Dog`); none when it is not in `from`.
pub fn moved(keyword: &str, from: &str, to: &str) -> Option<String> {
    in_branch(keyword, from).then(|| format!("{to}{}", &keyword[from.len()..]))
}

/// The parent of a path (`Animals` for `Animals|Dog`); none at the top.
pub fn parent(path: &str) -> Option<&str> {
    path.rfind(SEPARATOR).map(|at| &path[..at])
}

/// The last level of a path (`Dog` for `Animals|Dog`).
pub fn leaf(path: &str) -> &str {
    path.rfind(SEPARATOR).map_or(path, |at| &path[at + 1..])
}

/// How many photos carry a keyword of each branch, from each photo's
/// keywords: a photo with `Animals|Dog` and `Animals|Cat` counts once
/// under `Animals`. Every level of every path is a branch.
pub fn counts<'a>(photos: impl IntoIterator<Item = &'a [String]>) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    let mut branches = std::collections::BTreeSet::new();
    for keywords in photos {
        branches.clear();
        for k in keywords {
            let mut end = 0;
            for (at, c) in k.char_indices() {
                if c == SEPARATOR {
                    branches.insert(&k[..at]);
                }
                end = at + c.len_utf8();
            }
            branches.insert(&k[..end]);
        }
        for b in &branches {
            *counts.entry((*b).to_string()).or_insert(0) += 1;
        }
    }
    counts
}

/// What a branch move did: the step, and how many photos it changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    pub changes: Changes,
    pub photos: usize,
}

impl<S: Store> Vault<S> {
    /// Renames the branch `from` to `to` (Lightroom's "Rename" on a
    /// keyword): every keyword of it moves, on every photo of the
    /// Library, in one step.
    ///
    /// # Errors
    /// A path that is not one, no photo with `from`, `to` carried already
    /// (that is a merge: [`Vault::merge_keyword`]), or `to` inside `from`.
    pub fn rename_keyword(
        &self,
        from: &str,
        to: &str,
        who: &str,
        now: u64,
    ) -> Result<Moved, String> {
        self.move_branch(from, to, false, who, now)
    }

    /// Merges the branch `from` into `into` (Lightroom's "Merge"): every
    /// keyword of `from` moves under `into`, those both have becoming one,
    /// on every photo of the Library, in one step.
    ///
    /// # Errors
    /// A path that is not one, no photo with `from` or with `into`, or
    /// `into` inside `from`.
    pub fn merge_keyword(
        &self,
        from: &str,
        into: &str,
        who: &str,
        now: u64,
    ) -> Result<Moved, String> {
        self.move_branch(from, into, true, who, now)
    }

    fn move_branch(
        &self,
        from: &str,
        to: &str,
        merge: bool,
        who: &str,
        now: u64,
    ) -> Result<Moved, String> {
        let (from, to) = (path(from)?, path(to)?);
        if in_branch(&to, &from) {
            return Err(format!(
                "“{to}” is in “{from}”: a branch cannot go inside itself"
            ));
        }
        let carrying = |store: &S, branch: &str| {
            store.select(&Query {
                keywords: vec![branch.to_string()],
                ..Query::default()
            })
        };
        let (moved_step, events) = self.transaction(|store| {
            let ids = carrying(&*store, &from)?;
            if ids.is_empty() {
                return Err(format!("no photo has the keyword “{from}”"));
            }
            let target = !carrying(&*store, &to)?.is_empty();
            if merge && !target {
                return Err(format!("no photo has the keyword “{to}”: rename instead"));
            }
            if !merge && target {
                return Err(format!(
                    "“{to}” is a keyword already: merge into it instead"
                ));
            }
            let mut images = Vec::new();
            let mut events = Vec::new();
            for id in &ids {
                let before =
                    rows_of(&*store, id)?.ok_or_else(|| format!("no image {}", id.hex()))?;
                let old = store.field(id, TAGS)?;
                let tags: Vec<Value> = old
                    .as_ref()
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|t| Value::from(moved(t, &from, &to).unwrap_or_else(|| t.to_string())))
                    .collect();
                let new = Some(fields::normalize(TAGS, &Value::Array(tags))?);
                if old == new {
                    continue;
                }
                store.set_field(id, TAGS, new.as_ref())?;
                let audit = store.add_audit(&AuditEntry {
                    seq: 0,
                    at: now,
                    who: who.into(),
                    job: None,
                    image: *id,
                    field: TAGS.into(),
                    old: old.clone(),
                    new: new.clone(),
                })?;
                events.push(FieldChanged {
                    image: id.hex(),
                    field: TAGS.into(),
                    old,
                    new,
                    who: who.into(),
                    job: None,
                    audit,
                });
                images.push(Change {
                    image: *id,
                    before: Some(before),
                    after: rows_of(&*store, id)?,
                });
            }
            let photos = images.len();
            Ok((
                Moved {
                    changes: Changes {
                        images,
                        ..Changes::default()
                    },
                    photos,
                },
                events,
            ))
        })?;
        if let Some(bus) = &self.bus {
            for event in &events {
                bus.publish("vault", None, event);
            }
        }
        if !moved_step.changes.is_empty() {
            self.catalog.lock().expect("catalog lock").version += 1;
        }
        Ok(moved_step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_branches_and_moves() {
        assert_eq!(path(" Animals | Dog ").unwrap(), "Animals|Dog");
        assert!(path("Animals||Dog").is_err());
        assert!(path("").is_err());
        assert!(in_branch("Animals|Dog", "Animals"));
        assert!(in_branch("Animals", "Animals"));
        assert!(!in_branch("Animalsx|Dog", "Animals"), "a whole level only");
        assert_eq!(moved("Animals|Dog", "Animals", "Pets").unwrap(), "Pets|Dog");
        assert_eq!(moved("Animals", "Animals", "Pets|All").unwrap(), "Pets|All");
        assert_eq!(moved("Places", "Animals", "Pets"), None);
        assert_eq!(parent("Animals|Dog"), Some("Animals"));
        assert_eq!(parent("Animals"), None);
        assert_eq!(leaf("Animals|Dog"), "Dog");
    }

    #[test]
    fn a_photo_counts_once_per_branch() {
        let a = vec!["Animals|Dog".to_string(), "Animals|Cat".to_string()];
        let b = vec!["Animals".to_string(), "Été".to_string()];
        let counts = counts([a.as_slice(), b.as_slice()]);
        assert_eq!(counts["Animals"], 2);
        assert_eq!(counts["Animals|Dog"], 1);
        assert_eq!(counts["Animals|Cat"], 1);
        assert_eq!(counts["Été"], 1);
        assert_eq!(counts.len(), 4);
    }
}
