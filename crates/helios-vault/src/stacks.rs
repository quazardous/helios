//! Stacks and an image's signals: what the library groups and what it
//! measured at import.
//!
//! - A **stack** gathers images that are one shot in practice (a burst, a
//!   bracket): its members, the one shown on top, and whether the library
//!   shows it expanded or collapsed. An image is in one stack at most.
//!   Making or dissolving a stack never touches its images: dissolving
//!   only forgets the grouping. Stacks are documents of their own
//!   (namespace `stacks`), each change announced on the bus.
//! - An image's **signals** are what an import measured to find near
//!   duplicates (a perceptual hash, the capture time, the exposure, a
//!   sharpness): a JSON document per image (namespace `signals`), whose
//!   shape belongs to whoever measures it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use helios_core::Event;

use crate::{Changes, DocumentChange, ImageId, Store, Vault};

const STACKS: &str = "stacks";
pub(crate) const SIGNALS: &str = "signals";

/// A group of images shown as one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    /// Derived from the members when made: the same members make the same id.
    pub id: String,
    /// The images, in the order given.
    pub members: Vec<String>,
    /// The one shown for the stack when collapsed.
    pub top: String,
    pub expanded: bool,
    /// Who made it (a user, or `decision:<id>` when an accepted
    /// suggestion did) and when.
    pub who: String,
    pub at: u64,
}

impl Stack {
    pub fn contains(&self, image: &ImageId) -> bool {
        let hex = image.hex();
        self.members.contains(&hex)
    }
}

/// A stack was made, changed or dissolved.
#[derive(Serialize)]
pub struct StackChanged {
    pub stack: String,
    /// `created`, `dissolved`, `top` or `expanded`.
    pub change: &'static str,
    pub members: Vec<String>,
    pub top: String,
    pub expanded: bool,
    pub who: String,
}

impl Event for StackChanged {
    const KIND: &'static str = "vault.stack_changed";
}

fn stack_id(members: &[ImageId]) -> String {
    let mut sorted: Vec<ImageId> = members.to_vec();
    sorted.sort();
    let mut hasher = blake3::Hasher::new();
    for id in &sorted {
        hasher.update(&id.0.0);
    }
    hasher.finalize().to_hex()[..16].to_string()
}

fn read(doc: Value) -> Result<Stack, String> {
    serde_json::from_value(doc).map_err(|e| format!("a stored stack: {e}"))
}

/// A stack with its members off the Library left out (its top moving to
/// the first member left when it was one); none when fewer than two are
/// left. The stored stack is kept as it is.
fn in_library<S: Store>(store: &S, mut stack: Stack) -> Result<Option<Stack>, String> {
    let mut members = Vec::with_capacity(stack.members.len());
    for m in stack.members {
        let flagged = match ImageId::from_hex(&m) {
            Ok(id) => store.library_flag(&id)?.is_some_and(|f| f.in_library),
            Err(_) => false,
        };
        if flagged {
            members.push(m);
        }
    }
    if members.len() < 2 {
        return Ok(None);
    }
    if !members.contains(&stack.top) {
        stack.top.clone_from(&members[0]);
    }
    stack.members = members;
    Ok(Some(stack))
}

/// A stack's document before and after a step.
fn document_change(id: &str, before: Option<Value>, after: Option<Value>) -> DocumentChange {
    DocumentChange {
        ns: STACKS.into(),
        key: id.into(),
        before,
        after,
    }
}

fn all<S: Store>(store: &S) -> Result<Vec<Stack>, String> {
    store
        .documents(STACKS)?
        .into_iter()
        .map(|(_, doc)| read(doc))
        .collect()
}

impl<S: Store> Vault<S> {
    fn announce(&self, stack: &Stack, change: &'static str) {
        if let Some(bus) = &self.bus {
            bus.publish(
                "vault",
                None,
                &StackChanged {
                    stack: stack.id.clone(),
                    change,
                    members: stack.members.clone(),
                    top: stack.top.clone(),
                    expanded: stack.expanded,
                    who: stack.who.clone(),
                },
            );
        }
    }

    /// Every stack, by id, as the Library shows it: only its members in
    /// the Library ([`in_library`]).
    pub fn stacks(&self) -> Result<Vec<Stack>, String> {
        self.read(|store| {
            let mut stacks = Vec::new();
            for stack in all(store)? {
                stacks.extend(in_library(store, stack)?);
            }
            stacks.sort_by(|a, b| a.id.cmp(&b.id));
            Ok(stacks)
        })
    }

