//! `helios.nodes`: nodes of the workspace's network and the wires
//! between them, a sub-network.
//!
//! A copy takes the selected nodes (a source, the workspace's image,
//! never), each with its parameters as the value store holds them now,
//! the wires between them, the wires coming in from outside ("cut" at
//! the boundary, by the names of the nodes they come from) and the
//! nodes' places in the graph view. A paste adds them to a network:
//! a name the network holds already is made unique (`blur` → `blur_2`),
//! the wires rewritten with the new names; a cut wire is wired again
//! when the node it came from is in the target network (part `inputs`
//! of paste special); places are shifted so the copy does not cover its
//! original. It is `network.paste`, one workspace step.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::AppHandle;
use egui_pop::api::clipboard::{ClipItem, ClipSource, ClipTarget, FormatId, Part, Paste, TEXT};
use helios_template::format::{NetworkDesc, NodeDesc};
pub use helios_template::network::{Clip, INPUTS, Pasted, SHIFT, paste};
use helios_values::Store;
use serde_json::{Value as Json, json};

use super::NODES;

/// The type's version.
pub const VERSION: u32 = 1;

/// The item a copy makes.
pub fn item(clip: &Clip) -> ClipItem {
    ClipItem::new(NODES, VERSION, clip.to_json(), &clip.title())
        .fallback(TEXT, Json::String(clip.text()))
}

/// The egui memory key of the graph's selection while it has the focus.
const SELECTION: &str = "helios.clipboard.graph";

/// The parameters of a node as the store holds them now
/// (`node/<node>/<param>`), over the network's.
fn live(node: &mut NodeDesc, store: &Store) {
    let prefix = format!("node/{}/", node.name);
    for (path, e) in store.list(&prefix) {
        if let Some(param) = path.strip_prefix(&prefix)
            && !param.contains('/')
        {
            node.params.set(param, e.value.clone());
        }
    }
}

/// Copies nodes of a network: those named in `selected` (sources and
/// unknown names left out), their wires, their places.
pub fn take(
    net: &NetworkDesc,
    positions: &BTreeMap<String, [f32; 2]>,
    selected: &[String],
    store: Option<&Store>,
) -> Result<Clip, String> {
    let nodes: Vec<NodeDesc> = net
        .nodes
        .iter()
        .filter(|n| selected.contains(&n.name))
        .cloned()
        .map(|mut n| {
            if let Some(s) = store {
                live(&mut n, s);
            }
            n
        })
        .collect();
    if nodes.is_empty() {
        return Err("no node of the network is selected (an image source is not copied)".into());
    }
    let has = |name: &str| nodes.iter().any(|n| n.name == name);
    Ok(Clip {
        connections: net
            .connections
            .iter()
            .filter(|c| has(&c.from) && has(&c.to))
            .cloned()
            .collect(),
        inputs: net
            .connections
            .iter()
            .filter(|c| !has(&c.from) && has(&c.to))
            .cloned()
            .collect(),
        positions: positions
            .iter()
            .filter(|(n, _)| has(n))
            .map(|(n, p)| (n.clone(), *p))
            .collect(),
        nodes,
    })
}

/// The parts paste special offers: each node, and the cut inputs.
pub fn parts(payload: &Json) -> Vec<Part> {
    let Ok(clip) = Clip::from_json(payload) else {
        return Vec::new();
    };
    let mut out: Vec<Part> = clip
        .nodes
        .iter()
        .map(|n| Part::new(&n.name, &n.name))
        .collect();
    if !clip.inputs.is_empty() {
        out.push(Part::new(INPUTS, "the wires coming in"));
    }
    out
}

/// The workspace's network and places, as the store holds them.
pub fn network(store: &Store) -> Option<(NetworkDesc, BTreeMap<String, [f32; 2]>)> {
    let helios_core::Value::Text(t) = &store.get(crate::shell::WORKSPACE_STATE)?.value else {
        return None;
    };
    let j: Json = serde_json::from_str(t).ok()?;
    let net = serde_json::from_value(j["template"]["network"].clone()).ok()?;
    let positions = serde_json::from_value(j["template"]["positions"].clone()).unwrap_or_default();
    Some((net, positions))
}

