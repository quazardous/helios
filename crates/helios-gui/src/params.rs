//! The Parameters panel: each node's parameters as a card of
//! helios-rack (`crates/helios-rack`), which resolves them through the
//! controls layer (`helios_controls`, `docs/design/controls.md`) and has
//! egui_hud draw them with egui-pop's widgets; this panel draws none. It
//! mounts the rack and does the shell's part:
//!
//! - what only the shell knows, given to the rack as data: a mask group's
//!   operator named by its effect (union, intersection, …) with the Masks
//!   panel's preview of each, its per-port lists (opacity, invert) as a
//!   line per wired port, the member's name and its value in words
//!   ("spot: opacity 30 %");
//! - what the rack asks of it: each part's picture's name
//!   (`param:<path>`), its context menu (reset, copy the path or the
//!   value, paste: commands, like everything else), Edit › Copy and
//!   Paste onto the focused part (`crate::clipboard::param`), the notes a
//!   typed value earns;
//! - the panel around the cards: each module's header with its changed
//!   mark and Reset (`node.reset`, `crate::edit_marks`), a macro's
//!   sections.
//!
//! The rack binds the parts to the value store (gestures, steps in a
//! row, typed values, reset, reveal): see its crate's notes.

use std::collections::BTreeMap;
use std::sync::Mutex;

use egui::{Sense, Ui};
use egui_pop::core::icon::{Icon, Tone};
use egui_pop::core::ui::UiPop as _;
use helios_controls::ContextKeys;
use helios_controls::commands::SharedContexts;
use helios_core::{Message, Value};
use helios_rack::{Extra, Output, Rack};
use helios_rack::{Line, Preview};
use helios_shell::commands::Origin;
use helios_values::{Descriptor, Store};
use serde_json::json;

use crate::dialogs::{Level, Request, request};

pub use helios_rack::{PICKER_EVENT, choice, description, label, shown_in};

/// The panel.
pub const PANEL: &str = "helios.params";

/// The node whose parameters the panel shows (a node name; empty or absent:
/// every node). The graph editor sets it.
pub const SELECTION: &str = "view/graph/selection";

/// Each mask group's members by port, by the group's node name: what its
/// per-port lists are about (see [`crate::mask_tree::members_by_port`]).
pub type Members = BTreeMap<String, Vec<Option<String>>>;

/// Parses a typed number or expression as the sliders do: units after it
/// are ignored, a comma is a decimal separator.
pub fn parse_typed(text: &str) -> Option<f64> {
    egui_pop::core::slider::parse_typed(text)
}

/// A value as shown in English: display scale, precision, unit.
pub fn shown(d: &Descriptor, v: &Value) -> String {
    shown_in("en", d, v)
}

/// The panel's rack, and the application the parts' menus are of.
pub struct Params {
    rack: Rack,
}

impl Default for Params {
    fn default() -> Self {
        Self::new(SharedContexts::default())
    }
}

/// What the shell adds to a mask group's parameters: its operator by its
/// effect, with the Masks panel's preview of each; its per-port lists as
/// a line per member.
fn extras(
    ctx: &egui::Context,
    store: &Store,
    paths: &[&str],
    members: &Members,
) -> BTreeMap<String, Extra> {
    let mut out = BTreeMap::new();
    for path in paths {
        let Some(e) = store.get(path) else {
            continue;
        };
        let Some((node, name)) = helios_rack::group_param(&e.descriptor, path) else {
            continue;
        };
        let extra = match name {
            "operator" => Extra {
                choices: e
                    .descriptor
                    .choices
                    .iter()
                    .map(|o| {
                        (
                            o.clone(),
                            crate::i18n::text(ctx, crate::mask_tree::operator_effect(o)),
                        )
                    })
                    .collect(),
                previews: e
                    .descriptor
                    .choices
                    .iter()
                    .map(|o| (o.clone(), crate::mask_tree::operator_preview(ctx, o)))
                    .collect::<BTreeMap<String, Preview>>(),
                lines: None,
            },
            "opacity" | "invert" => match members.get(node) {
                Some(m) => Extra {
                    lines: Some(port_lines(ctx, &e.value, m)),
                    ..Extra::default()
                },
                None => continue,
            },
            _ => continue,
        };
        out.insert((*path).to_owned(), extra);
    }
    out
}

impl Params {
    /// The panel's memory, resolving cards in `contexts` (the ones the
    /// `controls.*` commands share).
    pub fn new(contexts: SharedContexts) -> Self {
        Self {
            rack: Rack::new(
                contexts,
                std::sync::Arc::clone(crate::controls::registry()),
                std::sync::Arc::clone(crate::controls::steps()),
            ),
        }
    }

    /// The controls' context the panel's cards resolve in.
    pub fn contexts(&self) -> &SharedContexts {
        self.rack.contexts()
    }

