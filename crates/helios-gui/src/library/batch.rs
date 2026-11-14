//! The Library's selection and batch commands (`docs/design/library.md`,
//! L7): select none, invert, select the photos similar to the selected
//! ones (the same capture day, the same stack), and the marks of one
//! photo (rating, colour labels, tags, title) copied and pasted onto
//! others, merged or overwriting, as darktable's metadata copy.
//!
//! The selection is the focus's (each window its own); the copied marks
//! too. A paste writes as the marks do: rating, labels and tags through
//! egui-pop's library steps (a plugin may veto them), the title as a
//! field, all queued for the host's `library.fields.set`.
//!
//! "Remove from Library" (L8) takes the selection out of the Library:
//! the flag only, no file touched, through the node's
//! `library.images.remove`; the host records the step it answers in the
//! library's history, so Ctrl+Z puts the photos back.
//!
//! "Rotate Left" and "Rotate Right" (L9, Ctrl+[ and Ctrl+], as Lightroom
//! and darktable) and "Reset Rotation" (darktable's) turn the selection
//! through the node's `library.images.rotate`: the photo's `rotation`
//! field, one step of the library's history; the views turn the
//! thumbnails they hold, no picture decoded again.
//!
//! "Move to Trash…" and "Delete rejected…" (L8) send photos' files to the
//! system trash: asked first (the node plans the job, the host asks with
//! the count and the place), then run by the node's
//! `library.images.trash` as a journaled job. Not a step of the library's
//! history: the system trash is their undo.

use egui_pop::views::library::{Change as Mark, ColorLabel};
use serde_json::{Value, json};

use super::{LABELS, Library, Op};
use helios_vault::fields::TITLE;
pub use helios_vault::rotation::Turn;

/// Selects nothing, the cursor gone too (darktable's Ctrl+Shift+A).
pub const SELECT_NONE_COMMAND: &str = "library.select.none";
/// Selects the photos shown that are not selected, and only them
/// (darktable's Ctrl+I).
pub const SELECT_INVERT_COMMAND: &str = "library.select.invert";
/// Selects the photos shown like the selected ones (`by`: `day`, the
/// same capture day, or `stack`, the same stack).
pub const SELECT_SIMILAR_COMMAND: &str = "library.select.similar";
/// Copies a photo's marks (`image`, else the photo under the cursor).
pub const MARKS_COPY_COMMAND: &str = "library.marks.copy";
/// Pastes the copied marks on photos (`images`, else the selection;
/// `marks`, `mode`).
pub const MARKS_PASTE_COMMAND: &str = "library.marks.paste";

/// Takes photos out of the Library (`images`, else the selection), their
/// files untouched (Lightroom's "Remove from Catalog", darktable's
/// "remove"): one undoable step.
pub const REMOVE_COMMAND: &str = "library.remove";

/// Turns photos a quarter counter-clockwise (`images`, else the
/// selection; Lightroom's "Rotate Left", Ctrl+[).
pub const ROTATE_LEFT_COMMAND: &str = "library.rotateLeft";

/// Turns photos a quarter clockwise (`images`, else the selection;
/// Lightroom's "Rotate Right", Ctrl+]).
pub const ROTATE_RIGHT_COMMAND: &str = "library.rotateRight";

/// Turns photos back to their file's orientation (`images`, else the
/// selection; darktable's "reset rotation").
pub const RESET_ROTATION_COMMAND: &str = "library.resetRotation";

/// Makes a new version of photos from their current edit (`images`,
/// else the selection; Lightroom's "Create Virtual Copy", Ctrl+',
/// darktable's "duplicate", Ctrl+D).
pub const DUPLICATE_COMMAND: &str = "library.duplicate";

/// Makes a new version of photos with no edit (`images`, else the
/// selection; darktable's "duplicate virgin", Ctrl+Shift+D).
pub const DUPLICATE_ORIGINAL_COMMAND: &str = "library.duplicateOriginal";

