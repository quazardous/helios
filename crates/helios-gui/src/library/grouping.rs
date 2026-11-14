//! Manual stacks in the Library (L11): Lightroom's "Group into Stack"
//! (Ctrl+G), "Unstack" (Ctrl+Shift+G), "Move to Top of Stack", and
//! "Stack RAW+JPEG Pairs" for photos imported before (an import stacks
//! its own pairs). Each is asked of the host, which sends it to the node
//! (`library.stacks.group`, `library.stacks.ungroup`,
//! `library.stacks.top`, `library.stacks.pairs`) and records the step it
//! answers in the library's history: Ctrl+Z takes it back.

use super::{Batch, Library};

/// Groups photos into one stack (`images`, else the selection; `top`,
/// else the photo under the cursor when it is one of them), the stacks
/// they are in merged into it.
pub const GROUP_COMMAND: &str = "library.stack.group";

/// Ungroups the stacks photos are in (`images`, else the selection), the
/// photos untouched.
pub const UNGROUP_COMMAND: &str = "library.stack.ungroup";

/// Puts a photo on top of its stack (`image`, else the photo under the
/// cursor).
pub const STACK_TOP_COMMAND: &str = "library.stack.top";

/// Stacks the RAW+JPEG pairs among photos (`images`, else the
/// selection), the raw on top.
pub const PAIRS_COMMAND: &str = "library.stack.pairs";

/// What the commands asked of the stacks, for the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StackAsk {
    /// Group `images`, `top` on top.
    Group {
        images: Vec<String>,
        top: Option<String>,
    },
    /// Ungroup the stacks `images` are in.
    Ungroup { images: Vec<String> },
    /// Put `image` on top of `stack`.
    Top { stack: String, image: String },
    /// Stack the RAW+JPEG pairs among `images`.
    Pairs { images: Vec<String> },
}

impl Library {
    /// The photos named the library holds, else the selection.
    fn named_or_targets(&self, images: Option<Vec<String>>) -> Result<Vec<String>, String> {
        match images {
            Some(ids) => {
                let keys: Vec<String> = ids.into_iter().filter(|i| self.holds(i)).collect();
                if keys.is_empty() {
                    Err("none of the `images` is in the library".into())
                } else {
                    Ok(keys)
                }
            }
            None => self.targets(),
        }
    }

    /// The photo under the cursor.
    fn cursor_id(&self) -> Option<String> {
        self.selected
            .and_then(|i| self.entries.get(i))
            .map(|e| e.id.clone())
    }

    /// Asks `images` (else the selection) grouped into one stack, `top`
    /// (else the photo under the cursor when it is one of them) on top;
    /// how many photos.
    ///
    /// # Errors
    /// Fewer than two photos (counted with the stacks they are in), or a
    /// `top` not among them.
    pub fn group_stack(
        &mut self,
        images: Option<Vec<String>>,
        top: Option<String>,
    ) -> Result<usize, String> {
        let keys = self.named_or_targets(images)?;
        let mut members: Vec<&str> = keys.iter().map(String::as_str).collect();
        for k in &keys {
            if let Some(s) = self.at(k).and_then(|i| self.entries[i].stack.as_ref()) {
                for e in &self.entries {
                    if e.stack.as_ref().is_some_and(|o| o.id == s.id) && !members.contains(&&*e.id)
                    {
                        members.push(&e.id);
                    }
                }
            }
        }
        if members.len() < 2 {
            return Err("a stack needs two photos or more".into());
        }
        let top = match top {
            Some(t) if keys.contains(&t) => Some(t),
            Some(t) => return Err(format!("{t} is not one of the photos grouped")),
            None => self.cursor_id().filter(|c| keys.contains(c)),
        };
        let count = keys.len();
        self.stack_asks.push((
            StackAsk::Group { images: keys, top },
            crate::library_history::current_batch(),
        ));
        Ok(count)
    }

