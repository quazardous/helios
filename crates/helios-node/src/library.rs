//! The node's commands on its vault's library that are not control
//! verbs: the stacks (`library.stacks.*`) and darktable's sidecars kept
//! as versions (`library.importDarktable`). They are listed with the
//! verbs ([`crate::commands::registry`]) and run here, on the vault the
//! node owns, by whoever the call's origin names; the stacks' changes
//! are announced on the node's bus.
//!
//! An image is named by its id or by an absolute path the vault knows.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::Bus;
use helios_library::stacks::{self as suggest, Resolvers, RuleResolver};
use helios_node_core::commands::{CommandSpec, Need, object};
use helios_vault::{ImageId, Store, Vault};
use serde_json::{Value, json};

/// The ids of the commands run here.
pub const IDS: &[&str] = &[
    "library.importDarktable",
    "library.stacks.list",
    "library.stacks.create",
    "library.stacks.dissolve",
    "library.stacks.expand",
    "library.stacks.top",
    "library.stacks.suggest",
    "library.stacks.accept",
    "library.stacks.decline",
    "library.stacks.decisions",
];

/// Whether `id` is one of the commands run here.
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

fn string() -> Value {
    json!({ "type": "string" })
}

fn images() -> Value {
    json!({ "type": "array", "items": { "type": "string" },
            "description": "image ids, or absolute paths the vault knows" })
}

/// The commands' specs.
pub fn specs() -> Vec<CommandSpec> {
    let stack = || ("stack", string(), true);
    let decision = || ("decision", string(), true);
    let library = |id: &str, label: &str| CommandSpec::new(id, label, "Library").needs(Need::Vault);
    vec![
        library(
            "library.importDarktable",
            "Keep darktable's sidecars as versions",
        )
        .params(object(&[(
            "paths",
            json!({ "type": "array", "items": { "type": "string", "description": "an absolute path" } }),
            true,
        )])),
        library("library.stacks.list", "List the stacks").read_only(),
        library("library.stacks.create", "Stack images").params(object(&[
            ("images", images(), true),
            ("top", json!({ "type": ["string", "null"] }), false),
        ])),
        library("library.stacks.dissolve", "Dissolve a stack").params(object(&[stack()])),
        library("library.stacks.expand", "Expand or collapse a stack").params(object(&[
            stack(),
            ("expanded", json!({ "type": "boolean" }), true),
        ])),
        library("library.stacks.top", "Put an image on top of its stack")
            .params(object(&[stack(), ("image", string(), true)])),
        library("library.stacks.suggest", "Suggest stacks").params(object(&[
            ("images", images(), false),
            ("resolver", json!({ "type": ["string", "null"] }), false),
            (
                "window",
                json!({ "type": ["number", "null"], "minimum": 0 }),
                false,
            ),
            (
                "distance",
                json!({ "type": ["integer", "null"], "minimum": 0 }),
                false,
            ),
        ])),
        library("library.stacks.accept", "Accept suggested stacks").params(object(&[
            decision(),
            (
                "groups",
                json!({ "type": ["array", "null"], "items": { "type": "integer", "minimum": 0 } }),
                false,
            ),
        ])),
        library("library.stacks.decline", "Decline suggested stacks")
            .params(object(&[decision()])),
        library("library.stacks.decisions", "List the stack suggestions").read_only(),
    ]
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("library values serialize")
}

/// An image named by its id, or by an absolute path the vault knows.
fn image<S: Store>(vault: &Vault<S>, text: &str) -> Result<ImageId, String> {
    if let Ok(id) = ImageId::from_hex(text) {
        return Ok(id);
    }
    vault
        .at_path(Path::new(text))?
        .map(|(id, _)| id)
        .ok_or_else(|| format!("`{text}` is neither an image id nor a file the vault knows"))
}

fn named<S: Store>(vault: &Vault<S>, params: &Value, field: &str) -> Result<Vec<ImageId>, String> {
    params[field]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|n| image(vault, n))
        .collect()
}

fn text<'a>(params: &'a Value, field: &str) -> &'a str {
    params[field].as_str().unwrap_or_default()
}

