//! The `controls.*` commands (section 3.10), what agents, scripts and
//! menus run alike:
//!
//! - `controls.catalogue`: every registered control, one line each;
//! - `controls.describe {id}`: one control's full description (purpose,
//!   connector, gestures, detents, variants, each variant's slot
//!   policies, orientations and integration level per surface with its
//!   fallback, warnings, the JSON schema of its settings, its patches);
//! - `controls.set {node, control, value, force}`,
//!   `controls.nudge {node, control, steps, fine, coarse}`,
//!   `controls.reset {node, control}`: one gesture each, through the
//!   `controls.change` step, one undo step.
//! - `controls.context`: the context keys, the box, the density and the
//!   hand (gesture open, pointer in the panel, the part handled); set any
//!   of them with the same names (`control-contexts.md`, section 6.5);
//! - `controls.find {node, query}`: every control of a node's card whose
//!   key, id or label holds the query, shown or folded, with its
//!   placement and reason;
//! - `controls.reveal {node, key}`, `controls.reveal_all {node}`: shows
//!   a folded part, or every part, for the moment (until the task
//!   changes);
//! - `controls.describe {id, node}`: with a node, adds the control's
//!   placement on that node's card and its reasons.
//!
//! [`instances`] is what the `controls` collector reads: every node
//! parameter with the control it is edited by (patched or implicit).

use std::sync::{Arc, Mutex, PoisonError};

use helios_shell::commands::{CommandSpec, Origin, Registry as Commands};
use helios_values::Store;
use serde_json::{Value as Json, json};

use std::collections::BTreeMap;

use crate::card::{Card, CardItem};
use crate::context::ContextKeys;
use crate::flow::{self, Cause, SharedSteps};
use crate::registry::{Registry, implicit};
use crate::resolve::{self, CardPlacement, Layouts, Moment, Values};

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

/// The context the `controls.*` commands resolve cards in: the keys, the
/// layouts, the hand, and per node what the person revealed and the last
/// resolution (for hysteresis and anti-jump).
#[derive(Debug, Default)]
pub struct Contexts {
    pub keys: ContextKeys,
    pub layouts: Layouts,
    /// The hand: a gesture open, the pointer in the panel, the part
    /// handled (its `revealed` and `reveal_all` are unused).
    pub hand: Moment,
    /// Per node: the parts revealed, "show all".
    pub moments: BTreeMap<String, Moment>,
    /// Per node: the last resolution.
    pub previous: BTreeMap<String, CardPlacement>,
}

pub type SharedContexts = Arc<Mutex<Contexts>>;

impl Contexts {
    /// The moment of a node's card: its reveals and the hand.
    fn moment(&self, node: &str) -> Moment {
        let mut m = self.moments.get(node).cloned().unwrap_or_default();
        m.gesture_open = self.hand.gesture_open;
        m.pointer_in_panel = self.hand.pointer_in_panel;
        m.handled.clone_from(&self.hand.handled);
        m
    }
}

/// A node's card: one item per parameter (keyed by its name) with the
/// control patched to it or its implicit one, the registry with those
/// implicit controls added, and the values.
pub fn node_card(
    store: &Store,
    registry: &Registry,
    node: &str,
) -> Result<(Card, Registry, Values), String> {
    let prefix = format!("node/{node}/");
    let mut reg = registry.clone();
    let mut items = Vec::new();
    let mut values = Values::new();
    for (path, e) in store.list(&prefix) {
        let param = &path[prefix.len()..];
        let id = if let Some(c) = registry.control_for(path, &e.descriptor) {
            c.id.clone()
        } else {
            let c = implicit(path, &e.descriptor);
            let id = c.id.clone();
            if reg.get(&id).is_none() {
                reg.register(c)?;
            }
            id
        };
        items.push(CardItem::control(param, &id));
        values.insert(param.to_owned(), helios_values::value_to_json(&e.value));
    }
    if items.is_empty() {
        return Err(format!("no node `{node}`"));
    }
    Ok((Card::new(items), reg, values))
}

/// Resolves a node's card in the current context, and keeps the result
/// as the node's previous one.
pub fn resolve_node(
    store: &Store,
    registry: &Registry,
    contexts: &mut Contexts,
    node: &str,
) -> Result<CardPlacement, String> {
    let (card, reg, values) = node_card(store, registry, node)?;
    let moment = contexts.moment(node);
    let out = resolve::resolve(
        &reg,
        &card,
        &contexts.keys,
        &contexts.layouts,
        &values,
        &moment,
        contexts.previous.get(node),
    )?;
    contexts.previous.insert(node.to_owned(), out.clone());
    Ok(out)
}

/// Reads one optional argument as `T`.
fn arg<T: serde::de::DeserializeOwned>(args: &Json, key: &str) -> Result<Option<T>, String> {
    args.get(key)
        .map(|v| serde_json::from_value(v.clone()).map_err(|e| format!("`{key}`: {e}")))
        .transpose()
}