/// Moves photos' files to the system trash (`images`, else the
/// selection), after a question unless `confirmed` (Lightroom's "Remove
/// and Delete from Disk", darktable's "delete (trash)").
pub const TRASH_COMMAND: &str = "library.trash";

/// Moves the rejected photos shown to the system trash, after a question
/// unless `confirmed` (`images`, the rejected photos the question named;
/// Lightroom's "Delete Rejected Photos").
pub const DELETE_REJECTED_COMMAND: &str = "library.deleteRejected";

/// Photos whose files the user asked moved to the trash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashAsk {
    pub images: Vec<String>,
    /// Answered yes already: run it; else ask first.
    pub confirmed: bool,
    /// Asked by "Delete rejected…".
    pub rejected: bool,
}

/// The marks a copy holds and a paste may take.
pub const MARK_KINDS: [&str; 4] = ["rating", "labels", "tags", "title"];

/// What makes photos similar for [`SELECT_SIMILAR_COMMAND`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Similar {
    /// Taken the same day (by the camera's clock).
    Day,
    /// In the same stack.
    Stack,
}

impl Similar {
    /// `day` or `stack`.
    ///
    /// # Errors
    /// Any other word.
    pub fn parse(by: &str) -> Result<Self, String> {
        match by {
            "day" => Ok(Self::Day),
            "stack" => Ok(Self::Stack),
            other => Err(format!("`by`: `{other}` is not day or stack")),
        }
    }
}

/// One photo's marks, as copied.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CopiedMarks {
    /// The photo they were copied from.
    pub from: String,
    pub rating: i64,
    pub labels: Vec<String>,
    pub tags: Vec<String>,
    pub title: Option<String>,
}

impl CopiedMarks {
    /// As a command answers it.
    pub fn to_json(&self) -> Value {
        json!({
            "from": self.from,
            "rating": self.rating,
            "labels": self.labels,
            "tags": self.tags,
            "title": self.title,
        })
    }
}

/// How a paste meets the marks a photo has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteMode {
    /// Labels and tags added to the photo's; the rating and title set
    /// where the copy has one (no stars, no title: left as they are).
    Merge,
    /// Each mark pasted becomes the copy's: labels and tags it lacks
    /// taken off, no stars or no title written as such.
    Overwrite,
}

impl PasteMode {
    /// `merge` or `overwrite`.
    ///
    /// # Errors
    /// Any other word.
    pub fn parse(mode: &str) -> Result<Self, String> {
        match mode {
            "merge" => Ok(Self::Merge),
            "overwrite" => Ok(Self::Overwrite),
            other => Err(format!("`mode`: `{other}` is not merge or overwrite")),
        }
    }
}

impl Library {
    /// Selects nothing: no photo selected, no cursor.
    pub fn select_none(&mut self) {
        self.state.set_selection(Vec::<String>::new(), None);
        self.selected = None;
    }

    /// Selects the photos shown that were not selected; the cursor goes
    /// to the first of them. How many are selected.
    pub fn invert_selection(&mut self) -> usize {
        let selected: std::collections::HashSet<String> =
            self.state.selection().keys().iter().cloned().collect();
        let keys: Vec<String> = self
            .order
            .iter()
            .map(|&i| self.entries[i].id.clone())
            .filter(|id| !selected.contains(id))
            .collect();
        self.set_selected(keys)
    }

