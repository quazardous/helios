//! An extractor's result for one image, and where results are cached.
//!
//! Every use of a definition reads the same result through its cache key
//! ([`crate::cache`]): a re-run, another carrier, another diagram. A
//! result is either **kept** on the photo (the vault's `extract/<id>`
//! namespace, one document per image, the latest version winning) or
//! **run-only** (it goes with its run). [`ResultCache`] is the seam: the
//! call path asks it before calling a model and hands it what came back;
//! the vault implements it, [`MemoryCache`] is the reference in memory.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::answer::{Adjustment, Value};

/// One extractor's result for one image, as cached and kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Extracted {
    /// The cache key it was computed under.
    pub key: String,
    /// The image's content id.
    pub image: String,
    /// The definition's id and version.
    pub extractor: String,
    pub version: u32,
    /// The hash of what the definition computes ([`crate::def_hash`]).
    pub def_hash: String,
    /// The route the key was made with, `provider/model`; empty for a
    /// built-in extractor.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// The model as the runtime named it in its answer (its version or
    /// digest when it gives one).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub answered: String,
    /// The typed values per key (empty: none).
    pub values: BTreeMap<String, Vec<Value>>,
    /// What the check adjusted instead of refusing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adjustments: Vec<Adjustment>,
    /// When it was computed, in seconds since the Unix epoch.
    pub at: u64,
}

impl Extracted {
    /// `id@version`.
    pub fn pinned(&self) -> String {
        format!("{}@{}", self.extractor, self.version)
    }
}

/// Where results are looked up before a model is called, and stored
/// after.
pub trait ResultCache {
    /// The result cached under `key` for `image` by extractor `extractor`
    /// (its id), if any.
    fn cached(&self, key: &str, image: &str, extractor: &str) -> Result<Option<Extracted>, String>;
    /// Stores a result: kept on the photo when `keep`, else with the run.
    fn store(&self, result: &Extracted, keep: bool) -> Result<(), String>;
}

/// Results in memory, the reference a store is read against: kept
/// results by (extractor, image), the latest version winning; run-only
/// ones by key.
#[derive(Debug, Default)]
pub struct MemoryCache {
    kept: Mutex<BTreeMap<(String, String), Extracted>>,
    run: Mutex<BTreeMap<String, Extracted>>,
}

impl MemoryCache {
    /// The result kept on an image by an extractor.
    pub fn kept(&self, extractor: &str, image: &str) -> Option<Extracted> {
        self.kept
            .lock()
            .expect("cache lock")
            .get(&(extractor.to_owned(), image.to_owned()))
            .cloned()
    }

    /// Forgets the run-only results, as removing their run does.
    pub fn end_run(&self) {
        self.run.lock().expect("cache lock").clear();
    }
}

impl ResultCache for MemoryCache {
    fn cached(&self, key: &str, image: &str, extractor: &str) -> Result<Option<Extracted>, String> {
        if let Some(kept) = self.kept(extractor, image).filter(|r| r.key == key) {
            return Ok(Some(kept));
        }
        Ok(self.run.lock().expect("cache lock").get(key).cloned())
    }

    fn store(&self, result: &Extracted, keep: bool) -> Result<(), String> {
        if keep {
            let mut kept = self.kept.lock().expect("cache lock");
            let slot = (result.extractor.clone(), result.image.clone());
            if kept
                .get(&slot)
                .is_none_or(|old| old.version <= result.version)
            {
                kept.insert(slot, result.clone());
                return Ok(());
            }
        }
        self.run
            .lock()
            .expect("cache lock")
            .insert(result.key.clone(), result.clone());
        Ok(())
    }
}
