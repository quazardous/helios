//! Ollama: a local model runtime with its own API.
//!
//! - `GET /api/version`: health;
//! - `GET /api/tags`: the models installed, then `POST /api/show` per model
//!   for what it can do (`capabilities`: `completion`, `vision`,
//!   `embedding`), its name's family when the runtime is too old to say;
//! - `POST /api/chat`: chat and vision (images as base64), structured
//!   output through `format` (a JSON schema, or `"json"`), `temperature`
//!   and `seed` in `options`;
//! - `POST /api/embed`: embeddings;
//! - `POST /api/pull`: a download, its progress streamed as JSON lines.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use crate::Capability;
use crate::provider::{
    ChatReply, ChatRequest, ModelInfo, ModelProvider, ProviderConfig, ProviderKind, PullProgress,
};
use crate::transport::{HttpCall, Transport, json_reply, send_json};

/// Where Ollama listens unless told otherwise.
pub const DEFAULT_URL: &str = "http://127.0.0.1:11434";

/// How long a pull may take: models are gigabytes.
const PULL_TIMEOUT: Duration = Duration::from_hours(12);

pub struct Ollama {
    id: String,
    url: String,
    capabilities: Vec<Capability>,
    remote: bool,
    timeout: Duration,
    transport: Arc<dyn Transport>,
}

impl Ollama {
    pub fn new(config: &ProviderConfig, url: &str, transport: Arc<dyn Transport>) -> Self {
        Ollama {
            id: config.id.clone(),
            url: url.to_string(),
            capabilities: config.capabilities(),
            remote: config.remote(),
            timeout: config.timeout(),
            transport,
        }
    }

    fn get(&self, path: &str, timeout: Duration) -> Result<Value, String> {
        send_json(
            self.transport.as_ref(),
            &HttpCall::get(&format!("{}{path}", self.url), timeout),
        )
    }

    fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        send_json(
            self.transport.as_ref(),
            &HttpCall::post(&format!("{}{path}", self.url), body, self.timeout),
        )
    }
}

/// What a model can do, from `/api/show`'s `capabilities` when given, else
/// from its name.
pub fn model_capabilities(name: &str, show: Option<&Value>) -> Vec<Capability> {
    if let Some(list) = show
        .and_then(|s| s.get("capabilities"))
        .and_then(Value::as_array)
    {
        let said: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
        let mut caps = Vec::new();
        if said.contains(&"completion") {
            caps.extend([Capability::Chat, Capability::Structured]);
        }
        if said.contains(&"vision") {
            caps.push(Capability::Vision);
        }
        if said.contains(&"embedding") {
            caps.push(Capability::Embeddings);
        }
        caps.sort();
        return caps;
    }
    let lower = name.to_ascii_lowercase();
    if lower.contains("embed") || lower.contains("bge-") || lower.starts_with("all-minilm") {
        return vec![Capability::Embeddings];
    }
    let vision = [
        "llava",
        "bakllava",
        "qwen2.5vl",
        "qwen2.5-vl",
        "qwen3-vl",
        "llama3.2-vision",
        "llama4",
        "minicpm-v",
        "moondream",
        "granite3.2-vision",
        "mistral-small3.1",
        "mistral-small3.2",
        "gemma3:4b",
        "gemma3:12b",
        "gemma3:27b",
    ];
    let mut caps = vec![Capability::Chat, Capability::Structured];
    if vision.iter().any(|v| lower.starts_with(v)) {
        caps.push(Capability::Vision);
    }
    caps.sort();
    caps
}

/// A progress line of `/api/pull`, or the error it carries.
pub fn pull_line(line: &str) -> Result<PullProgress, String> {
    let v: Value =
        serde_json::from_str(line).map_err(|e| format!("a pull progress line: {e}: {line}"))?;
    if let Some(error) = v.get("error").and_then(Value::as_str) {
        return Err(error.to_string());
    }
    Ok(PullProgress {
        status: v["status"].as_str().unwrap_or_default().to_string(),
        completed: v["completed"].as_u64().unwrap_or(0),
        total: v["total"].as_u64().unwrap_or(0),
    })
}