/// Says the graph has the focus, with its selection: the Edit commands
/// copy those nodes and paste into the network.
pub fn focus(app: &AppHandle, selection: Vec<String>, widget: egui::Id) {
    app.egui()
        .data_mut(|d| d.insert_temp(egui::Id::new(SELECTION), selection));
    egui_pop::api::clipboard::bind(app, super::GRAPH_ENDPOINT, widget);
}

/// The graph's endpoint: copies its selection, pastes into the
/// workspace's network through `network.paste`.
pub struct Endpoint {
    values: Arc<Mutex<Store>>,
    /// The selection a command or a test gives instead of the graph's.
    pinned: RefCell<Option<Vec<String>>>,
}

impl Endpoint {
    pub fn new(values: Arc<Mutex<Store>>) -> Self {
        Self {
            values,
            pinned: RefCell::new(None),
        }
    }

    /// Copies take these nodes until [`Endpoint::unpin`].
    pub fn pin(&self, nodes: Vec<String>) {
        *self.pinned.borrow_mut() = Some(nodes);
    }

    pub fn unpin(&self) {
        *self.pinned.borrow_mut() = None;
    }

    fn selection(&self, app: &AppHandle) -> Vec<String> {
        if let Some(p) = self.pinned.borrow().clone() {
            return p;
        }
        app.egui()
            .data(|d| d.get_temp::<Vec<String>>(egui::Id::new(SELECTION)))
            .unwrap_or_default()
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.values.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ClipSource for Endpoint {
    fn copy(&self, app: &AppHandle) -> Option<ClipItem> {
        let store = self.store();
        let (net, positions) = network(&store)?;
        take(&net, &positions, &self.selection(app), Some(&store))
            .ok()
            .map(|c| item(&c))
    }

    fn can_copy(&self, app: &AppHandle) -> bool {
        let selected = self.selection(app);
        !selected.is_empty()
            && network(&self.store())
                .is_some_and(|(net, _)| net.nodes.iter().any(|n| selected.contains(&n.name)))
    }
}

impl ClipTarget for Endpoint {
    fn accepts(&self, _: &AppHandle) -> Vec<FormatId> {
        if network(&self.store()).is_some() {
            vec![NODES]
        } else {
            Vec::new()
        }
    }

    fn paste(&self, app: &AppHandle, p: &Paste<'_>) -> Result<Json, String> {
        let clip = Clip::from_json(&p.repr.data)?;
        let (mut net, mut positions) = network(&self.store()).ok_or("no workspace is open")?;
        // What it would do, on a copy of the network.
        let plan = paste(&mut net, &mut positions, &clip, p.parts)?;
        if p.dry_run {
            return Ok(json!(plan.describe()));
        }
        // The network is the workspace's: its edit is a workspace step,
        // run by the shell at the next frame.
        let mut args = json!({ "clip": clip.to_json() });
        if let Some(parts) = p.parts {
            args["parts"] = json!(parts);
        }
        super::defer(app, "network.paste", args, p.origin);
        Ok(serde_json::to_value(&plan).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::Value;
    use helios_template::format::{Connection, Endpoint as Out};

    fn wire(from: &str, port: &str, to: &str, input: &str) -> Connection {
        Connection {
            from: from.into(),
            port: port.into(),
            to: to.into(),
            input: input.into(),
        }
    }

    fn node(name: &str, module: &str) -> NodeDesc {
        NodeDesc {
            name: name.into(),
            module: Some(module.into()),
            ..NodeDesc::default()
        }
    }

    /// source → exposure → sigmoid → blur.
    fn net() -> (NetworkDesc, BTreeMap<String, [f32; 2]>) {
        let net = NetworkDesc {
            sources: vec!["source".into()],
            nodes: vec![
                node("exposure", "exposure"),
                node("sigmoid", "sigmoid"),
                node("blur", "blur"),
            ],
            connections: vec![
                wire("source", "image", "exposure", "input"),
                wire("exposure", "output", "sigmoid", "input"),
                wire("sigmoid", "output", "blur", "input"),
            ],
            view: Out {
                node: "blur".into(),
                port: "output".into(),
            },
            presets: Vec::new(),
        };
        let positions = [("exposure", [0.0, 0.0]), ("sigmoid", [200.0, 0.0])]
            .into_iter()
            .map(|(n, p)| (n.to_string(), p))
            .collect();
        (net, positions)
    }

    #[test]
    fn a_copy_takes_the_selection_its_wires_and_the_cut_inputs() {
        let (net, positions) = net();
        let mut store = Store::new();
        store.declare_with(
            "node/exposure/exposure",
            helios_values::Descriptor::float("exposure", -3.0, 4.0, 0.0),
            Value::Float(0.7),
        );
        let clip = take(
            &net,
            &positions,
            &["exposure".into(), "sigmoid".into(), "source".into()],
            Some(&store),
        )
        .unwrap();
        assert_eq!(clip.names(), ["exposure", "sigmoid"]);
        assert_eq!(
            clip.connections,
            [wire("exposure", "output", "sigmoid", "input")]
        );
        assert_eq!(clip.inputs, [wire("source", "image", "exposure", "input")]);
        assert_eq!(clip.positions.len(), 2);
        // The parameter as the store holds it now.
        assert_eq!(
            clip.nodes[0].params.get("exposure"),
            Some(&Value::Float(0.7))
        );
        assert_eq!(clip.text(), "exposure, sigmoid");
        assert_eq!(clip.title(), "2 nodes");
        assert!(take(&net, &positions, &["source".into()], None).is_err());
    }

    #[test]
    fn a_paste_renames_on_a_clash_and_rewires() {
        let (mut net, mut positions) = net();
        let clip = take(
            &net,
            &positions,
            &["exposure".into(), "sigmoid".into()],
            None,
        )
        .unwrap();
        let out = paste(&mut net, &mut positions, &clip, None).unwrap();
        assert_eq!(out.added, ["exposure_2", "sigmoid_2"]);
        assert_eq!(out.renamed.len(), 2);
        assert_eq!(out.reconnected, 1);
        assert!(
            net.connections
                .contains(&wire("exposure_2", "output", "sigmoid_2", "input"))
        );
        assert!(
            net.connections
                .contains(&wire("source", "image", "exposure_2", "input"))
        );
        assert_eq!(positions["exposure_2"], [SHIFT, SHIFT]);
        // The original is untouched.
        assert!(
            net.connections
                .contains(&wire("exposure", "output", "sigmoid", "input"))
        );
        // Pasted again: the next free names.
        let out = paste(&mut net, &mut positions, &clip, None).unwrap();
        assert_eq!(out.added, ["exposure_3", "sigmoid_3"]);
        assert!(
            out.describe().contains("exposure as exposure_3"),
            "{}",
            out.describe()
        );
    }

    #[test]
    fn into_another_network_names_are_kept_and_parts_choose() {
        let (net, positions) = net();
        let clip = take(&net, &positions, &["sigmoid".into(), "blur".into()], None).unwrap();
        // A network without them nor the node their input came from.
        let mut other = NetworkDesc {
            sources: vec!["source".into()],
            ..NetworkDesc::default()
        };
        let mut places = BTreeMap::new();
        let out = paste(&mut other, &mut places, &clip, None).unwrap();
        assert_eq!(out.added, ["sigmoid", "blur"]);
        assert!(out.renamed.is_empty());
        // `exposure` is not there: its wire into `sigmoid` stays cut.
        assert_eq!(out.reconnected, 0);
        assert_eq!(
            other.connections,
            [wire("sigmoid", "output", "blur", "input")]
        );
        // Paste special: only `blur`, no inputs.
        let mut third = NetworkDesc::default();
        let parts = vec!["blur".to_string()];
        let out = paste(&mut third, &mut places, &clip, Some(&parts)).unwrap();
        assert_eq!(out.added, ["blur"]);
        assert!(third.connections.is_empty());
        let listed: Vec<String> = super::parts(&clip.to_json())
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(listed, ["sigmoid", "blur", INPUTS]);
    }
}
