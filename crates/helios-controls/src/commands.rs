//! The `controls.*` commands (section 3.10), what agents, scripts and
//! menus run alike:
//!
//! - `controls.catalogue`: every registered control, one line each;
//! - `controls.describe {id}`: one control's full description (purpose,
//!   footprints, connector, gestures, detents, variants, the JSON schema
//!   of its settings, its patches);
//! - `controls.set {node, control, value, force}`,
//!   `controls.nudge {node, control, steps, fine, coarse}`,
//!   `controls.reset {node, control}`: one gesture each, through the
//!   `controls.change` step, one undo step.
//!
//! [`instances`] is what the `controls` collector reads: every node
//! parameter with the control it is edited by (patched or implicit).

use std::sync::{Arc, Mutex, PoisonError};

use helios_shell::commands::{CommandSpec, Origin, Registry as Commands};
use helios_values::Store;
use serde_json::{Value as Json, json};

use crate::flow::{self, Cause, SharedSteps};
use crate::registry::{Registry, implicit};

/// The store path a control edits on a node: the parameter its patch
/// names, when the node is of the patch's module.
pub fn path_of(
    store: &Store,
    registry: &Registry,
    node: &str,
    control: &str,
) -> Result<String, String> {
    let prefix = format!("node/{node}/");
    for (path, e) in store.list(&prefix) {
        let Some(module) = e.descriptor.module.as_deref() else {
            continue;
        };
        let param = &path[prefix.len()..];
        if registry
            .patch_of(module, param)
            .is_some_and(|p| p.control == control)
        {
            return Ok(path.to_owned());
        }
    }
    Err(format!("node `{node}` has no control `{control}`"))
}

fn str_arg<'a>(args: &'a Json, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("`{key}` is missing"))
}

fn device_of(origin: &Origin) -> &'static str {
    match origin {
        Origin::User => "pointer",
        Origin::Agent(_) => "agent",
        Origin::Script(_) => "script",
        Origin::Midi => "midi",
        Origin::Remote(_) => "remote",
    }
}

/// What a change command does to the value it finds.
enum Ask {
    To(f64, bool),
    Nudge(i64, bool, bool),
    Reset,
}

fn change(
    store: &Mutex<Store>,
    registry: &Registry,
    steps: &SharedSteps,
    args: &Json,
    origin: &Origin,
    ask: &Ask,
) -> Result<Json, String> {
    let node = str_arg(args, "node")?;
    let id = str_arg(args, "control")?;
    let control = registry
        .get(id)
        .ok_or_else(|| format!("no control `{id}`"))?;
    let mut s = store.lock().unwrap_or_else(PoisonError::into_inner);
    let path = path_of(&s, registry, node, id)?;
    let now = s
        .get(&path)
        .and_then(|e| e.value.as_f64())
        .ok_or_else(|| format!("`{path}` is not a number"))?;
    let default = control.value.number().map_or(0.0, |n| n.default);
    let (to, force, cause) = match *ask {
        Ask::To(v, force) => (v, force, Cause::Set),
        Ask::Nudge(n, fine, coarse) => (control.nudged(now, n, fine, coarse), false, Cause::Nudge),
        Ask::Reset => (default, true, Cause::Reset),
    };
    let steps = steps.lock().unwrap_or_else(PoisonError::into_inner);
    let outcome = flow::apply(
        &mut s,
        &steps,
        control,
        &path,
        flow::OneShot {
            to,
            force,
            origin: origin.clone(),
            cause,
            device: device_of(origin),
        },
    )?;
    let value = s.get(&path).and_then(|e| e.value.as_f64());
    let mut out = serde_json::to_value(&outcome).unwrap_or(Json::Null);
    out["path"] = json!(path);
    out["value"] = json!(value);
    out["shown"] = json!(value.zip(control.value.number()).map(|(v, n)| n.format(v)));
    Ok(out)
}

/// Declares the `controls.*` commands.
pub fn register(
    commands: &mut Commands,
    store: Arc<Mutex<Store>>,
    registry: Arc<Registry>,
    steps: SharedSteps,
) -> Result<(), String> {
    let r = registry.clone();
    commands.register(
        CommandSpec::new(
            "controls.catalogue",
            "List the registered controls",
            "Controls",
        )
        .read_only(),
        move |_, _| Ok(r.catalogue()),
    )?;
    let r = registry.clone();
    commands.register(
        CommandSpec::new(
            "controls.describe",
            "Describe a control (`id`): purpose, footprints, connector, gestures, detents, variants, settings schema",
            "Controls",
        )
        .read_only(),
        move |args, _| {
            let id = str_arg(args, "id")?;
            r.describe(id).ok_or_else(|| format!("no control `{id}`"))
        },
    )?;
    let (s, r, st) = (store.clone(), registry.clone(), steps.clone());
    commands.register(
        CommandSpec::new(
            "controls.set",
            "Set a control on a node (`node`, `control`, `value`; `force`)",
            "Controls",
        ),
        move |args, origin| {
            let value = args
                .get("value")
                .and_then(Json::as_f64)
                .ok_or("`value` is missing")?;
            let force = args.get("force").and_then(Json::as_bool).unwrap_or(false);
            change(&s, &r, &st, args, origin, &Ask::To(value, force))
        },
    )?;
    let (s, r, st) = (store.clone(), registry.clone(), steps.clone());
    commands.register(
        CommandSpec::new(
            "controls.nudge",
            "Step a control on a node (`node`, `control`, `steps`; `fine`, `coarse`)",
            "Controls",
        ),
        move |args, origin| {
            let n = args.get("steps").and_then(Json::as_i64).unwrap_or(1);
            let fine = args.get("fine").and_then(Json::as_bool).unwrap_or(false);
            let coarse = args.get("coarse").and_then(Json::as_bool).unwrap_or(false);
            change(&s, &r, &st, args, origin, &Ask::Nudge(n, fine, coarse))
        },
    )?;
    let (s, r, st) = (store, registry, steps);
    commands.register(
        CommandSpec::new(
            "controls.reset",
            "Reset a control on a node to its default (`node`, `control`)",
            "Controls",
        ),
        move |args, origin| change(&s, &r, &st, args, origin, &Ask::Reset),
    )?;
    Ok(())
}

/// Every node parameter and the control that edits it: `control` (its
/// id), `implicit` when built from the descriptor, the value and how the
/// control shows it.
pub fn instances(store: &Store, registry: &Registry) -> Json {
    let mut out = Vec::new();
    for (path, e) in store.list("node/") {
        let d = &e.descriptor;
        let (control, is_implicit) = match registry.control_for(path, d) {
            Some(c) => (c.clone(), false),
            None => (implicit(path, d), true),
        };
        let shown = e
            .value
            .as_f64()
            .zip(control.value.number())
            .map(|(v, n)| n.format(v));
        out.push(json!({
            "path": path,
            "node": flow::node_of(path),
            "control": control.id,
            "implicit": is_implicit,
            "value": helios_values::value_to_json(&e.value),
            "shown": shown,
        }));
    }
    Json::Array(out)
}