    /// A stack, as the Library shows it ([`in_library`]).
    pub fn stack(&self, id: &str) -> Result<Option<Stack>, String> {
        self.read(
            |store| match store.document(STACKS, id)?.map(read).transpose()? {
                Some(stack) => in_library(store, stack),
                None => Ok(None),
            },
        )
    }

    /// The stack an image is in, if any.
    pub fn stack_of(&self, image: &ImageId) -> Result<Option<Stack>, String> {
        Ok(self.stacks()?.into_iter().find(|s| s.contains(image)))
    }

    /// Makes a stack of `members` (two or more images of the vault, none
    /// in a stack already), collapsed, with `top` on top (the first member
    /// when `None`).
    pub fn create_stack(
        &self,
        members: &[ImageId],
        top: Option<ImageId>,
        who: &str,
        now: u64,
    ) -> Result<Stack, String> {
        let mut unique: Vec<ImageId> = Vec::new();
        for m in members {
            if !unique.contains(m) {
                unique.push(*m);
            }
        }
        if unique.len() < 2 {
            return Err("a stack needs two images or more".into());
        }
        let top = top.unwrap_or(unique[0]);
        if !unique.contains(&top) {
            return Err(format!("{} is not a member of the stack", top.hex()));
        }
        let stack = Stack {
            id: stack_id(&unique),
            members: unique.iter().map(ImageId::hex).collect(),
            top: top.hex(),
            expanded: false,
            who: who.into(),
            at: now,
        };
        // Built before the transaction: the writer is held for the checks
        // and the write alone.
        let doc = serde_json::to_value(&stack).expect("stacks serialize");
        self.transaction(|store| {
            for m in &unique {
                if store.image(m)?.is_none() {
                    return Err(format!("no image {}", m.hex()));
                }
            }
            for other in all(store)? {
                if let Some(m) = unique.iter().find(|m| other.contains(m)) {
                    return Err(format!("{} is already in stack {}", m.hex(), other.id));
                }
            }
            store.set_document(STACKS, &stack.id, Some(&doc))
        })?;
        self.announce(&stack, "created");
        Ok(stack)
    }

    /// Forgets a stack; its images stay as they are.
    pub fn dissolve_stack(&self, id: &str) -> Result<Stack, String> {
        let stack = self.transaction(|store| {
            let stack = store
                .document(STACKS, id)?
                .map(read)
                .transpose()?
                .ok_or_else(|| format!("no stack {id}"))?;
            store.set_document(STACKS, id, None)?;
            Ok(stack)
        })?;
        self.announce(&stack, "dissolved");
        Ok(stack)
    }

    /// Changes a stored stack in one transaction; answers it after, with
    /// the step ([`Changes`]) the vault's revert undoes.
    fn change_stack(
        &self,
        id: &str,
        change: &'static str,
        f: impl FnOnce(&mut Stack) -> Result<(), String>,
    ) -> Result<(Stack, Changes), String> {
        let (stack, changes) = self.transaction(|store| {
            let before = store
                .document(STACKS, id)?
                .ok_or_else(|| format!("no stack {id}"))?;
            let mut stack = read(before.clone())?;
            f(&mut stack)?;
            let doc = serde_json::to_value(&stack).expect("stacks serialize");
            store.set_document(STACKS, id, Some(&doc))?;
            let mut changes = Changes::default();
            if before != doc {
                changes
                    .documents
                    .push(document_change(id, Some(before), Some(doc)));
            }
            Ok((stack, changes))
        })?;
        self.announce(&stack, change);
        Ok((stack, changes))
    }

    /// Puts a member on top of its stack.
    pub fn set_stack_top(&self, id: &str, top: &ImageId) -> Result<Stack, String> {
        self.set_stack_top_step(id, top).map(|(stack, _)| stack)
    }

    /// Puts a member on top of its stack; answers the stack and the step
    /// ([`Changes`]) the vault's revert undoes (none when it was on top).
    pub fn set_stack_top_step(&self, id: &str, top: &ImageId) -> Result<(Stack, Changes), String> {
        self.change_stack(id, "top", |stack| {
            if !stack.contains(top) {
                return Err(format!("{} is not in stack {id}", top.hex()));
            }
            stack.top = top.hex();
            Ok(())
        })
    }

