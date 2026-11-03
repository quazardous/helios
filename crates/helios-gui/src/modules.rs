//! The Modules panel, and bypassing a module.
//!
//! - **The panel** lists the modules of the network shown, in the order
//!   they were added (inside a sub-workspace, its modules only): a row selects its module (`view/graph/selection`,
//!   what the Parameters and Masks panels follow), and its Bypass toggle
//!   bypasses it or brings it back.
//! - **A bypass** is a value of the store, `bypass/<node>`
//!   (`helios_values::bypass_path`), declared for each module with an
//!   input to pass through: bypassed, the module keeps its parameters and
//!   the image renders as if it were absent. Changing it is one undo step
//!   ("bypass exposure", "restore exposure"), saved with the image's edit
//!   like a parameter, and the viewport's graph follows at the next frame.
//! - **The command** [`COMMAND`] (`node.bypass`) takes `node` (the selected
//!   module by default) and `on` (by default, the other way round);
//!   [`KEY`] runs it on the selected module, the panel's toggle on its
//!   row's. Agents reach the same command, and a node's control endpoint
//!   answers `node.bypass` too (see docs/cli.md).
//! - **Macros.** A folded macro is one row with a disclosure arrow and
//!   its version; the arrow opens it (`macro.open`), its inner modules
//!   then listed under a heading row whose arrow folds it
//!   (`macro.fold`). Shift+click selects a run of rows, Ctrl+click one
//!   more (`view/graph/selected`), and "Make macro…" packages the rows
//!   selected (`macro.make`).

use std::cell::RefCell;
use std::sync::{Mutex, PoisonError};

use egui::Ui;
use helios_core::{Graph, Message, Value};
use helios_shell::commands::Origin;
use helios_values::{Change, Set, Store, bypass_path};

use crate::dialogs::{Level, Request, request};

/// The panel's id.
pub const PANEL: &str = "helios.modules";
/// Bypasses a module or brings it back.
pub const COMMAND: &str = "node.bypass";
/// The key running [`COMMAND`] on the selected module: `Ctrl+/`, written
/// with egui's name of the key, as egui-pop's keymap matches it.
pub const KEY: &str = "Ctrl+Slash";

/// A module of the network shown, as the panel lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The node's name.
    pub node: String,
    /// The module it runs.
    pub module: String,
    /// Whether it is bypassed; `None` when it cannot be (nothing to pass
    /// through).
    pub bypassed: Option<bool>,
}

/// The modules of `graph`, in the order they were added, with their
/// bypass as the store holds it; inside a sub-workspace (the store's
/// scope), its modules only.
pub fn rows(graph: &Graph, store: &Store) -> Vec<Row> {
    graph
        .ids()
        .filter_map(|id| {
            let manifest = graph.manifest(id).ok()??;
            let node = graph.name(id).ok()?.to_string();
            if !store.in_scope(&node) {
                return None;
            }
            let bypassed = store
                .get(&bypass_path(&node))
                .and_then(|e| e.value.as_bool());
            Some(Row {
                node,
                module: manifest.name,
                bypassed,
            })
        })
        .collect()
}

/// The selected module's name, if one is.
pub fn selected(store: &Store) -> Option<String> {
    match &store.get(crate::params::SELECTION)?.value {
        Value::Text(t) if !t.is_empty() => Some(t.clone()),
        _ => None,
    }
}

/// Bypasses `node` (the selected module by default) when `on`, or brings
/// it back; without `on`, the other way round. One undo step, labelled
/// "bypass <node>" or "restore <node>"; nothing when it already is.
/// Returns the node and whether it is now bypassed.
pub fn bypass(
    store: &mut Store,
    node: Option<&str>,
    on: Option<bool>,
    origin: &Origin,
) -> Result<(String, bool), String> {
    let node = match node {
        Some(n) => n.to_string(),
        None => selected(store).ok_or("no module is selected")?,
    };
    let path = bypass_path(&node);
    let current = store
        .get(&path)
        .and_then(|e| e.value.as_bool())
        .ok_or_else(|| format!("`{node}` is not a module that can be bypassed"))?;
    let on = on.unwrap_or(!current);
    let label = if on {
        format!("bypass {node}")
    } else {
        format!("restore {node}")
    };
    let group = store.begin_group(origin.clone(), &label);
    let set =
        store.set(Set::new(&path, Change::To(Value::Bool(on)), origin.clone()).gesture(group));
    store.end(group);
    set.map_err(|e| e.to_string())?;
    Ok((node, on))
}

