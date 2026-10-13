//! The Masks panel: the selected module's mask network as a tree
//! (egui-pop's `TreeView`), kept live and edited in place.
//!
//! - **What it shows.** The network is read from the graph and the value
//!   store every frame ([`network`]): the module's mask (the group a
//!   `mask_blend` reads) is the root, each `mask_combine` a branch with
//!   its operator, each member a leaf (a drawn shape, a parametric,
//!   luminance, raster or AI object member) with its icon, its refinements (blur,
//!   feather, levels between it and its group), its inversion and its
//!   opacity. Members are listed in port order: the order they fold in.
//! - **Live.** What changed since the last frame reaches the view through
//!   its `TreeHandle` (a branch's children set again, a label updated),
//!   whoever changed it: the viewport, the Parameters panel, an agent, an
//!   undo. The selection follows the viewport's selected shape and
//!   `view/graph/selection`, and selecting in the tree sets them.
//! - **Edits** are commands (`mask.member.*`, `mask.add.*`): a rename
//!   (F2) is a label in the store (`label/<node>`), a lock a flag there
//!   (`lock/<node>`), an inversion or an opacity the group's values, each
//!   one undo step. A row's eye and lock, and its context menu, act on
//!   that row's member whatever is selected. Moving a member
//!   (drag and drop) or deleting one rewires the network, which the store
//!   does not hold: not an undo step, and a drop the network cannot take
//!   ([`drop_place`]) is refused while dragging and vetoed before the
//!   view moves anything (`TreeView::can_drop`).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::sync::{Mutex, PoisonError};

use egui::Ui;
use egui_pop::api::AppHandle;
use egui_pop::core::icon::{Icon, Source, Tone};
use egui_pop::model::ids::MenuId;
use egui_pop::views::tree::{Node, Position, Signal, Sink, TreeSource, TreeState, TreeView};
use helios_core::{Graph, Message, NodeId, Value};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, Set, Store, node_path};
use serde_json::Value as Json;

use crate::dialogs::{Level, MenuItem, Request, request};
use crate::masks::{self, Command};

/// The panel's id.
pub const PANEL: &str = "helios.masks";
/// The tree's id in egui-pop's steps (`tree.drop`...).
pub const TREE: &str = "helios.masks.tree";

/// A row's context menu: commands on the member right-clicked, which get
/// `{"tree": TREE, "key": <its node>}` (see [`row_member`]).
pub const ROW_MENU: MenuId = MenuId::from_static("helios.masks.row");
/// Its Add submenu.
pub const ROW_MENU_ADD: MenuId = MenuId::from_static("helios.masks.row.add");
/// The list filling a row's menu: its commands under short labels
/// ([`row_entries`]), asked for each time the menu opens.
pub const ROW_LIST: &str = "helios.masks.row.member";
/// The list filling its Add submenu ([`add_entries`]).
pub const ROW_ADD_LIST: &str = "helios.masks.row.add";
/// The commands of a row's menu, in order.
pub const ROW_COMMANDS: [&str; 5] = [
    "mask.member.rename",
    "mask.member.invert",
    "mask.member.opacity",
    "mask.member.lock",
    "mask.member.delete",
];
/// What Add adds: a shape or a parametric member.
pub const ADD_KINDS: [&str; 5] = ["circle", "ellipse", "gradient", "path", "parametric"];

/// A row's menu: each of [`ROW_COMMANDS`] with its short label in the
/// language of `ctx`. The palette keeps the commands' titles, which say
/// their arguments; the menu acts on its row, so a verb is enough.
pub fn row_entries(ctx: &egui::Context) -> Vec<(String, &'static str)> {
    let labels = [
        Message::new("gui-masks-menu-rename", "Rename…"),
        Message::new("gui-masks-menu-invert", "Invert"),
        Message::new("gui-masks-menu-opacity", "Opacity off or full"),
        Message::new("gui-masks-menu-lock", "Lock or unlock"),
        Message::new("gui-masks-menu-delete", "Take out of the mask"),
    ];
    labels
        .into_iter()
        .map(|m| crate::i18n::text(ctx, m))
        .zip(ROW_COMMANDS)
        .collect()
}

/// The Add submenu: each of [`ADD_KINDS`] with its short label in the
/// language of `ctx`, and its command (`mask.add.<kind>`).
pub fn add_entries(ctx: &egui::Context) -> Vec<(String, String)> {
    let labels = [
        Message::new("gui-masks-menu-add-circle", "Circle"),
        Message::new("gui-masks-menu-add-ellipse", "Ellipse"),
        Message::new("gui-masks-menu-add-gradient", "Gradient"),
        Message::new("gui-masks-menu-add-path", "Path"),
        Message::new("gui-masks-menu-add-parametric", "Parametric"),
    ];
    labels
        .into_iter()
        .map(|m| crate::i18n::text(ctx, m))
        .zip(ADD_KINDS.map(|k| format!("mask.add.{k}")))
        .collect()
}

/// The member a command's arguments name: `member`, else the row its
/// menu was opened on (`key`, with `tree` this tree).
pub fn row_member(args: &Json) -> Option<String> {
    let text = |k: &str| args.get(k).and_then(Json::as_str).map(String::from);
    text("member").or_else(|| {
        (args.get("tree").and_then(Json::as_str) == Some(TREE))
            .then(|| text("key"))
            .flatten()
    })
}

