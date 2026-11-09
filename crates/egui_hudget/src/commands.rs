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
//! [`instances`] is what a `controls` collector reads: every node
//! parameter with the control it is edited by (patched or implicit).
//!
//! The commands are plain functions: [`COMMANDS`] says what each is,
//! [`run`] runs one; the host declares them in its own command registry.

use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value as Json, json};

use std::collections::BTreeMap;

use crate::card::{Card, CardItem};
use crate::context::ContextKeys;
use crate::flow::{self, Cause, SharedSteps};
use crate::registry::{Registry, implicit};
use crate::resolve::{self, CardPlacement, Layouts, Moment, Values};
use crate::value::{ValueSource, ValueStore};

/// The store path a control edits on a node: the parameter its patch
/// names, when the node is of the patch's module.
pub fn path_of<S: ValueSource + ?Sized>(
    store: &S,
    registry: &Registry,
    node: &str,
    control: &str,
) -> Result<String, String> {
    let prefix = format!("node/{node}/");
    for p in store.params(&prefix) {
        let Some(module) = p.descriptor.module.as_deref() else {
            continue;
        };
        let param = &p.path[prefix.len()..];
        if registry
            .patch_of(module, param)
            .is_some_and(|p| p.control == control)
        {
            return Ok(p.path);
        }
    }
    Err(format!("node `{node}` has no control `{control}`"))
}

fn str_arg<'a>(args: &'a Json, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("`{key}` is missing"))
}

/// What a change command does to the value it finds.
enum Ask {
    To(f64, bool),
    Nudge(i64, bool, bool),
    Reset,
}

fn change<S: ValueStore>(
    s: &mut S,
    registry: &Registry,
    steps: &SharedSteps,
    args: &Json,
    origin: &S::Origin,
    ask: &Ask,
) -> Result<Json, String> {
    let node = str_arg(args, "node")?;
    let id = str_arg(args, "control")?;
    let control = registry
        .get(id)
        .ok_or_else(|| format!("no control `{id}`"))?;
    let path = path_of(s, registry, node, id)?;
    let now = s
        .get(&path)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| format!("`{path}` is not a number"))?;
    let default = control.value.number().map_or(0.0, |n| n.default);
    let (to, force, cause) = match *ask {
        Ask::To(v, force) => (v, force, Cause::Set),
        Ask::Nudge(n, fine, coarse) => (control.nudged(now, n, fine, coarse), false, Cause::Nudge),
        Ask::Reset => (default, true, Cause::Reset),
    };
    let steps = steps.lock().unwrap_or_else(PoisonError::into_inner);
    let outcome = flow::apply(
        s,
        &steps,
        control,
        &path,
        flow::OneShot {
            to,
            force,
            origin: origin.clone(),
            cause,
            device: S::device(origin),
        },
    )?;
    let value = s.get(&path).and_then(|v| v.as_f64());
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
pub fn node_card<S: ValueSource + ?Sized>(
    store: &S,
    registry: &Registry,
    node: &str,
) -> Result<(Card, Registry, Values), String> {
    let prefix = format!("node/{node}/");
    let mut reg = registry.clone();
    let mut items = Vec::new();
    let mut values = Values::new();
    for e in store.params(&prefix) {
        let path = e.path.as_str();
        let param = &path[prefix.len()..];
        let id = if let Some(c) = registry.control_at(path, e.descriptor.module.as_deref()) {
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
        values.insert(param.to_owned(), e.value.to_json());
    }
    if items.is_empty() {
        return Err(format!("no node `{node}`"));
    }
    Ok((Card::new(items), reg, values))
}

/// Resolves a node's card in the current context, and keeps the result
/// as the node's previous one.
pub fn resolve_node<S: ValueSource + ?Sized>(
    store: &S,
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
pub fn set_context(c: &mut Contexts, args: &Json) -> Result<(), String> {
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

/// What a command is: its name, what it does, whether it only reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: &'static str,
    pub help: &'static str,
    pub read_only: bool,
}

/// The menu group the commands belong to.
pub const GROUP: &str = "Controls";

/// Every `controls.*` command, in the order a host declares them.
pub const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "controls.catalogue",
        help: "List the registered controls",
        read_only: true,
    },
    CommandInfo {
        name: "controls.describe",
        help: "Describe a control (`id`): purpose, connector, gestures, detents, variants, slot policies, orientations, integration and fallback per surface, warnings, settings schema; with `node`, its placement on that node's card and why",
        read_only: true,
    },
    CommandInfo {
        name: "controls.context",
        help: "Read the controls' context keys, box, density and hand; set any of them (`task`, `detail`, `image`, `tool`, `device`, `target`, `density`, `box`, `layouts`, `gesture_open`, `pointer_in_panel`, `handled`)",
        read_only: false,
    },
    CommandInfo {
        name: "controls.find",
        help: "Search a node's controls (`node`, `query`), shown or folded, with their placement and reason",
        read_only: true,
    },
    CommandInfo {
        name: "controls.reveal",
        help: "Show a folded part of a node's card for the moment (`node`, `key`)",
        read_only: false,
    },
    CommandInfo {
        name: "controls.reveal_all",
        help: "Show every part of a node's card for the moment (`node`)",
        read_only: false,
    },
    CommandInfo {
        name: "controls.set",
        help: "Set a control on a node (`node`, `control`, `value`; `force`)",
        read_only: false,
    },
    CommandInfo {
        name: "controls.nudge",
        help: "Step a control on a node (`node`, `control`, `steps`; `fine`, `coarse`)",
        read_only: false,
    },
    CommandInfo {
        name: "controls.reset",
        help: "Reset a control on a node to its default (`node`, `control`)",
        read_only: false,
    },
];

