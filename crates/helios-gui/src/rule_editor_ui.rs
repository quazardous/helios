//! The rule editor's dialog ([`crate::rule_editor`]): the tree drawn as
//! nested groups, each condition's field, operator and value boxes by
//! the field's kind, its fault said under it. The dialog changes nothing
//! itself: each edit is asked as the arguments of
//! `library.collection.rules.edit`, which the shell runs.
//!
//! A tree does not fit the declarative forms (flat fields, one command):
//! this is the one hand-drawn dialog of the Library.

use egui::{Rect, Ui};
use egui_pop::core::input::Input;
use egui_pop::core::overlay::{self, Kind as OverlayKind, OverlaySpec};
use egui_pop::core::tooltip::TooltipExt as _;
use egui_pop::core::ui::{Space, TextRole, Tone, UiPop as _};
use egui_pop::core::widgets;
use helios_ui::{Action, Scroll};
use helios_vault::rules::{Match, RULE_FIELDS, RuleKind, Unit, rule_field};
use serde_json::{Value, json};

use crate::rule_editor::{Node, PERIODS, Path, RuleEditor};
use helios_core::Message;

/// The dialog's named rectangle (tests and pictures).
pub const RECT: &str = "helios.rule_editor";

/// The dialog's words, in the interface's language.
pub struct Words {
    title: String,
    add_condition: String,
    add_group: String,
    remove: String,
    up: String,
    down: String,
    undo: String,
    redo: String,
    cancel: String,
    apply: String,
    of_rules: String,
    matches: Vec<(Match, String)>,
    fields: Vec<(&'static str, String)>,
    ops: Vec<(&'static str, String)>,
    periods: Vec<(&'static str, String)>,
    units: Vec<(Unit, String)>,
    and: String,
    /// Screen reader names of the boxes that have no word beside them.
    match_name: String,
    field_name: String,
    op_name: String,
    value_name: String,
    high_name: String,
    unit_name: String,
}

impl Words {
    pub fn new(t: &dyn Fn(Message) -> String) -> Words {
        let mut ops: Vec<&'static str> = Vec::new();
        for f in RULE_FIELDS {
            for o in f.kind.ops() {
                if !ops.contains(o) {
                    ops.push(o);
                }
            }
        }
        Words {
            title: t(Message::new("gui-rule-title", "The smart album's rules")),
            add_condition: t(Message::new("gui-rule-add-condition", "+ Condition")),
            add_group: t(Message::new("gui-rule-add-group", "+ Group")),
            remove: t(Message::new("gui-rule-remove", "Remove")),
            up: t(Message::new("gui-rule-up", "Move up")),
            down: t(Message::new("gui-rule-down", "Move down")),
            undo: t(Message::new("gui-rule-undo", "Undo")),
            redo: t(Message::new("gui-rule-redo", "Redo")),
            cancel: t(Message::new("gui-rule-cancel", "Cancel")),
            apply: t(Message::new("gui-rule-apply", "Apply")),
            of_rules: t(Message::new("gui-rule-of-rules", "of these rules")),
            matches: vec![
                (Match::All, t(Message::new("gui-rule-match-all", "All"))),
                (Match::Any, t(Message::new("gui-rule-match-any", "Any"))),
                (Match::None, t(Message::new("gui-rule-match-none", "None"))),
            ],
            fields: RULE_FIELDS
                .iter()
                .map(|f| (f.name, t(field_message(f.name))))
                .collect(),
            ops: ops.into_iter().map(|o| (o, t(op_message(o)))).collect(),
            periods: vec![
                ("today", t(Message::new("gui-rule-period-today", "today"))),
                (
                    "this_year",
                    t(Message::new("gui-rule-period-this-year", "this year")),
                ),
                ("last", t(Message::new("gui-rule-period-last", "the last"))),
            ],
            units: vec![
                (Unit::Days, t(Message::new("gui-rule-unit-days", "days"))),
                (Unit::Weeks, t(Message::new("gui-rule-unit-weeks", "weeks"))),
                (
                    Unit::Months,
                    t(Message::new("gui-rule-unit-months", "months")),
                ),
            ],
            and: t(Message::new("gui-rule-and", "and")),
            match_name: t(Message::new("gui-rule-match-name", "Rules to match")),
            field_name: t(Message::new("gui-rule-field-name", "Field")),
            op_name: t(Message::new("gui-rule-op-name", "Comparison")),
            value_name: t(Message::new("gui-rule-value-name", "Value")),
            high_name: t(Message::new("gui-rule-high-name", "Second value")),
            unit_name: t(Message::new("gui-rule-unit-name", "Unit")),
        }
    }

    /// Names `r` for screen readers (and tests) when no word beside it does.
    fn name(ui: &egui::Ui, r: &egui::Response, name: &str) {
        ui.ctx()
            .accesskit_node_builder(r.id, |node| node.set_label(name));
    }

    fn of<'a, K: PartialEq>(list: &'a [(K, String)], key: &K) -> &'a str {
        list.iter()
            .find(|(k, _)| k == key)
            .map_or("", |(_, w)| w.as_str())
    }
}

/// Draws the dialog over `ctx`'s window; answers the edit asked (the
/// arguments of `library.collection.rules.edit`), if any, and where the
/// dialog is. `albums`: the other albums, by id and name.
pub fn show(
    ctx: &egui::Context,
    editor: &RuleEditor,
    albums: &[(String, String)],
    words: &Words,
) -> (Option<Value>, Rect) {
    let mut asked = None;
    let faults = editor.faults();
    let title = format!("{} · {}", words.title, editor.name);
    // Escape or a click beside it cancels, as Cancel does.
    let mut spec = OverlaySpec::new(RECT, &title, OverlayKind::Modal).width(DIALOG_WIDTH);
    spec.dismiss_on_outside = true;
    let mut open = true;
    overlay::show(ctx, &spec, &mut open, None, |ui| {
        ui.text_role(TextRole::Title, title.clone());
        ui.space_role(Space::Inline);
        helios_ui::scroll(ui, "helios.rule_editor.tree", Scroll::Dialog, |ui| {
            let mut path = Vec::new();
            node(
                ui,
                &editor.tree,
                &mut path,
                0,
                &faults,
                albums,
                words,
                &mut asked,
            );
        });
        ui.space_role(Space::Group);
        ui.divider();
        ui.horizontal(|ui| {
            let button = |ui: &mut Ui, enabled: bool, kind: Action, word: &str| {
                ui.add_enabled_ui(enabled, |ui| helios_ui::action(ui, kind, word))
                    .inner
                    .clicked()
            };
            if button(ui, editor.can_undo(), Action::Secondary, &words.undo) {
                asked = Some(json!({ "edit": "undo" }));
            }
            if button(ui, editor.can_redo(), Action::Secondary, &words.redo) {
                asked = Some(json!({ "edit": "redo" }));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, faults.is_empty(), Action::Primary, &words.apply) {
                    asked = Some(json!({ "edit": "apply" }));
                }
                if button(ui, true, Action::Secondary, &words.cancel) {
                    asked = Some(json!({ "edit": "cancel" }));
                }
            });
        });
    });
    if !open && asked.is_none() {
        asked = Some(json!({ "edit": "cancel" }));
    }
    let rect = overlay::rect(ctx, spec.id).unwrap_or(Rect::NOTHING);
    (asked, rect)
}

