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
//!
//! A chat model answers in words, not images: a mask (the AI object
//! member) goes to an endpoint speaking Helios's own JSON instead
//! ([`Api::Helios`]): the command resolver's request with its inputs
//! inline (`png_base64`) is posted as it is, and the reply is the
//! command's answer, its mask inline too (see [`crate::command`]). A
//! segmentation model behind a small local server fits there.
//!
//! The chat endpoints are `helios-ai`'s providers, given the endpoint
//! whole (see [`crate::model::chat_resolution`]): the same wire formats
//! serve decisions and the AI service.

use std::sync::Arc;
use std::time::Duration;

use helios_ai::anthropic::Anthropic;
use helios_ai::openai::OpenAiCompat;
use helios_ai::provider::{ModelProvider, ProviderConfig, ProviderKind};
use helios_ai::secrets::{KeySource, MemorySecrets};
use helios_ai::transport::{HttpCall, Ureq, send_json};
use helios_core::{ParamKind, ParamSpec};
use serde_json::Value;

use crate::command::{request_inline, resolution_json};
use crate::model::chat_resolution;
use crate::{Request, Resolution, Resolver};

/// Which protocol the endpoint speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    /// `POST …/chat/completions`, a bearer key.
    OpenAi,
    /// `POST …/v1/messages`, an `x-api-key`.
    Anthropic,
    /// The command resolver's JSON, posted as it is; a bearer key if any.
    Helios,
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
            timeout: Duration::from_mins(2),
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

impl Http {
    /// Posts `body` to a `helios` endpoint, returns the reply's JSON.
    fn post(&self, body: &Value) -> Result<Value, String> {
        let mut call = HttpCall::post(&self.url, body.clone(), self.timeout);
        if let Some(key) = self.key()? {
            call = call.header("Authorization", format!("Bearer {key}"));
        }
        send_json(&Ureq, &call)
    }

    /// The chat provider this endpoint is, through `helios-ai`.
    fn provider(&self) -> Box<dyn ModelProvider> {
        let kind = if self.api == Api::Anthropic {
            ProviderKind::Anthropic
        } else {
            ProviderKind::OpenAi
        };
        let mut entry = ProviderConfig::new("http", kind);
        entry.url = Some(self.url.clone());
        entry.key = self.key_env.clone().map(KeySource::Env);
        entry.timeout = Some(self.timeout.as_secs());
        let transport = Arc::new(Ureq);
        // The key comes from the environment, never from a keyring here.
        let secrets = Arc::new(MemorySecrets::default());
        match kind {
            ProviderKind::Anthropic => Box::new(
                Anthropic::new(&entry, &self.url, transport, secrets).with_messages_url(&self.url),
            ),
            _ => Box::new(
                OpenAiCompat::new(&entry, &self.url, transport, secrets).with_chat_url(&self.url),
            ),
        }
    }
}

impl Resolver for Http {
    fn resolve(&self, request: &Request) -> Result<Resolution, String> {
        if self.api == Api::Helios {
            let reply = self.post(&request_inline(request)?)?;
            return resolution_json(request, &reply);
        }
        if request.wants_mask() {
            return Err(format!(
                "{} is a chat endpoint, which cannot answer a mask: use a command or a `helios` endpoint",
                self.url
            ));
        }
        chat_resolution(self.provider().as_ref(), &self.model, request)
    }
}