    /// Groups `members` into one stack (Lightroom's "Group into Stack"),
    /// collapsed: two images or more of the vault, counted with the
    /// members of the stacks they are in already, which are merged into
    /// it. `top` goes on top, else the top of the first stack merged,
    /// else the first member. One transaction; answers the stack, the
    /// step ([`Changes`]) the vault's revert undoes and the stacks merged.
    pub fn group_stack(
        &self,
        members: &[ImageId],
        top: Option<ImageId>,
        who: &str,
        now: u64,
    ) -> Result<(Stack, Changes, Vec<Stack>), String> {
        let (stack, changes, merged) = self.transaction(|store| {
            let mut unique: Vec<ImageId> = Vec::new();
            for m in members {
                if store.image(m)?.is_none() {
                    return Err(format!("no image {}", m.hex()));
                }
                if !unique.contains(m) {
                    unique.push(*m);
                }
            }
            let merged: Vec<Stack> = all(store)?
                .into_iter()
                .filter(|s| unique.iter().any(|m| s.contains(m)))
                .collect();
            for s in &merged {
                for m in &s.members {
                    let id = ImageId::from_hex(m)?;
                    if !unique.contains(&id) {
                        unique.push(id);
                    }
                }
            }
            if unique.len() < 2 {
                return Err("a stack needs two images or more".into());
            }
            let top = match top {
                Some(top) => top,
                None => match merged.first() {
                    Some(s) => ImageId::from_hex(&s.top)?,
                    None => unique[0],
                },
            };
            if !unique.contains(&top) {
                return Err(format!("{} is not a member of the stack", top.hex()));
            }
            let stack = Stack {
                id: stack_id(&unique),
                members: unique.iter().map(ImageId::hex).collect(),
                top: top.hex(),
                expanded: false,
                who: who.into(),
                at: now,
            };
            let mut changes = Changes::default();
            for s in &merged {
                if s.id != stack.id {
                    let before = store.document(STACKS, &s.id)?;
                    store.set_document(STACKS, &s.id, None)?;
                    changes.documents.push(document_change(&s.id, before, None));
                }
            }
            let before = store.document(STACKS, &stack.id)?;
            let mut kept = stack.clone();
            if let Some(old) = before.clone().map(read).transpose()? {
                // The same members grouped again: only the top may move.
                kept.who = old.who;
                kept.at = old.at;
                kept.expanded = old.expanded;
            }
            let doc = serde_json::to_value(&kept).expect("stacks serialize");
            if before.as_ref() != Some(&doc) {
                store.set_document(STACKS, &kept.id, Some(&doc))?;
                changes
                    .documents
                    .push(document_change(&kept.id, before, Some(doc)));
            }
            Ok((kept, changes, merged))
        })?;
        for s in merged.iter().filter(|s| s.id != stack.id) {
            self.announce(s, "dissolved");
        }
        if !changes.is_empty() {
            self.announce(&stack, "created");
        }
        Ok((stack, changes, merged))
    }

    /// Ungroups the stacks any of `images` is in (Lightroom's "Unstack"):
    /// the groupings are forgotten, the images stay. One transaction;
    /// answers the stacks and the step ([`Changes`]) the vault's revert
    /// undoes.
    ///
    /// # Errors
    /// None of `images` is in a stack.
    pub fn ungroup_stacks(&self, images: &[ImageId]) -> Result<(Vec<Stack>, Changes), String> {
        let (gone, changes) = self.transaction(|store| {
            let gone: Vec<Stack> = all(store)?
                .into_iter()
                .filter(|s| images.iter().any(|m| s.contains(m)))
                .collect();
            if gone.is_empty() {
                return Err("none of the images is in a stack".into());
            }
            let mut changes = Changes::default();
            for s in &gone {
                let before = store.document(STACKS, &s.id)?;
                store.set_document(STACKS, &s.id, None)?;
                changes.documents.push(document_change(&s.id, before, None));
            }
            Ok((gone, changes))
        })?;
        for s in &gone {
            self.announce(s, "dissolved");
        }
        Ok((gone, changes))
    }

