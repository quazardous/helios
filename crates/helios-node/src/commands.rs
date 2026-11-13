//! The node's verbs as commands (`helios_node_core::commands`): each
//! [`ControlRequest`](helios_inspect::control::ControlRequest) verb is a
//! command of the same id, whose params are the request's fields. Running
//! one (`command.run`) builds that request and answers it as the endpoint
//! would, so a command and its request behave the same.

use std::sync::OnceLock;

use helios_node_core::commands::{CommandSpec, Need, Registry, object};
use serde_json::{Value, json};

fn string() -> Value {
    json!({ "type": "string" })
}

fn text(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn absolute() -> Value {
    text("an absolute path")
}

fn boolean() -> Value {
    json!({ "type": "boolean" })
}

fn count() -> Value {
    json!({ "type": "integer", "minimum": 0 })
}

/// The same schema, `null` also taken (an optional field).
fn or_null(mut schema: Value) -> Value {
    let named = match schema.get("type") {
        Some(Value::String(one)) => vec![json!(one)],
        Some(Value::Array(many)) => many.clone(),
        _ => return schema,
    };
    let mut types = named;
    types.push(json!("null"));
    schema["type"] = Value::Array(types);
    schema
}

fn strings() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
}

fn region() -> Value {
    object(&[
        ("x", json!({ "type": "integer" }), true),
        ("y", json!({ "type": "integer" }), true),
        ("width", count(), true),
        ("height", count(), true),
    ])
}

fn file_format() -> Value {
    json!({ "type": "string", "enum": ["jpeg", "png", "tiff"] })
}

fn quality() -> Value {
    json!({ "type": ["integer", "null"], "minimum": 1, "maximum": 100,
            "description": "JPEG quality, 92 when not given" })
}

/// A parameter value: one of its kinds, `{ "float": 1.0 }`.
fn value() -> Value {
    let one = |kind: &str, schema: Value| object(&[(kind, schema, true)]);
    json!({ "anyOf": [
        one("float", json!({ "type": "number" })),
        one("int", json!({ "type": "integer" })),
        one("bool", boolean()),
        one("choice", string()),
        one("list", json!({ "type": "array", "items": { "type": "number" } })),
        one("ints", json!({ "type": "array", "items": { "type": "integer" } })),
        one("text", string()),
    ] })
}

/// Parameters by name.
fn params() -> Value {
    json!({ "type": "object", "additionalProperties": value() })
}

/// Parameters by node name, then by name.
fn node_params() -> Value {
    json!({ "type": "object", "additionalProperties": params(),
            "description": "parameters changed for this request only, by node name" })
}

fn run_item() -> Value {
    json!({ "anyOf": [
        absolute(),
        object(&[("path", absolute(), true), ("params", node_params(), false)]),
    ] })
}

fn named(field: &str) -> Value {
    object(&[(field, string(), true)])
}

fn on_run(extra: &[(&str, Value, bool)]) -> Value {
    let mut fields = vec![("run", string(), true)];
    fields.extend(extra.iter().cloned());
    object(&fields)
}

