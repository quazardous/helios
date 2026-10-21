//! The Masks panel: the selected module's mask network as a tree
//! (egui-pop's `TreeView`), kept live and edited in place.
//!
//! - **What it shows.** The network is read from the graph and the value
//!   store every frame ([`network`]): the module's mask (the group a
//!   `mask_blend` reads) is the root, each `mask_combine` a branch with
//!   its operator, each member a row (a drawn shape, a parametric,
//!   luminance, raster or AI object member) with its icon, its inversion
//!   and its opacity. Members are listed in port order: the order they
//!   fold in, a member wired to two ports once per port. The refinements
//!   (blur, feather, levels) between a member and its group, or between
//!   the module's mask and its blend, are rows under what they refine,
//!   selected to edit their parameters.
//! - **Live.** What changed since the last frame reaches the view through
//!   its `TreeHandle` (a branch's children set again, a label updated),
//!   whoever changed it: the viewport, the Parameters panel, an agent, an
//!   undo. The selection follows the viewport's selected shape and
//!   `view/graph/selection`, and selecting in the tree sets them.
//! - **Edits** are commands (`mask.member.*`, `mask.add.*`): a rename
//!   (F2) is a label in the store (`label/<node>`), a lock a flag there
//!   (`lock/<node>`), an inversion or an opacity the group's values, each
//!   one undo step. Labels and locks are saved with the image's edit, as
//!   its `marks` ([`marks`], [`restore_marks`]). A row's eye and lock, and its context menu, act on
//!   that row's member whatever is selected. Moving a member
//!   (drag and drop) or deleting one rewires the network, which the store
//!   does not hold: not an undo step, and a drop the network cannot take
//!   ([`drop_place`]) is refused while dragging and vetoed before the
//!   view moves anything (`TreeView::can_drop`).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, PoisonError};

use egui::Ui;
use egui_pop::api::AppHandle;
use egui_pop::core::icon::{Icon, Source, Tone};
use egui_pop::model::ids::MenuId;
use egui_pop::views::tree::{Node, Position, Signal, Sink, TreeSource, TreeState, TreeView};
use helios_core::{Graph, Message, NodeId, Value};
use helios_module_masks::{Operator, fold};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, Set, Store, node_path};
use serde_json::Value as Json;

use crate::dialogs::{Confirm, Level, MenuItem, Request, request};
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

fn label_descriptor(node: &str) -> Descriptor {
    Descriptor::text(&format!("label of {node}"), "")
}

fn lock_descriptor(node: &str) -> Descriptor {
    Descriptor::bool(&format!("{node} locked"), false)
}

/// A node's marks in an edit's `marks`, made when missing.
fn mark<'a>(
    out: &'a mut serde_json::Map<String, Json>,
    node: &str,
) -> &'a mut serde_json::Map<String, Json> {
    match out
        .entry(node)
        .or_insert_with(|| Json::Object(serde_json::Map::new()))
    {
        Json::Object(m) => m,
        _ => unreachable!("a node's marks are an object"),
    }
}

/// The labels and locks of the nodes, as an image's edit keeps them
/// (its `marks`): `{ "<node>": { "label": "Face", "locked": true } }`,
/// with only what a node was given (a label, a lock).
pub fn marks(store: &Store) -> serde_json::Map<String, Json> {
    let mut out = serde_json::Map::new();
    for (path, e) in store.list("label/") {
        if let (Some(node), Value::Text(t)) = (path.strip_prefix("label/"), &e.value)
            && !t.trim().is_empty()
        {
            mark(&mut out, node).insert("label".into(), Json::from(t.as_str()));
        }
    }
    for (path, e) in store.list("lock/") {
        if let Some(node) = path.strip_prefix("lock/")
            && e.value.as_bool() == Some(true)
        {
            mark(&mut out, node).insert("locked".into(), Json::Bool(true));
        }
    }
    out
}

