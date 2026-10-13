//! The provider contract: what a model runtime or API offers Helios.
//!
//! A [`ModelProvider`] declares its [`Capability`]s and answers the calls
//! that go with them. A call it cannot make says so (`unsupported`). A
//! provider is built from its entry in `ai.toml` ([`ProviderConfig`]) by
//! [`build`].

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Capability;
use crate::secrets::{KeySource, Secrets};
use crate::transport::Transport;

/// An image sent to a model, already encoded.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageData {
    /// `image/png` or `image/jpeg`.
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// A chat call: a prompt, maybe images, maybe a schema the answer must fit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub prompt: String,
    pub images: Vec<ImageData>,
    /// The answer must be JSON fitting this schema (structured output).
    pub schema: Option<Value>,
    /// The answer must be a JSON object (any).
    pub json: bool,
    pub temperature: Option<f64>,
    pub seed: Option<u64>,
    pub max_tokens: u32,
}

/// A chat answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ChatReply {
    pub text: String,
    /// The model as the runtime names it (its exact version when it says).
    pub model: String,
}

/// A segmentation call: one image, what to find.
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentRequest {
    pub model: String,
    pub image: ImageData,
    /// What to find, in words or as a kind (`sky`, `the red car`).
    pub prompt: String,
}

/// A mask: a grayscale PNG (luminance is coverage) and a confidence.
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentReply {
    pub png: Vec<u8>,
    pub confidence: f64,
    pub model: String,
}

/// A model a provider offers.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelInfo {
    pub name: String,
    /// On disk, when the runtime says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// What the model itself can do, as far as Helios can tell.
    pub capabilities: Vec<Capability>,
}

/// Progress of a model download.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PullProgress {
    pub status: String,
    pub completed: u64,
    pub total: u64,
}

/// The kinds of provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Ollama,
    /// Any OpenAI-compatible server: llama.cpp's, LM Studio, `vLLM`, OpenAI.
    #[serde(rename = "openai")]
    OpenAi,
    Anthropic,
    /// A local program speaking JSON.
    Command,
}

impl ProviderKind {
    pub fn name(self) -> &'static str {
        match self {
            ProviderKind::Ollama => "ollama",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Command => "command",
        }
    }

    pub fn parse(name: &str) -> Option<ProviderKind> {
        [
            ProviderKind::Ollama,
            ProviderKind::OpenAi,
            ProviderKind::Anthropic,
            ProviderKind::Command,
        ]
        .into_iter()
        .find(|k| k.name() == name)
    }

    /// What a provider of this kind can do, unless its entry narrows it.
    pub fn capabilities(self) -> &'static [Capability] {
        match self {
            ProviderKind::Ollama | ProviderKind::OpenAi => &[
                Capability::Chat,
                Capability::Vision,
                Capability::Embeddings,
                Capability::Structured,
            ],
            ProviderKind::Anthropic => {
                &[Capability::Chat, Capability::Vision, Capability::Structured]
            }
            ProviderKind::Command => &[Capability::Segmentation],
        }
    }

    /// Its URL when the entry gives none.
    pub fn default_url(self) -> Option<&'static str> {
        match self {
            ProviderKind::Ollama => Some(crate::ollama::DEFAULT_URL),
            ProviderKind::OpenAi => Some("https://api.openai.com/v1"),
            ProviderKind::Anthropic => Some("https://api.anthropic.com/v1"),
            ProviderKind::Command => None,
        }
    }
}

/// A provider's entry in `ai.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub kind: ProviderKind,
    /// The base URL (`http://127.0.0.1:11434`, `http://localhost:8080/v1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<KeySource>,
    /// A command provider's program and arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// Narrows what the kind declares (a text-only server: no vision).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<Capability>>,
    /// Seconds a call may take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

impl ProviderConfig {
    pub fn new(id: &str, kind: ProviderKind) -> Self {
        ProviderConfig {
            id: id.into(),
            kind,
            url: None,
            key: None,
            command: Vec::new(),
            capabilities: None,
            timeout: None,
        }
    }

    pub fn url(&self) -> Option<String> {
        self.url
            .clone()
            .or_else(|| self.kind.default_url().map(str::to_string))
            .map(|u| u.trim_end_matches('/').to_string())
    }

    pub fn capabilities(&self) -> Vec<Capability> {
        match &self.capabilities {
            Some(narrowed) => self
                .kind
                .capabilities()
                .iter()
                .copied()
                .filter(|c| narrowed.contains(c))
                .collect(),
            None => self.kind.capabilities().to_vec(),
        }
    }

