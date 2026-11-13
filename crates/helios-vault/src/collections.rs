//! The Library's collections (`docs/design/library.md`, L6): documents in
//! the namespace [`COLLECTIONS`], one per collection, keyed by its id.
//!
//! This part holds **albums**: an explicit set of photos the user fills,
//! `{ "kind": "album", "name": "…", "images": ["<id>", …] }`, the images
//! by id in the order they were added. A collection belongs to the vault,
//! so every window and every node sees the same; which one a window shows
//! is that window's (a [`crate::Query::album`]).
//!
//! A **smart album** holds a rule tree instead of photos,
//! `{ "kind": "smart", "name": "…", "rules": {"match": "all", …} }`
//! ([`crate::rules`]): its photos are those the rules select, now, which
//! each store reads in the same query when [`crate::Query::album`] names
//! it.
//!
//! The **quick collection** is one more document, under the key [`QUICK`]
//! and of that kind, `{ "kind": "quick", "images": [...] }`: a set of
//! photos the user gathers for a while (B toggles the selection in and
//! out, as Lightroom's quick collection), always there (no document yet
//! is an empty one), never listed with the albums nor renamed or deleted;
//! [`crate::Query::album`] naming it selects its photos as an album's.
//! "Save as album" turns it into a plain album, emptying it or not.
//!
//! Each change answers what it changed as [`Changes`] holding the
//! document before and after it, so the Library's undo takes it back
//! ([`crate::Vault::revert`]) and puts it again ([`crate::Vault::replay`]).

use serde::Serialize;
use serde_json::{Value, json};

use crate::rules::Rule;
use crate::undo::DocumentChange;
use crate::{Changes, ImageId, Query, Store, Vault};

/// The collections' namespace.
pub const COLLECTIONS: &str = "library/collections";
/// A plain album's `kind`.
pub const ALBUM: &str = "album";
/// A smart album's `kind`.
pub const SMART: &str = "smart";
/// The quick collection's key and `kind`.
pub const QUICK: &str = "quick";

/// An album: a name and the photos put in it, or for a smart album the
/// rules choosing them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Album {
    /// Its key in [`COLLECTIONS`]: made from its first name
    /// ([`album_key`]), kept when it is renamed.
    pub id: String,
    pub name: String,
    /// Its photos, by id, in the order they were added (none for a smart
    /// album).
    pub images: Vec<String>,
    /// A smart album's rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Rule>,
    /// How many photos of the Library a smart album selects, when
    /// [`Vault::albums`] read it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}

impl Album {
    /// Whether it is a smart album.
    pub fn is_smart(&self) -> bool {
        self.rules.is_some()
    }

    fn of(id: &str, doc: &Value) -> Option<Album> {
        let rules = match doc.get("kind").and_then(Value::as_str) {
            Some(ALBUM) => None,
            // A tree that does not parse (written by hand) is shown as
            // an empty `all` group; the store refuses to run it.
            Some(SMART) => Some(
                Rule::parse(doc.get("rules").unwrap_or(&Value::Null))
                    .unwrap_or_else(|_| Rule::group(crate::rules::Match::All, Vec::new())),
            ),
            _ => return None,
        };
        Some(Album {
            id: id.to_string(),
            name: doc.get("name").and_then(Value::as_str)?.to_string(),
            rules,
            count: None,
            images: doc
                .get("images")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    fn doc(&self) -> Value {
        match &self.rules {
            Some(rules) => json!({ "kind": SMART, "name": self.name, "rules": rules.to_json() }),
            None => json!({ "kind": ALBUM, "name": self.name, "images": self.images }),
        }
    }
}

/// A new album's key from its name: its letters and digits lowercased,
/// the rest one dash (`Trip 2024!` gives `trip-2024`), else `album`;
/// followed by `-2`, `-3`… while `taken` says the key is in use.
pub fn album_key(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut base = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            base.extend(c.to_lowercase());
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    let base = match base.trim_end_matches('-') {
        "" => "album".to_string(),
        b => b.to_string(),
    };
    // The quick collection's key is never an album's.
    let taken = |k: &str| k == QUICK || taken(k);
    if !taken(&base) {
        return base;
    }
    let mut n = 2u64;
    loop {
        let key = format!("{base}-{n}");
        if !taken(&key) {
            return key;
        }
        n += 1;
    }
}

/// A name as an album keeps it: trimmed, refused empty.
fn album_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("an album needs a name".into());
    }
    Ok(name.to_string())
}

/// The albums of a store, by name (whatever the case), then id.
fn albums_of<S: Store + ?Sized>(store: &S) -> Result<Vec<Album>, String> {
    let mut albums: Vec<Album> = store
        .documents(COLLECTIONS)?
        .iter()
        .filter_map(|(k, d)| Album::of(k, d))
        .collect();
    albums.sort_by(|a, b| {
        (crate::fields::fold_case(&a.name), &a.id).cmp(&(crate::fields::fold_case(&b.name), &b.id))
    });
    Ok(albums)
}

/// Refuses a name another album than `id` has, whatever the case.
fn check_free<S: Store + ?Sized>(store: &S, name: &str, id: &str) -> Result<(), String> {
    let folded = crate::fields::fold_case(name);
    match albums_of(store)?
        .into_iter()
        .find(|a| a.id != id && crate::fields::fold_case(&a.name) == folded)
    {
        Some(a) => Err(format!("an album is already named “{}”", a.name)),
        None => Ok(()),
    }
}

impl<S: Store> Vault<S> {
    /// Every album, by name; a smart album with how many photos of the
    /// Library its rules select now (one store query each).
    pub fn albums(&self) -> Result<Vec<Album>, String> {
        let store = self.store.lock().expect("store lock");
        let mut albums = albums_of(&*store)?;
        for album in albums.iter_mut().filter(|a| a.is_smart()) {
            let query = Query {
                album: Some(album.id.clone()),
                ..Query::default()
            };
            album.count = store.select(&query).ok().map(|ids| ids.len());
        }
        Ok(albums)
    }

