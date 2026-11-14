//! A photo turned by the user (`docs/design/library.md`, L9): the
//! [`ROTATION`] field, quarter turns clockwise on top of the file's EXIF
//! orientation, as Lightroom's and darktable's "rotate" in the library.
//!
//! [`Vault::rotate`] turns photos left, right or back to the file's
//! orientation in one transaction, with an audit row per photo, and
//! returns the step ([`Changes`]) that [`Vault::revert`] undoes. No file
//! is touched and no pixel decoded: the views turn what they show.

use crate::AuditEntry;
use crate::catalog::FieldChanged;
use crate::undo::{Change, Changes, rows_of};
use crate::{ImageId, Store, Vault};

pub use crate::fields::ROTATION;

/// How photos are turned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// A quarter turn counter-clockwise (Lightroom's "Rotate Left").
    Left,
    /// A quarter turn clockwise (Lightroom's "Rotate Right").
    Right,
    /// Back to the file's own orientation (darktable's "reset rotation").
    Reset,
}

impl Turn {
    /// `left`, `right` or `reset`.
    ///
    /// # Errors
    /// Any other word.
    pub fn parse(word: &str) -> Result<Self, String> {
        match word {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "reset" => Ok(Self::Reset),
            other => Err(format!("`turn`: `{other}` is not left, right or reset")),
        }
    }

    /// The quarter turns clockwise after this turn, from `now` (0 to 3).
    pub fn from(self, now: u8) -> u8 {
        match self {
            Self::Left => (now + 3) % 4,
            Self::Right => (now + 1) % 4,
            Self::Reset => 0,
        }
    }
}

/// A photo's quarter turns clockwise, as the field holds them (0 when
/// none).
pub fn quarter_turns(value: Option<&serde_json::Value>) -> u8 {
    value
        .and_then(serde_json::Value::as_i64)
        .map_or(0, |n| u8::try_from(n.rem_euclid(4)).unwrap_or(0))
}

impl<S: Store> Vault<S> {
    /// Turns photos, by `who` at `now`, in one transaction: each photo's
    /// [`ROTATION`] moves by `turn` (none left at 0: the field is
    /// cleared), with its audit row; a photo named twice turns once.
    /// Returns the step; photos already so (a reset of an unturned
    /// photo) are left out. An unknown image fails the whole step.
    pub fn rotate(
        &self,
        ids: &[ImageId],
        turn: Turn,
        who: &str,
        now: u64,
    ) -> Result<Changes, String> {
        let (changes, events) = self.transaction(|store| {
            let mut images = Vec::new();
            let mut events = Vec::new();
            let mut seen = std::collections::BTreeSet::new();
            for id in ids.iter().filter(|id| seen.insert(**id)) {
                let before =
                    rows_of(&*store, id)?.ok_or_else(|| format!("no image {}", id.hex()))?;
                let old = store.field(id, ROTATION)?;
                let turns = turn.from(quarter_turns(old.as_ref()));
                let new = (turns > 0).then(|| serde_json::Value::from(turns));
                if old == new {
                    continue;
                }
                store.set_field(id, ROTATION, new.as_ref())?;
                let audit = store.add_audit(&AuditEntry {
                    seq: 0,
                    at: now,
                    who: who.into(),
                    job: None,
                    image: *id,
                    field: ROTATION.into(),
                    old: old.clone(),
                    new: new.clone(),
                })?;
                events.push(FieldChanged {
                    image: id.hex(),
                    field: ROTATION.into(),
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
