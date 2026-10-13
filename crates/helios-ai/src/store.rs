//! The AI settings of this machine (`ai.toml`) and where AI files live.
//!
//! ```toml
//! [[providers]]
//! id = "lan"
//! kind = "openai"
//! url = "http://192.168.1.20:8080/v1"
//!
//! [routing.capabilities]
//! vision = { provider = "ollama", model = "qwen2.5vl:7b" }
//!
//! [privacy]
//! remote_image_side = 1024
//!
//! [limits]
//! calls = 2
//! ```
//!
//! With no provider listed, Helios knows one: Ollama on loopback
//! ([`AiConfig::providers`]). A provider listed with that id replaces it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::provider::{ProviderConfig, ProviderKind};
use crate::routing::Routing;

/// The id of the provider Helios knows without being told.
pub const DEFAULT_PROVIDER: &str = "ollama";

/// Where AI files live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// `ai.toml`.
    pub config: PathBuf,
    /// User prompt versions.
    pub prompts: PathBuf,
    /// `calls.jsonl`.
    pub log: PathBuf,
    /// Downloads and temporary files (a runtime's archive).
    pub cache: PathBuf,
}

impl Paths {
    /// Everything under one directory (a sandbox, a test).
    pub fn under(dir: &Path) -> Self {
        Paths {
            config: dir.join("ai.toml"),
            prompts: dir.join("prompts"),
            log: dir.join("calls.jsonl"),
            cache: dir.join("cache"),
        }
    }

    /// `$HELIOS_AI_DIR` when set; else the user's configuration, state and
    /// cache directories (`~/.config/helios/ai`, `~/.local/state/helios/ai`,
    /// `~/.cache/helios/ai`).
    pub fn default_for_user() -> Self {
        if let Some(dir) = std::env::var_os("HELIOS_AI_DIR") {
            return Paths::under(Path::new(&dir));
        }
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let xdg = |var: &str, fallback: &str| {
            std::env::var_os(var)
                .map_or_else(|| home.join(fallback), PathBuf::from)
                .join("helios/ai")
        };
        let config = xdg("XDG_CONFIG_HOME", ".config");
        Paths {
            config: config.join("ai.toml"),
            prompts: config.join("prompts"),
            log: xdg("XDG_STATE_HOME", ".local/state").join("calls.jsonl"),
            cache: xdg("XDG_CACHE_HOME", ".cache"),
        }
    }
}

/// How images leave the process.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Privacy {
    /// The longest side of an image sent to a remote provider, in pixels.
    #[serde(default = "remote_side")]
    pub remote_image_side: u32,
    /// The longest side of an image sent to a local provider.
    #[serde(default = "local_side")]
    pub local_image_side: u32,
}

fn remote_side() -> u32 {
    1024
}

fn local_side() -> u32 {
    2048
}

impl Default for Privacy {
    fn default() -> Self {
        Privacy {
            remote_image_side: remote_side(),
            local_image_side: local_side(),
        }
    }
}

/// How many calls at once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    #[serde(default = "two")]
    pub calls: u64,
}

fn two() -> u64 {
    2
}

impl Default for Limits {
    fn default() -> Self {
        Limits { calls: two() }
    }
}

/// `ai.toml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default, rename = "providers", skip_serializing_if = "Vec::is_empty")]
    pub listed: Vec<ProviderConfig>,
    #[serde(default)]
    pub routing: Routing,
    #[serde(default)]
    pub privacy: Privacy,
    #[serde(default)]
    pub limits: Limits,
}

impl AiConfig {
    /// The file at `path`; the defaults when there is none.
    pub fn load(path: &Path) -> Result<AiConfig, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AiConfig::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes it whole, through a temporary file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| format!("ai.toml: {e}"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let partial = path.with_extension("toml.partial");
        std::fs::write(&partial, text)
            .and_then(|()| std::fs::rename(&partial, path))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The providers in effect: the listed ones, and Ollama on loopback
    /// unless one is listed under its id. `OLLAMA_HOST` moves it, as it
    /// moves Ollama.
    pub fn providers(&self) -> Vec<ProviderConfig> {
        let mut all = self.listed.clone();
        if !all.iter().any(|p| p.id == DEFAULT_PROVIDER) {
            let mut ollama = ProviderConfig::new(DEFAULT_PROVIDER, ProviderKind::Ollama);
            ollama.url = std::env::var("OLLAMA_HOST").ok().map(|h| {
                if h.starts_with("http://") || h.starts_with("https://") {
                    h
                } else {
                    format!("http://{h}")
                }
            });
            all.insert(0, ollama);
        }
        all
    }

    /// Adds `provider`, or replaces the one with its id.
    pub fn put(&mut self, provider: ProviderConfig) {
        match self.listed.iter_mut().find(|p| p.id == provider.id) {
            Some(slot) => *slot = provider,
            None => self.listed.push(provider),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Capability;
    use crate::routing::Route;
    use crate::secrets::KeySource;

    #[test]
    fn ollama_is_known_without_being_listed() {
        let config = AiConfig::default();
        let providers = config.providers();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].kind, ProviderKind::Ollama);
        assert!(!providers[0].remote());
    }

    #[test]
    fn the_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("helios-ai-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = Paths::under(&dir);
        assert_eq!(AiConfig::load(&paths.config).unwrap(), AiConfig::default());
        let mut config = AiConfig::default();
        let mut remote = ProviderConfig::new("openai", ProviderKind::OpenAi);
        remote.key = Some(KeySource::Env("OPENAI_API_KEY".into()));
        config.put(remote);
        config
            .routing
            .capabilities
            .insert(Capability::Vision, Route::new("ollama", "qwen2.5vl:7b"));
        config.save(&paths.config).unwrap();
        let text = std::fs::read_to_string(&paths.config).unwrap();
        assert!(text.contains("OPENAI_API_KEY") && !text.contains("sk-"));
        assert_eq!(AiConfig::load(&paths.config).unwrap(), config);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
