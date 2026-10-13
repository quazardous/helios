//! Any OpenAI-compatible server: llama.cpp's server, LM Studio, `vLLM`,
//! OpenAI itself. The base URL ends with `/v1`; calls go to
//! `/chat/completions` (images as `data:` URLs, structured output through
//! `response_format`), `/embeddings` and `/models`. A key, when the entry
//! names one, goes as a bearer token.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::Capability;
use crate::provider::{
    ChatReply, ChatRequest, ModelInfo, ModelProvider, ProviderConfig, ProviderKind,
};
use crate::secrets::{KeySource, Secrets};
use crate::transport::{HttpCall, Transport, send_json};

pub struct OpenAiCompat {
    id: String,
    url: String,
    /// The chat endpoint, `{url}/chat/completions` unless given whole.
    chat_url: String,
    key: Option<KeySource>,
    capabilities: Vec<Capability>,
    remote: bool,
    timeout: Duration,
    transport: Arc<dyn Transport>,
    secrets: Arc<dyn Secrets>,
}

impl OpenAiCompat {
    pub fn new(
        config: &ProviderConfig,
        url: &str,
        transport: Arc<dyn Transport>,
        secrets: Arc<dyn Secrets>,
    ) -> Self {
        OpenAiCompat {
            id: config.id.clone(),
            url: url.to_string(),
            chat_url: format!("{url}/chat/completions"),
            key: config.key.clone(),
            capabilities: config.capabilities(),
            remote: config.remote(),
            timeout: config.timeout(),
            transport,
            secrets,
        }
    }

    /// Calls go to this chat endpoint, given whole (a decision's `http`
    /// resolver names the endpoint, not the base URL).
    pub fn with_chat_url(mut self, chat_url: &str) -> Self {
        self.chat_url = chat_url.to_string();
        self
    }

    fn call(&self, call: HttpCall) -> Result<Value, String> {
        let call = match &self.key {
            Some(source) => {
                let key = source.resolve(self.secrets.as_ref())?;
                call.header("Authorization", format!("Bearer {key}"))
            }
            None => call,
        };
        send_json(self.transport.as_ref(), &call)
    }
}

/// Vectors from a JSON array of arrays of numbers.
pub fn vectors(v: &Value) -> Result<Vec<Vec<f32>>, String> {
    v.as_array()
        .ok_or("the reply holds no embeddings")?
        .iter()
        .map(|row| -> Result<Vec<f32>, String> {
            row.as_array()
                .ok_or_else(|| "an embedding is not a list".to_string())?
                .iter()
                .map(|x| {
                    x.as_f64()
                        .map(|f| f as f32)
                        .ok_or_else(|| "an embedding holds a non-number".to_string())
                })
                .collect()
        })
        .collect()
}

/// The `messages` of a chat call, OpenAI's way.
pub fn messages(request: &ChatRequest) -> Value {
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(json!({ "role": "system", "content": system }));
    }
    let mut content = vec![json!({ "type": "text", "text": request.prompt })];
    for image in &request.images {
        content.push(json!({
            "type": "image_url",
            "image_url": { "url": format!("data:{};base64,{}", image.mime, STANDARD.encode(&image.bytes)) }
        }));
    }
    messages.push(json!({ "role": "user", "content": content }));
    json!(messages)
}

impl ModelProvider for OpenAiCompat {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAi
    }

    fn capabilities(&self) -> Vec<Capability> {
        self.capabilities.clone()
    }

    fn remote(&self) -> bool {
        self.remote
    }

    fn health(&self) -> Result<String, String> {
        let n = self.models()?.len();
        Ok(format!("{n} model(s)"))
    }

    fn models(&self) -> Result<Vec<ModelInfo>, String> {
        let reply = self.call(HttpCall::get(
            &format!("{}/models", self.url),
            Duration::from_secs(15),
        ))?;
        Ok(reply["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["id"].as_str())
            .map(|name| {
                let capabilities = if name.to_ascii_lowercase().contains("embed") {
                    vec![Capability::Embeddings]
                } else {
                    self.capabilities
                        .iter()
                        .copied()
                        .filter(|c| *c != Capability::Embeddings)
                        .collect()
                };
                ModelInfo {
                    name: name.to_string(),
                    size_bytes: None,
                    capabilities,
                }
            })
            .collect())
    }

    fn chat(&self, request: &ChatRequest) -> Result<ChatReply, String> {
        let mut body = json!({
            "model": request.model,
            "messages": messages(request),
        });
        if let Some(schema) = &request.schema {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": { "name": "answer", "schema": schema }
            });
        } else if request.json {
            body["response_format"] = json!({ "type": "json_object" });
        }
        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(s) = request.seed {
            body["seed"] = json!(s);
        }
        if request.max_tokens > 0 {
            body["max_tokens"] = json!(request.max_tokens);
        }
        let reply = self.call(HttpCall::post(&self.chat_url, body, self.timeout))?;
        let text = reply
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or("the reply holds no message text")?;
        Ok(ChatReply {
            text: text.to_string(),
            model: reply["model"].as_str().unwrap_or_default().to_string(),
        })
    }

    fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let reply = self.call(HttpCall::post(
            &format!("{}/embeddings", self.url),
            json!({ "model": model, "input": texts }),
            self.timeout,
        ))?;
        let rows: Vec<Value> = reply["data"]
            .as_array()
            .ok_or("the reply holds no embeddings")?
            .iter()
            .map(|d| d["embedding"].clone())
            .collect();
        vectors(&Value::Array(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ImageData;

    #[test]
    fn images_go_as_data_urls_after_the_text() {
        let request = ChatRequest {
            model: "m".into(),
            system: Some("be brief".into()),
            prompt: "what is it?".into(),
            images: vec![ImageData {
                mime: "image/jpeg".into(),
                bytes: vec![1, 2, 3],
            }],
            ..ChatRequest::default()
        };
        let m = messages(&request);
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[1]["content"][0]["text"], "what is it?");
        assert_eq!(
            m[1]["content"][1]["image_url"]["url"],
            "data:image/jpeg;base64,AQID"
        );
    }

    #[test]
    fn vectors_are_read() {
        assert_eq!(
            vectors(&json!([[1.0, 2.5], [0]])).unwrap(),
            vec![vec![1.0, 2.5], vec![0.0]]
        );
        assert!(vectors(&json!([["x"]])).is_err());
    }
}
