//! Commands and cursors: how the vault is written and read (see
//! `docs/architecture.md`, "Data access").
//!
//! - **Commands** change fields, one field of one image each. A batch is
//!   checked whole before anything is written, coalesced (the last change
//!   to a field wins), and a change equal to the stored value is skipped:
//!   no write, no audit entry, no event. Every real change is audited (who,
//!   job, old and new value) and announced on the bus.
//! - **Cursors** are server-side snapshots of a query: opening one is one
//!   store query, reading a window of rows with chosen fields is one more,
//!   and refreshing costs nothing unless the catalog changed, then returns
//!   what was inserted and removed.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;
use serde_json::{Value, json};

use helios_core::Event;

use crate::fields::{self, AuditEntry, Command, Query};
use crate::{ImageId, Store, Vault};

/// What the vault remembers between calls.
#[derive(Default)]
pub(crate) struct State {
    /// Bumped by every change that can alter what a query selects.
    pub(crate) version: u64,
    next_cursor: u64,
    cursors: HashMap<u64, Cursor>,
}

struct Cursor {
    query: Query,
    ids: Vec<ImageId>,
    version: u64,
}

impl State {
    pub(crate) fn describe(&self) -> Value {
        json!({ "version": self.version, "cursors": self.cursors.len() })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct CursorId(pub u64);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CursorInfo {
    pub id: CursorId,
    pub count: usize,
    pub version: u64,
}

/// One image of a cursor window, with the fields asked for.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Row {
    pub id: ImageId,
    pub fields: BTreeMap<String, Value>,
}

/// What changed in a cursor since it was opened or last refreshed.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Delta {
    pub changed: bool,
    pub version: u64,
    pub count: usize,
    pub inserted: Vec<ImageId>,
    pub removed: Vec<ImageId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ApplyReport {
    pub changed: u64,
    /// Equal to the stored value: nothing written.
    pub unchanged: u64,
    /// Superseded by a later command on the same field in the batch.
    pub coalesced: u64,
}

/// A field changed.
#[derive(Serialize)]
pub struct FieldChanged {
    pub image: String,
    pub field: String,
    pub old: Option<Value>,
    pub new: Option<Value>,
    pub who: String,
    pub job: Option<u64>,
    pub audit: u64,
}

impl Event for FieldChanged {
    const KIND: &'static str = "vault.field_changed";
}

impl<S: Store> Vault<S> {
    fn count_query(&self) {
        self.reads.lock().expect("reads lock").queries += 1;
    }

    /// Applies a batch of commands, all or nothing: the checks, the reads
    /// of the old values, the writes and their audit rows run in one store
    /// transaction, so a crash or a failed write leaves no part of the
    /// batch, and another process writing the same vault cannot slip in
    /// between an old value and its change. Events go out once it is
    /// committed.
    pub fn apply(
        &self,
        commands: &[Command],
        who: &str,
        job: Option<u64>,
        now: u64,
    ) -> Result<ApplyReport, String> {
        let mut report = ApplyReport::default();
        // Check the values first, and keep only the last change of each field.
        let mut last: BTreeMap<(ImageId, String), Option<Value>> = BTreeMap::new();
        for c in commands {
            let value = if let Some(v) = &c.value {
                Some(fields::normalize(&c.field, v)?)
            } else {
                fields::spec(&c.field)?;
                None
            };
            if last.insert((c.image, c.field.clone()), value).is_some() {
                report.coalesced += 1;
            }
        }
        let events = self.transaction(|store| {
            for (image, _) in last.keys() {
                if store.image(image)?.is_none() {
                    return Err(format!("no image {}", image.hex()));
                }
            }
            let mut events = Vec::new();
            for ((image, field), new) in last {
                let old = store.field(&image, &field)?;
                if old == new {
                    report.unchanged += 1;
                    continue;
                }
                store.set_field(&image, &field, new.as_ref())?;
                let entry = AuditEntry {
                    seq: 0,
                    at: now,
                    who: who.into(),
                    job,
                    image,
                    field: field.clone(),
                    old: old.clone(),
                    new: new.clone(),
                };
                let seq = store.add_audit(&entry)?;
                events.push(FieldChanged {
                    image: image.hex(),
                    field,
                    old,
                    new,
                    who: who.into(),
                    job,
                    audit: seq,
                });
                report.changed += 1;
            }
            Ok(events)
        })?;
        if let Some(bus) = &self.bus {
            for event in &events {
                bus.publish("vault", job, event);
            }
        }
        if report.changed > 0 {
            self.catalog.lock().expect("catalog lock").version += 1;
        }
        Ok(report)
    }