/// The tree's rows drawn in `ui`, without the dialog around them: the
/// search's Advanced rows (`docs/design/library-search.md`, G5). The same
/// boxes as the dialog; answers the edit asked, as [`show`] does. With
/// `focus`, the first row's field box takes the keyboard (Ctrl+Shift+F).
pub fn rows(
    ui: &mut Ui,
    editor: &RuleEditor,
    albums: &[(String, String)],
    words: &Words,
    focus: bool,
) -> Option<Value> {
    let mut asked = None;
    let faults = editor.faults();
    if focus {
        ui.data_mut(|d| d.insert_temp(focus_first_id(), true));
    }
    node(
        ui,
        &editor.tree,
        &mut Vec::new(),
        0,
        &faults,
        albums,
        words,
        &mut asked,
    );
    // No row to take it (no rule yet): the wish ends with the pass.
    ui.data_mut(|d| d.remove_temp::<bool>(focus_first_id()));
    asked
}

/// Where [`rows`] leaves the wish that the first row's field box take
/// the keyboard, until a row is drawn.
fn focus_first_id() -> egui::Id {
    egui::Id::unique("helios.rules.focus-first")
}

/// How wide the dialog is, in points.
const DIALOG_WIDTH: f32 = 560.0;

/// A drop-down named `name` for screen readers, showing `shown`, one row
/// per option (its key and its word): egui-pop's combo box and rows.
/// The key picked, when it is not `current`.
fn pick<'a, K: PartialEq>(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    shown: &str,
    name: &str,
    current: &K,
    options: impl IntoIterator<Item = (&'a K, &'a str)>,
    focus: bool,
) -> Option<&'a K> {
    let mut picked = None;
    let r = ui
        .combo_box(widgets::combo_box(salt).selected_text(shown), |ui| {
            for (k, w) in options {
                if widgets::selectable_label(ui, k == current, w).clicked() && k != current {
                    picked = Some(k);
                }
            }
        })
        .response;
    Words::name(ui, &r, name);
    if focus {
        r.request_focus();
    }
    picked
}

