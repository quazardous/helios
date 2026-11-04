//! The macro interface editor (`macro.interface` without `exposed`, see
//! `docs/design/macros.md`, "The exposed-parameter editor"): the body of
//! an egui-pop custom dialog on a macro node.
//!
//! - **Left: the inner parameters**, by inner node, each with a check box
//!   (exposed or not) and its value in the macro's file. Typing in the
//!   filter narrows the list.
//! - **Right: the interface**, the parameters shown in their order: a
//!   row selects one, the arrows move it, the cross takes it out. Giving
//!   two rows the same name links them (one value sets both).
//! - **Below: the row selected**: its name, label, description, unit and
//!   group; for a number, the range it keeps within the inner one, a soft
//!   range within that and its own default; for a choice, the options it
//!   keeps and its default; for a switch or a text, its default.
//!
//! Every change is checked as the step would be (a dry run of
//! `macro_interfaced`): the reason shows under the lists as one edits;
//! the step refuses an interface that does not fit, with that reason. Accepting runs `macro.interface`
//! with the list as `exposed`, so an agent answers the dialog as a form
//! with that same list.

use egui::{RichText, Ui};
use egui_pop::api::dialogs::CustomPass;
use egui_pop::core::slider::{RangeSlider, Slider};
use helios_core::{Message, ParamKind, Value};
use helios_template::format::Exposed;
use helios_template::macros::InnerParam;
use serde_json::json;

/// Checks an interface: `Ok` when the step would take it.
pub type Check = Box<dyn Fn(&[Exposed]) -> Result<(), String>>;

/// The editor's state, kept by the dialog's body from frame to frame.
pub struct Editor {
    inner: Vec<InnerParam>,
    original: Vec<Exposed>,
    exposed: Vec<Exposed>,
    selected: Option<usize>,
    filter: String,
    check: Check,
    /// The last interface checked and its verdict.
    verdict: Option<(Vec<Exposed>, Result<(), String>)>,
}

fn tr(ui: &Ui, m: Message) -> String {
    crate::i18n::text(ui.ctx(), m)
}

/// A name for `param` of `node` free among `exposed`: the parameter's,
/// else `<node>_<param>`, else numbered.
fn free_name(exposed: &[Exposed], node: &str, param: &str) -> String {
    let taken = |n: &str| exposed.iter().any(|x| x.name == n);
    if !taken(param) {
        return param.to_string();
    }
    let base = format!("{node}_{param}");
    if !taken(&base) {
        return base;
    }
    (2..=exposed.len() + 2)
        .map(|i| format!("{base}_{i}"))
        .find(|n| !taken(n))
        .expect("a free name")
}

/// An optional text field: empty is `None`.
fn text_field(ui: &mut Ui, label: &str, value: &mut Option<String>) {
    ui.horizontal(|ui| {
        let l = ui.label(label);
        let mut text = value.clone().unwrap_or_default();
        if ui
            .text_edit_singleline(&mut text)
            .labelled_by(l.id)
            .changed()
        {
            *value = (!text.trim().is_empty()).then(|| text.trim().to_string());
        }
    });
}

impl Editor {
    pub fn new(inner: Vec<InnerParam>, exposed: Vec<Exposed>, check: Check) -> Self {
        Self {
            inner,
            original: exposed.clone(),
            selected: (!exposed.is_empty()).then_some(0),
            exposed,
            filter: String::new(),
            check,
            verdict: None,
        }
    }

    /// The interface as it stands.
    pub fn exposed(&self) -> &[Exposed] {
        &self.exposed
    }

    fn is_exposed(&self, node: &str, param: &str) -> bool {
        self.exposed
            .iter()
            .any(|x| x.node == node && x.param == param)
    }

    fn inner_of(&self, x: &Exposed) -> Option<&InnerParam> {
        self.inner
            .iter()
            .find(|p| p.node == x.node && p.spec.name == x.param)
    }