    /// The application whose declared context menu the parts open.
    pub fn set_app(&mut self, app: egui_pop::api::AppHandle) {
        self.rack.set_app(app);
    }

    /// One parameter alone: a card of its own, in the width available.
    pub fn param_ui(&mut self, ui: &mut Ui, store: &Mutex<Store>, path: &str) {
        let Some((node, _)) = helios_rack::node_param(path) else {
            return;
        };
        let node = node.to_owned();
        let keys = self.rack.keys(ui, false);
        self.card_ui(
            ui,
            store,
            &node,
            &format!("{path}#alone"),
            &[path],
            &Members::new(),
            keys,
        );
    }

    /// One card through the rack, then the shell's part of it: names,
    /// menus, the clipboard's focus, notes.
    #[expect(
        clippy::too_many_arguments,
        reason = "the card's node, id, parameters, members and keys, and where to draw"
    )]
    fn card_ui(
        &mut self,
        ui: &mut Ui,
        store: &Mutex<Store>,
        node: &str,
        id: &str,
        paths: &[&str],
        members: &Members,
        keys: ContextKeys,
    ) {
        let ctx = ui.ctx().clone();
        let extras = {
            let s = store.lock().expect("value store lock");
            extras(&ctx, &s, paths, members)
        };
        let out = self.rack.card(ui, store, node, id, paths, &extras, keys);
        self.touch(&ctx, out);
    }

    /// The shell's touches on what a card drew.
    fn touch(&self, ctx: &egui::Context, out: Output) {
        for (name, at) in out.named {
            crate::name_rect(ctx, &name, at);
        }
        for t in out.touches {
            crate::name_rect(ctx, &format!("param:{}", t.path), t.response.rect);
            if !t.menu {
                continue;
            }
            if let Some(app) = self.rack.app() {
                if t.response.has_focus() {
                    // The Edit commands copy it and paste onto it.
                    crate::clipboard::param::focus(app, &t.path, t.response.id);
                }
                egui_pop::api::menus::context_menu_with(
                    t.menu_on.as_ref().unwrap_or(&t.response),
                    app,
                    &crate::plugins::PARAM,
                    json!({ "path": t.path }),
                );
            }
        }
        for n in out.notes {
            request(ctx, Request::Notify(Level::Info, n));
        }
    }

    /// The Parameters panel: the selected node's values, or every node's,
    /// grouped by node in the manifest's order; inside a sub-workspace
    /// (the store's scope), its nodes' only. Each node is a card of the
    /// rack. A node whose parameters name groups (a macro's exposed ones)
    /// shows them by section, a heading per group that folds (see
    /// [`sections`]), each section a card. A mask group's per-port lists
    /// say which member each value is for, from `members`.
    pub fn panel_ui(
        &mut self,
        ui: &mut Ui,
        store: &Mutex<Store>,
        selection: Option<&str>,
        members: &Members,
        marks: &crate::edit_marks::Marks,
    ) {
        self.rack.end_steps(store, ui.input(|i| i.time));
        let paths: Vec<(String, String, Option<String>)> = {
            let s = store.lock().expect("value store lock");
            let mut entries = s.list("node/");
            // In the order the manifests declare them.
            entries.sort_by_key(|(_, e)| e.order);
            entries
                .into_iter()
                .filter_map(|(p, e)| {
                    let (node, _) = helios_rack::node_param(p)?;
                    Some((node.to_string(), p.to_string(), e.descriptor.group.clone()))
                })
                .filter(|(node, _, _)| selection.is_none_or(|s| s == node))
                .filter(|(node, _, _)| s.in_scope(node))
                .collect()
        };
        if paths.is_empty() {
            ui.weak("No parameters: nothing is shown yet");
            return;
        }
        ui.scroll_area(
            egui::ScrollArea::vertical().id_salt("helios.params"),
            |ui| {
                let keys = self.rack.keys(ui, true);
                let mut start = 0;
                while start < paths.len() {
                    let node = &paths[start].0;
                    let end = paths[start..]
                        .iter()
                        .position(|(n, _, _)| n != node)
                        .map_or(paths.len(), |i| start + i);
                    ui.add_space(4.0);
                    header_ui(ui, node, marks.changed(node));
                    // A module's own header (the chart profile's choice).
                    crate::calibrate::header_ui(ui, store, node);
                    // The Camera look entry's (fit, switch, scores).
                    crate::calibrate::look_header_ui(ui, store, node);
                    let rows: Vec<(&str, Option<&str>)> = paths[start..end]
                        .iter()
                        .map(|(_, p, g)| (p.as_str(), g.as_deref()))
                        .collect();
                    for (group, paths) in sections(&rows) {
                        if let Some(group) = group
                            && !section_row(ui, node, group)
                        {
                            continue;
                        }
                        // The node's own card keeps the node's name (what
                        // `controls.*` resolves); a section's is named after it.
                        let id = group.map_or_else(|| node.clone(), |g| format!("{node}#{g}"));
                        self.card_ui(ui, store, node, &id, &paths, members, keys.clone());
                    }
                    start = end;
                }
            },
        );
    }
}