#[allow(clippy::too_many_arguments)]
fn node(
    ui: &mut egui::Ui,
    n: &Node,
    path: &mut Path,
    siblings: usize,
    faults: &[(Path, String)],
    albums: &[(String, String)],
    words: &Words,
    asked: &mut Option<Value>,
) {
    let at = json!(path);
    match n {
        Node::Group { mode, rules } => {
            ui.card(Tone::Neutral, |ui| {
                ui.horizontal(|ui| {
                    let options = words.matches.iter().map(|(m, w)| (m, w.as_str()));
                    let shown = Words::of(&words.matches, mode);
                    let salt = ("rule-match", path.clone());
                    if let Some(m) = pick(ui, salt, shown, &words.match_name, mode, options, false)
                    {
                        *asked = Some(json!({ "edit": "match", "at": at, "match": m.name() }));
                    }
                    ui.text_role(TextRole::Body, &words.of_rules);
                    if small(ui, true, &words.add_condition).clicked() {
                        *asked = Some(json!({ "edit": "add_condition", "at": at }));
                    }
                    if path.len() + 2 <= helios_vault::rules::MAX_DEPTH
                        && small(ui, true, &words.add_group).clicked()
                    {
                        *asked = Some(json!({ "edit": "add_group", "at": at }));
                    }
                    if !path.is_empty() {
                        moves(ui, path, siblings, words, asked);
                    }
                });
                ui.indent(("rules", path.clone()), |ui| {
                    for (i, r) in rules.iter().enumerate() {
                        path.push(i);
                        node(ui, r, path, rules.len(), faults, albums, words, asked);
                        path.pop();
                    }
                });
            });
        }
        Node::Condition(d) => {
            let id = |what: &'static str| (what, path.clone());
            ui.horizontal_wrapped(|ui| {
                let field_word = words
                    .fields
                    .iter()
                    .find(|(f, _)| *f == d.field)
                    .map_or(d.field.as_str(), |(_, w)| w.as_str());
                let options = words.fields.iter().map(|(f, w)| (f, w.as_str()));
                let current = d.field.as_str();
                let focus = ui
                    .data_mut(|d| d.remove_temp::<bool>(focus_first_id()))
                    .is_some();
                if let Some(f) = pick(
                    ui,
                    id("field"),
                    field_word,
                    &words.field_name,
                    &current,
                    options,
                    focus,
                ) {
                    *asked = Some(json!({ "edit": "field", "at": at, "field": f }));
                }
                let kind = rule_field(&d.field).ok().map(|f| f.kind);
                if let Some(kind) = kind {
                    let op_word = words
                        .ops
                        .iter()
                        .find(|(o, _)| *o == d.op)
                        .map_or(d.op.as_str(), |(_, w)| w.as_str());
                    let options = kind.ops().iter().map(|o| {
                        let w = words
                            .ops
                            .iter()
                            .find(|(k, _)| k == o)
                            .map_or(*o, |(_, w)| w.as_str());
                        (o, w)
                    });
                    let current = d.op.as_str();
                    if let Some(o) = pick(
                        ui,
                        id("op"),
                        op_word,
                        &words.op_name,
                        &current,
                        options,
                        false,
                    ) {
                        *asked = Some(json!({ "edit": "op", "at": at, "op": o }));
                    }
                    value(ui, kind, d, &at, path, albums, words, asked);
                }
                moves(ui, path, siblings, words, asked);
            });
            if let Some((_, why)) = faults.iter().find(|(p, _)| p == path) {
                ui.text_tone(Tone::Danger, why.as_str());
            }
        }
    }
}