/// Gives the nodes the labels and locks of an edit's `marks` (see
/// [`marks`]; null: none), every other node unnamed and unlocked. Outside
/// the undo history: what an image opens with.
pub fn restore_marks(store: &mut Store, marks: &Json) {
    let held: Vec<String> = store
        .list("label/")
        .into_iter()
        .chain(store.list("lock/"))
        .map(|(p, _)| p.to_string())
        .collect();
    for path in held {
        if let Some(d) = store.get(&path).map(|e| e.descriptor.clone()) {
            let none = d.default.clone();
            store.declare_with(&path, d, none);
        }
    }
    for (node, m) in marks.as_object().into_iter().flatten() {
        if let Some(label) = m.get("label").and_then(Json::as_str) {
            store.declare_with(
                &label_path(node),
                label_descriptor(node),
                Value::Text(label.into()),
            );
        }
        if m.get("locked").and_then(Json::as_bool) == Some(true) {
            store.declare_with(&lock_path(node), lock_descriptor(node), Value::Bool(true));
        }
    }
}

/// One row of the tree: a group, a member or a refinement.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The row's key in the tree: the node's name, or for a member's
    /// second port and on, the name with its place (`spot@mask#3`).
    pub key: String,
    /// The node: a `mask_combine` for a group, the member's node otherwise.
    pub node: String,
    pub module: String,
    /// The group it belongs to (`None`: the module's mask, the root, or a
    /// refinement).
    pub group: Option<String>,
    /// The group's port it is wired to.
    pub port: usize,
    /// The node wired to that port: the member, or its last refinement.
    pub head: String,
    /// For a refinement, the key of the row it refines (a member, a
    /// group, the root).
    pub refines: Option<String>,
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

    /// A refinement (blur, feather, levels) of a member or a group: a row
    /// of its own, its parameters edited when it is selected, but nothing
    /// that moves, hides or inverts in a group.
    pub fn is_refinement(&self) -> bool {
        self.refines.is_some()
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
            // A brush stroke: drawn, as with a pencil.
            "mask_brush" => "✏",
            "mask_parametric" => "☰",
            "mask_from_luma" => "☆",
            "mask_raster" => "🖼",
            // An AI object member: a model found it.
            "mask_object" => "✨",
            // Refinements: a blur, a feather, levels.
            "mask_blur" => "≈",
            "mask_feather" => "〰",
            "mask_levels" => "📶",
            _ => "⛶",
        }
    }

    /// The row's label in English (see [`Item::label_with`]).
    pub fn text(&self) -> String {
        self.label_with(&|m: Message| m.english)
    }

    /// The row's label, its words said by `tr`: the name, a group's
    /// operator or a refinement's kind, then what changes the member
    /// (inversion, opacity). The icon is drawn apart ([`Item::icon`]).
    pub fn label_with(&self, tr: &dyn Fn(Message) -> String) -> String {
        let mut s = self.name().to_string();
        if let Some(op) = &self.operator {
            s.push_str(" — ");
            s.push_str(&tr(operator_effect(op)));
        }
        if let Some(kind) = refinement_kind(&self.module) {
            s.push_str(" — ");
            s.push_str(&tr(kind));
        }
        // The member's place in its group (the root: its own), in words:
        // what the group's per-port lists hold for it.
        if self.invert {
            s.push_str(" · ");
            s.push_str(&tr(inverted_words(true)));
        }
        if self.opacity < 1.0 {
            s.push_str(" · ");
            s.push_str(&tr(opacity_words(self.opacity)));
        }
        s
    }

    /// Its node in the view: the decorated label, the plain name a
    /// rename starts from; a branch when rows go under it.
    fn tree_node(&self, label: String, branch: bool) -> Node<String> {
        let node = if branch {
            Node::branch(self.key.clone(), label)
        } else {
            Node::leaf(self.key.clone(), label)
        };
        node.with_name(self.name())
    }
}

