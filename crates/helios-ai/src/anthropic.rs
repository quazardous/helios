//! Anthropic's messages API: chat and vision. Structured output is asked
//! for in words (the schema goes with the prompt) and checked by the
//! caller against the schema. The key goes as `x-api-key`.

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

/// The API version Helios speaks.
pub const VERSION: &str = "2023-06-01";

pub struct Anthropic {
    id: String,
    url: String,
    /// The messages endpoint, `{url}/messages` unless given whole.
    messages_url: String,
    key: Option<KeySource>,
    capabilities: Vec<Capability>,
    remote: bool,
    timeout: Duration,
    transport: Arc<dyn Transport>,
    secrets: Arc<dyn Secrets>,
}

impl Anthropic {
    pub fn new(
        config: &ProviderConfig,
        url: &str,
        transport: Arc<dyn Transport>,
        secrets: Arc<dyn Secrets>,
    ) -> Self {
        Anthropic {
            id: config.id.clone(),
            url: url.to_string(),
            messages_url: format!("{url}/messages"),
            key: config.key.clone(),
            capabilities: config.capabilities(),
            remote: config.remote(),
            timeout: config.timeout(),
            transport,
            secrets,
        }
    }

    /// Calls go to this messages endpoint, given whole.
    pub fn with_messages_url(mut self, url: &str) -> Self {
        self.messages_url = url.to_string();
        self
    }

    fn call(&self, call: HttpCall) -> Result<Value, String> {
        let mut call = call.header("anthropic-version", VERSION);
        if let Some(source) = &self.key {
            call = call.header("x-api-key", source.resolve(self.secrets.as_ref())?);
        }
        send_json(self.transport.as_ref(), &call)
    }
}

/// The body of a messages call: images first, then the text (and the
/// schema, in words, when one is asked for).
pub fn body(request: &ChatRequest) -> Value {
    let mut content: Vec<Value> = request
        .images
        .iter()
        .map(|i| {
            json!({ "type": "image", "source":
                { "type": "base64", "media_type": i.mime, "data": STANDARD.encode(&i.bytes) } })
        })
        .collect();
    let mut text = request.prompt.clone();
    if let Some(schema) = &request.schema {
        text = format!("{text}\n\nReply with JSON only, fitting this JSON schema:\n{schema}");
    } else if request.json {
        text.push_str("\n\nReply with a JSON object only.");
    }
    content.push(json!({ "type": "text", "text": text }));
    let mut body = json!({
        "model": request.model,
        "max_tokens": if request.max_tokens > 0 { request.max_tokens } else { 1024 },
        "messages": [{ "role": "user", "content": content }],
    });
    if let Some(system) = &request.system {
        body["system"] = json!(system);
    }
    if let Some(t) = request.temperature {
        body["temperature"] = json!(t);
    }
    body
}

impl ModelProvider for Anthropic {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
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
            .map(|name| ModelInfo {
                name: name.to_string(),
                size_bytes: None,
                capabilities: self.capabilities.clone(),
            })
            .collect())
    }

    fn chat(&self, request: &ChatRequest) -> Result<ChatReply, String> {
        let reply = self.call(HttpCall::post(
            &self.messages_url,
            body(request),
            self.timeout,
        ))?;
        let text = reply
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .ok_or("the reply holds no message text")?;
        Ok(ChatReply {
            text: text.to_string(),
            model: reply["model"].as_str().unwrap_or_default().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ImageData;

    #[test]
    fn images_come_first_and_the_schema_goes_in_words() {
        let request = ChatRequest {
            model: "m".into(),
            prompt: "rate it".into(),
            images: vec![ImageData {
                mime: "image/png".into(),
                bytes: vec![0],
            }],
            schema: Some(json!({ "type": "object" })),
            ..ChatRequest::default()
        };
        let b = body(&request);
        assert_eq!(
            b.pointer("/messages/0/content/0/source/media_type"),
            Some(&json!("image/png"))
        );
        let text = b.pointer("/messages/0/content/1/text").unwrap();
        assert!(text.as_str().unwrap().contains("JSON schema"));
        assert_eq!(b["max_tokens"], 1024);
    }
}
