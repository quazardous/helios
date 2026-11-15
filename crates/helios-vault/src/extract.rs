//! Extractor results in the vault (`docs/design/extractors.md`, section
//! 7): kept on the photo, or run-only.
//!
//! - **Kept:** namespace `extract/<id>` per extractor, one document per
//!   image (keyed by its content id): the values, the version, the
//!   route, when. The latest version wins per image: a result of an older
//!   version than the one kept is stored with its run instead. Kept data
//!   outlives every run.
//! - **Run-only:** `workflow/<run>/extract`, one document per cache key;
//!   removing the run ([`crate::workflow::WorkflowTx::remove_run`])
//!   removes them.
//!
//! A lookup by cache key reads the image's kept document first, then the
//! run's own results, then those of the other runs still kept: a re-run
//! or another diagram calls no model while a result for the same key is
//! known.

use helios_extract::{Extracted, ResultCache};
use serde_json::Value;

use crate::{Store, Vault};

/// The kept results of an extractor.
pub fn kept_namespace(extractor: &str) -> String {
    format!("extract/{extractor}")
}

/// A run's own results.
pub fn run_namespace(run: &str) -> String {
    format!("workflow/{run}/extract")
}

fn parse(doc: Value) -> Result<Extracted, String> {
    serde_json::from_value(doc).map_err(|e| format!("an extractor result does not read: {e}"))
}

fn to_doc(result: &Extracted) -> Result<Value, String> {
    serde_json::to_value(result).map_err(|e| e.to_string())
}

impl<S: Store> Vault<S> {
    /// The result an extractor (its id) keeps on an image.
    pub fn kept_extract(&self, extractor: &str, image: &str) -> Result<Option<Extracted>, String> {
        self.read(|s| s.document(&kept_namespace(extractor), image))?
            .map(parse)
            .transpose()
    }

    /// Every result an extractor keeps, one per image.
    pub fn kept_extracts(&self, extractor: &str) -> Result<Vec<Extracted>, String> {
        self.read(|s| s.documents(&kept_namespace(extractor)))?
            .into_iter()
            .map(|(_, doc)| parse(doc))
            .collect()
    }

    /// Forgets what an extractor keeps on an image; false if it kept
    /// nothing.
    pub fn forget_kept_extract(&self, extractor: &str, image: &str) -> Result<bool, String> {
        let ns = kept_namespace(extractor);
        self.transaction(|s| {
            let had = s.document(&ns, image)?.is_some();
            if had {
                s.set_document(&ns, image, None)?;
            }
            Ok(had)
        })
    }

    /// The results a run holds, by cache key.
    pub fn run_extracts(&self, run: &str) -> Result<Vec<Extracted>, String> {
        self.read(|s| s.documents(&run_namespace(run)))?
            .into_iter()
            .map(|(_, doc)| parse(doc))
            .collect()
    }

    /// The result known under a cache key: kept on the image by this
    /// extractor, held by `run`, or held by another run still kept.
    pub fn cached_extract(
        &self,
        key: &str,
        image: &str,
        extractor: &str,
        run: Option<&str>,
    ) -> Result<Option<Extracted>, String> {
        if let Some(kept) = self
            .kept_extract(extractor, image)?
            .filter(|r| r.key == key)
        {
            return Ok(Some(kept));
        }
        self.read(|s| {
            if let Some(run) = run
                && let Some(doc) = s.document(&run_namespace(run), key)?
            {
                return parse(doc).map(Some);
            }
            for (other, _) in s.documents("workflow/runs")? {
                if Some(other.as_str()) == run {
                    continue;
                }
                if let Some(doc) = s.document(&run_namespace(&other), key)? {
                    return parse(doc).map(Some);
                }
            }
            Ok(None)
        })
    }

    /// Stores a result: kept on the photo when `keep` (unless a newer
    /// version is kept there), else with `run`. A run-only result outside
    /// a run is not stored. Says where it went.
    pub fn put_extract(
        &self,
        result: &Extracted,
        keep: bool,
        run: Option<&str>,
    ) -> Result<Stored, String> {
        let doc = to_doc(result)?;
        let kept_ns = kept_namespace(&result.extractor);
        self.transaction(|s| {
            if keep {
                let newer = match s.document(&kept_ns, &result.image)? {
                    Some(old) => parse(old)?.version > result.version,
                    None => false,
                };
                if !newer {
                    s.set_document(&kept_ns, &result.image, Some(&doc))?;
                    return Ok(Stored::Kept);
                }
            }
            match run {
                Some(run) => {
                    s.set_document(&run_namespace(run), &result.key, Some(&doc))?;
                    Ok(Stored::Run)
                }
                None => Ok(Stored::Nowhere),
            }
        })
    }

    /// The cache a run's extractor calls read and write.
    pub fn extract_cache(&self, run: Option<&str>) -> RunCache<'_, S> {
        RunCache {
            vault: self,
            run: run.map(str::to_owned),
        }
    }
}

/// Where [`Vault::put_extract`] stored a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stored {
    /// On the photo, `extract/<id>`.
    Kept,
    /// With the run.
    Run,
    /// Run-only, with no run: not stored.
    Nowhere,
}

/// The vault as the [`ResultCache`] of one run (or of no run: kept
/// results only).
pub struct RunCache<'a, S: Store> {
    vault: &'a Vault<S>,
    run: Option<String>,
}

impl<S: Store> ResultCache for RunCache<'_, S> {
    fn cached(&self, key: &str, image: &str, extractor: &str) -> Result<Option<Extracted>, String> {
        self.vault
            .cached_extract(key, image, extractor, self.run.as_deref())
    }

    fn store(&self, result: &Extracted, keep: bool) -> Result<(), String> {
        self.vault
            .put_extract(result, keep, self.run.as_deref())
            .map(|_| ())
    }
}