    /// An album by id.
    pub fn album(&self, id: &str) -> Result<Option<Album>, String> {
        let store = self.store.lock().expect("store lock");
        Ok(store
            .document(COLLECTIONS, id)?
            .and_then(|d| Album::of(id, &d)))
    }

    /// Makes an album named `name` holding `images` (each once), under
    /// `id` when given (refused when taken), else a key made from the
    /// name ([`album_key`]). A name another album has is refused. The
    /// album, and what changed.
    pub fn create_album(
        &self,
        id: Option<&str>,
        name: &str,
        images: &[ImageId],
    ) -> Result<(Album, Changes), String> {
        self.create_collection(id, name, images, None)
    }

    /// Makes a smart album named `name` choosing its photos by `rules`
    /// (a group, [`crate::rules`]), under `id` or a key made from the
    /// name, as [`Vault::create_album`].
    pub fn create_smart_album(
        &self,
        id: Option<&str>,
        name: &str,
        rules: Rule,
    ) -> Result<(Album, Changes), String> {
        if !matches!(rules, Rule::Group { .. }) {
            return Err("a smart album's rules are a group".into());
        }
        self.create_collection(id, name, &[], Some(rules))
    }

    /// Sets a smart album's rules; the same again changes nothing.
    pub fn set_album_rules(&self, id: &str, rules: Rule) -> Result<Changes, String> {
        if !matches!(rules, Rule::Group { .. }) {
            return Err("a smart album's rules are a group".into());
        }
        self.change_album(id, |_, album| {
            if !album.is_smart() {
                return Err(format!("`{id}` is not a smart album: it holds its photos"));
            }
            album.rules = Some(rules);
            Ok(())
        })
    }

    fn create_collection(
        &self,
        id: Option<&str>,
        name: &str,
        images: &[ImageId],
        rules: Option<Rule>,
    ) -> Result<(Album, Changes), String> {
        let name = album_name(name)?;
        let images: Vec<String> = images.iter().map(ImageId::hex).collect();
        self.transaction(|store| create_in(store, id, name, images, rules))
    }

    /// Renames an album; the same name again changes nothing.
    pub fn rename_album(&self, id: &str, name: &str) -> Result<Changes, String> {
        let name = album_name(name)?;
        self.change_album(id, |store, album| {
            check_free(store, &name, id)?;
            album.name.clone_from(&name);
            Ok(())
        })
    }

    /// Forgets an album (its photos stay in the Library).
    pub fn delete_album(&self, id: &str) -> Result<Changes, String> {
        self.transaction(|store| {
            let before = store
                .document(COLLECTIONS, id)?
                .filter(|d| Album::of(id, d).is_some())
                .ok_or_else(|| format!("no album `{id}`"))?;
            put(store, id, Some(before), None)
        })
    }

    /// Puts photos in an album, after those it holds; one it holds
    /// already stays where it is.
    pub fn add_to_album(&self, id: &str, images: &[ImageId]) -> Result<Changes, String> {
        self.change_album(id, |_, album| {
            refuse_smart(album)?;
            for i in images {
                let hex = i.hex();
                if !album.images.contains(&hex) {
                    album.images.push(hex);
                }
            }
            Ok(())
        })
    }

    /// Takes photos out of an album (they stay in the Library).
    pub fn remove_from_album(&self, id: &str, images: &[ImageId]) -> Result<Changes, String> {
        let gone: Vec<String> = images.iter().map(ImageId::hex).collect();
        self.change_album(id, |_, album| {
            refuse_smart(album)?;
            album.images.retain(|i| !gone.contains(i));
            Ok(())
        })
    }

    /// The photos of the quick collection ([`QUICK`]), by id, in the
    /// order they were put in it (none before it was ever filled).
    pub fn quick_collection(&self) -> Result<Vec<String>, String> {
        let store = self.store.lock().expect("store lock");
        Ok(quick_of(store.document(COLLECTIONS, QUICK)?.as_ref()))
    }

