//! The `ai.*` requests, as the MCP server sends them: a JSON object with
//! its `cmd` (`ai.providers.list`...) and its arguments, defaults already
//! filled. [`call`] answers each with JSON, or an error for people.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::prompts::{Prompt, PromptRef};
use crate::provider::{ProviderConfig, ProviderKind};
use crate::routing::Route;
use crate::secrets::KeySource;
use crate::service::{BenchRequest, read_image};
use crate::{Ai, Capability};

/// Every request this module answers.
pub const CMDS: &[&str] = &[
    "ai.providers.list",
    "ai.providers.add",
    "ai.providers.remove",
    "ai.providers.test",
    "ai.models.list",
    "ai.routing.get",
    "ai.routing.set",
    "ai.prompts.list",
    "ai.prompts.get",
    "ai.prompts.set",
    "ai.prompts.try",
    "ai.local.probe",
    "ai.local.status",
    "ai.local.recommend",
    "ai.local.pull",
    "ai.local.bench",
    "ai.local.setup",
    "ai.jobs.events",
    "ai.jobs.wait",
    "ai.jobs.cancel",
    "ai.doctor",
];

fn text<'a>(request: &'a Value, name: &str) -> Option<&'a str> {
    request[name].as_str().filter(|s| !s.is_empty())
}

fn required<'a>(request: &'a Value, name: &str) -> Result<&'a str, String> {
    text(request, name).ok_or_else(|| format!("`{name}` is required"))
}

fn strings(request: &Value, name: &str) -> Vec<String> {
    request[name]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn reference(request: &Value) -> Result<PromptRef, String> {
    let mut r = PromptRef::parse(required(request, "prompt")?)?;
    if let Some(v) = request["version"].as_u64() {
        r.version = Some(u32::try_from(v).map_err(|_| "`version` is too large".to_string())?);
    }
    Ok(r)
}

fn provider_entry(request: &Value) -> Result<ProviderConfig, String> {
    let id = required(request, "id")?;
    let kind = required(request, "kind")?;
    let kind = ProviderKind::parse(kind)
        .ok_or_else(|| format!("unknown kind `{kind}`: ollama, openai, anthropic or command"))?;
    let mut entry = ProviderConfig::new(id, kind);
    entry.url = text(request, "url").map(str::to_string);
    entry.key = text(request, "key_env").map(|v| KeySource::Env(v.to_string()));
    entry.command = strings(request, "command");
    if request["capabilities"].is_array() {
        entry.capabilities = Some(
            strings(request, "capabilities")
                .iter()
                .map(|c| Capability::parse(c).ok_or_else(|| format!("unknown capability `{c}`")))
                .collect::<Result<_, _>>()?,
        );
    }
    entry.timeout = request["timeout_s"].as_u64();
    Ok(entry)
}

/// Answers one `ai.*` request. `images` are the images `ai.prompts.try`
/// looks at when the caller resolved them already (a node's render); its
/// `file` argument is read otherwise.
pub fn call(ai: &Ai, request: &Value, images: Vec<Vec<u8>>) -> Result<Value, String> {
    let cmd = request["cmd"].as_str().unwrap_or_default();
    match cmd {
        "ai.providers.list" => Ok(ai.providers_list()),
        "ai.providers.add" => ai.add_provider(provider_entry(request)?, text(request, "api_key")),
        "ai.providers.remove" => ai.remove_provider(required(request, "id")?),
        "ai.providers.test" => ai.test_provider(required(request, "id")?, text(request, "model")),
        "ai.models.list" => ai.models_list(text(request, "provider")),
        "ai.routing.get" => Ok(ai.routing_get()),
        "ai.routing.set" => {
            let capability = text(request, "capability")
                .map(|c| Capability::parse(c).ok_or_else(|| format!("unknown capability `{c}`")))
                .transpose()?;
            let route = match (text(request, "provider"), text(request, "model")) {
                (Some(p), Some(m)) => Some(Route::new(p, m)),
                (None, None) => None,
                _ => return Err("give both `provider` and `model`, or neither to clear".into()),
            };
            ai.routing_set(capability, text(request, "prompt"), route)
        }
        "ai.prompts.list" => Ok(ai.prompts_list()),
        "ai.prompts.get" => {
            let prompt = ai.prompt(&reference(request)?)?;
            Ok(json!({ "prompt": prompt, "hash": prompt.hash() }))
        }
        "ai.prompts.set" => {
            let mut given = request["prompt"].clone();
            if given["version"].is_null() {
                given["version"] = json!(1);
            }
            let prompt: Prompt =
                serde_json::from_value(given).map_err(|e| format!("the prompt: {e}"))?;
            let written = ai.set_prompt(prompt)?;
            Ok(json!({ "written": written.reference(), "hash": written.hash() }))
        }
        "ai.prompts.try" => {
            let mut images = images;
            if let Some(file) = text(request, "file") {
                images.push(read_image(Path::new(file))?);
            }
            let route = match (text(request, "provider"), text(request, "model")) {
                (Some(p), Some(m)) => Some(Route::new(p, m)),
                (None, None) => None,
                _ => {
                    return Err(
                        "give both `provider` and `model`, or neither to use the routing".into(),
                    );
                }
            };
            let input = Some(request["input"].clone()).filter(|v| !v.is_null());
            let result = ai.try_prompt(&reference(request)?, input, images, route)?;
            Ok(serde_json::to_value(result).expect("results serialize"))
        }
        "ai.local.probe" => Ok(serde_json::to_value(ai.probe()).expect("probes serialize")),
        "ai.local.status" => Ok(ai.status()),
        "ai.local.recommend" => Ok(ai.recommend()),
        "ai.local.pull" => ai.pull(
            text(request, "provider"),
            required(request, "model")?,
            request["size_mib"].as_u64(),
        ),
        "ai.local.bench" => {
            let image = text(request, "file")
                .map(|f| read_image(Path::new(f)))
                .transpose()?
                .or_else(|| images.into_iter().next());
            ai.bench(BenchRequest {
                prompts: strings(request, "prompts"),
                image,
                apply: request["apply"] == true,
                runs: u32::try_from(request["runs"].as_u64().unwrap_or(1)).unwrap_or(1),
                providers: strings(request, "providers"),
            })
        }
        "ai.local.setup" => Ok(ai.setup(&strings(request, "confirm"))),
        "ai.jobs.events" => Ok(json!({
            "events": ai.events(request["since"].as_u64().unwrap_or(0)),
            "last": ai.bus().last_seq(),
        })),
        "ai.jobs.wait" => Ok(ai.job_wait(
            request["job"].as_u64().ok_or("`job` is required")?,
            Duration::from_millis(request["timeout_ms"].as_u64().unwrap_or(10_000)),
        )),
        "ai.jobs.cancel" => {
            let job = request["job"].as_u64().ok_or("`job` is required")?;
            Ok(json!({ "job": job, "cancelled": ai.job_cancel(job) }))
        }
        "ai.doctor" => Ok(ai.doctor()),
        other => Err(format!("no request `{other}`")),
    }
}