/// What a group's operator does with its members, as its row says it:
/// darktable's union, intersection, difference and exclusion, by their
/// effect rather than their formula (see `helios_module_masks::Operator`).
pub fn operator_effect(operator: &str) -> Message {
    match operator {
        "maximum" => Message::new("gui-masks-op-maximum", "union"),
        "screen" => Message::new("gui-masks-op-screen", "soft union"),
        "minimum" => Message::new("gui-masks-op-minimum", "intersection"),
        "product" => Message::new("gui-masks-op-product", "multiply"),
        "sum" => Message::new("gui-masks-op-sum", "add"),
        "difference" => Message::new("gui-masks-op-difference", "subtract from the first"),
        "exclusion" => Message::new("gui-masks-op-exclusion", "exclude"),
        other => Message::plain(other),
    }
}

/// A member's inversion in its group, in words ("inverted", or "not
/// inverted", which a row leaves unsaid).
pub fn inverted_words(invert: bool) -> Message {
    if invert {
        Message::new("gui-masks-row-inverted", "inverted")
    } else {
        Message::new("gui-masks-row-not-inverted", "not inverted")
    }
}

/// A member's opacity in its group, in words: "hidden" at none, else
/// "opacity 30 %".
pub fn opacity_words(opacity: f64) -> Message {
    if opacity <= 0.0 {
        Message::new("gui-masks-row-hidden", "hidden")
    } else {
        Message::new("gui-masks-row-opacity", "opacity { $percent } %")
            .with("percent", format!("{:.0}", opacity * 100.0))
    }
}

/// A group operator's hover and accessible label: its effect, then the
/// operator's own name.
pub fn operator_hint(ctx: &egui::Context, operator: &str) -> String {
    crate::i18n::text(
        ctx,
        Message::new(
            "gui-masks-operator-hint",
            "{ $effect }: how the members fold, in port order (the operator { $operator })",
        )
        .with("effect", crate::i18n::text(ctx, operator_effect(operator)))
        .with("operator", operator),
    )
}

/// Each mask group's members by port, by the group's node name: the name
/// the Masks panel gives each (its label, else its node's name), past its
/// refinements; `None` for a port nothing is wired to.
pub fn members_by_port(graph: &Graph, store: &Store) -> BTreeMap<String, Vec<Option<String>>> {
    graph
        .ids()
        .filter(|&id| module_of(graph, id).as_deref() == Some("mask_combine"))
        .filter_map(|id| {
            let ports = graph.input_nodes(id).ok()?;
            let names = ports
                .into_iter()
                .map(|head| {
                    let (node, _) = subject(graph, head?);
                    let name = name_of(graph, node);
                    Some(label_of(store, &name).unwrap_or(name))
                })
                .collect();
            Some((name_of(graph, id), names))
        })
        .collect()
}

/// What a refinement module does, as its row says it.
fn refinement_kind(module: &str) -> Option<Message> {
    Some(match module {
        "mask_blur" => Message::new("gui-masks-refine-blur", "blur"),
        "mask_feather" => Message::new("gui-masks-refine-feather", "feather"),
        "mask_levels" => Message::new("gui-masks-refine-levels", "levels"),
        _ => return None,
    })
}

/// A module's mask network as the tree shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Network {
    /// The module's mask: the group the blend reads.
    pub root: Option<String>,
    /// The blend applying it (its `input` is what parametric members read).
    pub blend: Option<String>,
    /// The module it masks: the node the blend's `processed` reads.
    pub masked: Option<String>,
    /// The rows, by key.
    pub items: BTreeMap<String, Item>,
    /// Each group's members, in port order.
    pub children: BTreeMap<String, Vec<String>>,
    /// The refinements under a row (a member, a group, the root), from the
    /// one nearest what they refine.
    pub refined: BTreeMap<String, Vec<String>>,
    /// The rows' labels in the interface's language, by key
    /// ([`Network::translate`]); English without them.
    pub labels: BTreeMap<String, String>,
}