/// The refinement modules: between a member and its group's port.
const REFINEMENTS: [&str; 3] = ["mask_blur", "mask_feather", "mask_levels"];

/// Where a node's label lives in the store (a text, empty: its name).
pub fn label_path(node: &str) -> String {
    format!("label/{node}")
}

/// A node's label, if it was given one.
pub fn label_of(store: &Store, node: &str) -> Option<String> {
    match store.get(&label_path(node)).map(|e| &e.value) {
        Some(Value::Text(t)) if !t.trim().is_empty() => Some(t.clone()),
        _ => None,
    }
}

/// Where a node's lock lives in the store (true: locked).
pub fn lock_path(node: &str) -> String {
    format!("lock/{node}")
}

/// Whether a node is locked: a member that does not move, is not taken
/// out and whose shape is not edited; a group whose members stay.
pub fn is_locked(store: &Store, node: &str) -> bool {
    store.get(&lock_path(node)).and_then(|e| e.value.as_bool()) == Some(true)
}

/// One node of the tree: a group or a member.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The node: a `mask_combine` for a group, the member's node otherwise.
    pub node: String,
    pub module: String,
    /// The group it belongs to (`None`: the module's mask, the root).
    pub group: Option<String>,
    /// The group's port it is wired to.
    pub port: usize,
    /// The node wired to that port: the member, or its last refinement.
    pub head: String,
    /// The refinements between the member and the port, member first.
    pub refinements: Vec<String>,
    /// A group's operator.
    pub operator: Option<String>,
    /// Inverted before its opacity: by its group, or the root's own.
    pub invert: bool,
    pub opacity: f64,
    pub label: Option<String>,
    pub locked: bool,
}

impl Item {
    pub fn is_group(&self) -> bool {
        self.module == "mask_combine"
    }

    /// A drawn shape, which the viewport edits.
    pub fn is_shape(&self) -> bool {
        masks::KINDS.iter().any(|(_, m)| *m == self.module)
    }

    /// What the tree calls it: its label, or its node's name.
    pub fn name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.node)
    }

    /// Its icon: a glyph egui's own fonts have.
    pub fn icon(&self) -> &'static str {
        match self.module.as_str() {
            "mask_combine" => "▣",
            "mask_circle" => "○",
            "mask_ellipse" => "◎",
            "mask_gradient" => "◑",
            "mask_path" => "⬟",
            "mask_parametric" => "☰",
            "mask_from_luma" => "☆",
            "mask_raster" => "🖼",
            // An AI object member: a model found it.
            "mask_object" => "✨",
            _ => "⛶",
        }
    }

    /// The row's label: the name, a group's operator, then what changes
    /// the member (refinements, inversion, opacity). The icon is drawn
    /// apart ([`Item::icon`]).
    pub fn text(&self) -> String {
        let mut s = self.name().to_string();
        if let Some(op) = &self.operator {
            s.push_str(" — ");
            s.push_str(op);
        }
        for r in &self.refinements {
            s.push_str(" · ");
            s.push_str(r.strip_prefix("mask_").unwrap_or(r));
        }
        if self.invert {
            s.push_str(" · inverted");
        }
        if self.opacity < 1.0 {
            let _ = write!(s, " · {:.0} %", self.opacity * 100.0);
        }
        s
    }

    /// Its node in the view: the decorated label, the plain name a
    /// rename starts from.
    fn tree_node(&self) -> Node<String> {
        let node = if self.is_group() {
            Node::branch(self.node.clone(), self.text())
        } else {
            Node::leaf(self.node.clone(), self.text())
        };
        node.with_name(self.name())
    }
}

/// A module's mask network as the tree shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Network {
    /// The module's mask: the group the blend reads.
    pub root: Option<String>,
    /// The blend applying it (its `input` is what parametric members read).
    pub blend: Option<String>,
    pub items: BTreeMap<String, Item>,
    /// Each group's members, in port order.
    pub children: BTreeMap<String, Vec<String>>,
}

impl Network {
    pub fn item(&self, key: &str) -> Result<&Item, String> {
        self.items
            .get(key)
            .ok_or_else(|| format!("`{key}` is not in the mask"))
    }

    /// The nodes of a branch (`None`: the root), as the view takes them.
    pub fn nodes_of(&self, parent: Option<&String>) -> Vec<Node<String>> {
        let keys: Vec<&String> = match parent {
            None => self.root.iter().collect(),
            Some(g) => self
                .children
                .get(g)
                .map(|c| c.iter().collect())
                .unwrap_or_default(),
        };
        keys.into_iter()
            .filter_map(|k| self.items.get(k))
            .map(Item::tree_node)
            .collect()
    }

    /// Whether `key` is `ancestor` or under it.
    pub fn is_within(&self, key: &str, ancestor: &str) -> bool {
        let mut at = Some(key.to_string());
        while let Some(k) = at {
            if k == ancestor {
                return true;
            }
            at = self.items.get(&k).and_then(|i| i.group.clone());
        }
        false
    }

    /// The groups above a node, nearest first.
    pub fn ancestors(&self, key: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut at = self.items.get(key).and_then(|i| i.group.clone());
        while let Some(g) = at {
            at = self.items.get(&g).and_then(|i| i.group.clone());
            out.push(g);
        }
        out
    }
}

