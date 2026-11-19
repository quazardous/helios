//! The Modules panel, and bypassing a module.
//!
//! - **The panel** lists the modules of the network shown, in the order
//!   they were added (inside a sub-workspace, its modules only): a row selects its module (`view/graph/selection`,
//!   what the Parameters and Masks panels follow), and its Bypass toggle
//!   bypasses it or brings it back. A module whose parameters differ
//!   from their defaults carries a mark after its name
//!   (`module:<node>:changed`, `crate::edit_marks`).
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
//!   selected (`macro.make`). A folded macro's "Interface…" opens its
//!   interface editor (`macro.interface`); one pinned at an older version
//!   than the workspace or Helios has offers "Update to 2"
//!   (`macro.upgrade`).

use std::cell::RefCell;
use std::sync::{Mutex, PoisonError};

use egui::Ui;
use egui_pop::core::areas::{self, AreaKind, AreaSpec};
use egui_pop::core::fold::{self, TreeRow};
use egui_pop::core::tooltip::TooltipExt as _;
use egui_pop::core::ui::{TextRole, UiPop as _, role_text};
use egui_pop::core::widgets::{self, ToggleChip};
use helios_core::{Graph, Message, Value};
use helios_shell::commands::Origin;
use helios_ui::{Action, Scroll};
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
/// "bypass \<node\>" or "restore \<node\>"; nothing when it already is.
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

/// A macro of the workspace's network: its version, whether it is
/// opened, and a newer version on offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MacroRow {
    version: u32,
    open: bool,
    newer: Option<u32>,
    /// Its file carries a workshop block.
    workshop: bool,
}

/// The newest version of each composite Helios ships, by id.
fn shipped_newest() -> &'static std::collections::BTreeMap<String, u32> {
    static NEWEST: std::sync::OnceLock<std::collections::BTreeMap<String, u32>> =
        std::sync::OnceLock::new();
    NEWEST.get_or_init(|| {
        let mut out = std::collections::BTreeMap::new();
        for f in helios_template::shipped::composites() {
            let v: &mut u32 = out.entry(f.id).or_default();
            *v = (*v).max(f.version);
        }
        out
    })
}

/// The workspace's macros, by node: their version, whether each is
/// opened, and the newer version the workspace's composites or the
/// shipped ones offer.
fn macros(store: &Store) -> std::collections::BTreeMap<String, MacroRow> {
    let Some(Value::Text(t)) = store.get(crate::shell::WORKSPACE_STATE).map(|e| &e.value) else {
        return std::collections::BTreeMap::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(t) else {
        return std::collections::BTreeMap::new();
    };
    let mut newest = shipped_newest().clone();
    // The files with a workshop block, `id@version`.
    let mut workshops: std::collections::BTreeSet<String> = helios_template::shipped::composites()
        .into_iter()
        .filter(|f| f.workshop.is_some())
        .map(|f| f.reference().to_string())
        .collect();
    for f in json["composites"].as_array().into_iter().flatten() {
        if !f["workshop"].is_null()
            && let (Some(id), Some(v)) = (f["id"].as_str(), f["version"].as_u64())
        {
            workshops.insert(format!("{id}@{v}"));
        }
        if let (Some(id), Some(v)) = (f["id"].as_str(), f["version"].as_u64()) {
            let v = u32::try_from(v).unwrap_or_default();
            let at: &mut u32 = newest.entry(id.to_string()).or_default();
            *at = (*at).max(v);
        }
    }
    json["template"]["network"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let version = u32::try_from(n["composite"]["version"].as_u64()?).unwrap_or_default();
            let newer = n["composite"]["id"]
                .as_str()
                .and_then(|id| newest.get(id))
                .copied()
                .filter(|v| *v > version);
            let workshop = n["composite"]["id"]
                .as_str()
                .is_some_and(|id| workshops.contains(&format!("{id}@{version}")));
            Some((
                n["name"].as_str()?.to_string(),
                MacroRow {
                    version,
                    open: n["open"].as_bool().unwrap_or(false),
                    newer,
                    workshop,
                },
            ))
        })
        .collect()
}