fn verbs() -> Vec<CommandSpec> {
    let node = || ("node", string(), true);
    vec![
        CommandSpec::new("render", "Render a node's port over a region", "Render").params(object(
            &[node(), ("port", string(), true), ("region", region(), true)],
        )),
        CommandSpec::new("set_param", "Set a module parameter", "Edit").params(object(&[
            node(),
            ("param", string(), true),
            ("value", value(), true),
        ])),
        CommandSpec::new("node.bypass", "Bypass a module or bring it back", "Edit")
            .params(object(&[node(), ("on", boolean(), true)])),
        CommandSpec::new("load_source", "Give a source node a file's image", "Edit")
            .params(object(&[node(), ("path", absolute(), true)])),
        CommandSpec::new("export", "Export a node's output to a file", "Export").params(object(&[
            node(),
            ("port", text("`output` when not given"), false),
            ("region", or_null(region()), false),
            ("path", absolute(), true),
            ("format", file_format(), true),
            ("quality", quality(), false),
            ("params", node_params(), false),
            ("batch", or_null(text("the batch job it is part of")), false),
            ("as_mask", boolean(), false),
        ])),
        CommandSpec::new("cancel", "Cancel a scheduler job", "Jobs").params(object(&[(
            "job",
            count(),
            true,
        )])),
        CommandSpec::new("job.cancel", "Cancel a job by its id", "Jobs").params(object(&[(
            "id",
            text("`export-3`, `batch-1`"),
            true,
        )])),
        CommandSpec::new("batch_start", "Start a batch job", "Jobs").params(object(&[
            ("title", string(), true),
            ("total", count(), true),
            ("hold", boolean(), false),
            ("idle_timeout_s", or_null(count()), false),
        ])),
        CommandSpec::new("batch_end", "End a batch job", "Jobs").params(object(&[
            ("id", string(), true),
            ("failed", count(), false),
            ("cancelled", boolean(), false),
        ])),
        CommandSpec::new("pause", "Stop starting jobs", "Jobs"),
        CommandSpec::new("resume", "Start jobs again", "Jobs"),
        CommandSpec::new("fault", "Make a node's next computation fail once", "Debug").params(
            object(&[
                node(),
                (
                    "kind",
                    json!({ "type": "string", "enum": ["compute", "source_read"] }),
                    true,
                ),
            ]),
        ),
        CommandSpec::new("decisions", "List the network's decisions", "Decisions").read_only(),
        CommandSpec::new(
            "decision_resolve",
            "Ask the model about the waiting decisions",
            "Decisions",
        ),
        CommandSpec::new(
            "decision_accept",
            "Accept a decision's proposal",
            "Decisions",
        )
        .params(object(&[
            node(),
            ("option", count(), false),
            (
                "who",
                text("who decided: the run's origin when not given"),
                false,
            ),
        ])),
        CommandSpec::new("decision_decide", "Give a decision's answer", "Decisions").params(
            object(&[
                node(),
                ("values", params(), true),
                (
                    "who",
                    text("who decided: the run's origin when not given"),
                    false,
                ),
            ]),
        ),
        CommandSpec::new(
            "decision_reask",
            "Withdraw a decision's answer",
            "Decisions",
        )
        .params(named("node")),
        CommandSpec::new("workflow.submit", "Submit images as a run", "Workflow").params(object(
            &[
                ("title", or_null(string()), false),
                (
                    "items",
                    json!({ "type": "array", "items": run_item() }),
                    true,
                ),
                ("out", absolute(), true),
                ("format", file_format(), true),
                ("quality", quality(), false),
                (
                    "name",
                    text("the file name: {stem} and {ext} replaced"),
                    false,
                ),
                ("node", or_null(string()), false),
                ("source", or_null(string()), false),
                ("again", boolean(), false),
                ("concurrency", or_null(count()), false),
            ],
        )),
        CommandSpec::new("workflow.runs", "List the runs", "Workflow").read_only(),
        CommandSpec::new("workflow.items", "List a run's items", "Workflow")
            .params(on_run(&[
                ("node", or_null(string()), false),
                ("state", or_null(string()), false),
                ("offset", count(), false),
                ("limit", count(), false),
            ]))
            .read_only(),
        CommandSpec::new("workflow.forget", "Remove a run not at work", "Workflow")
            .params(on_run(&[])),
        CommandSpec::new("workflow.cancel", "Cancel a run's items", "Workflow").params(on_run(&[
            ("items", or_null(strings()), false),
            ("node", or_null(string()), false),
        ])),
        CommandSpec::new("workflow.retry", "Retry a run's failed items", "Workflow").params(
            on_run(&[
                ("items", or_null(strings()), false),
                ("node", or_null(string()), false),
            ]),
        ),
        CommandSpec::new(
            "workflow.prioritize",
            "Move a run's items first or last",
            "Workflow",
        )
        .params(on_run(&[
            ("items", strings(), true),
            (
                "to",
                json!({ "type": "string", "enum": ["top", "bottom"] }),
                true,
            ),
        ])),
        CommandSpec::new("workflow.pause", "Pause a run", "Workflow").params(on_run(&[])),
        CommandSpec::new("workflow.resume", "Resume a run", "Workflow").params(on_run(&[])),
        helios_node_core::library::import_spec(),
        CommandSpec::new("settings.layer.get", "Read a settings layer", "Settings")
            .params(object(&[(
                "layer",
                text("`user` or `project/<library folder>`"),
                true,
            )]))
            .needs(Need::Vault)
            .read_only(),
        CommandSpec::new("settings.layer.set", "Write a settings layer", "Settings")
            .params(object(&[
                ("layer", text("`user` or `project/<library folder>`"), true),
                ("text", text("TOML"), true),
            ]))
            .needs(Need::Vault),
        CommandSpec::new("cache.stats", "Show what the caches hold", "Cache").read_only(),
        CommandSpec::new("cache.clear", "Clear the caches", "Cache").params(object(&[(
            "kind",
            or_null(string()),
            false,
        )])),
        crate::calibrate::chart_spec(),
    ]
}

/// The node's commands: its control verbs, and its library's commands
/// ([`crate::library`]).
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = Registry::new();
        for spec in verbs()
            .into_iter()
            .chain(crate::library::specs())
            .chain(crate::calibrate::profile_specs())
        {
            registry.register(spec).expect("each verb is declared once");
        }
        registry
    })
}

/// The request a command runs: its params with its id as `cmd`.
pub fn request(id: &str, params: &Value) -> Value {
    let mut request = match params {
        Value::Object(fields) => fields.clone(),
        _ => serde_json::Map::new(),
    };
    request.insert("cmd".into(), json!(id));
    Value::Object(request)
}

#[cfg(test)]
mod tests {
    use helios_inspect::control::ControlRequest;
    use helios_node_core::commands::validate;

    use super::*;