/// Runs command `id` (params already checked against its schema) on
/// `vault`, made by `origin`.
pub fn run<S: Store>(
    vault: &Vault<S>,
    bus: &Arc<Bus>,
    id: &str,
    params: &Value,
    origin: &str,
) -> Result<Value, String> {
    let bus = Some(&**bus);
    match id {
        "library.importDarktable" => {
            let paths: Vec<PathBuf> = params["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect();
            let (mut files, mut warnings) = (Vec::new(), Vec::new());
            let kept = helios_library::darktable::import(
                vault,
                &paths,
                now(),
                &mut |line| files.push(line.clone()),
                &mut |path, warning| {
                    warnings.push(json!({ "path": path, "warning": warning }));
                },
            )?;
            Ok(json!({ "kept": kept, "files": files, "warnings": warnings }))
        }
        "library.stacks.list" => vault.stacks().map(|s| json!({ "stacks": to_json(&s) })),
        "library.stacks.create" => {
            let members = named(vault, params, "images")?;
            let top = params["top"]
                .as_str()
                .map(|t| image(vault, t))
                .transpose()?;
            vault
                .create_stack(&members, top, origin, now())
                .map(|s| json!({ "stack": to_json(&s) }))
        }
        "library.stacks.dissolve" => vault
            .dissolve_stack(text(params, "stack"))
            .map(|s| json!({ "stack": to_json(&s) })),
        "library.stacks.expand" => vault
            .set_stack_expanded(
                text(params, "stack"),
                params["expanded"].as_bool().unwrap_or(true),
            )
            .map(|s| json!({ "stack": to_json(&s) })),
        "library.stacks.top" => {
            let top = image(vault, text(params, "image"))?;
            vault
                .set_stack_top(text(params, "stack"), &top)
                .map(|s| json!({ "stack": to_json(&s) }))
        }
        "library.stacks.suggest" => {
            let selection = named(vault, params, "images")?;
            let window = params["window"].as_f64();
            let distance = params["distance"].as_u64();
            let resolvers = if window.is_some() || distance.is_some() {
                let mut rules = suggest::Rules::default();
                if let Some(window) = window {
                    rules.window = window;
                }
                if let Some(distance) = distance {
                    rules.distance = u32::try_from(distance).unwrap_or(u32::MAX);
                }
                Resolvers::new().with(suggest::RULES, Arc::new(RuleResolver { rules }))
            } else {
                Resolvers::default()
            };
            let resolver = params["resolver"].as_str().unwrap_or(suggest::RULES);
            suggest::suggest(vault, bus, &resolvers, resolver, &selection, now())
                .map(|d| json!({ "decision": suggest::to_json(&d) }))
        }
        "library.stacks.accept" => {
            let groups: Option<Vec<usize>> = params["groups"].as_array().map(|g| {
                g.iter()
                    .filter_map(Value::as_u64)
                    .filter_map(|n| usize::try_from(n).ok())
                    .collect()
            });
            suggest::accept(
                vault,
                bus,
                text(params, "decision"),
                groups.as_deref(),
                origin,
                now(),
            )
            .map(|d| json!({ "decision": suggest::to_json(&d) }))
        }
        "library.stacks.decline" => {
            suggest::decline(vault, bus, text(params, "decision"), origin, now())
                .map(|d| json!({ "decision": suggest::to_json(&d) }))
        }
        "library.stacks.decisions" => {
            suggest::decisions(vault).map(|d| json!({ "decisions": to_json(&d) }))
        }
        other => Err(format!("no library command `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use helios_node_core::commands::validate;

    use super::*;

    #[test]
    fn every_command_is_declared_once_with_a_schema_its_example_fits() {
        let ids: Vec<String> = specs().iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, IDS);
        let examples = [
            json!({ "paths": ["/photos"] }),
            json!({}),
            json!({ "images": ["/p/a.raf", "0a"], "top": null }),
            json!({ "stack": "s" }),
            json!({ "stack": "s", "expanded": false }),
            json!({ "stack": "s", "image": "/p/a.raf" }),
            json!({ "images": [], "resolver": null, "window": 3.0, "distance": 8 }),
            json!({ "decision": "d", "groups": [0, 2] }),
            json!({ "decision": "d" }),
            json!({}),
        ];
        for (spec, example) in specs().iter().zip(examples) {
            validate(&spec.params, &example).unwrap_or_else(|e| panic!("{}: {e}", spec.id));
        }
    }

    #[test]
    fn stacks_are_made_and_listed_on_the_vault() {
        let vault = Vault::new(helios_vault::MemoryStore::default());
        let bus = Arc::new(Bus::new());
        let listed = run(&vault, &bus, "library.stacks.list", &json!({}), "cli").unwrap();
        assert_eq!(listed, json!({ "stacks": [] }));
        let refused = run(
            &vault,
            &bus,
            "library.stacks.create",
            &json!({ "images": ["/nowhere/a.raf"] }),
            "cli",
        );
        assert!(refused.unwrap_err().contains("neither an image id"));
    }
}