    /// Shows a stack expanded (every member) or collapsed (its top).
    pub fn set_stack_expanded(&self, id: &str, expanded: bool) -> Result<Stack, String> {
        self.change_stack(id, "expanded", |stack| {
            stack.expanded = expanded;
            Ok(())
        })
        .map(|(stack, _)| stack)
    }

    /// Keeps an image's signals (or with `None` forgets them), and its
    /// culling fields from them ([`crate::cull`]) in the same transaction.
    pub fn set_signals(&self, image: &ImageId, signals: Option<&Value>) -> Result<(), String> {
        self.transaction(|store| {
            if store.image(image)?.is_none() {
                return Err(format!("no image {}", image.hex()));
            }
            store.set_document(SIGNALS, &image.hex(), signals)?;
            set_cull_fields(store, image, signals)
        })?;
        self.placed([*image]);
        Ok(())
    }

    /// An image's signals, as kept.
    pub fn signals(&self, image: &ImageId) -> Result<Option<Value>, String> {
        self.read(|s| s.document(SIGNALS, &image.hex()))
    }

    /// Adds keys to images' signals (a key already there is replaced, the
    /// others kept; an image with none starts from an empty document), and
    /// with `progress` sets a library document (`library/<ns>`, `key`), in
    /// one transaction: a backfill's batch lands with its progress. An image
    /// gone meanwhile is passed over. The culling fields follow the merged
    /// documents ([`crate::cull`]). Returns how many images were written.
    pub fn merge_signals(
        &self,
        merges: &[(ImageId, serde_json::Map<String, Value>)],
        progress: Option<(&str, &str, &Value)>,
    ) -> Result<usize, String> {
        let written = self.transaction(|store| {
            let mut written = Vec::new();
            for (id, keys) in merges {
                if store.image(id)?.is_none() {
                    continue;
                }
                let mut doc = match store.document(SIGNALS, &id.hex())? {
                    Some(Value::Object(map)) => map,
                    _ => serde_json::Map::new(),
                };
                doc.extend(keys.iter().map(|(k, v)| (k.clone(), v.clone())));
                let doc = Value::Object(doc);
                store.set_document(SIGNALS, &id.hex(), Some(&doc))?;
                set_cull_fields(store, id, Some(&doc))?;
                written.push(*id);
            }
            if let Some((ns, key, value)) = progress {
                store.set_document(&format!("library/{ns}"), key, Some(value))?;
            }
            Ok(written)
        })?;
        let count = written.len();
        self.placed(written);
        Ok(count)
    }

    /// A library document (`library/<ns>`): what library operations keep
    /// besides images, stacks and signals (their decisions, settings).
    pub fn library_document(&self, ns: &str, key: &str) -> Result<Option<Value>, String> {
        self.read(|s| s.document(&format!("library/{ns}"), key))
    }

    /// Sets (or with `None` removes) a library document.
    pub fn set_library_document(
        &self,
        ns: &str,
        key: &str,
        value: Option<&Value>,
    ) -> Result<(), String> {
        self.store
            .lock()
            .expect("store lock")
            .set_document(&format!("library/{ns}"), key, value)
    }

    /// Every library document of a namespace, by key.
    pub fn library_documents(&self, ns: &str) -> Result<Vec<(String, Value)>, String> {
        self.read(|s| s.documents(&format!("library/{ns}")))
    }
}

