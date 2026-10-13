//! AI models for Helios (see `docs/ai-models.md`): who answers model
//! calls, for what, with which prompt, and what each call did.
//!
//! - **Providers** ([`provider::ModelProvider`]) declare their
//!   [`Capability`]s: Ollama ([`ollama`]), any OpenAI-compatible server
//!   ([`openai`]), Anthropic ([`anthropic`]) and a local program
//!   ([`command`], segmentation).
//! - **Routing** ([`routing::Routing`]): which provider and model answer a
//!   capability, or one prompt.
//! - **Prompts** ([`prompts`]) are versioned files with an input and an
//!   output schema; shipped ones never change, user ones are added as new
//!   versions, and a caller can pin `id@version`.
//! - **The local runtime** ([`local`]): probe the machine, recommend
//!   models, set up Ollama step by step (only confirmed steps run, never as
//!   root), pull models as jobs.
//! - **The service** ([`Ai`]): every call runs as a scheduler job, is
//!   published on the AI bus and appended to the call log; images are
//!   downscaled and re-encoded before they leave the process.
//!
//! Helios ships no model and calls no network on its own: with nothing
//! configured, the only provider is Ollama on loopback, and nothing is
//! routed until an agent (or the bench) says so.
//!
//! Nothing here is a chat window: MCP is the control surface
//! ([`tools::call`] answers the `ai.*` requests).

pub mod anthropic;
pub mod command;
pub mod fake;
pub mod images;
pub mod local;
pub mod ollama;
pub mod openai;
pub mod prompts;
pub mod provider;
pub mod routing;
pub mod schema;
pub mod secrets;
mod service;
pub mod store;
pub mod tools;
pub mod transport;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use service::{AI_CALLS, Ai, BenchRequest, CallMeta, CallRecord, TryResult, read_image};

/// What a provider (or a model) can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Text in, text out.
    Chat,
    /// Text and images in, text out.
    Vision,
    /// Text in, vectors out.
    Embeddings,
    /// An answer fitting a JSON schema.
    Structured,
    /// An image and a prompt in, a mask out.
    Segmentation,
}

impl Capability {
    pub const ALL: [Capability; 5] = [
        Capability::Chat,
        Capability::Vision,
        Capability::Embeddings,
        Capability::Structured,
        Capability::Segmentation,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Capability::Chat => "chat",
            Capability::Vision => "vision",
            Capability::Embeddings => "embeddings",
            Capability::Structured => "structured",
            Capability::Segmentation => "segmentation",
        }
    }

    pub fn parse(name: &str) -> Option<Capability> {
        Capability::ALL.into_iter().find(|c| c.name() == name)
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The JSON object in a model's text, fenced in a code block or not.
pub fn json_in(text: &str) -> Result<serde_json::Value, String> {
    let start = text.find('{').ok_or("the model's reply holds no JSON")?;
    let end = text.rfind('}').ok_or("the model's reply holds no JSON")?;
    if end < start {
        return Err("the model's reply holds no JSON".into());
    }
    serde_json::from_str(&text[start..=end]).map_err(|e| format!("the model's JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_round_trip_by_name() {
        for c in Capability::ALL {
            assert_eq!(Capability::parse(c.name()), Some(c));
        }
        assert_eq!(Capability::parse("telepathy"), None);
    }

    #[test]
    fn json_is_found_in_a_fenced_reply() {
        let v = json_in("Sure:\n```json\n{\"a\": 1}\n```").unwrap();
        assert_eq!(v["a"], 1);
        assert!(json_in("no braces").is_err());
        assert!(json_in("} then {").is_err());
    }
}
