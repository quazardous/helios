//! The node's verbs as commands (`helios_node_core::commands`): each
//! [`ControlRequest`](helios_inspect::control::ControlRequest) verb is a
//! command of the same id, whose params are the request's fields. Running
//! one (`command.run`) builds that request and answers it as the endpoint
//! would, so a command and its request behave the same.
//!
//! **Each verb is declared by the crate that owns its concept**, as its own
//! `verbs()` set ([`SETS`]): the workflow's ([`crate::workflow::verbs`]),
//! the decisions' (`helios_decision::verbs`), the settings layers'
//! (`helios_node_core::settings::verbs`), the caches'
//! (`helios_cache::verbs`). The node declares only its own ([`verbs`]):
//! those answered by the network and the scheduler it runs (render, edit,
//! export, jobs, a fault for tests), which no other crate owns.

use std::sync::OnceLock;

use helios_command::schema::{absolute, boolean, count, node_params, or_null, string, text, value};
use helios_node_core::commands::{CommandSpec, Registry, object};
use serde_json::{Value, json};

fn region() -> Value {
    object(&[
        ("x", json!({ "type": "integer" }), true),
        ("y", json!({ "type": "integer" }), true),
        ("width", count(), true),
        ("height", count(), true),
    ])
}

/// The file formats a node writes.
pub(crate) fn file_format() -> Value {
    json!({ "type": "string", "enum": ["jpeg", "png", "tiff"] })
}

/// A JPEG quality.
pub(crate) fn quality() -> Value {
    json!({ "type": ["integer", "null"], "minimum": 1, "maximum": 100,
            "description": "JPEG quality, 92 when not given" })
}

/// The node's own verbs: its network's (render, edit, export, a fault for
/// tests) and its scheduler's (jobs and batches), plus the library import
/// and the chart calibration, declared by their owners and kept here in
/// their place.
pub fn verbs() -> Vec<CommandSpec> {
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
        helios_node_core::library::import_spec(),
        crate::calibrate::chart_spec(),
    ]
}

/// Every set of verbs the node answers, each declared by the crate that
/// owns its concept: the node's own first.
pub const SETS: &[fn() -> Vec<CommandSpec>] = &[
    verbs,
    helios_decision::verbs::verbs,
    crate::workflow::verbs,
    helios_node_core::settings::verbs,
    helios_cache::verbs::verbs,
];

/// The node's commands: every verb of [`SETS`], and its library's
/// commands ([`crate::library`]).
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = Registry::new();
        for spec in SETS
            .iter()
            .flat_map(|set| set())
            .chain(crate::library::specs())
            .chain(crate::cursor::specs())
            .chain(crate::promises::specs())
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
                !crate::library::handles(id)
                    && !crate::cursor::handles(id)
                    && !crate::promises::handles(id)
                    && !crate::calibrate::PROFILE_IDS.contains(id)
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

    /// Every command `verbs()` held when the node declared them all, before
    /// each set moved to the crate that owns it (generated from the
    /// registry on `next` before the move), with
    /// its label, category, params schema, needs and read-only flag: the
    /// MCP `helios_*` tools, the inspect `describe` list and what agents
    /// may run are built from these, so none may change.
    const BEFORE_THE_MOVE: &str = include_str!("../tests/fixtures/verbs-before-the-move.json");

    #[test]
    fn every_verb_is_declared_once_and_unchanged_by_the_move() {
        let mut ids: Vec<String> = SETS
            .iter()
            .flat_map(|set| set())
            .chain(crate::library::specs())
            .chain(crate::cursor::specs())
            .chain(crate::promises::specs())
            .chain(crate::calibrate::profile_specs())
            .map(|s| s.id)
            .collect();
        let all = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all, "a verb declared twice");

        let before: Vec<Value> = serde_json::from_str(BEFORE_THE_MOVE).unwrap();
        for spec in &before {
            let id = spec["id"].as_str().unwrap();
            let now = registry()
                .spec(id)
                .unwrap_or_else(|| panic!("`{id}` is gone"));
            assert_eq!(&serde_json::to_value(now).unwrap(), spec, "`{id}`");
        }
        let moved: usize = SETS[1..].iter().map(|set| set().len()).sum();
        assert_eq!(verbs().len() + moved, before.len(), "no verb added or lost");
    }

    /// The node declares only its own verbs: its network's and its
    /// scheduler's, plus the two its owners declare and it lists in place.
    #[test]
    fn the_node_declares_only_its_own_verbs() {
        let own: Vec<String> = verbs().into_iter().map(|s| s.id).collect();
        assert_eq!(
            own,
            [
                "render",
                "set_param",
                "node.bypass",
                "load_source",
                "export",
                "cancel",
                "job.cancel",
                "batch_start",
                "batch_end",
                "pause",
                "resume",
                "fault",
                "library.import",
                "calibrate.chart",
            ]
        );
    }
}
