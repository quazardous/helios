//! The rule editor's dialog ([`crate::rule_editor`]): the tree drawn as
//! nested groups, each condition's field, operator and value boxes by
//! the field's kind, its fault said under it. The dialog changes nothing
//! itself: each edit is asked as the arguments of
//! `library.collection.rules.edit`, which the shell runs.
//!
//! A tree does not fit the declarative forms (flat fields, one command):
//! this is the one hand-drawn dialog of the Library.

use egui::{Rect, RichText};
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
        let field = |name: &str| -> Message {
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
                "clip_white" => {
                    Message::new("gui-rule-field-clip-white", "blown highlights (share)")
                }
                "clip_black" => {
                    Message::new("gui-rule-field-clip-black", "crushed shadows (share)")
                }
                other => Message::plain(other),
            }
        };
        let op = |name: &str| -> Message {
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
        };
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
                .map(|f| (f.name, t(field(f.name))))
                .collect(),
            ops: ops.into_iter().map(|o| (o, t(op(o)))).collect(),
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
    let shown = egui::Modal::new(egui::Id::unique(RECT)).show(ctx, |ui| {
        ui.set_min_width(560.0);
        ui.heading(format!("{} · {}", words.title, editor.name));
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .max_height(ctx.content_rect().height() * 0.6)
            .show(ui, |ui| {
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
        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(editor.can_undo(), egui::Button::new(&words.undo))
                .clicked()
            {
                asked = Some(json!({ "edit": "undo" }));
            }
            if ui
                .add_enabled(editor.can_redo(), egui::Button::new(&words.redo))
                .clicked()
            {
                asked = Some(json!({ "edit": "redo" }));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(faults.is_empty(), egui::Button::new(&words.apply))
                    .clicked()
                {
                    asked = Some(json!({ "edit": "apply" }));
                }
                if ui.button(&words.cancel).clicked() {
                    asked = Some(json!({ "edit": "cancel" }));
                }
            });
        });
    });
    if shown.should_close() && asked.is_none() {
        asked = Some(json!({ "edit": "cancel" }));
    }
    (asked, shown.response.rect)
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
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let r = egui::ComboBox::from_id_salt(("rule-match", path.clone()))
                        .selected_text(Words::of(&words.matches, mode))
                        .show_ui(ui, |ui| {
                            for (m, w) in &words.matches {
                                if ui.selectable_label(m == mode, w).clicked() && m != mode {
                                    *asked = Some(
                                        json!({ "edit": "match", "at": at, "match": m.name() }),
                                    );
                                }
                            }
                        })
                        .response;
                    Words::name(ui, &r, &words.match_name);
                    ui.label(&words.of_rules);
                    if ui.small_button(&words.add_condition).clicked() {
                        *asked = Some(json!({ "edit": "add_condition", "at": at }));
                    }
                    if path.len() + 2 <= helios_vault::rules::MAX_DEPTH
                        && ui.small_button(&words.add_group).clicked()
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
                let field_id = id("field");
                let r = egui::ComboBox::from_id_salt(field_id)
                    .selected_text(field_word)
                    .show_ui(ui, |ui| {
                        for (f, w) in &words.fields {
                            if ui.selectable_label(*f == d.field, w).clicked() && *f != d.field {
                                *asked = Some(json!({ "edit": "field", "at": at, "field": f }));
                            }
                        }
                    })
                    .response;
                Words::name(ui, &r, &words.field_name);
                let kind = rule_field(&d.field).ok().map(|f| f.kind);
                if let Some(kind) = kind {
                    let op_word = words
                        .ops
                        .iter()
                        .find(|(o, _)| *o == d.op)
                        .map_or(d.op.as_str(), |(_, w)| w.as_str());
                    let r = egui::ComboBox::from_id_salt(id("op"))
                        .selected_text(op_word)
                        .show_ui(ui, |ui| {
                            for o in kind.ops() {
                                let w = words
                                    .ops
                                    .iter()
                                    .find(|(k, _)| k == o)
                                    .map_or(*o, |(_, w)| w.as_str());
                                if ui.selectable_label(*o == d.op, w).clicked() && *o != d.op {
                                    *asked = Some(json!({ "edit": "op", "at": at, "op": o }));
                                }
                            }
                        })
                        .response;
                    Words::name(ui, &r, &words.op_name);
                    value(ui, kind, d, &at, path, albums, words, asked);
                }
                moves(ui, path, siblings, words, asked);
            });
            if let Some((_, why)) = faults.iter().find(|(p, _)| p == path) {
                ui.label(
                    RichText::new(why)
                        .small()
                        .color(ui.visuals().error_fg_color),
                );
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
        let r = ui.add(egui::TextEdit::singleline(&mut s).desired_width(width));
        let name = if key == "high" {
            &words.high_name
        } else {
            &words.value_name
        };
        Words::name(ui, &r, name);
        r.changed().then(|| {
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
        let mut out = None;
        let r = egui::ComboBox::from_id_salt(salt)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for (v, w) in choices {
                    if ui.selectable_label(*v == d.value, w).clicked() && *v != d.value {
                        out = Some(json!({ "edit": "value", "at": at, "value": v }));
                    }
                }
            })
            .response;
        Words::name(ui, &r, &words.value_name);
        out
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
                let r = egui::ComboBox::from_id_salt(id("unit"))
                    .selected_text(Words::of(&words.units, &unit))
                    .show_ui(ui, |ui| {
                        for (u, w) in &words.units {
                            if ui.selectable_label(*u == unit, w).clicked() && *u != unit {
                                ask(Some(json!({
                                    "edit": "value", "at": at, "value": d.value, "unit": u.name(),
                                })));
                            }
                        }
                    })
                    .response;
                Words::name(ui, &r, &words.unit_name);
            }
        }
        (RuleKind::Number | RuleKind::Date | RuleKind::Imported, "between") => {
            ask(typed(ui, &d.value, "value", 96.0));
            ui.label(&words.and);
            ask(typed(ui, &d.high, "high", 96.0));
        }
        (RuleKind::Flag, _) => {
            let flags: Vec<(String, String)> = helios_vault::fields::FLAGS
                .iter()
                .map(|f| (f.to_string(), f.to_string()))
                .collect();
            ask(choose(ui, id("flag"), &flags));
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
    if ui
        .add_enabled(i > 0, egui::Button::new("↑").small())
        .on_hover_text(&words.up)
        .clicked()
    {
        *asked = Some(json!({ "edit": "move", "at": at, "to": i - 1 }));
    }
    if ui
        .add_enabled(i + 1 < siblings, egui::Button::new("↓").small())
        .on_hover_text(&words.down)
        .clicked()
    {
        *asked = Some(json!({ "edit": "move", "at": at, "to": i + 1 }));
    }
    if ui.small_button("×").on_hover_text(&words.remove).clicked() {
        *asked = Some(json!({ "edit": "remove", "at": at }));
    }
}
