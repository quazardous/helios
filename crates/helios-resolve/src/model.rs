//! Decisions answered through `helios-ai`'s providers.
//!
//! [`chat_resolution`] asks any [`ModelProvider`] that chats: the node's
//! inputs as PNG images, the question in words ([`crate::http::prompt`]),
//! a JSON answer read back as a [`Resolution`]. The `http` resolver's
//! OpenAI-compatible and Anthropic endpoints go through it.
//!
//! [`ThroughAi`] asks the provider and model the AI routing map names
//! (`vision` for a decision, `segmentation` for a mask, or the capability
//! the decision names): every call is then a scheduler job of the AI
//! service, logged, its images downscaled for a remote provider. `Routed`
//! falls back on it when it is given the AI service.

use std::sync::Arc;

use helios_ai::provider::{ChatReply, ChatRequest, ImageData, ModelProvider, SegmentRequest};
use helios_ai::{Ai, CallMeta, Capability};
use helios_decision::Model;
use helios_export::{Format, encode};
use serde_json::json;

use crate::command::resolution_json;
use crate::{Request, Resolution, Resolver, mask_answer};

/// The chat call for a decision: its inputs as PNG images, the question
/// in words, a JSON object asked for.
pub fn chat_request(model: &str, request: &Request) -> Result<ChatRequest, String> {
    let images = request
        .inputs
        .iter()
        .map(|i| {
            encode(&i.image, &Format::Png { bits: 8 }).map(|png| ImageData {
                mime: "image/png".into(),
                bytes: png,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ChatRequest {
        model: model.into(),
        prompt: crate::http::prompt(request),
        images,
        json: true,
        ..ChatRequest::default()
    })
}

/// A decision's resolution from a chat reply: its JSON, the model named
/// as configured and versioned as the runtime says.
pub fn resolution_from_reply(
    model: &str,
    request: &Request,
    reply: &ChatReply,
) -> Result<Resolution, String> {
    let mut answer = helios_ai::json_in(&reply.text)?;
    // The model is the one configured; its name is not the model's to give.
    answer["model"] = json!({ "name": model, "version": reply.model });
    resolution_json(request, &answer)
}

/// Asks `provider`'s `model` about a decision.
pub fn chat_resolution(
    provider: &dyn ModelProvider,
    model: &str,
    request: &Request,
) -> Result<Resolution, String> {
    let reply = provider.chat(&chat_request(model, request)?)?;
    resolution_from_reply(model, request, &reply)
}

/// What a mask decision asks to find: its prompt, or its kind.
fn what_to_find(request: &Request) -> String {
    match request.asked("kind") {
        "prompt" | "" => request.asked("prompt").to_string(),
        kind => kind.to_string(),
    }
}

/// Answers decisions through the AI routing map.
pub struct ThroughAi {
    ai: Arc<Ai>,
    /// The capability routed; `vision` (or `segmentation` for a mask) when
    /// none.
    capability: Option<Capability>,
}

impl ThroughAi {
    pub fn new(ai: Arc<Ai>, capability: Option<Capability>) -> Self {
        ThroughAi { ai, capability }
    }
}

impl Resolver for ThroughAi {
    fn resolve(&self, request: &Request) -> Result<Resolution, String> {
        let meta = CallMeta {
            purpose: format!("decision {}", request.node),
            ..CallMeta::default()
        };
        if request.wants_mask() {
            let capability = self.capability.unwrap_or(Capability::Segmentation);
            let route = self.ai.route(capability, None)?;
            let input = request
                .inputs
                .first()
                .ok_or("a mask decision has no input")?;
            let png = encode(&input.image, &Format::Png { bits: 8 })?;
            let reply = self.ai.segment(
                &route,
                SegmentRequest {
                    model: route.model.clone(),
                    image: ImageData {
                        mime: "image/png".into(),
                        bytes: png,
                    },
                    prompt: what_to_find(request),
                },
                meta,
            )?;
            let answer = mask_answer(request, &reply.png, reply.confidence)?;
            return Ok(Resolution {
                options: vec![answer],
                confidence: reply.confidence,
                rationale: format!("segmented by {}", route.model),
                model: Model {
                    name: route.model,
                    version: reply.model,
                },
            });
        }
        let route = self
            .ai
            .route(self.capability.unwrap_or(Capability::Vision), None)?;
        let chat = chat_request(&route.model, request)?;
        let reply = self.ai.chat(&route, chat, meta)?;
        resolution_from_reply(&route.model, request, &reply)
    }
}