/// The workspace's macros, by node: their version, and whether each is
/// opened.
fn macros(store: &Store) -> std::collections::BTreeMap<String, (u32, bool)> {
    let Some(Value::Text(t)) = store.get(crate::shell::WORKSPACE_STATE).map(|e| &e.value) else {
        return std::collections::BTreeMap::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(t) else {
        return std::collections::BTreeMap::new();
    };
    json["template"]["network"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let version = n["composite"]["version"].as_u64()?;
            Some((
                n["name"].as_str()?.to_string(),
                (
                    u32::try_from(version).unwrap_or_default(),
                    n["open"].as_bool().unwrap_or(false),
                ),
            ))
        })
        .collect()
}

/// Draws the panel: one row per module of the viewport's network.
pub fn ui(ui: &mut Ui, viewport: &RefCell<crate::viewport::Viewport>, values: &Mutex<Store>) {
    let ctx = ui.ctx().clone();
    // Named, to be pictured and found by tests.
    crate::name_rect(&ctx, PANEL, ui.max_rect());
    let tr = |m: Message| crate::i18n::text(&ctx, m);
    let (rows, selection, scope, macros, mut many) = {
        let store = values.lock().unwrap_or_else(PoisonError::into_inner);
        let rows = viewport
            .borrow()
            .graph()
            .map(|g| rows(g, &store))
            .unwrap_or_default();
        (
            rows,
            selected(&store),
            store.scope().map(|s| s.name.clone()),
            macros(&store),
            crate::shell::selected_nodes(&store),
        )
    };
    if rows.is_empty() {
        ui.weak(tr(match scope {
            Some(name) => Message::new(
                "gui-modules-none-in-scope",
                "No module of the sub-workspace { $name } in the image's network",
            )
            .with("name", name),
            None => Message::new(
                "gui-modules-none",
                "No module: open an image to see its network's modules",
            ),
        }));
        return;
    }
    let mut select = None;
    let mut toggle = None;
    let mut run: Option<(&str, serde_json::Value)> = None;
    let mut heading: Option<String> = None;
    let names: Vec<String> = rows.iter().map(|r| r.node.clone()).collect();
    let many_before = many.clone();
    egui::ScrollArea::vertical()
        .id_salt(PANEL)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for row in &rows {
                // An opened macro's modules, under a heading row.
                let inside = row
                    .node
                    .split_once('.')
                    .filter(|(m, _)| macros.get(*m).is_some_and(|(_, open)| *open))
                    .map(|(m, _)| m.to_string());
                if inside != heading {
                    if let Some(m) = &inside {
                        let version = macros.get(m).map_or(0, |(v, _)| *v);
                        ui.horizontal(|ui| {
                            if ui
                                .small_button("⏷")
                                .on_hover_text(tr(Message::new(
                                    "gui-modules-fold-hint",
                                    "Fold { $node } back into one node",
                                )
                                .with("node", m)))
                                .clicked()
                            {
                                run = Some(("macro.fold", serde_json::json!({ "node": m })));
                            }
                            ui.strong(m);
                            ui.weak(tr(Message::new("gui-modules-macro", "macro { $version }")
                                .with("version", version)));
                        });
                    }
                    heading.clone_from(&inside);
                }
                ui.horizontal(|ui| {
                    if inside.is_some() {
                        ui.add_space(16.0);
                    }
                    if let Some((version, false)) = macros.get(&row.node) {
                        if ui
                            .small_button("⏵")
                            .on_hover_text(tr(Message::new(
                                "gui-modules-open-hint",
                                "Open { $node }: its modules shown",
                            )
                            .with("node", &row.node)))
                            .clicked()
                        {
                            run = Some(("macro.open", serde_json::json!({ "node": row.node })));
                        }
                        let _ = version;
                    }
                    let bypassed = row.bypassed == Some(true);
                    if let Some(b) = row.bypassed {
                        let hint = if b {
                            Message::new(
                                "gui-modules-restore-hint",
                                "Bring { $node } back: the image renders it again ({ $key })",
                            )
                        } else {
                            Message::new(
                                "gui-modules-bypass-hint",
                                "Bypass { $node }: the image renders as if it were absent, its parameters kept ({ $key })",
                            )
                        };
                        let hint = tr(hint.with("node", &row.node).with("key", KEY));
                        if ui
                            .selectable_label(b, tr(Message::new("gui-modules-bypass", "Bypass")))
                            .on_hover_text(hint)
                            .clicked()
                        {
                            toggle = Some(row.node.clone());
                        }
                    }
                    let mut name = egui::RichText::new(&row.node);
                    if bypassed {
                        name = name.strikethrough().weak();
                    }
                    let shown = selection.as_deref() == Some(row.node.as_str())
                        || many.len() > 1 && many.contains(&row.node);
                    let clicked = ui.selectable_label(shown, name);
                    if clicked.clicked() {
                        let modifiers = ui.input(|i| i.modifiers);
                        many = pick(&names, &many, selection.as_deref(), &row.node, modifiers);
                        select = Some(row.node.clone());
                    }
                    match macros.get(&row.node) {
                        Some((version, false)) => {
                            ui.weak(tr(Message::new("gui-modules-macro", "macro { $version }")
                                .with("version", *version)));
                        }
                        _ => {
                            ui.weak(&row.module);
                        }
                    }
                });
            }
            if !many.is_empty()
                && ui
                    .button(tr(Message::new("gui-modules-make-macro", "Make macro…")))
                    .on_hover_text(tr(Message::new(
                        "gui-modules-make-macro-hint",
                        "Package the modules selected as one node (Ctrl+G in the graph)",
                    )))
                    .clicked()
            {
                run = Some(("macro.make", serde_json::json!({ "nodes": many })));
            }
        });
    if let Some((id, args)) = run {
        request(ui.ctx(), Request::Run(id.into(), args, Origin::User));
    }
    let mut store = values.lock().unwrap_or_else(PoisonError::into_inner);
    if many != many_before {
        let _ = store.set(Set::new(
            crate::shell::GRAPH_SELECTED,
            Change::To(Value::Text(serde_json::json!(many).to_string())),
            Origin::User,
        ));
    }
    if let Some(node) = select {
        // Which module one looks at is not an edit: a transient value.
        let _ = store.set(Set::new(
            crate::params::SELECTION,
            Change::To(Value::Text(node)),
            Origin::User,
        ));
    }
    if let Some(node) = toggle
        && let Err(e) = bypass(&mut store, Some(&node), None, &Origin::User)
    {
        request(ui.ctx(), Request::Notify(Level::Warning, e.into()));
    }
}

/// The rows selected after a click on `node`: Shift adds the run from the
/// selected row, Ctrl (Cmd) adds or takes out one row, a plain click
/// selects it alone.
pub fn pick(
    rows: &[String],
    before: &[String],
    anchor: Option<&str>,
    node: &str,
    modifiers: egui::Modifiers,
) -> Vec<String> {
    if modifiers.shift
        && let Some(a) = anchor.and_then(|a| rows.iter().position(|r| r == a))
        && let Some(b) = rows.iter().position(|r| r == node)
    {
        let (lo, hi) = (a.min(b), a.max(b));
        return rows[lo..=hi].to_vec();
    }
    if modifiers.command {
        let mut v = before.to_vec();
        if anchor.is_some_and(|a| !v.iter().any(|x| x == a)) {
            v.extend(anchor.map(String::from));
        }
        if let Some(i) = v.iter().position(|x| x == node) {
            v.remove(i);
        } else {
            v.push(node.to_string());
        }
        return v;
    }
    vec![node.to_string()]
}
