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

use crate::{ImageId, Store, Vault};

const STACKS: &str = "stacks";
const SIGNALS: &str = "signals";

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

    /// Every stack, by id.
    pub fn stacks(&self) -> Result<Vec<Stack>, String> {
        let mut stacks = all(&*self.store.lock().expect("store lock"))?;
        stacks.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(stacks)
    }

    pub fn stack(&self, id: &str) -> Result<Option<Stack>, String> {
        self.store
            .lock()
            .expect("store lock")
            .document(STACKS, id)?
            .map(read)
            .transpose()
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
            let doc = serde_json::to_value(&stack).expect("stacks serialize");
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

    fn change_stack(
        &self,
        id: &str,
        change: &'static str,
        f: impl FnOnce(&mut Stack) -> Result<(), String>,
    ) -> Result<Stack, String> {
        let stack = self.transaction(|store| {
            let mut stack = store
                .document(STACKS, id)?
                .map(read)
                .transpose()?
                .ok_or_else(|| format!("no stack {id}"))?;
            f(&mut stack)?;
            let doc = serde_json::to_value(&stack).expect("stacks serialize");
            store.set_document(STACKS, id, Some(&doc))?;
            Ok(stack)
        })?;
        self.announce(&stack, change);
        Ok(stack)
    }

    /// Puts a member on top of its stack.
    pub fn set_stack_top(&self, id: &str, top: &ImageId) -> Result<Stack, String> {
        self.change_stack(id, "top", |stack| {
            if !stack.contains(top) {
                return Err(format!("{} is not in stack {id}", top.hex()));
            }
            stack.top = top.hex();
            Ok(())
        })
    }

    /// Shows a stack expanded (every member) or collapsed (its top).
    pub fn set_stack_expanded(&self, id: &str, expanded: bool) -> Result<Stack, String> {
        self.change_stack(id, "expanded", |stack| {
            stack.expanded = expanded;
            Ok(())
        })
    }

    /// Keeps an image's signals (or with `None` forgets them).
    pub fn set_signals(&self, image: &ImageId, signals: Option<&Value>) -> Result<(), String> {
        let mut store = self.store.lock().expect("store lock");
        if store.image(image)?.is_none() {
            return Err(format!("no image {}", image.hex()));
        }
        store.set_document(SIGNALS, &image.hex(), signals)
    }

    /// An image's signals, as kept.
    pub fn signals(&self, image: &ImageId) -> Result<Option<Value>, String> {
        self.store
            .lock()
            .expect("store lock")
            .document(SIGNALS, &image.hex())
    }

    /// A library document (`library/<ns>`): what library operations keep
    /// besides images, stacks and signals (their decisions, settings).
    pub fn library_document(&self, ns: &str, key: &str) -> Result<Option<Value>, String> {
        self.store
            .lock()
            .expect("store lock")
            .document(&format!("library/{ns}"), key)
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
        self.store
            .lock()
            .expect("store lock")
            .documents(&format!("library/{ns}"))
    }
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
                    s.add_image(&Image {
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
    fn signals_belong_to_known_images() {
        let (vault, ids) = vault_with(1);
        let doc = serde_json::json!({ "phash": "00ff" });
        vault.set_signals(&ids[0], Some(&doc)).unwrap();
        assert_eq!(vault.signals(&ids[0]).unwrap(), Some(doc));
        let stranger = ImageId(ContentHash([9; 32]));
        assert!(vault.set_signals(&stranger, None).is_err());
    }
}