    /// B: when the quick collection holds every photo of `images`, takes
    /// them out; else puts in those it lacks. Whether they were put in,
    /// and what changed.
    pub fn toggle_quick(&self, images: &[ImageId]) -> Result<(bool, Changes), String> {
        let ids: Vec<String> = images.iter().map(ImageId::hex).collect();
        self.transaction(|store| {
            let before = store.document(COLLECTIONS, QUICK)?;
            let mut quick = quick_of(before.as_ref());
            let all_in = !ids.is_empty() && ids.iter().all(|i| quick.contains(i));
            if all_in {
                quick.retain(|i| !ids.contains(i));
            } else {
                for i in &ids {
                    if !quick.contains(i) {
                        quick.push(i.clone());
                    }
                }
            }
            let changes = put(store, QUICK, before, Some(quick_doc(&quick)))?;
            Ok((!all_in, changes))
        })
    }

    /// Empties the quick collection (its photos stay in the Library).
    pub fn clear_quick(&self) -> Result<Changes, String> {
        self.transaction(|store| {
            let before = store.document(COLLECTIONS, QUICK)?;
            if quick_of(before.as_ref()).is_empty() {
                return Ok(Changes::default());
            }
            put(store, QUICK, before, Some(quick_doc(&[])))
        })
    }

    /// "Save as album": a plain album named `name` (under `id` or a key
    /// made from it) holding the quick collection's photos, which is
    /// emptied when `clear` says so; one change. An empty quick
    /// collection is refused.
    pub fn save_quick(
        &self,
        id: Option<&str>,
        name: &str,
        clear: bool,
    ) -> Result<(Album, Changes), String> {
        let name = album_name(name)?;
        self.transaction(|store| {
            let before = store.document(COLLECTIONS, QUICK)?;
            let images = quick_of(before.as_ref());
            if images.is_empty() {
                return Err("the quick collection is empty".into());
            }
            let (album, mut changes) = create_in(store, id, name, images, None)?;
            if clear {
                let cleared = put(store, QUICK, before, Some(quick_doc(&[])))?;
                changes.documents.extend(cleared.documents);
            }
            Ok((album, changes))
        })
    }

    fn change_album(
        &self,
        id: &str,
        f: impl FnOnce(&S, &mut Album) -> Result<(), String>,
    ) -> Result<Changes, String> {
        self.transaction(|store| {
            let before = store.document(COLLECTIONS, id)?;
            let mut album = before
                .as_ref()
                .and_then(|d| Album::of(id, d))
                .ok_or_else(|| format!("no album `{id}`"))?;
            f(&*store, &mut album)?;
            put(store, id, before, Some(album.doc()))
        })
    }
}

/// Makes a collection in `store` (see [`Vault::create_album`]).
fn create_in<S: Store>(
    store: &mut S,
    id: Option<&str>,
    name: String,
    images: Vec<String>,
    rules: Option<Rule>,
) -> Result<(Album, Changes), String> {
    check_free(&*store, &name, "")?;
    let id = match id {
        Some(id) if id.trim().is_empty() => return Err("an album needs an id".into()),
        Some(QUICK) => return Err(format!("`{QUICK}` is the quick collection's")),
        Some(id) => {
            if store.document(COLLECTIONS, id)?.is_some() {
                return Err(format!("a collection `{id}` exists already"));
            }
            id.to_string()
        }
        None => {
            let keys: Vec<String> = store
                .documents(COLLECTIONS)?
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            album_key(&name, |k| keys.iter().any(|t| t == k))
        }
    };
    let mut album = Album {
        id,
        name,
        images: Vec::new(),
        rules,
        count: None,
    };
    for hex in images {
        if !album.images.contains(&hex) {
            album.images.push(hex);
        }
    }
    let changes = put(store, &album.id, None, Some(album.doc()))?;
    Ok((album, changes))
}

/// The quick collection's photos in its document (none when absent).
fn quick_of(doc: Option<&Value>) -> Vec<String> {
    doc.and_then(|d| d.get("images"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// The quick collection's document holding `images`.
fn quick_doc(images: &[String]) -> Value {
    json!({ "kind": QUICK, "images": images })
}

/// Refuses to put photos in or take them out of a smart album.
fn refuse_smart(album: &Album) -> Result<(), String> {
    if album.is_smart() {
        return Err(format!(
            "“{}” is a smart album: its rules choose its photos",
            album.name
        ));
    }
    Ok(())
}

/// Writes a collection's document, answering the change (none when it
/// is the same).
fn put<S: Store>(
    store: &mut S,
    key: &str,
    before: Option<Value>,
    after: Option<Value>,
) -> Result<Changes, String> {
    if before == after {
        return Ok(Changes::default());
    }
    store.set_document(COLLECTIONS, key, after.as_ref())?;
    Ok(Changes {
        documents: vec![DocumentChange {
            ns: COLLECTIONS.into(),
            key: key.into(),
            before,
            after,
        }],
        ..Changes::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_made_from_the_name_and_kept_free() {
        assert_eq!(album_key("Trip 2024!", |_| false), "trip-2024");
        assert_eq!(album_key("  ", |_| false), "album");
        assert_eq!(album_key("Été", |_| false), "été");
        assert_eq!(album_key("a", |k| k == "a" || k == "a-2"), "a-3");
    }
}