impl ModelProvider for Ollama {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }

    fn capabilities(&self) -> Vec<Capability> {
        self.capabilities.clone()
    }

    fn remote(&self) -> bool {
        self.remote
    }

    fn health(&self) -> Result<String, String> {
        let v = self.get("/api/version", Duration::from_secs(5))?;
        Ok(format!(
            "ollama {}",
            v["version"].as_str().unwrap_or("(unknown version)")
        ))
    }

    fn models(&self) -> Result<Vec<ModelInfo>, String> {
        let tags = self.get("/api/tags", Duration::from_secs(10))?;
        let mut models = Vec::new();
        for m in tags["models"].as_array().into_iter().flatten() {
            let Some(name) = m["name"].as_str().or_else(|| m["model"].as_str()) else {
                continue;
            };
            let show = self.post("/api/show", json!({ "model": name })).ok();
            models.push(ModelInfo {
                name: name.to_string(),
                size_bytes: m["size"].as_u64(),
                capabilities: model_capabilities(name, show.as_ref()),
            });
        }
        Ok(models)
    }

    fn chat(&self, request: &ChatRequest) -> Result<ChatReply, String> {
        let mut messages = Vec::new();
        if let Some(system) = &request.system {
            messages.push(json!({ "role": "system", "content": system }));
        }
        let mut user = json!({ "role": "user", "content": request.prompt });
        if !request.images.is_empty() {
            user["images"] = json!(
                request
                    .images
                    .iter()
                    .map(|i| STANDARD.encode(&i.bytes))
                    .collect::<Vec<_>>()
            );
        }
        messages.push(user);
        let mut options = Map::new();
        if let Some(t) = request.temperature {
            options.insert("temperature".into(), json!(t));
        }
        if let Some(s) = request.seed {
            options.insert("seed".into(), json!(s));
        }
        if request.max_tokens > 0 {
            options.insert("num_predict".into(), json!(request.max_tokens));
        }
        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "stream": false,
            "options": options,
        });
        if let Some(schema) = &request.schema {
            body["format"] = schema.clone();
        } else if request.json {
            body["format"] = json!("json");
        }
        let reply = self.post("/api/chat", body)?;
        let text = reply
            .pointer("/message/content")
            .and_then(Value::as_str)
            .ok_or("the reply holds no message text")?;
        Ok(ChatReply {
            text: text.to_string(),
            model: reply["model"]
                .as_str()
                .unwrap_or(&request.model)
                .to_string(),
        })
    }

    fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let reply = self.post("/api/embed", json!({ "model": model, "input": texts }))?;
        crate::openai::vectors(&reply["embeddings"])
    }

    fn pull(
        &self,
        model: &str,
        progress: &mut dyn FnMut(&PullProgress) -> bool,
    ) -> Result<(), String> {
        let call = HttpCall::post(
            &format!("{}/api/pull", self.url),
            json!({ "model": model, "stream": true }),
            PULL_TIMEOUT,
        );
        let mut failed = None;
        let mut success = false;
        let mut stopped = false;
        let mut error_body = String::new();
        let status = self.transport.stream(&call, &mut |line| {
            match pull_line(line) {
                Ok(p) => {
                    success |= p.status == "success";
                    if !progress(&p) {
                        stopped = true;
                        return false;
                    }
                }
                Err(e) => {
                    error_body = line.to_string();
                    failed = Some(e);
                    return false;
                }
            }
            true
        })?;
        if !(200..300).contains(&status) {
            json_reply(&call, status, &error_body)?;
        }
        if let Some(e) = failed {
            return Err(format!("pulling `{model}`: {e}"));
        }
        if stopped {
            return Err(format!("pulling `{model}` was stopped"));
        }
        if success {
            Ok(())
        } else {
            Err(format!("pulling `{model}` ended without success"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_come_from_show_then_the_name() {
        let show = json!({ "capabilities": ["completion", "vision"] });
        assert_eq!(
            model_capabilities("anything", Some(&show)),
            vec![Capability::Chat, Capability::Vision, Capability::Structured]
        );
        assert_eq!(
            model_capabilities("nomic-embed-text:latest", None),
            vec![Capability::Embeddings]
        );
        assert!(model_capabilities("qwen2.5vl:7b", None).contains(&Capability::Vision));
        assert!(!model_capabilities("qwen3:8b", None).contains(&Capability::Vision));
    }

    #[test]
    fn pull_lines_are_read_and_errors_kept() {
        let p =
            pull_line(r#"{"status":"pulling abc","digest":"x","total":10,"completed":4}"#).unwrap();
        assert_eq!((p.completed, p.total), (4, 10));
        assert_eq!(
            pull_line(r#"{"error":"pull model manifest: file does not exist"}"#).unwrap_err(),
            "pull model manifest: file does not exist"
        );
    }
}