impl Network {
    pub fn item(&self, key: &str) -> Result<&Item, String> {
        self.items
            .get(key)
            .ok_or_else(|| format!("`{key}` is not in the mask"))
    }

    /// The rows' labels said in the interface's language by `tr`.
    pub fn translate(&mut self, tr: &dyn Fn(Message) -> String) {
        self.labels = self
            .items
            .iter()
            .map(|(k, i)| (k.clone(), i.label_with(tr)))
            .collect();
    }

    /// The rows under a row: a group's members in port order, then the
    /// refinements of what the row is.
    pub fn under(&self, key: &str) -> Vec<&String> {
        self.children
            .get(key)
            .into_iter()
            .flatten()
            .chain(self.refined.get(key).into_iter().flatten())
            .collect()
    }

    /// The rows that have rows under them.
    pub fn branches(&self) -> BTreeSet<&String> {
        self.children.keys().chain(self.refined.keys()).collect()
    }

    /// The nodes of a branch (`None`: the root), as the view takes them.
    pub fn nodes_of(&self, parent: Option<&String>) -> Vec<Node<String>> {
        let keys: Vec<&String> = match parent {
            None => self.root.iter().collect(),
            Some(k) => self.under(k),
        };
        keys.into_iter()
            .filter_map(|k| self.items.get(k))
            .map(|i| {
                let label = self.labels.get(&i.key).cloned().unwrap_or_else(|| i.text());
                let branch = (i.is_group() && self.children.contains_key(&i.key))
                    || self.refined.contains_key(&i.key);
                i.tree_node(label, branch)
            })
            .collect()
    }

    /// The row a row is under: a refinement's, a member's group.
    pub fn parent(&self, key: &str) -> Option<String> {
        let item = self.items.get(key)?;
        item.refines.clone().or_else(|| item.group.clone())
    }

    /// Whether `key` is `ancestor` or under it.
    pub fn is_within(&self, key: &str, ancestor: &str) -> bool {
        let mut at = Some(key.to_string());
        while let Some(k) = at {
            if k == ancestor {
                return true;
            }
            at = self.parent(&k);
        }
        false
    }

    /// Whether the module's mask has no member: the group then lets the
    /// module apply everywhere (darktable's "no active mask element").
    pub fn is_empty(&self) -> bool {
        self.root
            .as_ref()
            .is_none_or(|r| self.children.get(r).is_none_or(Vec::is_empty))
    }

    /// The rows above a row, nearest first.
    pub fn ancestors(&self, key: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut at = self.parent(key);
        while let Some(g) = at {
            at = self.parent(&g);
            out.push(g);
        }
        out
    }

    /// A member of a group or the root, which a command moves, takes out,
    /// inverts, hides or locks; a refinement is none of these.
    pub fn member(&self, key: &str) -> Result<&Item, String> {
        let item = self.item(key)?;
        match &item.refines {
            None => Ok(item),
            Some(owner) => Err(format!(
                "`{}` refines `{}`: it has no place of its own in a group",
                item.name(),
                self.items.get(owner).map_or(owner.as_str(), Item::name)
            )),
        }
    }

    /// A key for a row of `node` at `place` (a group's port): the node's
    /// name, unless a row has it already.
    fn key_for(&self, node: &str, place: &str) -> String {
        if !self.items.contains_key(node) {
            return node.to_string();
        }
        let key = format!("{node}@{place}");
        let mut n = 2;
        let mut out = key.clone();
        while self.items.contains_key(&out) {
            out = format!("{key}.{n}");
            n += 1;
        }
        out
    }

