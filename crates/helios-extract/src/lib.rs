//! Extractors: blocks that compute typed data about an image and change
//! nothing (see `docs/design/extractors.md`).
//!
//! An extractor is not a diagram node: it snaps onto a carrier node (a
//! router, a filter, an updater), which reads its declared keys while the
//! diagram is designed and hands it each image at run time. This crate
//! holds the contract only, with no computation and no model call:
//!
//! - [`decl`]: the keys an extractor declares ([`KeyDecl`]), the reusable
//!   definition ([`ExtractorDef`]: an LLM prompt or a built-in
//!   computation), a use attached to a carrier ([`ExtractorUse`]), the
//!   manifest shape modules ship and the catalogue;
//! - [`schema`]: the JSON schema of a model's answer derived from the
//!   keys, and the words describing each key;
//! - [`answer`]: a reply parsed leniently, checked and typed, out-of-list
//!   values dropped or mapped to `other` and counted, errors ready for
//!   one corrective retry;
//! - [`cache`]: the key of a result shared by every use;
//! - [`builtins`]: the extractors Helios ships, declared.
//!
//! It depends on no other Helios crate, so carriers, the extractor
//! editor, module manifests and helios-ai's call path can all read it.

pub mod answer;
pub mod cache;
pub mod decl;
pub mod schema;

pub use answer::{Adjust, Answer, Value, check_reply, parse_reply, retry_note, validate};
pub use cache::{CacheInput, def_hash};
pub use decl::{
    BuiltinSpec, Capability, Catalog, ContextVar, DeclError, ExtractorDef, ExtractorUse, KeyDecl,
    KeyType, Kind, LlmSpec, Manifest, Values,
};
pub use schema::{answer_schema, key_instructions};

/// The extractors Helios ships, declared (`builtins.toml`).
pub fn builtins() -> Manifest {
    Manifest::from_toml(include_str!("builtins.toml")).expect("the built-in extractors are sound")
}

#[cfg(test)]
mod tests;