/// The value's boxes by the field's kind.
#[allow(clippy::too_many_arguments)]
fn value(
    ui: &mut egui::Ui,
    kind: RuleKind,
    d: &crate::rule_editor::Draft,
    at: &Value,
    path: &[usize],
    albums: &[(String, String)],
    words: &Words,
    asked: &mut Option<Value>,
) {
    let id = |what: &'static str| (what, path.to_vec());
    let typed = |ui: &mut egui::Ui, text: &str, key: &str, width: f32| -> Option<Value> {
        let mut s = text.to_string();
        let name = if key == "high" {
            &words.high_name
        } else {
            &words.value_name
        };
        let out = Input::text(&mut s, name)
            .id_salt(("rule-typed", key.to_string(), path.to_vec()))
            .width(width)
            .show(ui);
        out.changed.then(|| {
            let mut args = json!({ "edit": "value", "at": at, "value": d.value });
            args[key] = json!(s);
            args
        })
    };
    let choose = |ui: &mut egui::Ui, salt: (&'static str, Path), choices: &[(String, String)]| {
        let shown = choices
            .iter()
            .find(|(v, _)| *v == d.value)
            .map_or(d.value.as_str(), |(_, w)| w.as_str());
        let options = choices.iter().map(|(v, w)| (v, w.as_str()));
        pick(ui, salt, shown, &words.value_name, &d.value, options, false)
            .map(|v| json!({ "edit": "value", "at": at, "value": v }))
    };
    let mut ask = |a: Option<Value>| {
        if a.is_some() {
            *asked = a;
        }
    };
    match (kind, d.op.as_str()) {
        (RuleKind::Date | RuleKind::Imported, "within") => {
            let periods: Vec<(String, String)> = PERIODS
                .iter()
                .map(|p| (p.to_string(), Words::of(&words.periods, p).to_string()))
                .collect();
            ask(choose(ui, id("period"), &periods));
            if d.value == "last" {
                ask(typed(ui, &d.high, "high", 48.0));
                let unit = Unit::parse(&d.unit).unwrap_or(Unit::Days);
                let options = words.units.iter().map(|(u, w)| (u, w.as_str()));
                let shown = Words::of(&words.units, &unit);
                if let Some(u) = pick(
                    ui,
                    id("unit"),
                    shown,
                    &words.unit_name,
                    &unit,
                    options,
                    false,
                ) {
                    ask(Some(json!({
                        "edit": "value", "at": at, "value": d.value, "unit": u.name(),
                    })));
                }
            }
        }
        (RuleKind::Number | RuleKind::Date | RuleKind::Imported, "between") => {
            ask(typed(ui, &d.value, "value", 96.0));
            ui.text_role(TextRole::Body, &words.and);
            ask(typed(ui, &d.high, "high", 96.0));
        }
        // A flag, a presence, a backup or a culling state: one of a list.
        (k, _) if k.values().is_some() => {
            let values: Vec<(String, String)> = k
                .values()
                .unwrap_or_default()
                .iter()
                .map(|v| (v.to_string(), v.to_string()))
                .collect();
            ask(choose(ui, id("flag"), &values));
        }
        (RuleKind::Set, _) if d.field == "label" => {
            let labels: Vec<(String, String)> = crate::smart_album::SmartForm::labels()
                .into_iter()
                .skip(1)
                .map(|l| (l.to_string(), l.to_string()))
                .collect();
            ask(choose(ui, id("label"), &labels));
        }
        (RuleKind::Album, _) if !albums.is_empty() => {
            ask(choose(ui, id("album"), albums));
        }
        _ => ask(typed(ui, &d.value, "value", 160.0)),
    }
}

