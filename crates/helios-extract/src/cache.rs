//! The cache key of an extractor's result for one image.
//!
//! An LLM extractor's result is cached per image, and every use of the
//! same definition reads it: two carriers in one diagram, another
//! diagram, a later run. The key holds everything that can change the
//! answer, so a change is a miss, never a stale hit:
//!
//! - the image's content id, and its source (the embedded preview, or
//!   the id of the head version rendered small);
//! - the definition's `id@version`, and the hash of what it computes
//!   ([`def_hash`]: its kind and keys; not its title or description);
//! - the model that answers (`provider/model`, with the model's own
//!   version or digest when the runtime gives one);
//! - the use's parameters, and the context variables' values filled for
//!   this image.
//!
//! The key is BLAKE3 over a canonical JSON text (object keys sorted, no
//! spaces) of those parts, prefixed by a scheme number: [`SCHEME`]
//! changes only if the layout changes, which empties every cache once.

use std::collections::BTreeMap;

use serde_json::{Value as Json, json};

use crate::decl::{ExtractorDef, ExtractorUse};

/// The key layout's version.
pub const SCHEME: u32 = 1;

/// What an extractor's cached result depends on.
#[derive(Clone, Debug)]
pub struct CacheInput<'a> {
    /// The image's content id.
    pub image: &'a str,
    /// The head version rendered for the extractor, `None` for the
    /// embedded preview (or for extractors that read no pixels).
    pub image_version: Option<&'a str>,
    pub def: &'a ExtractorDef,
    /// The answering model, `provider/model[@digest]`; empty for a
    /// built-in extractor.
    pub model: &'a str,
    /// The use's parameters.
    pub params: &'a BTreeMap<String, Json>,
    /// The context variables' values for this image.
    pub inputs: &'a BTreeMap<String, String>,
}

impl<'a> CacheInput<'a> {
    /// The input of a use of `def` on an image, before inputs are
    /// filled.
    pub fn new(
        image: &'a str,
        def: &'a ExtractorDef,
        u: &'a ExtractorUse,
        model: &'a str,
        inputs: &'a BTreeMap<String, String>,
    ) -> Self {
        Self {
            image,
            image_version: None,
            def,
            model,
            params: &u.params,
            inputs,
        }
    }

    /// The key: 64 hexadecimal digits.
    pub fn key(&self) -> String {
        let doc = json!({
            "scheme": SCHEME,
            "image": self.image,
            "source": self.image_version.unwrap_or("preview"),
            "extractor": self.def.pinned(),
            "def": def_hash(self.def),
            "model": self.model,
            "params": self.params,
            "inputs": self.inputs,
        });
        blake3::hash(canonical(&doc).as_bytes())
            .to_hex()
            .to_string()
    }
}

/// The hash of what a definition computes: its kind (prompt, capability,
/// computation) and its keys, and its confidence floor.
pub fn def_hash(def: &ExtractorDef) -> String {
    let doc = json!({
        "kind": def.kind,
        "keys": def.keys,
        "floor": def.confidence_floor,
    });
    blake3::hash(canonical(&doc).as_bytes())
        .to_hex()
        .to_string()
}

/// JSON text with object keys sorted at every level and no spaces,
/// whatever map order `serde_json` was built with.
fn canonical(v: &Json) -> String {
    match v {
        Json::Object(map) => {
            let sorted: BTreeMap<&String, &Json> = map.iter().collect();
            let parts: Vec<String> = sorted
                .into_iter()
                .map(|(k, v)| format!("{}:{}", Json::String(k.clone()), canonical(v)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Json::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}
