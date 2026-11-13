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
        self.transaction(|store| {
            check_free(&*store, &name, "")?;
            let id = match id {
                Some(id) if id.trim().is_empty() => return Err("an album needs an id".into()),
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
            for i in images {
                let hex = i.hex();
                if !album.images.contains(&hex) {
                    album.images.push(hex);
                }
            }
            let changes = put(store, &album.id, None, Some(album.doc()))?;
            Ok((album, changes))
        })
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