    /// Whether calls leave this machine: a URL whose host is not loopback.
    pub fn remote(&self) -> bool {
        match self.kind {
            ProviderKind::Command => false,
            _ => self.url().is_none_or(|u| !is_loopback(&u)),
        }
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout.unwrap_or(300))
    }

    /// Whether the entry can be built; why not otherwise.
    pub fn check(&self) -> Result<(), String> {
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        {
            return Err(format!(
                "provider id `{}`: letters, digits, `-`, `_` and `.` only",
                self.id
            ));
        }
        match self.kind {
            ProviderKind::Command if self.command.is_empty() => {
                Err("a command provider names its program (`command`)".into())
            }
            ProviderKind::Command => Ok(()),
            _ => match self.url() {
                Some(u) if u.starts_with("http://") || u.starts_with("https://") => Ok(()),
                Some(u) => Err(format!("`{u}` is not an http(s) URL")),
                None => Err("no URL".into()),
            },
        }
    }
}

/// Whether a URL's host is this machine.
pub fn is_loopback(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split('/').next().unwrap_or(rest);
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or(bracketed)
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// What a provider does: declared capabilities, and the calls.
pub trait ModelProvider: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> ProviderKind;
    fn capabilities(&self) -> Vec<Capability>;
    /// Whether calls leave this machine.
    fn remote(&self) -> bool;
    /// Whether it answers; its version or a short status.
    fn health(&self) -> Result<String, String>;
    fn models(&self) -> Result<Vec<ModelInfo>, String>;
    fn chat(&self, request: &ChatRequest) -> Result<ChatReply, String>;

    fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let _ = (model, texts);
        Err(unsupported(self.id(), Capability::Embeddings))
    }

    fn segment(&self, request: &SegmentRequest) -> Result<SegmentReply, String> {
        let _ = request;
        Err(unsupported(self.id(), Capability::Segmentation))
    }

    /// Downloads a model, telling `progress` as it goes; stops when
    /// `progress` returns false.
    fn pull(
        &self,
        model: &str,
        progress: &mut dyn FnMut(&PullProgress) -> bool,
    ) -> Result<(), String> {
        let _ = (model, progress);
        Err(format!("`{}` cannot download models", self.id()))
    }
}

/// The error for a capability a provider does not have.
pub fn unsupported(id: &str, capability: Capability) -> String {
    format!("`{id}` does not do {capability}")
}

/// What a provider needs from its surroundings.
#[derive(Clone)]
pub struct Parts {
    pub transport: Arc<dyn Transport>,
    pub secrets: Arc<dyn Secrets>,
}

/// The provider an entry describes.
pub fn build(config: &ProviderConfig, parts: &Parts) -> Result<Arc<dyn ModelProvider>, String> {
    config.check()?;
    let url = config.url().unwrap_or_default();
    Ok(match config.kind {
        ProviderKind::Ollama => Arc::new(crate::ollama::Ollama::new(
            config,
            &url,
            parts.transport.clone(),
        )),
        ProviderKind::OpenAi => Arc::new(crate::openai::OpenAiCompat::new(
            config,
            &url,
            parts.transport.clone(),
            parts.secrets.clone(),
        )),
        ProviderKind::Anthropic => Arc::new(crate::anthropic::Anthropic::new(
            config,
            &url,
            parts.transport.clone(),
            parts.secrets.clone(),
        )),
        ProviderKind::Command => Arc::new(crate::command::CommandProvider::new(config)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_urls_are_local() {
        assert!(is_loopback("http://127.0.0.1:11434"));
        assert!(is_loopback("http://localhost:8080/v1"));
        assert!(is_loopback("http://[::1]:8080/v1"));
        assert!(!is_loopback("https://api.openai.com/v1"));
        assert!(!is_loopback("http://192.168.1.4:11434"));
        let mut c = ProviderConfig::new("o", ProviderKind::OpenAi);
        assert!(c.remote(), "OpenAI's default URL is remote");
        c.url = Some("http://localhost:1234/v1/".into());
        assert!(!c.remote());
        assert_eq!(c.url().unwrap(), "http://localhost:1234/v1");
    }

    #[test]
    fn an_entry_narrows_its_capabilities() {
        let mut c = ProviderConfig::new("llama", ProviderKind::OpenAi);
        c.capabilities = Some(vec![Capability::Chat, Capability::Segmentation]);
        assert_eq!(c.capabilities(), vec![Capability::Chat]);
    }

    #[test]
    fn entries_are_checked() {
        assert!(
            ProviderConfig::new("a b", ProviderKind::Ollama)
                .check()
                .is_err()
        );
        assert!(
            ProviderConfig::new("seg", ProviderKind::Command)
                .check()
                .is_err()
        );
        let mut c = ProviderConfig::new("x", ProviderKind::Ollama);
        c.url = Some("ftp://x".into());
        assert!(c.check().is_err());
        assert!(
            ProviderConfig::new("ollama", ProviderKind::Ollama)
                .check()
                .is_ok()
        );
    }
}