    /// One pass of the dialog's body.
    pub fn ui(&mut self, ui: &mut Ui, pass: &mut CustomPass) {
        ui.horizontal(|ui| {
            let l = ui.label(tr(ui, Message::new("gui-macro-iface-filter", "Filter")));
            ui.text_edit_singleline(&mut self.filter).labelled_by(l.id);
        });
        let height = (ui.available_height() * 0.5).max(120.0);
        ui.columns(2, |cols| {
            cols[0].strong(tr(
                &cols[0],
                Message::new("gui-macro-iface-inner", "Inner parameters"),
            ));
            egui::ScrollArea::vertical()
                .id_salt("helios.macro.iface.inner")
                .max_height(height)
                .show(&mut cols[0], |ui| self.inner_ui(ui));
            cols[1].strong(tr(
                &cols[1],
                Message::new("gui-macro-iface-shown", "Shown by the macro"),
            ));
            egui::ScrollArea::vertical()
                .id_salt("helios.macro.iface.shown")
                .max_height(height)
                .show(&mut cols[1], |ui| self.shown_ui(ui));
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("helios.macro.iface.row")
            .show(ui, |ui| self.row_ui(ui));
        // Checked as the step would be, once per change.
        if self
            .verdict
            .as_ref()
            .is_none_or(|(e, _)| *e != self.exposed)
        {
            let v = if self.exposed == self.original {
                Err(tr(
                    ui,
                    Message::new("gui-macro-iface-unchanged", "Nothing changed yet"),
                ))
            } else {
                (self.check)(&self.exposed)
            };
            self.verdict = Some((self.exposed.clone(), v));
        }
        let verdict = self.verdict.as_ref().map(|(_, v)| v.clone());
        if let Some(Err(e)) = &verdict {
            ui.label(RichText::new(e).color(ui.visuals().warn_fg_color));
        }
        // Always possible: an agent answers with a list of its own, and
        // the step refuses what does not fit, with its reason.
        pass.ready(true);
        pass.value("exposed", json!(self.exposed));
        pass.describe(json!({
            "exposed": self.exposed,
            "selected": self.selected,
            "inner": self.inner.iter().map(|p| json!({
                "node": p.node, "param": p.spec.name, "value": p.value,
            })).collect::<Vec<_>>(),
            "problem": verdict.and_then(Result::err),
        }));
    }

    /// The inner parameters, by node, each with its check box.
    fn inner_ui(&mut self, ui: &mut Ui) {
        let filter = self.filter.to_lowercase();
        let mut toggle: Option<(String, String, bool)> = None;
        let mut node: Option<&str> = None;
        for p in &self.inner {
            if !filter.is_empty()
                && !p.spec.name.to_lowercase().contains(&filter)
                && !p.node.to_lowercase().contains(&filter)
            {
                continue;
            }
            if node != Some(p.node.as_str()) {
                ui.weak(&p.node);
                node = Some(&p.node);
            }
            let d = helios_values::Descriptor::from_spec(&p.spec, p.hint.as_ref());
            let mut on = self.is_exposed(&p.node, &p.spec.name);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut on, &p.spec.name).changed() {
                    toggle = Some((p.node.clone(), p.spec.name.clone(), on));
                }
                ui.weak(crate::params::shown(&d, &p.value))
                    .on_hover_text(&p.spec.description);
            });
        }
        match toggle {
            Some((node, param, true)) => {
                let name = free_name(&self.exposed, &node, &param);
                self.exposed.push(Exposed {
                    name,
                    node,
                    param,
                    ..Exposed::default()
                });
                self.selected = Some(self.exposed.len() - 1);
            }
            Some((node, param, false)) => {
                self.exposed
                    .retain(|x| !(x.node == node && x.param == param));
                self.selected = None;
            }
            None => {}
        }
    }

    /// The interface, in its order: select, move, take out.
    fn shown_ui(&mut self, ui: &mut Ui) {
        if self.exposed.is_empty() {
            ui.weak(tr(
                ui,
                Message::new(
                    "gui-macro-iface-none",
                    "Nothing shown: tick inner parameters on the left",
                ),
            ));
        }
        let up = tr(ui, Message::new("gui-macro-iface-up", "Move up"));
        let down = tr(ui, Message::new("gui-macro-iface-down", "Move down"));
        let remove = tr(
            ui,
            Message::new("gui-macro-iface-remove", "Stop showing it"),
        );
        let mut action: Option<(usize, i32)> = None;
        let len = self.exposed.len();
        for (i, x) in self.exposed.iter().enumerate() {
            ui.horizontal(|ui| {
                let mut text = x.name.clone();
                if let Some(g) = &x.group {
                    text = format!("{g} › {text}");
                }
                if ui
                    .selectable_label(self.selected == Some(i), text)
                    .on_hover_text(format!("{}.{}", x.node, x.param))
                    .clicked()
                {
                    self.selected = Some(i);
                }
                if ui
                    .add_enabled(i > 0, egui::Button::new("↑").small())
                    .on_hover_text(&up)
                    .clicked()
                {
                    action = Some((i, -1));
                }
                if ui
                    .add_enabled(i + 1 < len, egui::Button::new("↓").small())
                    .on_hover_text(&down)
                    .clicked()
                {
                    action = Some((i, 1));
                }
                if ui.small_button("×").on_hover_text(&remove).clicked() {
                    action = Some((i, 0));
                }
            });
        }
        match action {
            Some((i, 0)) => {
                self.exposed.remove(i);
                self.selected = None;
            }
            Some((i, d)) => {
                let j = if d < 0 { i - 1 } else { i + 1 };
                self.exposed.swap(i, j);
                self.selected = Some(j);
            }
            None => {}
        }
    }

    /// The row selected: its name and how it is shown.
    fn row_ui(&mut self, ui: &mut Ui) {
        let Some(i) = self.selected.filter(|i| *i < self.exposed.len()) else {
            ui.weak(tr(
                ui,
                Message::new(
                    "gui-macro-iface-pick",
                    "Select a parameter shown to set its name, range and default",
                ),
            ));
            return;
        };
        let Some(inner) = self.inner_of(&self.exposed[i]).cloned() else {
            return;
        };
        let first = self
            .exposed
            .iter()
            .position(|x| x.name == self.exposed[i].name)
            .unwrap_or(i);
        let x = &mut self.exposed[i];
        ui.horizontal(|ui| {
            let l = ui.label(tr(ui, Message::new("gui-macro-iface-name", "Name")));
            ui.text_edit_singleline(&mut x.name)
                .labelled_by(l.id)
                .on_hover_text(tr(
                    ui,
                    Message::new(
                        "gui-macro-iface-name-hint",
                        "The same name on two rows links them: one value sets both",
                    ),
                ));
        });
        if first != i {
            ui.weak(tr(
                ui,
                Message::new(
                    "gui-macro-iface-linked",
                    "Linked to the first row named so: it is shown as that one is",
                ),
            ));
            return;
        }
        text_field(
            ui,
            &tr(ui, Message::new("gui-macro-iface-label", "Label")),
            &mut x.label,
        );
        text_field(
            ui,
            &tr(
                ui,
                Message::new("gui-macro-iface-description", "Description"),
            ),
            &mut x.description,
        );
        text_field(
            ui,
            &tr(ui, Message::new("gui-macro-iface-unit", "Unit")),
            &mut x.unit,
        );
        text_field(
            ui,
            &tr(ui, Message::new("gui-macro-iface-group", "Group")),
            &mut x.group,
        );
        match &inner.spec.kind {
            ParamKind::Float { min, max, .. } => number_ui(ui, x, &inner, *min, *max, false),
            #[allow(clippy::cast_precision_loss)]
            ParamKind::Int { min, max, .. } => {
                number_ui(ui, x, &inner, *min as f64, *max as f64, true);
            }
            ParamKind::Choice { options, .. } => choice_ui(ui, x, &inner, options),
            ParamKind::Bool { .. } => {
                own_default(ui, x, &inner, |ui, v, by| {
                    if let Value::Bool(b) = v {
                        ui.checkbox(b, "").labelled_by(by);
                    }
                });
            }
            ParamKind::Text { .. } => {
                own_default(ui, x, &inner, |ui, v, by| {
                    if let Value::Text(t) = v {
                        ui.text_edit_singleline(t).labelled_by(by);
                    }
                });
            }
            ParamKind::Floats { .. } | ParamKind::Ints { .. } => {}
        }
    }
}