    /// Selects the photos shown similar to the selected ones (else to the
    /// cursor's): taken the same day, or in the same stack. How many are
    /// selected.
    ///
    /// # Errors
    /// Nothing selected, or the selected photos have no capture day (not
    /// read yet) or are in no stack.
    pub fn select_similar(&mut self, by: Similar) -> Result<usize, String> {
        let places = self.places(&self.targets()?);
        let keys: Vec<String> = match by {
            Similar::Day => {
                let days: Vec<&String> = places
                    .iter()
                    .filter_map(|&i| self.entries[i].day.as_ref())
                    .collect();
                if days.is_empty() {
                    return Err("the selected photos have no capture day".into());
                }
                self.shown_where(|e| e.day.as_ref().is_some_and(|d| days.contains(&d)))
            }
            Similar::Stack => {
                let stacks: Vec<&String> = places
                    .iter()
                    .filter_map(|&i| self.entries[i].stack.as_ref().map(|s| &s.id))
                    .collect();
                if stacks.is_empty() {
                    return Err("the selected photos are in no stack".into());
                }
                self.shown_where(|e| e.stack.as_ref().is_some_and(|s| stacks.contains(&&s.id)))
            }
        };
        Ok(self.set_selected(keys))
    }

    /// The ids of the photos shown that `keep` takes, in order.
    fn shown_where(&self, keep: impl Fn(&super::Entry) -> bool) -> Vec<String> {
        self.order
            .iter()
            .map(|&i| &self.entries[i])
            .filter(|e| keep(e))
            .map(|e| e.id.clone())
            .collect()
    }

    /// Selects `keys`, the cursor on the first; how many.
    fn set_selected(&mut self, keys: Vec<String>) -> usize {
        let cursor = keys.first().cloned();
        self.selected = cursor.as_ref().and_then(|c| self.at(c));
        let count = keys.len();
        self.state.set_selection(keys, cursor);
        self.state.reveal_cursor();
        count
    }

    /// The marks copied last in this focus.
    pub fn copied_marks(&self) -> Option<&CopiedMarks> {
        self.copied.as_ref()
    }

    /// Copies the marks of `image`, else of the photo under the cursor,
    /// else of the one photo selected.
    ///
    /// # Errors
    /// No such photo, or several selected and no cursor.
    pub fn copy_marks(&mut self, image: Option<&str>) -> Result<&CopiedMarks, String> {
        let i = match image {
            Some(id) => self
                .at(id)
                .ok_or_else(|| format!("the library has no photo {id}"))?,
            None => match self.selected {
                Some(i) => i,
                None => match self.targets()?.as_slice() {
                    [one] => self.at(one).ok_or("no image selected")?,
                    _ => return Err("several photos are selected: name the `image`".into()),
                },
            },
        };
        let e = &self.entries[i];
        Ok(self.copied.insert(CopiedMarks {
            from: e.id.clone(),
            rating: e.rating,
            labels: e.labels.clone(),
            tags: e.tags.clone(),
            title: e.title.clone(),
        }))
    }