/// The view asks for a branch: answered at once from the network read
/// this frame.
impl TreeSource<String> for Network {
    fn load(&self, parent: Option<&String>, sink: Sink<String>) {
        sink.deliver(self.nodes_of(parent));
    }
}

fn module_of(graph: &Graph, id: NodeId) -> Option<String> {
    graph.manifest(id).ok().flatten().map(|m| m.name)
}

fn name_of(graph: &Graph, id: NodeId) -> String {
    graph.name(id).unwrap_or_default().to_string()
}

/// The node feeding an input port of a node, by the port's name.
fn input_of(graph: &Graph, id: NodeId, port: &str) -> Option<(NodeId, String)> {
    let m = graph.manifest(id).ok().flatten()?;
    let i = m.inputs.iter().position(|p| p.name == port)?;
    graph.input_ports(id).ok()?.get(i).cloned().flatten()
}

/// Each node's consumers, with the input port's name.
fn consumers(graph: &Graph) -> BTreeMap<NodeId, Vec<(NodeId, String)>> {
    let mut out: BTreeMap<NodeId, Vec<(NodeId, String)>> = BTreeMap::new();
    for id in graph.ids() {
        let (Ok(inputs), Ok(Some(m))) = (graph.input_nodes(id), graph.manifest(id)) else {
            continue;
        };
        for (i, src) in inputs.into_iter().enumerate() {
            if let Some(src) = src {
                out.entry(src)
                    .or_default()
                    .push((id, m.inputs[i].name.clone()));
            }
        }
    }
    out
}

/// A branch's member: the node behind its refinements, and them, member
/// first.
fn subject(graph: &Graph, head: NodeId) -> (NodeId, Vec<String>) {
    let mut at = head;
    let mut refinements = Vec::new();
    while let Some(m) = module_of(graph, at).filter(|m| REFINEMENTS.contains(&m.as_str())) {
        match input_of(graph, at, "mask") {
            Some((src, _)) => {
                refinements.push(m);
                at = src;
            }
            None => break,
        }
    }
    refinements.reverse();
    (at, refinements)
}

/// The mask network a node belongs to, or the one of the module it
/// masks: its root group and its blend.
fn root_of(
    graph: &Graph,
    consumers: &BTreeMap<NodeId, Vec<(NodeId, String)>>,
    node: NodeId,
) -> Option<(NodeId, NodeId)> {
    let blend_root = |blend: NodeId| {
        let (head, _) = input_of(graph, blend, "mask")?;
        let (root, _) = subject(graph, head);
        (module_of(graph, root).as_deref() == Some("mask_combine")).then_some((root, blend))
    };
    if module_of(graph, node).as_deref() == Some("mask_blend") {
        return blend_root(node);
    }
    // The module a blend applies: its mask.
    for (c, port) in consumers.get(&node).into_iter().flatten() {
        if port == "processed" && module_of(graph, *c).as_deref() == Some("mask_blend") {
            return blend_root(*c);
        }
    }
    // A node of a mask: down to the blend reading it.
    let mut seen = BTreeSet::new();
    let mut todo = VecDeque::from([node]);
    while let Some(at) = todo.pop_front() {
        if !seen.insert(at) {
            continue;
        }
        for (c, port) in consumers.get(&at).into_iter().flatten() {
            match module_of(graph, *c).as_deref() {
                Some("mask_blend") if port == "mask" => return blend_root(*c),
                Some("mask_combine") => todo.push_back(*c),
                Some(m) if REFINEMENTS.contains(&m) && port == "mask" => todo.push_back(*c),
                _ => {}
            }
        }
    }
    None
}