/// A module's header: its name, its changed mark when a parameter
/// differs from its default, and its Reset (`node.reset`: its
/// parameters back to their defaults, its bypass kept, one undo step),
/// offered only then.
fn header_ui(ui: &mut Ui, node: &str, changed: bool) {
    ui.horizontal(|ui| {
        ui.strong_text(node);
        if changed {
            crate::modules::changed_mark(ui, node);
            let ctx = ui.ctx().clone();
            let reset = crate::i18n::text(&ctx, Message::new("gui-param-module-reset", "Reset"));
            let r = helios_ui::row_action(ui, helios_ui::Action::Secondary, &reset);
            crate::name_rect(&ctx, &format!("module:{node}:reset"), r.rect);
            if r.clicked() {
                crate::dialogs::request(
                    &ctx,
                    crate::dialogs::Request::Run(
                        crate::edit_marks::RESET_NODE.into(),
                        json!({ "node": node }),
                        Origin::User,
                    ),
                );
            }
        }
    });
}

/// A node's parameters by section: the ungrouped ones first, then each
/// group in the order of its first parameter, its parameters in their
/// order (`docs/design/macros.md`, "The exposed-parameter editor").
pub fn sections<'a>(rows: &[(&'a str, Option<&'a str>)]) -> Vec<(Option<&'a str>, Vec<&'a str>)> {
    let mut out: Vec<(Option<&str>, Vec<&str>)> = Vec::new();
    if rows.iter().any(|(_, g)| g.is_none()) {
        out.push((None, Vec::new()));
    }
    for (path, group) in rows {
        match out.iter_mut().find(|(g, _)| g == group) {
            Some((_, paths)) => paths.push(path),
            None => out.push((*group, vec![path])),
        }
    }
    out
}

/// A section's heading: a chevron and the group's name, a click on
/// either folds or unfolds it (open at first). Returns whether it is
/// open.
fn section_row(ui: &mut Ui, node: &str, group: &str) -> bool {
    let ctx = ui.ctx().clone();
    let id = egui::Id::unique(("helios.params.section", node, group));
    let open = ctx.data(|m| m.get_temp::<bool>(id)).unwrap_or(true);
    let hint = crate::i18n::text(
        &ctx,
        if open {
            Message::new("gui-params-fold-section", "Fold { $group }")
        } else {
            Message::new("gui-params-unfold-section", "Unfold { $group }")
        }
        .with("group", group),
    );
    let icon = if open {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    let clicked = ui
        .horizontal(|ui| {
            let chevron = ui
                .add(icon.tone(Tone::Muted).label(&hint).sense_click())
                .on_hover_text(&hint);
            let title = ui
                .add(egui::Label::new(egui::RichText::new(group).strong()).sense(Sense::click()))
                .on_hover_text(&hint);
            chevron.clicked() || title.clicked()
        })
        .inner;
    if clicked {
        ctx.data_mut(|m| m.insert_temp(id, !open));
    }
    open
}

/// A mask group's per-port list (opacities or inversions) as lines, one
/// per wired port: the member's name and its value in words, as the
/// Masks panel's rows say them, with the port in the line's hint.
fn port_lines(ctx: &egui::Context, v: &Value, members: &[Option<String>]) -> Vec<Line> {
    use crate::mask_tree::{inverted_words, opacity_words};
    let words = |port: usize| match v {
        Value::List(l) => opacity_words(l.get(port).copied().unwrap_or(1.0)),
        Value::Ints(l) => inverted_words(l.get(port).is_some_and(|x| *x != 0)),
        _ => Message::plain(""),
    };
    let lines: Vec<Line> = members
        .iter()
        .enumerate()
        .filter_map(|(port, m)| {
            let member = m.as_deref()?;
            let line = crate::i18n::text(
                ctx,
                Message::new("gui-params-port-member", "{ $member }: { $value }")
                    .with("member", member)
                    .with("value", crate::i18n::text(ctx, words(port))),
            );
            let hint = crate::i18n::text(
                ctx,
                Message::new(
                    "gui-params-port-hint",
                    "Port { $port } of the group; the Masks panel edits it on { $member }'s row",
                )
                .with("port", port + 1)
                .with("member", member),
            );
            Some(Line::new(line).hint(hint))
        })
        .collect();
    if lines.is_empty() {
        let none = crate::i18n::text(ctx, Message::new("gui-params-no-members", "no members yet"));
        return vec![Line::new(none.clone()).hint(none)];
    }
    lines
}