    /// Pastes the copied marks named (`kinds`, of [`MARK_KINDS`]) on
    /// `images`, else on the selection; how many photos.
    ///
    /// # Errors
    /// Nothing copied, no photo to paste on, an unknown mark, or a plugin
    /// vetoed a mark.
    pub fn paste_marks(
        &mut self,
        images: Option<Vec<String>>,
        kinds: &[&str],
        mode: PasteMode,
    ) -> Result<usize, String> {
        if let Some(bad) = kinds.iter().find(|k| !MARK_KINDS.contains(k)) {
            return Err(format!(
                "`{bad}` is not a mark: {} are",
                MARK_KINDS.join(", ")
            ));
        }
        let copy = self
            .copied
            .clone()
            .ok_or("no marks copied: copy a photo's marks first")?;
        let keys = match images {
            Some(ids) => {
                let keys: Vec<String> = ids.into_iter().filter(|i| self.holds(i)).collect();
                if keys.is_empty() {
                    return Err("none of the `images` is in the library".into());
                }
                keys
            }
            None => self.targets()?,
        };
        let overwrite = mode == PasteMode::Overwrite;
        if kinds.contains(&"rating") && (overwrite || copy.rating > 0) {
            self.mark(Mark::Rate {
                keys: keys.clone(),
                rating: u8::try_from(copy.rating.clamp(0, 5)).unwrap_or(0),
            })?;
        }
        if kinds.contains(&"labels") {
            for label in LABELS {
                let on = copy.labels.iter().any(|l| l == label);
                if !on && !overwrite {
                    continue;
                }
                let Some(parsed) = ColorLabel::parse(label) else {
                    continue;
                };
                self.mark(Mark::Label {
                    keys: keys.clone(),
                    label: parsed,
                    on,
                })?;
            }
        }
        if kinds.contains(&"tags") {
            let remove: Vec<String> = if overwrite {
                let mut gone: Vec<String> = self
                    .places(&keys)
                    .into_iter()
                    .flat_map(|i| self.entries[i].tags.clone())
                    .filter(|t| !copy.tags.contains(t))
                    .collect();
                gone.sort_unstable();
                gone.dedup();
                gone
            } else {
                Vec::new()
            };
            if !copy.tags.is_empty() || !remove.is_empty() {
                self.mark(Mark::Tag {
                    keys: keys.clone(),
                    add: copy.tags.clone(),
                    remove,
                })?;
            }
        }
        if kinds.contains(&"title") && (overwrite || copy.title.is_some()) {
            let value = copy.title.clone().map(Value::from);
            for i in self.places(&keys) {
                if self.entries[i].title != copy.title {
                    self.change_at(i, TITLE, Op::Set(value.clone()));
                }
            }
        }
        Ok(keys.len())
    }

    /// Queues `images` (those shown), else the selection, to be taken out
    /// of the Library by the host ([`Library::take_removals`]); how many.
    ///
    /// # Errors
    /// No photo to take out.
    pub fn remove_from_library(&mut self, images: Option<Vec<String>>) -> Result<usize, String> {
        let keys = match images {
            Some(ids) => {
                let keys: Vec<String> = ids.into_iter().filter(|i| self.holds(i)).collect();
                if keys.is_empty() {
                    return Err("none of the `images` is in the library".into());
                }
                keys
            }
            None => self.targets()?,
        };
        let count = keys.len();
        self.removals
            .push((keys, crate::library_history::current_batch()));
        Ok(count)
    }

    /// The photos asked taken out of the Library since, one list per
    /// command, with its batch.
    pub fn take_removals(&mut self) -> Vec<(Vec<String>, super::Batch)> {
        std::mem::take(&mut self.removals)
    }

    /// Queues `images` (those shown), else the selection, to be turned by
    /// the host ([`Library::take_rotations`]); how many.
    ///
    /// # Errors
    /// No photo to turn.
    pub fn rotate(&mut self, images: Option<Vec<String>>, turn: Turn) -> Result<usize, String> {
        let keys = match images {
            Some(ids) => {
                let keys: Vec<String> = ids.into_iter().filter(|i| self.holds(i)).collect();
                if keys.is_empty() {
                    return Err("none of the `images` is in the library".into());
                }
                keys
            }
            None => self.targets()?,
        };
        let count = keys.len();
        self.rotations
            .push((keys, turn, crate::library_history::current_batch()));
        Ok(count)
    }

    /// The photos asked turned since, one list per command, with the
    /// turn and its batch.
    pub fn take_rotations(&mut self) -> Vec<(Vec<String>, Turn, super::Batch)> {
        std::mem::take(&mut self.rotations)
    }

    /// Queues `images` (those shown), else the selection, to be
    /// duplicated by the host ([`Library::take_duplicates`]); how many.
    ///
    /// # Errors
    /// No photo to duplicate.
    pub fn duplicate(
        &mut self,
        images: Option<Vec<String>>,
        from: helios_vault::versions::DuplicateFrom,
    ) -> Result<usize, String> {
        let keys = match images {
            Some(ids) => {
                let keys: Vec<String> = ids.into_iter().filter(|i| self.holds(i)).collect();
                if keys.is_empty() {
                    return Err("none of the `images` is in the library".into());
                }
                keys
            }
            None => self.targets()?,
        };
        let count = keys.len();
        self.duplicates.push((keys, from));
        Ok(count)
    }