    /// Adds the refinement rows of `chain` (member first) under `owner`.
    fn add_refinements(
        &mut self,
        graph: &Graph,
        store: &Store,
        owner: &str,
        chain: &[NodeId],
        place: &str,
    ) {
        for id in chain {
            let name = name_of(graph, *id);
            let key = self.key_for(&name, place);
            self.items.insert(
                key.clone(),
                Item {
                    key: key.clone(),
                    node: name.clone(),
                    module: module_of(graph, *id).unwrap_or_default(),
                    group: None,
                    port: 0,
                    head: name.clone(),
                    refines: Some(owner.to_string()),
                    operator: None,
                    invert: false,
                    opacity: 1.0,
                    label: label_of(store, &name),
                    locked: is_locked(store, &name),
                },
            );
            self.refined.entry(owner.to_string()).or_default().push(key);
        }
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
pub(crate) fn input_of(graph: &Graph, id: NodeId, port: &str) -> Option<(NodeId, String)> {
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
fn subject(graph: &Graph, head: NodeId) -> (NodeId, Vec<NodeId>) {
    let mut at = head;
    let mut refinements = Vec::new();
    while module_of(graph, at).is_some_and(|m| REFINEMENTS.contains(&m.as_str())) {
        match input_of(graph, at, "mask") {
            Some((src, _)) => {
                refinements.push(at);
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
    // The refinements between the module's mask and its blend: they change
    // the result, so they are rows under it, as a member's are under it.
    let mut root_refinements = Vec::new();
    let root = match found {
        Some((root, blend)) => {
            net.blend = Some(name_of(graph, blend));
            net.masked = input_of(graph, blend, "processed").map(|(m, _)| name_of(graph, m));
            if let Some((head, _)) = input_of(graph, blend, "mask") {
                root_refinements = subject(graph, head).1;
            }
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
            key: root_name.clone(),
            node: root_name.clone(),
            module: "mask_combine".into(),
            group: None,
            port: 0,
            head: root_name.clone(),
            refines: None,
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
    net.add_refinements(graph, store, &root_name, &root_refinements, "blend");
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
        // A node read twice shows once per port; a group's members show
        // under its first row only.
        let first = seen.insert(node);
        let module = module_of(graph, node).unwrap_or_default();
        let name = name_of(graph, node);
        let place = format!("{group_name}#{port}");
        let key = net.key_for(&name, &place);
        let operator = (module == "mask_combine")
            .then(|| masks::params_of(graph, store, node))
            .and_then(|p| {
                p.get("operator")
                    .and_then(Value::as_choice)
                    .map(String::from)
            });
        net.items.insert(
            key.clone(),
            Item {
                key: key.clone(),
                node: name.clone(),
                module: module.clone(),
                group: Some(group_name.clone()),
                port,
                head: name_of(graph, head),
                refines: None,
                operator,
                invert: invert.get(port).copied().unwrap_or(0) != 0,
                opacity: opacity.get(port).copied().unwrap_or(1.0),
                label: label_of(store, &name),
                locked: is_locked(store, &name),
            },
        );
        net.add_refinements(graph, store, &key, &refinements, &place);
        members.push(key);
        if module == "mask_combine" && first {
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

/// Selects a row of the tree: a group becomes the mask the viewport
/// edits, a member's group too and a shape the viewport's selected
/// shape; the Parameters panel shows the node (a refinement's too, the
/// mask of what it refines edited).
pub fn select(store: &mut Store, net: &Network, key: &str) {
    let Some(item) = net.items.get(key) else {
        return;
    };
    let owner = item
        .refines
        .as_deref()
        .and_then(|o| net.items.get(o))
        .unwrap_or(item);
    let group = if owner.is_group() {
        Some(owner.node.as_str())
    } else {
        owner.group.as_deref()
    };
    if let Some(g) = group {
        put(store, masks::MASK, Value::Text(g.into()));
    }
    if item.is_shape() {
        masks::select(store, &item.node);
    } else {
        put(store, masks::SHAPE, Value::Text(String::new()));
        put(store, masks::POINT, Value::Int(-1));
        put(
            store,
            crate::params::SELECTION,
            Value::Text(item.node.clone()),
        );
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
    if item.is_refinement() {
        return Err(Message::new(
            "gui-masks-refinement-stays",
            "`{ $name }` stays with what it refines",
        )
        .with("name", item.name()));
    }
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
    if t.is_refinement() && position != Position::Inside {
        return Err(Message::new(
            "gui-masks-beside-refinement",
            "nothing goes beside a refinement: drop on a member or into a group",
        ));
    }
    let group = match position {
        Position::Inside if t.is_group() => t.node.clone(),
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
    let item = net.member(member)?.clone();
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
    let item = net.member(member)?;
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
    let item = net.member(member)?.clone();
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

/// What a person is asked before taking a member out: nothing for a
/// shape or an empty group, a confirmation for a group that holds members,
/// which go with it while taking out is not an undo step. Its primary
/// action runs `mask.member.delete` again, `confirmed`.
pub fn confirm_take_out(ctx: &egui::Context, net: &Network, member: &str) -> Option<Confirm> {
    let item = net.items.get(member)?;
    let holds = net.children.get(member).is_some_and(|c| !c.is_empty());
    if !item.is_group() || item.group.is_none() || !holds {
        return None;
    }
    let tr = |m: Message| crate::i18n::text(ctx, m);
    Some(Confirm {
        title: tr(Message::new(
            "gui-masks-take-out-group",
            "Take the group { $name } out of the mask?",
        )
        .with("name", item.name())),
        detail: tr(Message::new(
            "gui-masks-take-out-group-detail",
            "Its members go with it, and taking out is not an undo step yet.",
        )),
        verb: tr(Message::new("gui-masks-take-out-verb", "Take out")),
        danger: true,
        command: "mask.member.delete".into(),
        args: serde_json::json!({ "member": member, "confirmed": true }),
    })
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
    let item = net.member(member)?;
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
    let item = net.member(member)?;
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
        store.declare(&path, label_descriptor(&item.node));
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
    let item = net.member(member)?;
    let path = lock_path(&item.node);
    if store.get(&path).is_none() {
        store.declare(&path, lock_descriptor(&item.node));
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
        for group in net.branches() {
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
        // A member shown once per port: any of its rows will do.
        let cursor_node = self
            .state
            .selection()
            .cursor()
            .and_then(|c| net.items.get(c))
            .map(|i| i.node.as_str());
        match &desired {
            Some(k) if cursor_node != net.items.get(k).map(|i| i.node.as_str()) => {
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
        // The mask display's mode, and the row it shows.
        let display: (crate::mask_display::Mode, Option<String>);
        let net = {
            let store = values.lock().unwrap_or_else(PoisonError::into_inner);
            let v = viewport.borrow();
            let mut net = v
                .graph()
                .map(|g| network(g, &store, None))
                .unwrap_or_default();
            let ctx = ui.ctx().clone();
            net.translate(&|m: Message| crate::i18n::text(&ctx, m));
            self.follow(&net);
            self.follow_selection(&net, &store);
            display = (
                crate::mask_display::mode(&store),
                crate::mask_display::shown_key(&net, &store)
                    .and_then(|k| net.items.get(&k).map(|i| i.name().to_string())),
            );
            net
        };
        self.shown = net.clone();
        if net.root.is_none() {
            ui.weak(crate::i18n::text(
                ui.ctx(),
                Message::new(
                    "gui-masks-none",
                    "No mask: the selected module is not masked",
                ),
            ));
            return;
        }
        // Whose mask this is: the panel follows the selection, and falls
        // back to the first masked module when the selected one has none.
        let ctx = ui.ctx().clone();
        let tr = |m: Message| crate::i18n::text(&ctx, m);
        if let Some(module) = &net.masked {
            ui.weak(tr(
                Message::new("gui-masks-of", "Mask of { $module }").with("module", module)
            ))
            .on_hover_text(tr(Message::new(
                "gui-masks-tree-hint",
                "F2 renames the selected row. Drag a row to reorder it or into a group. Right-click a row for its menu: it never deletes anything.",
            )));
        }
        // An empty mask changes nothing on the image: say why, rather
        // than leave an empty tree that looks broken. Above the tree,
        // which takes the rest of the panel's height.
        if net.is_empty() {
            let module = net.masked.clone().unwrap_or_default();
            let text = crate::i18n::text(
                ui.ctx(),
                Message::new(
                    "gui-masks-empty",
                    "The mask is empty: { $module } applies everywhere. Add a shape or a parametric member to limit it.",
                )
                .with("module", module),
            );
            ui.add(egui::Label::new(egui::RichText::new(text).weak()).wrap());
        }
        let mut commands = Vec::new();
        let mut cycle_display = false;
        // The row selected, when it is a member (or the root): a
        // refinement is neither inverted, hidden nor taken out.
        let current = self
            .state
            .selection()
            .cursor()
            .and_then(|k| net.items.get(k))
            .filter(|i| !i.is_refinement())
            .cloned();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(current.is_some(), |ui| {
                let (inverted, visible) = current
                    .as_ref()
                    .map_or((false, true), |c| (c.invert, c.opacity > 0.0));
                let member = current.as_ref().map(|c| c.key.clone());
                if ui
                    .selectable_label(inverted, tr(Message::new("gui-masks-invert", "Invert")))
                    .on_hover_text(tr(Message::new(
                        "gui-masks-invert-hint",
                        "Invert the selected member: it then covers everything except what it covered (for the module's mask, the whole mask)",
                    )))
                    .clicked()
                {
                    commands.push(Command::MemberInvert {
                        member: member.clone(),
                    });
                }
                if ui
                    .selectable_label(visible, tr(Message::new("gui-masks-opacity", "Opacity")))
                    .on_hover_text(tr(Message::new(
                        "gui-masks-opacity-hint",
                        "Turn the selected member's opacity off, or back to full (its row's eye does the same)",
                    )))
                    .clicked()
                {
                    commands.push(Command::MemberOpacity {
                        member: member.clone(),
                        opacity: None,
                    });
                }
                let deletable = current.as_ref().is_some_and(|c| c.group.is_some());
                if ui
                    .add_enabled(
                        deletable,
                        egui::Button::new(tr(Message::new("gui-masks-take-out", "Take out"))),
                    )
                    .on_hover_text(tr(Message::new(
                        "gui-masks-take-out-hint",
                        "Take the selected member out of the mask (Delete, the tree focused). Not an undo step yet.",
                    )))
                    .clicked()
                {
                    commands.push(Command::MemberDelete {
                        member: member.clone(),
                        confirmed: false,
                    });
                }
            });
            let add = ui
                .button(tr(Message::new("gui-masks-add", "Add…")))
                .on_hover_text(tr(Message::new(
                    "gui-masks-add-hint",
                    "Add a shape or a parametric member to the selected group. With the image focused: C a circle, E an ellipse, G a gradient, P a path.",
                )));
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
        // The mask display, on a row of its own (the actions fill the
        // panel's width): the same cycle as the command and M.
        ui.horizontal(|ui| {
            let (mode, shown) = &display;
            let hint = tr(Message::new(
                "gui-mask-display-hint",
                "Show { $mask } on the image, now { $mode }. This button, M with the image focused and the palette cycle a coloured overlay, the mask alone in grey, and off.",
            )
            .with("mask", shown.clone().unwrap_or_default())
            .with("mode", tr(mode.words())));
            if ui
                .selectable_label(
                    *mode != crate::mask_display::Mode::Off,
                    tr(Message::new("gui-mask-display", "Show mask")),
                )
                .on_hover_text(hint)
                .clicked()
            {
                cycle_display = true;
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
            commands.push(Command::MemberDelete {
                member: current.as_ref().map(|c| c.key.clone()),
                confirmed: false,
            });
        }
        // A rename asked for by a command starts from the node's name.
        if let Some(key) = self.rename.take()
            && self.state.renaming().is_none()
        {
            self.state.rename(&key);
        }
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
            // Parameters panel's), not one of the panel's own: the
            // normalized UI's test allows this call by name.
            view = view.app(app).context_menu(app, ROW_MENU);
        }
        let out = view.show(ui, &mut self.state, &net);
        commands.extend(toggles);
        let mut store = values.lock().unwrap_or_else(PoisonError::into_inner);
        if cycle_display && let Err(e) = crate::mask_display::cycle(&mut store, &Origin::User) {
            request(ui.ctx(), Request::Notify(Level::Warning, e.into()));
        }
        for signal in out.signals {
            match signal {
                Signal::Select {
                    cursor: Some(k), ..
                } => {
                    select(&mut store, &net, &k);
                    self.desired = selected(&net, &store);
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
            // A group that holds members is not taken out without asking.
            if let Command::MemberDelete {
                member,
                confirmed: false,
            } = &c
                && let Ok(key) = member_or_selected(&net, &store, member.as_deref())
                && let Some(ask) = confirm_take_out(ui.ctx(), &net, &key)
            {
                request(ui.ctx(), Request::Confirm(ask));
                continue;
            }
            if let Err(e) = viewport.borrow_mut().mask(c, Origin::User, &mut store) {
                request(ui.ctx(), Request::Notify(Level::Warning, e.into()));
            }
        }
    }
}

/// A row's eye and lock, right to left, and a group's operator drawn
/// before them: they act on that row's member, not on the selection.
fn row_toggles(ui: &mut Ui, item: &Item, commands: &mut Vec<Command>) {
    // A refinement has no opacity in a group and does not move.
    if item.is_refinement() {
        return;
    }
    let ctx = ui.ctx().clone();
    let name = item.name();
    let member = Some(item.key.clone());
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
    if let Some(op) = item.operator.as_deref().filter(|_| item.is_group()) {
        operator_glyph(ui, op, &operator_hint(&ctx, op));
    }
}

/// A group's operator, drawn: two soft discs folded by it a cell at a
/// time, the accent's strength the result, their outlines muted. The
/// picture is the operator's own result on two members, not a symbol.
pub fn operator_glyph(ui: &mut Ui, operator: &str, hint: &str) -> egui::Response {
    const W: usize = 20;
    const H: usize = 14;
    const R: f32 = 5.5;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(W as f32, H as f32), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let t = crate::theme::Tokens::of(ui.ctx());
        let op = Operator::from_name(operator).unwrap_or(Operator::Maximum);
        let centres = [egui::pos2(7.0, 7.0), egui::pos2(13.0, 7.0)];
        // Full inside, fading over the last two points of the radius.
        let disc = |c: egui::Pos2, p: egui::Pos2| ((R - c.distance(p)) / 2.0 + 0.5).clamp(0.0, 1.0);
        let mut mesh = egui::Mesh::default();
        for y in 0..H {
            for x in 0..W {
                let p = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                let v = fold(op, centres.map(|c| (disc(c, p), 1.0, false))).unwrap_or(0.0);
                if v > 0.0 {
                    let cell = egui::Rect::from_min_size(
                        rect.min + egui::vec2(x as f32, y as f32),
                        egui::vec2(1.0, 1.0),
                    );
                    mesh.add_colored_rect(cell, t.accent.default.gamma_multiply(v));
                }
            }
        }
        let painter = ui.painter();
        painter.add(egui::Shape::mesh(mesh));
        for c in centres {
            painter.circle_stroke(
                rect.min + c.to_vec2(),
                R,
                egui::Stroke::new(1.0, t.text.muted),
            );
        }
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Image, true, hint));
    response.on_hover_text(hint)
}

fn add_items() -> Vec<MenuItem> {
    ADD_KINDS
        .into_iter()
        .map(|k| MenuItem::command(&format!("mask.add.{k}")))
        .collect()
}