/// What the commands share: the registry, the `controls.change` step and
/// the context.
#[derive(Clone, Debug, Default)]
pub struct Shared {
    pub registry: Arc<Registry>,
    pub steps: SharedSteps,
    pub contexts: SharedContexts,
}

/// Runs the command `name` (one of [`COMMANDS`]) with `args`, as
/// `origin`, on `store`.
pub fn run<S: ValueStore>(
    name: &str,
    args: &Json,
    origin: &S::Origin,
    store: &mut S,
    shared: &Shared,
) -> Result<Json, String> {
    let r = &*shared.registry;
    let cx = &shared.contexts;
    match name {
        "controls.catalogue" => Ok(r.catalogue()),
        "controls.describe" => {
            let id = str_arg(args, "id")?;
            let Some(node) = args.get("node").and_then(Json::as_str) else {
                return r.describe(id).ok_or_else(|| format!("no control `{id}`"));
            };
            let (_, reg, _) = node_card(store, r, node)?;
            let mut out = reg
                .describe(id)
                .ok_or_else(|| format!("no control `{id}`"))?;
            let p = resolve_node(store, r, &mut lock(cx), node)?;
            let placements: Vec<_> = p
                .controls()
                .filter(|p| p.control.as_deref() == Some(id))
                .collect();
            out["placement"] = json!(placements);
            out["more"] = json!(p.more);
            Ok(out)
        }
        "controls.context" => {
            let mut c = lock(cx);
            set_context(&mut c, args)?;
            Ok(json!({
                "keys": c.keys.report(),
                "hand": c.hand,
                "frozen": c.hand.frozen(),
                "layouts": c.layouts,
            }))
        }
        "controls.find" => {
            let node = str_arg(args, "node")?;
            let query = args.get("query").and_then(Json::as_str).unwrap_or("");
            let p = resolve_node(store, r, &mut lock(cx), node)?;
            Ok(json!(resolve::find(&p, query)))
        }
        "controls.reveal" => {
            let node = str_arg(args, "node")?;
            let key = str_arg(args, "key")?;
            let mut c = lock(cx);
            let before = resolve_node(store, r, &mut c, node)?;
            if before.get(key).is_none() {
                return Err(format!("node `{node}` has no control `{key}`"));
            }
            c.moments
                .entry(node.to_owned())
                .or_default()
                .revealed
                .insert(key.to_owned());
            let p = resolve_node(store, r, &mut c, node)?;
            Ok(json!({"placement": p.get(key), "more": p.more}))
        }
        "controls.reveal_all" => {
            let node = str_arg(args, "node")?;
            let mut c = lock(cx);
            c.moments.entry(node.to_owned()).or_default().reveal_all = true;
            let p = resolve_node(store, r, &mut c, node)?;
            Ok(json!(p))
        }
        "controls.set" => {
            let value = args
                .get("value")
                .and_then(Json::as_f64)
                .ok_or("`value` is missing")?;
            let force = args.get("force").and_then(Json::as_bool).unwrap_or(false);
            change(
                store,
                r,
                &shared.steps,
                args,
                origin,
                &Ask::To(value, force),
            )
        }
        "controls.nudge" => {
            let n = args.get("steps").and_then(Json::as_i64).unwrap_or(1);
            let fine = args.get("fine").and_then(Json::as_bool).unwrap_or(false);
            let coarse = args.get("coarse").and_then(Json::as_bool).unwrap_or(false);
            change(
                store,
                r,
                &shared.steps,
                args,
                origin,
                &Ask::Nudge(n, fine, coarse),
            )
        }
        "controls.reset" => change(store, r, &shared.steps, args, origin, &Ask::Reset),
        _ => Err(format!("no command `{name}`")),
    }
}

/// Every node parameter and the control that edits it: `control` (its
/// id), `implicit` when built from the descriptor, the value and how the
/// control shows it.
pub fn instances<S: ValueSource + ?Sized>(store: &S, registry: &Registry) -> Json {
    let mut out = Vec::new();
    for e in store.params("node/") {
        let path = e.path.as_str();
        let d = &e.descriptor;
        let (control, is_implicit) = match registry.control_at(path, d.module.as_deref()) {
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
            "value": e.value.to_json(),
            "shown": shown,
        }));
    }
    Json::Array(out)
}