    /// Params each command takes, its optional fields given.
    fn examples() -> Vec<(&'static str, Value)> {
        let region = json!({ "x": 0, "y": 0, "width": 8, "height": 8 });
        vec![
            (
                "render",
                json!({ "node": "exp", "port": "output", "region": region }),
            ),
            (
                "set_param",
                json!({ "node": "exp", "param": "exposure", "value": { "float": 1.0 } }),
            ),
            ("node.bypass", json!({ "node": "exp", "on": true })),
            ("load_source", json!({ "node": "src", "path": "/in.png" })),
            (
                "export",
                json!({ "node": "exp", "port": "output", "region": null, "path": "/o.png",
                        "format": "png", "quality": 90,
                        "params": { "exp": { "exposure": { "float": 0.5 } } },
                        "batch": "batch-1", "as_mask": false }),
            ),
            ("cancel", json!({ "job": 3 })),
            ("job.cancel", json!({ "id": "export-3" })),
            (
                "batch_start",
                json!({ "title": "t", "total": 2, "hold": true, "idle_timeout_s": 5 }),
            ),
            (
                "batch_end",
                json!({ "id": "batch-1", "failed": 1, "cancelled": false }),
            ),
            ("pause", json!({})),
            ("resume", json!({})),
            ("fault", json!({ "node": "exp", "kind": "source_read" })),
            ("decisions", json!({})),
            ("decision_resolve", json!({})),
            (
                "decision_accept",
                json!({ "node": "d", "option": 1, "who": "user" }),
            ),
            (
                "decision_decide",
                json!({ "node": "d", "values": { "ev": { "float": 1.0 } }, "who": "agent:mcp" }),
            ),
            ("decision_reask", json!({ "node": "d" })),
            (
                "workflow.submit",
                json!({ "title": null, "items": ["/a.png", { "path": "/b.png", "params": {} }],
                        "out": "/out", "format": "jpeg", "quality": null, "name": "{stem}.{ext}",
                        "node": null, "source": "src", "again": true, "concurrency": 2 }),
            ),
            ("workflow.runs", json!({})),
            (
                "workflow.items",
                json!({ "run": "r", "node": null, "state": "waiting", "offset": 0, "limit": 10 }),
            ),
            ("workflow.forget", json!({ "run": "r" })),
            (
                "workflow.cancel",
                json!({ "run": "r", "items": ["/a.png"], "node": null }),
            ),
            ("workflow.retry", json!({ "run": "r", "items": null })),
            (
                "workflow.prioritize",
                json!({ "run": "r", "items": ["/a.png"], "to": "top" }),
            ),
            ("workflow.pause", json!({ "run": "r" })),
            ("workflow.resume", json!({ "run": "r" })),
            (
                "library.import",
                json!({ "paths": ["/photos"], "copy": true, "into": "/library",
                        "rename": "{stem}", "layout": "month", "duplicates": "keep",
                        "signals": false }),
            ),
            ("settings.layer.get", json!({ "layer": "user" })),
            (
                "settings.layer.set",
                json!({ "layer": "user", "text": "[a]\nb = 1\n" }),
            ),
            ("cache.stats", json!({})),
            ("cache.clear", json!({ "kind": null })),
            (
                "calibrate.chart",
                json!({ "path": "/chart.png", "image": null,
                        "corners": [[0, 0], [1, 0], [1, 1], [0, 1]],
                        "chart": null, "exclude": ["B3"], "dry_run": true,
                        "save": true, "name": "Flash" }),
            ),
        ]
    }

    /// Every verb is a command, and every command's params, checked by its
    /// schema, make the request of that verb.
    #[test]
    fn every_verb_is_a_command_its_schema_fits() {
        let examples = examples();
        let ids: Vec<&str> = registry()
            .specs()
            .map(|s| s.id.as_str())
            .filter(|id| {
                !crate::library::handles(id) && !crate::calibrate::PROFILE_IDS.contains(id)
            })
            .collect();
        let mut named: Vec<&str> = examples.iter().map(|(id, _)| *id).collect();
        named.sort_unstable();
        assert_eq!(ids, named, "one example per command");
        for (id, params) in examples {
            let spec = registry().spec(id).unwrap();
            validate(&spec.params, &params).unwrap_or_else(|e| panic!("{id}: {e}"));
            let made: ControlRequest = serde_json::from_value(request(id, &params))
                .unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(made.verb(), id);
            // Its required fields alone also make the request.
            let required: serde_json::Map<String, Value> = params
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, _)| {
                    spec.params["required"]
                        .as_array()
                        .is_some_and(|r| r.contains(&json!(k)))
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let required = Value::Object(required);
            validate(&spec.params, &required).unwrap_or_else(|e| panic!("{id}: {e}"));
            serde_json::from_value::<ControlRequest>(request(id, &required))
                .unwrap_or_else(|e| panic!("{id} with its required fields: {e}"));
        }
        assert!(registry().spec("commands.list").is_none());
        assert!(registry().spec("command.run").is_none());
    }
}
