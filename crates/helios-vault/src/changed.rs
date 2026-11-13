//! A Library photo whose file another application changed
//! (`docs/design/image-map.md`, section 3, "A file changed in place",
//! decided 2026-11-08): the photo stays linked. An image's id is the hash
//! of the content it was imported with and never changes, since its
//! marks, edits and history hang on it; the content its file holds now is
//! kept beside it, in the namespace [`CONTENT`] (one document per image,
//! by its hex id), with every content it held before and the mark
//! "changed outside Helios" until the user acknowledges it.
//!
//! No schema step: the documents table holds it. An image whose file
//! never changed has no document; its content is its id.

use serde::{Deserialize, Serialize};

use crate::{ImageId, Store, Vault, parse_hash};

/// The documents' namespace.
pub const CONTENT: &str = "content";

/// The bus event a mark set or cleared is announced as (`count`: how
/// many images), so a library shown reads its photos again.
pub const CHANGED_OUTSIDE: &str = "vault.changed_outside";

/// One content an image's file held.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    /// The full hash, in hex.
    pub hash: String,
    pub size: u64,
    /// The partial hash, in hex.
    pub partial: String,
    /// When it was found (none for the content imported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    /// When another content replaced it (none for the current one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
}

/// An image's [`CONTENT`] document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    /// What its file holds now.
    pub current: Held,
    /// When a change made outside Helios was found, until acknowledged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_outside: Option<u64>,
    /// What it held before, oldest first: the content imported first.
    #[serde(default)]
    pub history: Vec<Held>,
}

impl Content {
    /// The current content's id.
    ///
    /// # Errors
    /// The document holds a malformed hash.
    pub fn current_id(&self) -> Result<ImageId, String> {
        parse_hash(&self.current.hash).map(ImageId)
    }
}

fn read<S: Store>(store: &S, id: &ImageId) -> Result<Option<Content>, String> {
    store
        .document(CONTENT, &id.hex())?
        .map(|d| serde_json::from_value(d).map_err(|e| format!("content of {}: {e}", id.hex())))
        .transpose()
}

fn write<S: Store>(store: &mut S, id: &ImageId, content: &Content) -> Result<(), String> {
    let doc = serde_json::to_value(content).map_err(|e| e.to_string())?;
    store.set_document(CONTENT, &id.hex(), Some(&doc))
}

impl<S: Store> Vault<S> {
    /// An image's content record; `None` when its file never changed.
    ///
    /// # Errors
    /// The store cannot be read.
    pub fn content(&self, id: &ImageId) -> Result<Option<Content>, String> {
        self.read(|store| read(store, id))
    }

    /// The content an image's file holds now: its id, unless a change
    /// made outside Helios was recorded.
    ///
    /// # Errors
    /// The store cannot be read.
    pub fn content_of(&self, id: &ImageId) -> Result<ImageId, String> {
        match self.content(id)? {
            Some(c) => c.current_id(),
            None => Ok(*id),
        }
    }

    /// Records that the file of image `id` now holds the content `hash`
    /// (`size` bytes, partial hash `partial`), found at `at`: the content
    /// before goes to the history and the image is marked changed outside
    /// Helios. Its marks, edits and sightings are untouched.
    ///
    /// # Errors
    /// No such image, or the store cannot be written.
    pub fn changed_outside(
        &self,
        id: &ImageId,
        hash: ImageId,
        size: u64,
        partial: &helios_core::ContentHash,
        at: u64,
    ) -> Result<(), String> {
        self.transaction(|store| {
            let image = store
                .image(id)?
                .ok_or_else(|| format!("no image {}", id.hex()))?;
            let mut content = read(&*store, id)?.unwrap_or_else(|| Content {
                current: Held {
                    hash: id.hex(),
                    size: image.size,
                    partial: image.partial.to_string(),
                    since: None,
                    until: None,
                },
                changed_outside: None,
                history: Vec::new(),
            });
            if content.current.hash == hash.hex() {
                return Ok(());
            }
            let mut before = content.current.clone();
            before.until = Some(at);
            content.history.push(before);
            content.current = Held {
                hash: hash.hex(),
                size,
                partial: partial.to_string(),
                since: Some(at),
                until: None,
            };
            content.changed_outside = Some(at);
            write(store, id, &content)
        })?;
        self.announce_changed(1);
        Ok(())
    }