/// A rule's move and remove buttons.
fn moves(
    ui: &mut egui::Ui,
    path: &[usize],
    siblings: usize,
    words: &Words,
    asked: &mut Option<Value>,
) {
    let Some((&i, _)) = path.split_last() else {
        return;
    };
    let at = json!(path);
    if small(ui, i > 0, "↑").tip(&words.up).clicked() {
        *asked = Some(json!({ "edit": "move", "at": at, "to": i - 1 }));
    }
    if small(ui, i + 1 < siblings, "↓").tip(&words.down).clicked() {
        *asked = Some(json!({ "edit": "move", "at": at, "to": i + 1 }));
    }
    if small(ui, true, "×").tip(&words.remove).clicked() {
        *asked = Some(json!({ "edit": "remove", "at": at }));
    }
}

/// A small button in a rule's row, greyed when not `enabled`: helios-ui's
/// row action.
fn small(ui: &mut Ui, enabled: bool, label: &str) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        helios_ui::row_action(ui, Action::Secondary, label)
    })
    .inner
}

/// A rule field's name in words (`camera`, `capture date`).
pub(crate) fn field_message(name: &str) -> Message {
    match name {
        "rating" => Message::new("gui-rule-field-rating", "rating"),
        "label" => Message::new("gui-rule-field-label", "colour label"),
        "tag" => Message::new("gui-rule-field-tag", "tag"),
        "flag" => Message::new("gui-rule-field-flag", "flag"),
        "folder" => Message::new("gui-rule-field-folder", "folder"),
        "album" => Message::new("gui-rule-field-album", "album"),
        "captured" => Message::new("gui-rule-field-captured", "capture date"),
        "import" => Message::new("gui-rule-field-import", "import session"),
        "imported" => Message::new("gui-rule-field-imported", "import date"),
        "camera" => Message::new("gui-rule-field-camera", "camera"),
        "make" => Message::new("gui-rule-field-make", "camera make"),
        "lens" => Message::new("gui-rule-field-lens", "lens"),
        "file_type" => Message::new("gui-rule-field-file-type", "file type"),
        "title" => Message::new("gui-rule-field-title", "title"),
        "description" => Message::new("gui-rule-field-description", "description"),
        "iso" => Message::new("gui-rule-field-iso", "ISO"),
        "focal_length" => Message::new("gui-rule-field-focal-length", "focal length"),
        "aperture" => Message::new("gui-rule-field-aperture", "aperture"),
        "shutter" => Message::new("gui-rule-field-shutter", "shutter speed"),
        "focus" => Message::new("gui-rule-field-focus", "sharpness (focus)"),
        "clip_white" => Message::new("gui-rule-field-clip-white", "blown highlights (share)"),
        "clip_black" => Message::new("gui-rule-field-clip-black", "crushed shadows (share)"),
        "presence" => Message::new("gui-rule-field-presence", "presence"),
        "backup" => Message::new("gui-rule-field-backup", "backup"),
        "cull" => Message::new("gui-rule-field-cull", "culling state"),
        "text" => Message::new("gui-rule-field-text", "name or tag"),
        "keyword" => Message::new("gui-rule-field-keyword", "keyword"),
        other => Message::plain(other),
    }
}

/// A rule operator in words (`is at least`).
pub(crate) fn op_message(name: &str) -> Message {
    match name {
        "=" => Message::new("gui-rule-op-eq", "is"),
        "!=" => Message::new("gui-rule-op-ne", "is not"),
        "<" => Message::new("gui-rule-op-lt", "is below"),
        "<=" => Message::new("gui-rule-op-le", "is at most"),
        ">" => Message::new("gui-rule-op-gt", "is above"),
        ">=" => Message::new("gui-rule-op-ge", "is at least"),
        "between" => Message::new("gui-rule-op-between", "is between"),
        "within" => Message::new("gui-rule-op-within", "is in"),
        "has" => Message::new("gui-rule-op-has", "has"),
        "lacks" => Message::new("gui-rule-op-lacks", "lacks"),
        "is" => Message::new("gui-rule-op-is", "is"),
        "is_not" => Message::new("gui-rule-op-is-not", "is not"),
        "contains" => Message::new("gui-rule-op-contains", "contains"),
        "in" => Message::new("gui-rule-op-in", "is in"),
        "not_in" => Message::new("gui-rule-op-not-in", "is not in"),
        other => Message::plain(other),
    }
}
