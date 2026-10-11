//! A resolver that calls a vision model over HTTP: an OpenAI-compatible
//! chat completions endpoint (OpenAI, and the many local servers that
//! speak it) or Anthropic's messages endpoint.
//!
//! The model gets the node's inputs as sRGB PNG images and a question
//! built from the decision's manifest (what it decides, each parameter
//! with its kind and range) and the brief, and is asked to answer JSON:
//! `{"options": [{…}], "confidence": 0.8, "rationale": "…"}`.
//!
//! The key is read from an environment variable named in the settings,
//! never from a settings file, and never written anywhere by Helios.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use helios_core::{ParamKind, ParamSpec};
use helios_export::{Format, encode};
use serde_json::{Value, json};

use crate::command::resolution_json;
use crate::{Request, Resolution, Resolver};

/// Which protocol the endpoint speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    /// `POST …/chat/completions`, a bearer key.
    OpenAi,
    /// `POST …/v1/messages`, an `x-api-key`.
    Anthropic,
}

pub struct Http {
    pub api: Api,
    /// The full endpoint URL.
    pub url: String,
    pub model: String,
    /// The environment variable holding the key; none for a local server.
    pub key_env: Option<String>,
    pub timeout: Duration,
}

impl Http {
    pub fn new(api: Api, url: &str, model: &str) -> Self {
        Http {
            api,
            url: url.into(),
            model: model.into(),
            key_env: None,
            timeout: Duration::from_secs(120),
        }
    }

    pub fn key_from(mut self, variable: &str) -> Self {
        self.key_env = Some(variable.into());
        self
    }

    fn key(&self) -> Result<Option<String>, String> {
        match &self.key_env {
            None => Ok(None),
            Some(var) => std::env::var(var)
                .map(Some)
                .map_err(|_| format!("the key variable `{var}` is not set")),
        }
    }
}

fn describe(p: &ParamSpec) -> String {
    let kind = match &p.kind {
        ParamKind::Float { min, max, default } => {
            format!("a number from {min} to {max} (default {default})")
        }
        ParamKind::Int { min, max, default } => {
            format!("a whole number from {min} to {max} (default {default})")
        }
        ParamKind::Bool { default } => format!("true or false (default {default})"),
        ParamKind::Choice { options, default } => {
            format!("one of {} (default {default:?})", options.join(", "))
        }
        ParamKind::Floats {
            count, min, max, ..
        } => {
            format!("a list of {count} numbers from {min} to {max}")
        }
        ParamKind::Ints {
            count, min, max, ..
        } => {
            format!("a list of {count} whole numbers from {min} to {max}")
        }
        ParamKind::Text { capacity, .. } => format!("text of at most {capacity} bytes"),
    };
    format!("- `{}`: {}; {}", p.name, p.description, kind)
}

/// The question put to the model, in words.
pub fn prompt(request: &Request) -> String {
    let params: Vec<String> = request.answer.iter().map(describe).collect();
    let brief = request
        .brief
        .as_deref()
        .map(|b| format!("\nThe intended result, in the photographer's words: {b}\n"))
        .unwrap_or_default();
    format!(
        "You make one decision in a photo editing network: {} (`{}`).\n\
         The image{} attached {} what the decision looks at.\n{brief}\n\
         Answer with these parameters:\n{}\n\n\
         Reply with JSON only, no other text: \
         {{\"options\": [{{…}}, …], \"confidence\": 0.0 to 1.0, \"rationale\": \"one sentence\"}}. \
         `options` holds one or more answers, best first, each an object of the parameters above.",
        request.description,
        request.module,
        if request.inputs.len() == 1 { "" } else { "s" },
        if request.inputs.len() == 1 {
            "is"
        } else {
            "are"
        },
        params.join("\n"),
    )
}

/// The JSON object in a model's text, fenced in a code block or not.
fn json_in(text: &str) -> Result<Value, String> {
    let start = text.find('{').ok_or("the model's reply holds no JSON")?;
    let end = text.rfind('}').ok_or("the model's reply holds no JSON")?;
    serde_json::from_str(&text[start..=end]).map_err(|e| format!("the model's JSON: {e}"))
}

impl Resolver for Http {
    fn resolve(&self, request: &Request) -> Result<Resolution, String> {
        let images = request
            .inputs
            .iter()
            .map(|i| encode(&i.image, &Format::Png { bits: 8 }).map(|png| STANDARD.encode(png)))
            .collect::<Result<Vec<_>, _>>()?;
        let text = prompt(request);
        let body = match self.api {
            Api::OpenAi => {
                let mut content = vec![json!({"type": "text", "text": text})];
                for b64 in &images {
                    content.push(json!({
                        "type": "image_url",
                        "image_url": {"url": format!("data:image/png;base64,{b64}")}
                    }));
                }
                json!({
                    "model": self.model,
                    "messages": [{"role": "user", "content": content}],
                    "response_format": {"type": "json_object"}
                })
            }
            Api::Anthropic => {
                let mut content: Vec<Value> = images
                    .iter()
                    .map(|b64| {
                        json!({"type": "image", "source":
                            {"type": "base64", "media_type": "image/png", "data": b64}})
                    })
                    .collect();
                content.push(json!({"type": "text", "text": text}));
                json!({
                    "model": self.model,
                    "max_tokens": 1024,
                    "messages": [{"role": "user", "content": content}]
                })
            }
        };
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(self.timeout))
            .http_status_as_error(false)
            .build()
            .into();
        let mut call = agent.post(&self.url);
        match (self.api, self.key()?) {
            (Api::OpenAi, Some(key)) => {
                call = call.header("Authorization", format!("Bearer {key}"))
            }
            (Api::Anthropic, Some(key)) => call = call.header("x-api-key", key),
            (_, None) => {}
        }
        if self.api == Api::Anthropic {
            call = call.header("anthropic-version", "2023-06-01");
        }
        let mut response = call
            .send_json(&body)
            .map_err(|e| format!("{}: {e}", self.url))?;
        let status = response.status().as_u16();
        let reply = response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("{}: {e}", self.url))?;
        if !(200..300).contains(&status) {
            return Err(format!("{} answered {status}: {}", self.url, reply.trim()));
        }
        let reply: Value =
            serde_json::from_str(&reply).map_err(|e| format!("{}: {e}", self.url))?;
        let said = match self.api {
            Api::OpenAi => reply.pointer("/choices/0/message/content"),
            Api::Anthropic => reply.pointer("/content/0/text"),
        }
        .and_then(Value::as_str)
        .ok_or("the reply holds no message text")?;
        let mut answer = json_in(said)?;
        // The model is the one configured; its name is not the model's to give.
        answer["model"] = json!({"name": self.model, "version": reply
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")});
        resolution_json(request, &answer)
    }
}