/// Copies the culling measures of a signals document to the image's
/// read-only fields ([`crate::cull`]): one the document lacks is cleared.
fn set_cull_fields<S: Store>(
    store: &mut S,
    id: &ImageId,
    signals: Option<&Value>,
) -> Result<(), String> {
    for (name, value) in crate::cull::values(signals) {
        let value = value
            .map(|v| crate::fields::normalize(name, &v))
            .transpose()?;
        store.set_field(id, name, value.as_ref())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use helios_core::ContentHash;

    use super::*;
    use crate::{Image, MemoryStore};

    fn vault_with(n: u8) -> (Vault<MemoryStore>, Vec<ImageId>) {
        let vault = Vault::new(MemoryStore::default());
        let ids: Vec<ImageId> = (1..=n).map(|i| ImageId(ContentHash([i; 32]))).collect();
        for id in &ids {
            vault
                .transaction(|s| {
                    s.add_to_library(&Image {
                        id: *id,
                        size: 1,
                        partial: id.0,
                    })
                })
                .unwrap();
        }
        (vault, ids)
    }

    #[test]
    fn a_stack_is_made_changed_and_dissolved_without_touching_its_images() {
        let (vault, ids) = vault_with(4);
        let stack = vault
            .create_stack(&ids[..3], Some(ids[1]), "user", 10)
            .unwrap();
        assert_eq!(stack.top, ids[1].hex());
        assert!(!stack.expanded);
        assert_eq!(vault.stack_of(&ids[2]).unwrap(), Some(stack.clone()));
        assert_eq!(vault.stack_of(&ids[3]).unwrap(), None);
        // The same members, in another order, are the same stack.
        assert_eq!(stack.id, stack_id(&[ids[2], ids[0], ids[1]]));
        // An image is in one stack at most.
        let e = vault.create_stack(&ids[2..], None, "user", 11).unwrap_err();
        assert!(e.contains("already in stack"), "{e}");
        assert!(vault.create_stack(&ids[3..], None, "user", 11).is_err());
        let s = vault.set_stack_expanded(&stack.id, true).unwrap();
        assert!(s.expanded);
        let s = vault.set_stack_top(&stack.id, &ids[2]).unwrap();
        assert_eq!(s.top, ids[2].hex());
        assert!(vault.set_stack_top(&stack.id, &ids[3]).is_err());
        let gone = vault.dissolve_stack(&stack.id).unwrap();
        assert_eq!(gone.members.len(), 3);
        assert!(vault.stacks().unwrap().is_empty());
        assert_eq!(vault.images().unwrap().len(), 4);
        assert!(vault.dissolve_stack(&stack.id).is_err());
    }

    #[test]
    fn grouping_merges_stacks_and_each_step_is_undone_by_revert() {
        let (vault, ids) = vault_with(5);
        let first = vault
            .create_stack(&ids[..2], Some(ids[1]), "user", 10)
            .unwrap();
        // Grouping one member of a stack with two more merges it: its
        // top stays on top.
        let (stack, changes, merged) = vault
            .group_stack(&[ids[0], ids[2], ids[3]], None, "user", 11)
            .unwrap();
        assert_eq!(merged, [first.clone()]);
        assert_eq!(stack.members.len(), 4);
        assert_eq!(stack.top, ids[1].hex());
        assert_eq!(vault.stacks().unwrap(), [stack.clone()]);
        assert_eq!(changes.documents.len(), 2);
        vault.revert(&changes).unwrap();
        assert_eq!(vault.stacks().unwrap(), [first.clone()]);
        vault.replay(&changes).unwrap();
        assert_eq!(vault.stacks().unwrap(), [stack.clone()]);

        let (moved, top) = vault.set_stack_top_step(&stack.id, &ids[3]).unwrap();
        assert_eq!(moved.top, ids[3].hex());
        vault.revert(&top).unwrap();
        assert_eq!(vault.stack(&stack.id).unwrap().unwrap().top, ids[1].hex());
        vault.replay(&top).unwrap();
        // Already on top: nothing to undo.
        let (_, none) = vault.set_stack_top_step(&stack.id, &ids[3]).unwrap();
        assert!(none.is_empty());

        let (gone, ungrouped) = vault.ungroup_stacks(&[ids[2], ids[4]]).unwrap();
        assert_eq!(gone.len(), 1);
        assert!(vault.stacks().unwrap().is_empty());
        assert_eq!(vault.images().unwrap().len(), 5);
        vault.revert(&ungrouped).unwrap();
        assert_eq!(vault.stack(&stack.id).unwrap().unwrap().top, ids[3].hex());
        assert!(vault.ungroup_stacks(&[ids[4]]).is_err());
        assert!(vault.group_stack(&[ids[4]], None, "user", 12).is_err());
        assert!(
            vault
                .group_stack(&[ids[4], ids[0]], Some(ids[4]), "user", 12)
                .is_ok()
        );
    }

    #[test]
    fn signals_belong_to_known_images() {
        let (vault, ids) = vault_with(1);
        let doc = serde_json::json!({ "phash": "00ff" });
        vault.set_signals(&ids[0], Some(&doc)).unwrap();
        assert_eq!(vault.signals(&ids[0]).unwrap(), Some(doc));
        let stranger = ImageId(ContentHash([9; 32]));
        assert!(vault.set_signals(&stranger, None).is_err());
    }
}