fn text(store: &Store, path: &str) -> String {
    match store.get(path).map(|e| &e.value) {
        Some(Value::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

/// The mask network to show: the one of `prefer` (a node's name), else of
/// the selected node (`view/graph/selection`), the selected shape, the
/// mask the viewport edits, else the graph's first masked module's.
pub fn network(graph: &Graph, store: &Store, prefer: Option<&str>) -> Network {
    let consumers = consumers(graph);
    let candidates = [
        prefer.map(String::from),
        Some(text(store, crate::params::SELECTION)),
        Some(text(store, masks::SHAPE)),
        Some(text(store, masks::MASK)),
    ];
    let found = candidates
        .into_iter()
        .flatten()
        .filter(|n| !n.is_empty())
        .filter_map(|n| graph.find(&n))
        .chain(
            graph
                .ids()
                .filter(|id| module_of(graph, *id).as_deref() == Some("mask_blend")),
        )
        .find_map(|id| root_of(graph, &consumers, id));
    let mut net = Network::default();
    let root = match found {
        Some((root, blend)) => {
            net.blend = Some(name_of(graph, blend));
            root
        }
        // A mask no blend reads yet: the one the viewport edits.
        None => match masks::current_mask(graph, store).and_then(|m| graph.find(&m)) {
            Some(r) => r,
            None => return net,
        },
    };
    let params = masks::params_of(graph, store, root);
    let root_name = name_of(graph, root);
    net.items.insert(
        root_name.clone(),
        Item {
            node: root_name.clone(),
            module: "mask_combine".into(),
            group: None,
            port: 0,
            head: root_name.clone(),
            refinements: Vec::new(),
            operator: params
                .get("operator")
                .and_then(Value::as_choice)
                .map(String::from),
            invert: params.get("group_invert").and_then(Value::as_bool) == Some(true),
            opacity: params
                .get("group_opacity")
                .and_then(Value::as_f64)
                .unwrap_or(1.0),
            label: label_of(store, &root_name),
            locked: is_locked(store, &root_name),
        },
    );
    net.root = Some(root_name);
    let mut seen = BTreeSet::from([root]);
    walk(graph, store, root, &mut net, &mut seen);
    net
}

/// Adds a group's members to the network, and theirs.
fn walk(
    graph: &Graph,
    store: &Store,
    group: NodeId,
    net: &mut Network,
    seen: &mut BTreeSet<NodeId>,
) {
    let Ok(inputs) = graph.input_nodes(group) else {
        return;
    };
    let params = masks::params_of(graph, store, group);
    let opacity = params
        .get("opacity")
        .and_then(Value::as_list)
        .unwrap_or(&[])
        .to_vec();
    let invert: Vec<i64> = match params.get("invert") {
        Some(Value::Ints(v)) => v.clone(),
        _ => Vec::new(),
    };
    let group_name = name_of(graph, group);
    let mut members = Vec::new();
    for (port, head) in inputs.into_iter().enumerate() {
        let Some(head) = head else { continue };
        let (node, refinements) = subject(graph, head);
        // A node read twice shows once.
        if !seen.insert(node) {
            continue;
        }
        let module = module_of(graph, node).unwrap_or_default();
        let name = name_of(graph, node);
        let operator = (module == "mask_combine")
            .then(|| masks::params_of(graph, store, node))
            .and_then(|p| {
                p.get("operator")
                    .and_then(Value::as_choice)
                    .map(String::from)
            });
        net.items.insert(
            name.clone(),
            Item {
                node: name.clone(),
                module: module.clone(),
                group: Some(group_name.clone()),
                port,
                head: name_of(graph, head),
                refinements,
                operator,
                invert: invert.get(port).copied().unwrap_or(0) != 0,
                opacity: opacity.get(port).copied().unwrap_or(1.0),
                label: label_of(store, &name),
                locked: is_locked(store, &name),
            },
        );
        members.push(name);
        if module == "mask_combine" {
            walk(graph, store, node, net, seen);
        }
    }
    net.children.insert(group_name, members);
}

/// What a parametric member reads: the image the masked module reads
/// (the blend's `input`).
pub fn blend_input(graph: &Graph, net: &Network) -> Option<(NodeId, String)> {
    let blend = graph.find(net.blend.as_deref()?)?;
    input_of(graph, blend, "input")
}

/// The member a command acts on: the one named, else the selected shape,
/// else the selected node when it is in the mask.
pub fn member_or_selected(
    net: &Network,
    store: &Store,
    member: Option<&str>,
) -> Result<String, String> {
    if let Some(m) = member {
        net.item(m)?;
        return Ok(m.to_string());
    }
    selected(net, store).ok_or_else(|| "no member of the mask is selected".into())
}

/// What the tree shows selected: the selected shape, else the selected
/// node, when the network has it.
pub fn selected(net: &Network, store: &Store) -> Option<String> {
    [
        text(store, masks::SHAPE),
        text(store, crate::params::SELECTION),
    ]
    .into_iter()
    .find(|k| !k.is_empty() && net.items.contains_key(k))
}

fn put(store: &mut Store, path: &str, value: Value) {
    let _ = store.set(Set::new(path, Change::To(value), Origin::User));
}

/// Selects a node of the tree: a group becomes the mask the viewport
/// edits, a member's group too and a shape the viewport's selected
/// shape; the Parameters panel shows the node.
pub fn select(store: &mut Store, net: &Network, key: &str) {
    let Some(item) = net.items.get(key) else {
        return;
    };
    let group = if item.is_group() {
        Some(key)
    } else {
        item.group.as_deref()
    };
    if let Some(g) = group {
        put(store, masks::MASK, Value::Text(g.into()));
    }
    if item.is_shape() {
        masks::select(store, key);
    } else {
        put(store, masks::SHAPE, Value::Text(String::new()));
        put(store, masks::POINT, Value::Int(-1));
        put(store, crate::params::SELECTION, Value::Text(key.into()));
    }
}

/// Where a drop puts a member: its group and its index there, or why the
/// network cannot take it, by key.
pub fn drop_place(
    net: &Network,
    keys: &[String],
    target: &str,
    position: Position,
) -> Result<(String, usize), Message> {
    let [key] = keys else {
        return Err(Message::new(
            "gui-masks-one-at-a-time",
            "members move one at a time",
        ));
    };
    let item = net.item(key)?;
    if item.group.is_none() {
        return Err(Message::new(
            "gui-masks-root-stays",
            "the module's mask stays the root",
        ));
    }
    if item.locked {
        return Err(
            Message::new("gui-masks-locked", "`{ $name }` is locked").with("name", item.name())
        );
    }
    let t = net.item(target)?;
    let group = match position {
        Position::Inside if t.is_group() => target.to_string(),
        Position::Inside => {
            return Err(Message::new(
                "gui-masks-not-a-group",
                "`{ $name }` is not a group: only a group takes members",
            )
            .with("name", t.name()));
        }
        _ => t.group.clone().ok_or_else(|| {
            Message::new(
                "gui-masks-nothing-beside-root",
                "nothing goes beside the module's mask: drop inside it",
            )
        })?,
    };
    if net.is_within(&group, key) {
        return Err(Message::new(
            "gui-masks-into-itself",
            "a group cannot go into itself",
        ));
    }
    if let Some(g) = [item.group.as_deref(), Some(group.as_str())]
        .into_iter()
        .flatten()
        .filter_map(|g| net.items.get(g))
        .find(|g| g.locked)
    {
        return Err(Message::new(
            "gui-masks-group-locked",
            "the group `{ $group }` is locked: its members stay",
        )
        .with("group", g.name()));
    }
    let others: Vec<&String> = net
        .children
        .get(&group)
        .map(|c| c.iter().filter(|k| *k != key).collect())
        .unwrap_or_default();
    if item.group.as_deref() != Some(group.as_str())
        && others.len() >= helios_module_masks::MAX_MEMBERS
    {
        return Err(Message::new(
            "gui-masks-group-full",
            "the group `{ $group }` holds { $count } members already",
        )
        .with("group", &group)
        .with("count", helios_module_masks::MAX_MEMBERS));
    }
    let index = match position {
        Position::Inside => others.len(),
        Position::Before | Position::After => {
            let at = others
                .iter()
                .position(|k| *k == target)
                .unwrap_or(others.len());
            if position == Position::After {
                at + 1
            } else {
                at
            }
        }
    };
    Ok((group, index))
}

/// A member wired to a group's port, with its opacity and inversion.
#[derive(Clone, Debug, PartialEq)]
struct Slot {
    head: NodeId,
    out: String,
    opacity: f64,
    invert: i64,
}

/// A group's connected ports, in order, as slots (with each port's index).
fn slots(graph: &Graph, group: NodeId) -> Result<Vec<(usize, Slot)>, String> {
    let e = |e: helios_core::Error| e.to_string();
    let params = graph.params(group).map_err(e)?.unwrap_or_default();
    let opacity = params
        .get("opacity")
        .and_then(Value::as_list)
        .unwrap_or(&[])
        .to_vec();
    let invert: Vec<i64> = match params.get("invert") {
        Some(Value::Ints(v)) => v.clone(),
        _ => Vec::new(),
    };
    Ok(graph
        .input_ports(group)
        .map_err(e)?
        .into_iter()
        .enumerate()
        .filter_map(|(i, p)| {
            p.map(|(head, out)| {
                (
                    i,
                    Slot {
                        head,
                        out,
                        opacity: opacity.get(i).copied().unwrap_or(1.0),
                        invert: invert.get(i).copied().unwrap_or(0),
                    },
                )
            })
        })
        .collect())
}

/// Wires a group's members in this order from its first port, their
/// opacities and inversions with them; the other ports are left empty.
fn set_slots(graph: &mut Graph, group: NodeId, slots: &[Slot]) -> Result<(), String> {
    let e = |e: helios_core::Error| e.to_string();
    let manifest = graph
        .manifest(group)
        .map_err(e)?
        .ok_or("a group is a module")?;
    let ports: Vec<String> = manifest.inputs.iter().map(|p| p.name.clone()).collect();
    if slots.len() > ports.len() {
        return Err(format!("a group holds {} members at most", ports.len()));
    }
    for p in &ports {
        graph.disconnect(group, p).map_err(e)?;
    }
    let mut opacity = vec![1.0; ports.len()];
    let mut invert = vec![0; ports.len()];
    for (i, s) in slots.iter().enumerate() {
        graph.connect(s.head, &s.out, group, &ports[i]).map_err(e)?;
        opacity[i] = s.opacity;
        invert[i] = s.invert;
    }
    graph
        .set_param(group, "opacity", Value::List(opacity))
        .map_err(e)?;
    graph
        .set_param(group, "invert", Value::Ints(invert))
        .map_err(e)?;
    Ok(())
}

fn find(graph: &Graph, name: &str) -> Result<NodeId, String> {
    graph.find(name).ok_or_else(|| format!("no node `{name}`"))
}

/// Moves a member to `index` of `group` (its own or another), rewiring
/// the network; returns the groups rewired. Its opacity and inversion go
/// with it.
pub fn move_member(
    graph: &mut Graph,
    net: &Network,
    member: &str,
    group: &str,
    index: usize,
) -> Result<Vec<NodeId>, String> {
    let item = net.item(member)?.clone();
    let from_name = item
        .group
        .clone()
        .ok_or("the module's mask stays the root")?;
    unlocked(net, member, Some(group))?;
    let to_item = net.item(group)?;
    if !to_item.is_group() {
        return Err(format!("`{}` is not a group", to_item.name()));
    }
    if net.is_within(group, member) {
        return Err("a group cannot go into itself".into());
    }
    let (from, to) = (find(graph, &from_name)?, find(graph, group)?);
    let mut from_slots = slots(graph, from)?;
    let at = from_slots
        .iter()
        .position(|(port, _)| *port == item.port)
        .ok_or_else(|| format!("`{member}` is not wired to `{from_name}`"))?;
    let (_, moved) = from_slots.remove(at);
    let mut rest: Vec<Slot> = from_slots.into_iter().map(|(_, s)| s).collect();
    if from == to {
        rest.insert(index.min(rest.len()), moved);
        set_slots(graph, to, &rest)?;
        return Ok(vec![to]);
    }
    let mut into: Vec<Slot> = slots(graph, to)?.into_iter().map(|(_, s)| s).collect();
    if into.len() >= helios_module_masks::MAX_MEMBERS {
        return Err(format!(
            "the group `{group}` holds {} members already",
            helios_module_masks::MAX_MEMBERS
        ));
    }
    into.insert(index.min(into.len()), moved);
    set_slots(graph, from, &rest)?;
    set_slots(graph, to, &into)?;
    Ok(vec![from, to])
}

/// Refuses to move a member (to `group`, else out of the mask) when it,
/// its group or `group` is locked.
fn unlocked(net: &Network, member: &str, group: Option<&str>) -> Result<(), String> {
    let item = net.item(member)?;
    if item.locked {
        return Err(format!("`{}` is locked", item.name()));
    }
    for g in [item.group.as_deref(), group].into_iter().flatten() {
        if net.items.get(g).is_some_and(|g| g.locked) {
            return Err(format!("the group `{g}` is locked: its members stay"));
        }
    }
    Ok(())
}

/// Takes a member out of its group (a group with what is under it); the
/// node stays in the network, unconnected: the graph cannot remove one.
/// Returns the group rewired.
pub fn remove_member(graph: &mut Graph, net: &Network, member: &str) -> Result<NodeId, String> {
    let item = net.item(member)?.clone();
    let from_name = item
        .group
        .clone()
        .ok_or("the module's mask cannot be deleted from its tree")?;
    unlocked(net, member, None)?;
    let from = find(graph, &from_name)?;
    let rest: Vec<Slot> = slots(graph, from)?
        .into_iter()
        .filter(|(port, _)| *port != item.port)
        .map(|(_, s)| s)
        .collect();
    set_slots(graph, from, &rest)?;
    Ok(from)
}

/// Writes values as one undo step.
fn write_step(
    store: &mut Store,
    origin: &Origin,
    label: &str,
    writes: Vec<(String, Value)>,
) -> Result<(), String> {
    let g = store.begin_group(origin.clone(), label);
    let mut out = Ok(());
    for (path, value) in writes {
        if let Err(e) = store.set(Set::new(&path, Change::To(value), origin.clone()).gesture(g)) {
            out = Err(e.to_string());
            break;
        }
    }
    store.end(g);
    out
}

/// Inverts a member, or back (the root: its own inversion); one undo
/// step. Returns whether it is inverted now.
pub fn toggle_invert(
    store: &mut Store,
    net: &Network,
    member: &str,
    origin: &Origin,
) -> Result<bool, String> {
    let item = net.item(member)?;
    let on = !item.invert;
    let label = format!("invert {}", item.name());
    let write = match &item.group {
        None => (node_path(&item.node, "group_invert"), Value::Bool(on)),
        Some(g) => {
            let path = node_path(g, "invert");
            let mut v = match store.get(&path).map(|e| &e.value) {
                Some(Value::Ints(v)) => v.clone(),
                _ => vec![0; helios_module_masks::MAX_MEMBERS],
            };
            v.resize(v.len().max(item.port + 1), 0);
            v[item.port] = i64::from(on);
            (path, Value::Ints(v))
        }
    };
    write_step(store, origin, &label, vec![write])?;
    Ok(on)
}

/// Sets a member's opacity (the root: its own), or toggles it between
/// none and full; one undo step. Returns the opacity now.
pub fn set_opacity(
    store: &mut Store,
    net: &Network,
    member: &str,
    opacity: Option<f64>,
    origin: &Origin,
) -> Result<f64, String> {
    let item = net.item(member)?;
    let to = opacity
        .unwrap_or(if item.opacity > 0.0 { 0.0 } else { 1.0 })
        .clamp(0.0, 1.0);
    let label = format!("opacity of {}", item.name());
    let write = match &item.group {
        None => (node_path(&item.node, "group_opacity"), Value::Float(to)),
        Some(g) => {
            let path = node_path(g, "opacity");
            let mut v = store
                .get(&path)
                .and_then(|e| e.value.as_list())
                .map_or_else(
                    || vec![1.0; helios_module_masks::MAX_MEMBERS],
                    <[f64]>::to_vec,
                );
            v.resize(v.len().max(item.port + 1), 1.0);
            v[item.port] = to;
            (path, Value::List(v))
        }
    };
    write_step(store, origin, &label, vec![write])?;
    Ok(to)
}

/// Names a node of the mask (empty: its node's name again); one undo
/// step.
pub fn rename(
    store: &mut Store,
    net: &Network,
    member: &str,
    label: &str,
    origin: &Origin,
) -> Result<(), String> {
    let item = net.item(member)?;
    let path = label_path(&item.node);
    if store.get(&path).is_none() {
        store.declare(
            &path,
            Descriptor::text(&format!("label of {}", item.node), ""),
        );
    }
    let label = label.trim();
    write_step(
        store,
        origin,
        &format!("rename {} to {label}", item.name()),
        vec![(path, Value::Text(label.into()))],
    )
}

/// Locks a node of the mask or unlocks it (`locked`; `None`: the other
/// way round); one undo step. Returns whether it is locked now.
pub fn set_locked(
    store: &mut Store,
    net: &Network,
    member: &str,
    locked: Option<bool>,
    origin: &Origin,
) -> Result<bool, String> {
    let item = net.item(member)?;
    let path = lock_path(&item.node);
    if store.get(&path).is_none() {
        store.declare(
            &path,
            Descriptor::bool(&format!("{} locked", item.node), false),
        );
    }
    let on = locked.unwrap_or(!item.locked);
    let label = format!("{} {}", if on { "lock" } else { "unlock" }, item.name());
    write_step(store, origin, &label, vec![(path, Value::Bool(on))])?;
    Ok(on)
}

/// Whether the view's branch shows these nodes, in this order.
fn shows(state: &TreeState<String>, parent: Option<&String>, nodes: &[Node<String>]) -> bool {
    let model = state.model();
    model.loaded(parent).is_some_and(|keys| {
        keys.len() == nodes.len()
            && keys
                .iter()
                .zip(nodes)
                .all(|(k, n)| model.node(k) == Some(n))
    })
}

/// The Masks panel's state between frames: the view's, and the network
/// it showed last.
pub struct MaskTree {
    state: TreeState<String>,
    shown: Network,
    /// Groups seen so far: a group opens the first time it shows.
    known: BTreeSet<String>,
    /// The selection the store asked for last.
    desired: Option<String>,
    /// A rename asked for by a command, started at the next pass.
    rename: Option<String>,
    app: Option<AppHandle>,
}

impl Default for MaskTree {
    fn default() -> Self {
        Self::new()
    }
}

impl MaskTree {
    pub fn new() -> Self {
        Self {
            state: TreeState::new(),
            shown: Network::default(),
            known: BTreeSet::new(),
            desired: None,
            rename: None,
            app: None,
        }
    }

    /// Starts renaming a node (the selected one by default) in place, at
    /// the next pass.
    pub fn start_rename(&mut self, key: Option<String>) -> Result<(), String> {
        let key = key
            .or_else(|| self.state.selection().cursor().cloned())
            .ok_or("nothing is selected in the Masks panel")?;
        self.rename = Some(key);
        Ok(())
    }

    /// Runs the tree's moments as steps of this application.
    pub fn set_app(&mut self, app: AppHandle) {
        self.app = Some(app);
    }

    /// The view's state: its rows, its selection.
    pub fn state(&self) -> &TreeState<String> {
        &self.state
    }

    /// The network shown last.
    pub fn network(&self) -> Network {
        self.shown.clone()
    }

    /// Brings the view up to the network: branches whose members or
    /// labels changed are set again through the view's handle.
    fn follow(&mut self, net: &Network) {
        let handle = self.state.handle();
        let roots = net.nodes_of(None);
        if self.state.model().children(None).is_some() && !shows(&self.state, None, &roots) {
            handle.set_children(None, Ok(roots));
        }
        for group in net.children.keys() {
            let parent = Some(group.clone());
            let nodes = net.nodes_of(parent.as_ref());
            if self.state.model().loaded(parent.as_ref()).is_some()
                && !shows(&self.state, parent.as_ref(), &nodes)
            {
                handle.set_children(parent, Ok(nodes));
            }
            if self.known.insert(group.clone()) {
                self.state.set_expanded(group, true);
            }
        }
    }

    /// The store's selection, shown in the view when it changed.
    fn follow_selection(&mut self, net: &Network, store: &Store) {
        let desired = selected(net, store);
        if desired == self.desired {
            return;
        }
        match &desired {
            Some(k) if self.state.selection().cursor() != Some(k) => {
                // A node the view has not loaded yet waits for its
                // branch, which opening its groups loads.
                for g in net.ancestors(k) {
                    self.state.set_expanded(&g, true);
                }
                self.state.set_selection([k.clone()], Some(k.clone()));
            }
            None if !self.state.selection().is_empty() => self.state.set_selection([], None),
            _ => {}
        }
        self.desired = desired;
    }

    /// Draws the panel: a row of actions on the selected member, the tree.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        viewport: &RefCell<crate::viewport::Viewport>,
        values: &Mutex<Store>,
    ) {
        let net = {
            let store = values.lock().unwrap_or_else(PoisonError::into_inner);
            let v = viewport.borrow();
            let net = v
                .graph()
                .map(|g| network(g, &store, None))
                .unwrap_or_default();
            self.follow(&net);
            self.follow_selection(&net, &store);
            net
        };
        self.shown = net.clone();
        if net.root.is_none() {
            ui.weak("No mask: the selected module is not masked");
            return;
        }
        let mut commands = Vec::new();
        let current = self
            .state
            .selection()
            .cursor()
            .and_then(|k| net.items.get(k))
            .cloned();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(current.is_some(), |ui| {
                let (inverted, visible) = current
                    .as_ref()
                    .map_or((false, true), |c| (c.invert, c.opacity > 0.0));
                if ui
                    .selectable_label(inverted, "Invert")
                    .on_hover_text(
                        "Invert the selected member (its group's inversion for the module's mask)",
                    )
                    .clicked()
                {
                    commands.push(Command::MemberInvert { member: None });
                }
                if ui
                    .selectable_label(visible, "Opacity")
                    .on_hover_text("Turn the selected member's opacity off, or back to full")
                    .clicked()
                {
                    commands.push(Command::MemberOpacity {
                        member: None,
                        opacity: None,
                    });
                }
                let deletable = current.as_ref().is_some_and(|c| c.group.is_some());
                if ui
                    .add_enabled(deletable, egui::Button::new("Delete"))
                    .on_hover_text("Take the selected member out of the mask (Delete)")
                    .clicked()
                {
                    commands.push(Command::MemberDelete { member: None });
                }
            });
            let add = ui
                .button("Add…")
                .on_hover_text("Add a shape or a parametric member to the selected group");
            if add.clicked() {
                request(
                    ui.ctx(),
                    Request::Menu {
                        at: add.rect.left_bottom(),
                        items: add_items(),
                    },
                );
            }
        });
        let id = egui::Id::new(("egui_pop_views::tree", TREE));
        let focused = ui.memory(|m| m.has_focus(id));
        // Delete in the tree takes the selected member out. Not a key
        // binding: egui-pop's keymap counts two bindings of one key as a
        // conflict even when their clauses never hold together, and the
        // image's Delete removes a path point.
        if focused
            && self.state.renaming().is_none()
            && current.as_ref().is_some_and(|c| c.group.is_some())
            && ui.input(|i| i.key_pressed(egui::Key::Delete))
        {
            commands.push(Command::MemberDelete { member: None });
        }
        // A rename asked for by a command starts from the node's name.
        if let Some(key) = self.rename.take()
            && self.state.renaming().is_none()
        {
            self.state.rename(&key);
        }
        let ctx = ui.ctx().clone();
        // What the rows' toggles ask, run with the panel's commands.
        let mut toggles = Vec::new();
        let mut view = TreeView::new(TREE)
            .label("Masks")
            .drag_drop(true)
            .rename(true)
            .icon(|n| net.items.get(&n.key).map(|i| Source::glyph(i.icon())))
            .row_widgets(|ui, n| {
                if let Some(item) = net.items.get(&n.key) {
                    row_toggles(ui, item, &mut toggles);
                }
            })
            // What the network cannot take is refused while dragging (the
            // target marked), and its drop vetoed with the reason.
            .can_drop(|keys, target, position| {
                drop_place(&net, keys, target, position)
                    .map(|_| ())
                    .map_err(|m| crate::i18n::text(&ctx, m))
            });
        if let Some(app) = &self.app {
            // A declared menu, which egui-pop's menus draw (as the
            // Parameters panel's), not one of the panel's own.
            view = TreeView::context_menu(view.app(app), app, ROW_MENU);
        }
        let out = view.show(ui, &mut self.state, &net);
        commands.extend(toggles);
        let mut store = values.lock().unwrap_or_else(PoisonError::into_inner);
        for signal in out.signals {
            match signal {
                Signal::Select {
                    cursor: Some(k), ..
                } => {
                    select(&mut store, &net, &k);
                    self.desired = Some(k);
                }
                Signal::Rename { key, to, .. } => commands.push(Command::MemberRename {
                    member: Some(key),
                    label: to,
                }),
                Signal::Drop {
                    keys,
                    target,
                    position,
                } => match drop_place(&net, &keys, &target, position) {
                    Ok((group, index)) => commands.push(Command::MemberMove {
                        member: keys.first().cloned(),
                        group: Some(group),
                        index: Some(index),
                    }),
                    Err(e) => request(ui.ctx(), Request::Notify(Level::Warning, e)),
                },
                Signal::Vetoed { reason, .. } => {
                    request(ui.ctx(), Request::Notify(Level::Warning, reason.into()));
                }
                _ => {}
            }
        }
        for c in commands {
            if let Err(e) = viewport.borrow_mut().mask(c, Origin::User, &mut store) {
                request(ui.ctx(), Request::Notify(Level::Warning, e.into()));
            }
        }
    }
}