/// What a click in one of the panel's rows asks for.
#[derive(Default)]
struct Asked {
    /// The row's module selected.
    select: bool,
    /// Its bypass toggled.
    toggle: bool,
    /// A command run on it, with its arguments.
    run: Option<(&'static str, serde_json::Value)>,
}

/// The macro of `node` opened or folded as `open`, kept as the
/// workspace has it: its tree row's state follows the workspace's, and a
/// click on its toggle runs `macro.open` or `macro.fold`.
fn macro_toggle(ui: &Ui, node: &str, open: bool) -> egui::Id {
    let id = ui.make_persistent_id(("helios.modules.macro", node));
    if fold::is_open(ui.ctx(), id) != Some(open) {
        fold::set_open(ui.ctx(), id, open);
    }
    id
}

/// Draws the panel: one row per module of the viewport's network; with
/// `workshops` on (the experimental switch), a macro without a workshop
/// block offers "Save as workshop…" (`workshop.save`).
pub fn ui(
    ui: &mut Ui,
    viewport: &RefCell<crate::viewport::Viewport>,
    values: &Mutex<Store>,
    marks: &crate::edit_marks::Marks,
    workshops: bool,
) {
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
        ui.muted(tr(match scope {
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
    // A network with macros: every row takes a toggle's room, so that the
    // macros' rows and the others line up.
    let tree = !macros.is_empty();
    helios_ui::scroll(ui, PANEL, Scroll::Region, |ui| {
        for row in &rows {
            // An opened macro's modules, under a heading row.
            let inside = row
                .node
                .split_once('.')
                .filter(|(m, _)| macros.get(*m).is_some_and(|r| r.open))
                .map(|(m, _)| m.to_string());
            if inside != heading {
                if let Some(m) = &inside {
                    let version = macros.get(m).map_or(0, |r| r.version);
                    let id = macro_toggle(ui, m, true);
                    let shown = TreeRow::new(id, m.as_str()).show(ui, |ui| {
                        ui.strong_text(m);
                        ui.muted(tr(Message::new("gui-modules-macro", "macro { $version }")
                            .with("version", version)));
                    });
                    if let Some(t) = &shown.toggle {
                        let _ = t.clone().tip(tr(Message::new(
                            "gui-modules-fold-hint",
                            "Fold { $node } back into one node",
                        )
                        .with("node", m)));
                    }
                    if shown.toggled {
                        run = Some(("macro.fold", serde_json::json!({ "node": m })));
                    }
                    shown.keep(ui.ctx());
                }
                heading.clone_from(&inside);
            }
            let folded = macros.get(&row.node).filter(|m| !m.open);
            let draw = |ui: &mut Ui| {
                // Wrapped: a macro's buttons go under its name in a
                // narrow panel rather than past its edge.
                ui.horizontal_wrapped(|ui| {
                    module_row(
                        ui,
                        row,
                        &RowState {
                            selected: selection.as_deref() == Some(row.node.as_str())
                                || many.len() > 1 && many.contains(&row.node),
                            changed: marks.changed(&row.node),
                            folded,
                            workshops,
                        },
                    )
                })
                .inner
            };
            let line = |ui: &mut Ui| -> Asked {
                if !tree {
                    return draw(ui);
                }
                // A folded macro's toggle opens it; another row only takes
                // the toggle's room.
                let id = if folded.is_some() {
                    macro_toggle(ui, &row.node, false)
                } else {
                    ui.make_persistent_id(("helios.modules.row", &row.node))
                };
                let mut shown = TreeRow::new(id, row.node.as_str())
                    .leaf(folded.is_none())
                    .show(ui, draw);
                let mut asked = std::mem::take(&mut shown.row.inner);
                if let Some(t) = &shown.toggle {
                    let _ = t.clone().tip(tr(Message::new(
                        "gui-modules-open-hint",
                        "Open { $node }: its modules shown",
                    )
                    .with("node", &row.node)));
                }
                if shown.toggled {
                    asked.run = Some(("macro.open", serde_json::json!({ "node": row.node })));
                }
                shown.keep(ui.ctx());
                asked
            };
            let asked = if inside.is_some() {
                ui.indent(("helios.modules.inside", &row.node), line).inner
            } else {
                line(ui)
            };
            if asked.select {
                let modifiers = ui.input(|i| i.modifiers);
                many = pick(&names, &many, selection.as_deref(), &row.node, modifiers);
                select = Some(row.node.clone());
            }
            if asked.toggle {
                toggle = Some(row.node.clone());
            }
            if asked.run.is_some() {
                run = asked.run;
            }
        }
        if !many.is_empty()
            && helios_ui::action(
                ui,
                Action::Secondary,
                &tr(Message::new("gui-modules-make-macro", "Make macro…")),
            )
            .tip(tr(Message::new(
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

/// How a module's row is shown.
struct RowState<'a> {
    /// It is selected (alone, or one of several).
    selected: bool,
    /// Its parameters differ from their defaults.
    changed: bool,
    /// It is a folded macro.
    folded: Option<&'a MacroRow>,
    /// The experimental switch: "Save as workshop…" offered.
    workshops: bool,
}

/// A module's row: its Bypass toggle, its name (struck through when
/// bypassed), its changed mark, then its module, or a folded macro's
/// version and actions.
fn module_row(ui: &mut Ui, row: &Row, state: &RowState<'_>) -> Asked {
    let ctx = ui.ctx().clone();
    let tr = |m: Message| crate::i18n::text(&ctx, m);
    let node = row.node.as_str();
    let mut asked = Asked::default();
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
        let hint = tr(hint.with("node", node).with("key", KEY));
        let mut on = b;
        let r = ui
            .add(ToggleChip::new(
                &mut on,
                tr(Message::new("gui-modules-bypass", "Bypass")),
            ))
            .tip(hint);
        // An edit area: it bypasses the module in the render (its hover
        // text says so, no card).
        areas::register(
            ui,
            &r,
            AreaSpec::new(format!("module.bypass.{node}"), AreaKind::Edit).path(bypass_path(node)),
        );
        asked.toggle = r.clicked();
    }
    let clicked = if row.bypassed == Some(true) {
        let name = role_text(ui, TextRole::Muted, node).strikethrough();
        widgets::selectable_label(ui, state.selected, name)
    } else {
        widgets::selectable_label(ui, state.selected, node)
    };
    // Selecting a module moves what the other panels show.
    areas::register(
        ui,
        &clicked,
        AreaSpec::navigate(format!("module.select.{node}")),
    );
    if state.changed {
        changed_mark(ui, node);
    }
    asked.select = clicked.clicked();
    let Some(m) = state.folded else {
        ui.muted(&row.module);
        return asked;
    };
    ui.muted(tr(
        Message::new("gui-modules-macro", "macro { $version }").with("version", m.version)
    ));
    if let Some(newer) = m.newer
        && helios_ui::row_action(
            ui,
            Action::Secondary,
            &tr(
                Message::new("gui-modules-macro-update", "Update to { $version }")
                    .with("version", newer),
            ),
        )
        .tip(tr(Message::new(
            "gui-modules-macro-update-hint",
            "Move { $node } to version { $version } of its macro, its values carried by name",
        )
        .with("node", node)
        .with("version", newer)))
        .clicked()
    {
        asked.run = Some((
            "macro.upgrade",
            serde_json::json!({ "node": node, "version": newer }),
        ));
    }
    if helios_ui::row_action(
        ui,
        Action::Secondary,
        &tr(Message::new("gui-modules-macro-interface", "Interface…")),
    )
    .tip(tr(Message::new(
        "gui-modules-macro-interface-hint",
        "Choose the parameters { $node } shows, saved as a new version",
    )
    .with("node", node)))
    .clicked()
    {
        asked.run = Some(("macro.interface", serde_json::json!({ "node": node })));
    }
    if state.workshops
        && !m.workshop
        && helios_ui::row_action(
            ui,
            Action::Secondary,
            &tr(Message::new("gui-modules-save-workshop", "Save as workshop…")),
        )
        .tip(tr(Message::new(
            "gui-modules-save-workshop-hint",
            "Give { $node } an intent and targets aimed per image, saved as a new version (experimental)",
        )
        .with("node", node)))
        .clicked()
    {
        asked.run = Some(("workshop.save", serde_json::json!({ "node": node })));
    }
    if helios_ui::row_action(
        ui,
        Action::Secondary,
        &tr(Message::new("gui-modules-macro-publish", "Publish…")),
    )
    .tip(tr(Message::new(
        "gui-modules-macro-publish-hint",
        "Copy this version of { $node } into your library, for every workspace's Add menu; asked first, and not undone",
    )
    .with("node", node)))
    .clicked()
    {
        asked.run = Some(("macro.publish", serde_json::json!({ "node": node })));
    }
    asked
}

/// The mark of a module whose parameters differ from their defaults,
/// named `module:<node>:changed` (the Modules panel's rows, the
/// Parameters panel's headers).
pub fn changed_mark(ui: &mut Ui, node: &str) {
    let ctx = ui.ctx().clone();
    let name = crate::i18n::text(
        &ctx,
        Message::new("gui-modules-changed", "{ $node } changed from its defaults")
            .with("node", node),
    );
    let r = ui.text_tone(egui_pop::core::ui::Tone::Info, "•");
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &name));
    crate::name_rect(&ctx, &format!("module:{node}:changed"), r.rect);
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