    /// The photos asked duplicated since, one list per command, with
    /// where each copy's edit comes from.
    pub fn take_duplicates(&mut self) -> Vec<(Vec<String>, helios_vault::versions::DuplicateFrom)> {
        std::mem::take(&mut self.duplicates)
    }

    /// Queues `images` (those shown), else the selection, for their files
    /// to go to the trash ([`Library::take_trash_asks`]); how many.
    ///
    /// # Errors
    /// No photo.
    pub fn ask_trash(
        &mut self,
        images: Option<Vec<String>>,
        confirmed: bool,
    ) -> Result<usize, String> {
        let keys = match images {
            Some(ids) => {
                let keys: Vec<String> = ids.into_iter().filter(|i| self.holds(i)).collect();
                if keys.is_empty() {
                    return Err("none of the `images` is in the library".into());
                }
                keys
            }
            None => self.targets()?,
        };
        let count = keys.len();
        self.trash_asks.push(TrashAsk {
            images: keys,
            confirmed,
            rejected: false,
        });
        Ok(count)
    }

    /// Queues the rejected photos shown (or `images`, those of them still
    /// rejected) for their files to go to the trash; how many.
    ///
    /// # Errors
    /// No rejected photo.
    pub fn ask_delete_rejected(
        &mut self,
        images: Option<Vec<String>>,
        confirmed: bool,
    ) -> Result<usize, String> {
        let rejected = self.shown_where(|e| e.rejected);
        let keys: Vec<String> = match images {
            Some(ids) => ids.into_iter().filter(|i| rejected.contains(i)).collect(),
            None => rejected,
        };
        if keys.is_empty() {
            return Err("no rejected photo shown".into());
        }
        let count = keys.len();
        self.trash_asks.push(TrashAsk {
            images: keys,
            confirmed,
            rejected: true,
        });
        Ok(count)
    }