    /// The recorded changes of an image, oldest first.
    pub fn audit(&self, id: &ImageId) -> Result<Vec<AuditEntry>, String> {
        self.store.lock().expect("store lock").audit(id)
    }

    /// Opens a cursor on a query: one store query.
    pub fn open_cursor(&self, query: Query) -> Result<CursorInfo, String> {
        let ids = self.store.lock().expect("store lock").select(&query)?;
        self.count_query();
        let mut catalog = self.catalog.lock().expect("catalog lock");
        catalog.next_cursor += 1;
        let id = catalog.next_cursor;
        let version = catalog.version;
        let count = ids.len();
        catalog.cursors.insert(
            id,
            Cursor {
                query,
                ids,
                version,
            },
        );
        Ok(CursorInfo {
            id: CursorId(id),
            count,
            version,
        })
    }

    fn cursor_ids(
        &self,
        cursor: CursorId,
        offset: usize,
        n: usize,
    ) -> Result<Vec<ImageId>, String> {
        let catalog = self.catalog.lock().expect("catalog lock");
        let c = catalog
            .cursors
            .get(&cursor.0)
            .ok_or_else(|| format!("no cursor {}", cursor.0))?;
        Ok(c.ids.iter().skip(offset).take(n).copied().collect())
    }

    /// A window of a cursor with the asked fields: one store query, however
    /// many rows.
    pub fn fetch(
        &self,
        cursor: CursorId,
        offset: usize,
        n: usize,
        names: &[&str],
    ) -> Result<Vec<Row>, String> {
        for name in names {
            fields::spec(name)?;
        }
        let ids = self.cursor_ids(cursor, offset, n)?;
        let values = self
            .store
            .lock()
            .expect("store lock")
            .fields_of(&ids, names)?;
        self.count_query();
        Ok(ids
            .into_iter()
            .zip(values)
            .map(|(id, fields)| Row { id, fields })
            .collect())
    }

    /// The position of an image in a cursor; no store query.
    pub fn rank_of(&self, cursor: CursorId, image: &ImageId) -> Result<Option<usize>, String> {
        let catalog = self.catalog.lock().expect("catalog lock");
        let c = catalog
            .cursors
            .get(&cursor.0)
            .ok_or_else(|| format!("no cursor {}", cursor.0))?;
        Ok(c.ids.iter().position(|id| id == image))
    }

    /// Brings a cursor up to date. Free when nothing changed since; otherwise
    /// one store query, and the images inserted and removed.
    pub fn refresh(&self, cursor: CursorId) -> Result<Delta, String> {
        let (query, version) = {
            let catalog = self.catalog.lock().expect("catalog lock");
            let c = catalog
                .cursors
                .get(&cursor.0)
                .ok_or_else(|| format!("no cursor {}", cursor.0))?;
            if c.version == catalog.version {
                return Ok(Delta {
                    changed: false,
                    version: c.version,
                    count: c.ids.len(),
                    ..Delta::default()
                });
            }
            (c.query.clone(), catalog.version)
        };
        let ids = self.store.lock().expect("store lock").select(&query)?;
        self.count_query();
        let mut catalog = self.catalog.lock().expect("catalog lock");
        let c = catalog
            .cursors
            .get_mut(&cursor.0)
            .ok_or_else(|| format!("no cursor {}", cursor.0))?;
        let before: BTreeSet<ImageId> = c.ids.iter().copied().collect();
        let after: BTreeSet<ImageId> = ids.iter().copied().collect();
        let delta = Delta {
            changed: true,
            version,
            count: ids.len(),
            inserted: ids
                .iter()
                .filter(|id| !before.contains(id))
                .copied()
                .collect(),
            removed: c
                .ids
                .iter()
                .filter(|id| !after.contains(id))
                .copied()
                .collect(),
        };
        c.ids = ids;
        c.version = version;
        Ok(delta)
    }

    pub fn close_cursor(&self, cursor: CursorId) {
        self.catalog
            .lock()
            .expect("catalog lock")
            .cursors
            .remove(&cursor.0);
    }
}