/// A row's eye and lock, right to left: they act on that row's member,
/// not on the selection.
fn row_toggles(ui: &mut Ui, item: &Item, commands: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    let name = item.name();
    let member = Some(item.node.clone());
    let visible = item.opacity > 0.0;
    let (icon, tone, hint) = if visible {
        (
            Icon::Visible,
            Tone::Default,
            Message::new("gui-masks-hide", "Hide { $name }: its opacity off"),
        )
    } else {
        (
            Icon::Hidden,
            Tone::Muted,
            Message::new("gui-masks-show", "Show { $name }: its opacity full"),
        )
    };
    let hint = crate::i18n::text(&ctx, hint.with("name", name));
    if ui
        .add(icon.tone(tone).label(&hint).sense_click())
        .on_hover_text(&hint)
        .clicked()
    {
        commands.push(Command::MemberOpacity {
            member: member.clone(),
            opacity: None,
        });
    }
    let (icon, tone, hint) = if item.locked {
        (
            Icon::Lock,
            Tone::Warning,
            Message::new("gui-masks-unlock", "Unlock { $name }"),
        )
    } else {
        (
            Icon::Unlock,
            Tone::Muted,
            Message::new("gui-masks-lock", "Lock { $name } against moves and edits"),
        )
    };
    let hint = crate::i18n::text(&ctx, hint.with("name", name));
    if ui
        .add(icon.tone(tone).label(&hint).sense_click())
        .on_hover_text(&hint)
        .clicked()
    {
        commands.push(Command::MemberLock {
            member,
            locked: None,
        });
    }
}

fn add_items() -> Vec<MenuItem> {
    ADD_KINDS
        .into_iter()
        .map(|k| MenuItem::command(&format!("mask.add.{k}")))
        .collect()
}