    /// Asks the stacks `images` (else the selection) are in ungrouped;
    /// how many stacks.
    ///
    /// # Errors
    /// None of the photos is in a stack.
    pub fn ungroup_stack(&mut self, images: Option<Vec<String>>) -> Result<usize, String> {
        let keys = self.named_or_targets(images)?;
        let mut stacks: Vec<&str> = Vec::new();
        for k in &keys {
            if let Some(s) = self.at(k).and_then(|i| self.entries[i].stack.as_ref())
                && !stacks.contains(&s.id.as_str())
            {
                stacks.push(&s.id);
            }
        }
        if stacks.is_empty() {
            return Err("none of the photos is in a stack".into());
        }
        let count = stacks.len();
        self.stack_asks.push((
            StackAsk::Ungroup { images: keys },
            crate::library_history::current_batch(),
        ));
        Ok(count)
    }

    /// Asks `image` (else the photo under the cursor) put on top of its
    /// stack; answers the stack.
    ///
    /// # Errors
    /// No photo, or a photo in no stack.
    pub fn top_of_stack(&mut self, image: Option<&str>) -> Result<String, String> {
        let image = match image {
            Some(id) if self.holds(id) => id.to_string(),
            Some(id) => return Err(format!("{id} is not in the library")),
            None => self.cursor_id().ok_or("no photo under the cursor")?,
        };
        let stack = self
            .at(&image)
            .and_then(|i| self.entries[i].stack.as_ref())
            .map(|s| s.id.clone())
            .ok_or("the photo is in no stack")?;
        self.stack_asks.push((
            StackAsk::Top {
                stack: stack.clone(),
                image,
            },
            crate::library_history::current_batch(),
        ));
        Ok(stack)
    }

    /// Asks the RAW+JPEG pairs among `images` (else the selection)
    /// stacked, the raw on top; how many photos are looked at. Which are
    /// pairs is the node's to say: it knows their files.
    ///
    /// # Errors
    /// Fewer than two photos.
    pub fn stack_pairs(&mut self, images: Option<Vec<String>>) -> Result<usize, String> {
        let keys = self.named_or_targets(images)?;
        if keys.len() < 2 {
            return Err("a pair needs two photos".into());
        }
        let count = keys.len();
        self.stack_asks.push((
            StackAsk::Pairs { images: keys },
            crate::library_history::current_batch(),
        ));
        Ok(count)
    }

    /// What was asked of the stacks since, each with its batch.
    pub fn take_stack_asks(&mut self) -> Vec<(StackAsk, Batch)> {
        std::mem::take(&mut self.stack_asks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping_ungrouping_and_the_top_are_asked_of_the_host() {
        let mut lib = Library::default();
        lib.set_entries(
            ["a", "b", "c"]
                .into_iter()
                .map(|id| crate::library::Entry {
                    id: id.into(),
                    name: format!("{id}.jpg"),
                    ..crate::library::Entry::default()
                })
                .collect(),
        );
        assert!(lib.group_stack(Some(vec!["a".into()]), None).is_err());
        assert!(lib.ungroup_stack(Some(vec!["a".into()])).is_err());
        assert!(lib.top_of_stack(Some("a")).is_err());
        lib.select_ids(&["a".into(), "b".into()]);
        assert_eq!(lib.group_stack(None, None), Ok(2));
        assert_eq!(
            lib.take_stack_asks(),
            [(
                StackAsk::Group {
                    images: vec!["a".into(), "b".into()],
                    top: Some("b".into()),
                },
                None
            )],
            "the cursor's photo on top"
        );
        let stack = helios_vault::Stack {
            id: "s".into(),
            members: vec!["a".into(), "b".into()],
            top: "a".into(),
            expanded: true,
            who: "user".into(),
            at: 0,
        };
        lib.set_stacks(&[stack]);
        // One member and another photo: the stack counts, so it is a group.
        assert_eq!(lib.group_stack(Some(vec!["c".into()]), None).ok(), None);
        assert_eq!(
            lib.group_stack(Some(vec!["a".into(), "c".into()]), None),
            Ok(2)
        );
        assert_eq!(lib.top_of_stack(Some("b")), Ok("s".into()));
        assert_eq!(lib.ungroup_stack(Some(vec!["b".into(), "c".into()])), Ok(1));
        let asks: Vec<StackAsk> = lib.take_stack_asks().into_iter().map(|(a, _)| a).collect();
        assert_eq!(
            asks[1],
            StackAsk::Top {
                stack: "s".into(),
                image: "b".into()
            }
        );
        assert_eq!(
            asks[2],
            StackAsk::Ungroup {
                images: vec!["b".into(), "c".into()]
            }
        );
    }
}
