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

use egui_pop::views::library::{Change as Mark, ColorLabel};
use serde_json::{Value, json};

use super::{LABELS, Library, Op};
use helios_vault::fields::TITLE;

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
        self.removals.push(keys);
        Ok(count)
    }

    /// The photos asked taken out of the Library since, one list per
    /// command.
    pub fn take_removals(&mut self) -> Vec<Vec<String>> {
        std::mem::take(&mut self.removals)
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
            [vec!["a".to_string(), "c".into()], vec!["d".to_string()]]
        );
        assert!(lib.take_removals().is_empty());
    }
}