    /// The trash asks since, in order.
    pub fn take_trash_asks(&mut self) -> Vec<TrashAsk> {
        std::mem::take(&mut self.trash_asks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Entry, StackOf};

    fn entry(id: &str, day: &str, stack: Option<&str>) -> Entry {
        Entry {
            id: id.into(),
            name: format!("{id}.jpg"),
            day: Some(day.into()),
            stack: stack.map(|s| StackOf {
                id: s.into(),
                top: "b".into(),
                count: 2,
            }),
            ..Entry::default()
        }
    }

    fn library() -> Library {
        let mut lib = Library::default();
        lib.set_entries(vec![
            entry("a", "2024-06-01", None),
            entry("b", "2024-06-01", Some("s")),
            entry("c", "2024-06-02", Some("s")),
            entry("d", "2024-06-03", None),
        ]);
        lib
    }

    #[test]
    fn selection_commands_select_none_invert_and_similar() {
        let mut lib = library();
        lib.select_ids(&["a".into()]);
        assert_eq!(lib.select_similar(Similar::Day), Ok(2));
        assert_eq!(lib.selection(), ["a", "b"]);
        assert_eq!(lib.invert_selection(), 2);
        assert_eq!(lib.selection(), ["c", "d"]);
        lib.select_ids(&["c".into()]);
        assert_eq!(lib.select_similar(Similar::Stack), Ok(2));
        assert_eq!(lib.selection(), ["b", "c"]);
        lib.select_ids(&["d".into()]);
        assert!(lib.select_similar(Similar::Stack).is_err(), "in no stack");
        lib.select_none();
        assert!(lib.selection().is_empty());
        assert!(lib.targets().is_err(), "no cursor left either");
        assert_eq!(lib.invert_selection(), 4);
        assert!(Similar::parse("week").is_err());
    }

    #[test]
    fn duplicate_queues_the_selection_or_the_photos_named() {
        use helios_vault::versions::DuplicateFrom;
        let mut lib = library();
        lib.select_none();
        assert!(lib.duplicate(None, DuplicateFrom::Current).is_err());
        lib.select_ids(&["b".into()]);
        assert_eq!(lib.duplicate(None, DuplicateFrom::Current), Ok(1));
        assert_eq!(
            lib.duplicate(Some(vec!["c".into()]), DuplicateFrom::Original),
            Ok(1)
        );
        assert_eq!(
            lib.take_duplicates(),
            [
                (vec!["b".to_string()], DuplicateFrom::Current),
                (vec!["c".to_string()], DuplicateFrom::Original)
            ]
        );
        lib.set_heads_of("b", 2);
        assert_eq!(lib.heads_of("b"), Some(2));
        lib.set_heads_of("b", 1);
        assert_eq!(lib.heads_of("b"), None, "one head: no badge");
    }

    #[test]
    fn rotate_queues_the_selection_or_the_photos_named() {
        let mut lib = library();
        lib.select_none();
        assert!(lib.rotate(None, Turn::Right).is_err(), "nothing selected");
        lib.select_ids(&["a".into(), "c".into()]);
        assert_eq!(lib.rotate(None, Turn::Left), Ok(2));
        assert_eq!(
            lib.rotate(Some(vec!["d".into(), "zz".into()]), Turn::Reset),
            Ok(1)
        );
        assert_eq!(
            lib.take_rotations(),
            [
                (vec!["a".to_string(), "c".into()], Turn::Left, None),
                (vec!["d".to_string()], Turn::Reset, None)
            ]
        );
        assert!(lib.take_rotations().is_empty());
    }

    #[test]
    fn remove_queues_the_selection_or_the_photos_named() {
        let mut lib = library();
        lib.select_none();
        assert!(lib.remove_from_library(None).is_err(), "nothing selected");
        lib.select_ids(&["a".into(), "c".into()]);
        assert_eq!(lib.remove_from_library(None), Ok(2));
        assert_eq!(
            lib.remove_from_library(Some(vec!["d".into(), "zz".into()])),
            Ok(1),
            "a photo not shown is left out"
        );
        assert!(lib.remove_from_library(Some(vec!["zz".into()])).is_err());
        assert_eq!(
            lib.take_removals(),
            [
                (vec!["a".to_string(), "c".into()], None),
                (vec!["d".to_string()], None)
            ]
        );
        assert!(lib.take_removals().is_empty());
    }

    #[test]
    fn delete_rejected_takes_only_the_rejected_photos_shown() {
        let mut lib = library();
        assert!(
            lib.ask_delete_rejected(None, false).is_err(),
            "none rejected"
        );
        let reject = |mut e: crate::library::Entry| {
            e.rejected = true;
            e
        };
        lib.set_entries(vec![
            entry("a", "2024-06-01", None),
            reject(entry("b", "2024-06-01", Some("s"))),
            entry("c", "2024-06-02", Some("s")),
            reject(entry("d", "2024-06-03", None)),
        ]);
        lib.select_ids(&["a".into()]);
        assert_eq!(lib.ask_delete_rejected(None, false), Ok(2));
        assert_eq!(
            lib.ask_delete_rejected(Some(vec!["a".into(), "d".into()]), true),
            Ok(1),
            "a photo not rejected is left out"
        );
        assert_eq!(lib.ask_trash(None, false), Ok(1), "the selection");
        let asks = lib.take_trash_asks();
        assert_eq!(asks[0].images, ["b", "d"]);
        assert!(asks[0].rejected && !asks[0].confirmed);
        assert_eq!(asks[1].images, ["d"]);
        assert!(asks[1].confirmed);
        assert_eq!(asks[2].images, ["a"]);
        assert!(!asks[2].rejected);
    }
}