    /// The Library's images marked changed outside Helios, with when it
    /// was found, oldest first.
    ///
    /// # Errors
    /// The store cannot be read.
    pub fn changed_outside_images(&self) -> Result<Vec<(ImageId, u64)>, String> {
        self.read(|store| {
            let mut out = Vec::new();
            for (key, doc) in store.documents(CONTENT)? {
                let content: Content =
                    serde_json::from_value(doc).map_err(|e| format!("content of {key}: {e}"))?;
                let Some(at) = content.changed_outside else {
                    continue;
                };
                let id = ImageId::from_hex(&key)?;
                if store.library_flag(&id)?.is_some_and(|f| f.in_library) {
                    out.push((id, at));
                }
            }
            out.sort_by_key(|(id, at)| (*at, *id));
            Ok(out)
        })
    }

    /// Clears the mark "changed outside Helios" of `ids`, or of every
    /// image when `ids` is empty; the content and its history stay. How
    /// many marks were cleared.
    ///
    /// # Errors
    /// The store cannot be read or written.
    pub fn acknowledge_changed(&self, ids: &[ImageId]) -> Result<usize, String> {
        let cleared = self.transaction(|store| {
            let keys: Vec<String> = if ids.is_empty() {
                store
                    .documents(CONTENT)?
                    .into_iter()
                    .map(|(k, _)| k)
                    .collect()
            } else {
                ids.iter().map(ImageId::hex).collect()
            };
            let mut cleared = 0;
            for key in keys {
                let id = ImageId::from_hex(&key)?;
                let Some(mut content) = read(&*store, &id)? else {
                    continue;
                };
                if content.changed_outside.take().is_some() {
                    write(store, &id, &content)?;
                    cleared += 1;
                }
            }
            Ok(cleared)
        })?;
        if cleared > 0 {
            self.announce_changed(cleared);
        }
        Ok(cleared)
    }

    fn announce_changed(&self, count: usize) {
        self.catalog.lock().expect("catalog lock").version += 1;
        if let Some(bus) = &self.bus {
            bus.publish_value(
                "vault",
                None,
                CHANGED_OUTSIDE,
                serde_json::json!({ "count": count }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use helios_core::ContentHash;

    use super::*;
    use crate::{Image, MemoryStore};

    fn h(n: u8) -> ContentHash {
        ContentHash([n; 32])
    }

    fn vault() -> Vault<MemoryStore> {
        let mut store = MemoryStore::default();
        for n in [1, 2] {
            store
                .add_to_library(&Image {
                    id: ImageId(h(n)),
                    size: 100,
                    partial: h(n + 10),
                })
                .unwrap();
        }
        // Off the Library: never listed as changed.
        store
            .add_image(&Image {
                id: ImageId(h(3)),
                size: 100,
                partial: h(13),
            })
            .unwrap();
        Vault::new(store)
    }

    #[test]
    fn a_change_keeps_the_id_and_the_history() {
        let v = vault();
        let one = ImageId(h(1));
        assert_eq!(v.content_of(&one).unwrap(), one);
        v.changed_outside(&one, ImageId(h(51)), 120, &h(61), 1000)
            .unwrap();
        v.changed_outside(&one, ImageId(h(52)), 130, &h(62), 2000)
            .unwrap();
        assert_eq!(v.content_of(&one).unwrap(), ImageId(h(52)));
        let c = v.content(&one).unwrap().unwrap();
        assert_eq!(c.changed_outside, Some(2000));
        let history: Vec<_> = c
            .history
            .iter()
            .map(|h| (h.hash.clone(), h.until))
            .collect();
        assert_eq!(
            history,
            [
                (h(1).to_string(), Some(1000)),
                (h(51).to_string(), Some(2000))
            ]
        );
        assert_eq!(c.history[0].size, 100);
        assert!(v.image(&one).unwrap().is_some(), "the image stays");
    }

    #[test]
    fn only_library_images_are_listed_and_acknowledging_clears_the_mark() {
        let v = vault();
        for n in [2, 1, 3] {
            v.changed_outside(&ImageId(h(n)), ImageId(h(n + 50)), 1, &h(0), u64::from(n))
                .unwrap();
        }
        assert_eq!(
            v.changed_outside_images().unwrap(),
            [(ImageId(h(1)), 1), (ImageId(h(2)), 2)]
        );
        assert_eq!(v.acknowledge_changed(&[ImageId(h(2))]).unwrap(), 1);
        assert_eq!(v.changed_outside_images().unwrap(), [(ImageId(h(1)), 1)]);
        assert_eq!(v.acknowledge_changed(&[]).unwrap(), 2, "1 and 3");
        assert!(v.changed_outside_images().unwrap().is_empty());
        assert_eq!(
            v.content_of(&ImageId(h(2))).unwrap(),
            ImageId(h(52)),
            "the content stays"
        );
        assert_eq!(v.acknowledge_changed(&[]).unwrap(), 0);
    }
}