/// Sets what `args` names on the context; a task change ends every
/// reveal (they last until the person leaves the card or the task).
fn set_context(c: &mut Contexts, args: &Json) -> Result<(), String> {
    if let Some(t) = arg(args, "task")? {
        if t != c.keys.task {
            c.moments.clear();
        }
        c.keys.task = t;
    }
    if let Some(d) = arg(args, "detail")? {
        c.keys.detail = d;
    }
    if let Some(i) = arg(args, "image")? {
        c.keys.image = i;
    }
    if let Some(t) = arg(args, "tool")? {
        c.keys.tool = t;
    }
    if let Some(d) = arg(args, "device")? {
        c.keys.device = d;
    }
    if let Some(t) = arg(args, "target")? {
        c.keys.target = t;
    }
    if let Some(d) = arg(args, "density")? {
        c.keys.density = d;
    }
    if let Some(b) = arg(args, "box")? {
        c.keys.set_box(b);
    }
    if args.get("space").is_some() {
        return Err("`space` is derived from the box: set `box`".into());
    }
    if let Some(l) = arg(args, "layouts")? {
        c.layouts = l;
    }
    if let Some(g) = arg(args, "gesture_open")? {
        c.hand.gesture_open = g;
    }
    if let Some(p) = arg(args, "pointer_in_panel")? {
        c.hand.pointer_in_panel = p;
    }
    if args.get("handled").is_some() {
        c.hand.handled = arg(args, "handled")?;
    }
    Ok(())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Declares the `controls.*` commands, with a context of their own.
pub fn register(
    commands: &mut Commands,
    store: Arc<Mutex<Store>>,
    registry: Arc<Registry>,
    steps: SharedSteps,
) -> Result<(), String> {
    register_with(commands, store, registry, steps, SharedContexts::default())
}

/// Declares the `controls.*` commands over a context the host shares
/// (it sets the box, the hand and the task as they change).
pub fn register_with(
    commands: &mut Commands,
    store: Arc<Mutex<Store>>,
    registry: Arc<Registry>,
    steps: SharedSteps,
    contexts: SharedContexts,
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
    let (s, r, cx) = (store.clone(), registry.clone(), contexts.clone());
    commands.register(
        CommandSpec::new(
            "controls.describe",
            "Describe a control (`id`): purpose, connector, gestures, detents, variants, slot policies, orientations, integration and fallback per surface, warnings, settings schema; with `node`, its placement on that node's card and why",
            "Controls",
        )
        .read_only(),
        move |args, _| {
            let id = str_arg(args, "id")?;
            let Some(node) = args.get("node").and_then(Json::as_str) else {
                return r.describe(id).ok_or_else(|| format!("no control `{id}`"));
            };
            let st = lock(&s);
            let (_, reg, _) = node_card(&st, &r, node)?;
            let mut out = reg
                .describe(id)
                .ok_or_else(|| format!("no control `{id}`"))?;
            let p = resolve_node(&st, &r, &mut lock(&cx), node)?;
            let placements: Vec<_> = p
                .controls()
                .filter(|p| p.control.as_deref() == Some(id))
                .collect();
            out["placement"] = json!(placements);
            out["more"] = json!(p.more);
            Ok(out)
        },
    )?;
    let cx = contexts.clone();
    commands.register(
        CommandSpec::new(
            "controls.context",
            "Read the controls' context keys, box, density and hand; set any of them (`task`, `detail`, `image`, `tool`, `device`, `target`, `density`, `box`, `layouts`, `gesture_open`, `pointer_in_panel`, `handled`)",
            "Controls",
        ),
        move |args, _| {
            let mut c = lock(&cx);
            set_context(&mut c, args)?;
            Ok(json!({
                "keys": c.keys.report(),
                "hand": c.hand,
                "frozen": c.hand.frozen(),
                "layouts": c.layouts,
            }))
        },
    )?;
    let (s, r, cx) = (store.clone(), registry.clone(), contexts.clone());
    commands.register(
        CommandSpec::new(
            "controls.find",
            "Search a node's controls (`node`, `query`), shown or folded, with their placement and reason",
            "Controls",
        )
        .read_only(),
        move |args, _| {
            let node = str_arg(args, "node")?;
            let query = args.get("query").and_then(Json::as_str).unwrap_or("");
            let p = resolve_node(&lock(&s), &r, &mut lock(&cx), node)?;
            Ok(json!(resolve::find(&p, query)))
        },
    )?;
    let (s, r, cx) = (store.clone(), registry.clone(), contexts.clone());
    commands.register(
        CommandSpec::new(
            "controls.reveal",
            "Show a folded part of a node's card for the moment (`node`, `key`)",
            "Controls",
        ),
        move |args, _| {
            let node = str_arg(args, "node")?;
            let key = str_arg(args, "key")?;
            let st = lock(&s);
            let mut c = lock(&cx);
            let before = resolve_node(&st, &r, &mut c, node)?;
            if before.get(key).is_none() {
                return Err(format!("node `{node}` has no control `{key}`"));
            }
            c.moments
                .entry(node.to_owned())
                .or_default()
                .revealed
                .insert(key.to_owned());
            let p = resolve_node(&st, &r, &mut c, node)?;
            Ok(json!({"placement": p.get(key), "more": p.more}))
        },
    )?;
    let (s, r, cx) = (store.clone(), registry.clone(), contexts);
    commands.register(
        CommandSpec::new(
            "controls.reveal_all",
            "Show every part of a node's card for the moment (`node`)",
            "Controls",
        ),
        move |args, _| {
            let node = str_arg(args, "node")?;
            let st = lock(&s);
            let mut c = lock(&cx);
            c.moments.entry(node.to_owned()).or_default().reveal_all = true;
            let p = resolve_node(&st, &r, &mut c, node)?;
            Ok(json!(p))
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
