//! An extractor run on one image through the cache
//! (`docs/design/extractors.md`, section 7).
//!
//! [`Ai::run_extract`] makes the cache key of the call (the image and its
//! source, the definition's `id@version` and hash, the route the call
//! resolves to, the use's parameters, the context inputs), asks the
//! [`ResultCache`] first, and calls the model only on a miss. What came
//! back is stored kept on the photo or with the run, as the use says.
//! Changing the extractor's version, what it computes or the model is a
//! miss, never a stale hit.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use helios_extract::{CacheInput, Extracted, ExtractorUse, ResultCache, def_hash};

use crate::service::{Ai, Attempt, PromptCall, PromptFailure, PromptRun};

/// One extractor use on one image.
#[derive(Clone, Debug)]
pub struct ExtractCall {
    /// The prompt call; its `extractor` must be set.
    pub call: PromptCall,
    /// The use: its pinned version, parameters and keep override.
    pub using: ExtractorUse,
    /// The image's content id.
    pub image: String,
    /// The head version rendered for the call, `None` for the embedded
    /// preview.
    pub image_version: Option<String>,
    /// The context variables' values filled for this image.
    pub inputs: BTreeMap<String, String>,
}

/// An extractor's result, and whether it came from the cache.
#[derive(Clone, Debug)]
pub struct Extraction {
    pub result: Extracted,
    /// True when no model was called.
    pub hit: bool,
    /// The model call, on a miss.
    pub run: Option<PromptRun>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Ai {
    /// Runs an extractor on one image, reading `cache` first: a hit
    /// calls no model; a miss runs the prompt (with its one corrective
    /// retry) and stores the typed answer, kept on the photo or with the
    /// run as the use says. A failed call stores nothing.
    pub fn run_extract(
        &self,
        x: ExtractCall,
        cache: &dyn ResultCache,
    ) -> Result<Extraction, PromptFailure> {
        let fail = |prompt: String, e: String| PromptFailure {
            prompt,
            route: None,
            attempts: vec![Attempt {
                raw: None,
                errors: vec![e],
            }],
        };
        let reference = x.call.reference.to_string();
        let Some(def) = x.call.extractor.clone() else {
            return Err(fail(reference, "no extractor given".into()));
        };
        let prompt = self
            .prompt(&x.call.reference)
            .map_err(|e| fail(reference.clone(), e))?;
        let (route, _) = self
            .resolve(prompt.capability, Some(&prompt.id), &x.call.overrides)
            .map_err(|e| fail(prompt.reference(), e))?;
        let model = format!("{}/{}", route.provider, route.model);
        let mut input = CacheInput::new(&x.image, &def, &x.using, &model, &x.inputs);
        input.image_version = x.image_version.as_deref();
        let key = input.key();
        let keep = x.using.keeps(&def);

        let cached = cache
            .cached(&key, &x.image, &def.id)
            .map_err(|e| fail(prompt.reference(), format!("the cache: {e}")))?;
        if let Some(result) = cached {
            return Ok(Extraction {
                result,
                hit: true,
                run: None,
            });
        }

        let run = self.run_prompt_with(x.call)?;
        let answer = run.answer.clone().unwrap_or_default();
        let result = Extracted {
            key,
            image: x.image,
            extractor: def.id.clone(),
            version: def.version,
            def_hash: def_hash(&def),
            model,
            answered: run.answered.clone(),
            values: answer.values,
            adjustments: answer.adjustments,
            at: now(),
        };
        cache.store(&result, keep).map_err(|e| PromptFailure {
            prompt: run.prompt.clone(),
            route: Some(run.route.clone()),
            attempts: vec![Attempt {
                raw: None,
                errors: vec![format!("the answer fitted, but storing it failed: {e}")],
            }],
        })?;
        Ok(Extraction {
            result,
            hit: false,
            run: Some(run),
        })
    }
}