/// "Its own default": a check box, then the default's widget when on.
fn own_default(
    ui: &mut Ui,
    x: &mut Exposed,
    inner: &InnerParam,
    widget: impl FnOnce(&mut Ui, &mut Value, egui::Id),
) {
    let mut on = x.default.is_some();
    let label = tr(
        ui,
        Message::new("gui-macro-iface-default", "Its own default"),
    );
    let check = ui.checkbox(&mut on, label);
    if check.changed() {
        x.default = on.then(|| inner.value.clone());
    }
    // The value's widget is named by the check box.
    if let Some(v) = &mut x.default {
        widget(ui, v, check.id);
    }
}

/// A number's range within the inner one, its soft range and default.
fn number_ui(ui: &mut Ui, x: &mut Exposed, inner: &InnerParam, lo: f64, hi: f64, whole: bool) {
    let round = |v: f64| if whole { v.round() } else { v };
    let mut low = x.min.unwrap_or(lo);
    let mut high = x.max.unwrap_or(hi);
    let range = tr(ui, Message::new("gui-macro-iface-range", "Range"));
    RangeSlider::new(&mut low, &mut high, &range, lo..=hi)
        .default(lo, hi)
        .show(ui);
    let (low, high) = (round(low), round(high));
    x.min = (low > lo).then_some(low);
    x.max = (high < hi).then_some(high);
    let mut soft = x.soft.is_some();
    let label = tr(ui, Message::new("gui-macro-iface-soft", "A soft range"));
    if ui.checkbox(&mut soft, label).changed() {
        x.soft = soft.then_some([low, high]);
    }
    if let Some([s_lo, s_hi]) = &mut x.soft {
        let label = tr(ui, Message::new("gui-macro-iface-soft-range", "Soft range"));
        RangeSlider::new(s_lo, s_hi, &label, low..=high)
            .default(low, high)
            .show(ui);
    }
    own_default(ui, x, inner, |ui, v, _| {
        let label = tr(ui, Message::new("gui-macro-iface-default-value", "Default"));
        match v {
            Value::Float(f) => {
                Slider::new(f, &label, low..=high).show(ui);
            }
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            Value::Int(n) => {
                let mut f = *n as f64;
                Slider::new(&mut f, &label, low..=high).show(ui);
                *n = f.round() as i64;
            }
            _ => {}
        }
    });
}

/// A choice's options kept and its default.
fn choice_ui(ui: &mut Ui, x: &mut Exposed, inner: &InnerParam, options: &[String]) {
    ui.label(tr(
        ui,
        Message::new("gui-macro-iface-options", "Options kept"),
    ));
    let mut kept: Vec<String> = x.options.clone().unwrap_or_else(|| options.to_vec());
    ui.horizontal_wrapped(|ui| {
        for o in options {
            let mut on = kept.contains(o);
            if ui.checkbox(&mut on, o).changed() {
                if on {
                    kept.push(o.clone());
                    kept.sort_by_key(|k| options.iter().position(|p| p == k));
                } else {
                    kept.retain(|k| k != o);
                }
            }
        }
    });
    x.options = (kept.len() < options.len()).then_some(kept.clone());
    own_default(ui, x, inner, |ui, v, _| {
        if let Value::Choice(c) = v {
            ui.horizontal_wrapped(|ui| {
                for o in &kept {
                    ui.selectable_value(c, o.clone(), o);
                }
            });
        }
    });
}
