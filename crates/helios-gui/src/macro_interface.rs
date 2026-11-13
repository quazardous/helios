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
//! - **Semantic sliders**: "New semantic slider" adds a number of the
//!   macro's own that moves several inner numbers, each along a
//!   piecewise-linear curve (`drives`): its row edits the range, each
//!   driven parameter and its curve's points, drawn beside them.
//!
//! Every change is checked as the step would be (a dry run of
//! `macro_interfaced`): the reason shows under the lists as one edits;
//! the step refuses an interface that does not fit, with that reason. Accepting runs `macro.interface`
//! with the list as `exposed`, so an agent answers the dialog as a form
//! with that same list.

use egui::{RichText, Ui};
use egui_pop::api::dialogs::CustomPass;
use egui_pop::core::slider::{RangeSlider, Slider};
use egui_pop::core::ui::UiPop as _;
use helios_core::{Message, ParamKind, Value};
use helios_template::format::{Drive, Exposed};
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
            .any(|x| !x.is_driven() && x.node == node && x.param == param)
    }

    /// The inner numbers, `(node, param, value)`: what a semantic slider
    /// can drive.
    fn numbers(&self) -> Vec<(String, String, f64)> {
        self.inner
            .iter()
            .filter_map(|p| {
                #[allow(clippy::cast_precision_loss)]
                let v = match p.value {
                    Value::Float(v) => v,
                    Value::Int(v) => v as f64,
                    _ => return None,
                };
                Some((p.node.clone(), p.spec.name.clone(), v))
            })
            .collect()
    }

    /// A new semantic slider, driving the first inner number not driven
    /// yet, flat at its value: valid at once, then shaped by hand.
    fn new_slider(&mut self) {
        let driven: Vec<(String, String)> = self
            .exposed
            .iter()
            .flat_map(|x| x.drives.iter().map(|d| (d.node.clone(), d.param.clone())))
            .collect();
        let numbers = self.numbers();
        let Some((node, param, v)) = numbers
            .iter()
            .find(|(n, p, _)| !driven.contains(&(n.clone(), p.clone())) && !self.is_exposed(n, p))
            .cloned()
        else {
            return;
        };
        let taken = |n: &str| self.exposed.iter().any(|x| x.name == n);
        let name = (1..=self.exposed.len() + 1)
            .map(|i| {
                if i == 1 {
                    "slider".to_string()
                } else {
                    format!("slider_{i}")
                }
            })
            .find(|n| !taken(n))
            .expect("a free name");
        self.exposed.push(Exposed {
            name,
            min: Some(0.0),
            max: Some(1.0),
            drives: vec![Drive {
                node,
                param,
                curve: vec![[0.0, v], [1.0, v]],
            }],
            ..Exposed::default()
        });
        self.selected = Some(self.exposed.len() - 1);
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
            cols[0].scroll_area(
                egui::ScrollArea::vertical()
                    .id_salt("helios.macro.iface.inner")
                    .max_height(height),
                |ui| self.inner_ui(ui),
            );
            cols[1].strong(tr(
                &cols[1],
                Message::new("gui-macro-iface-shown", "Shown by the macro"),
            ));
            cols[1].scroll_area(
                egui::ScrollArea::vertical()
                    .id_salt("helios.macro.iface.shown")
                    .max_height(height),
                |ui| self.shown_ui(ui),
            );
        });
        ui.separator();
        ui.scroll_area(
            egui::ScrollArea::vertical().id_salt("helios.macro.iface.row"),
            |ui| self.row_ui(ui),
        );
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
                let inner = x
                    .inner()
                    .iter()
                    .map(|(n, p)| format!("{n}.{p}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                if x.is_driven() {
                    text = format!("~ {text}");
                }
                if ui
                    .selectable_label(self.selected == Some(i), text)
                    .on_hover_text(inner)
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
        if !self.numbers().is_empty()
            && ui
                .button(tr(
                    ui,
                    Message::new("gui-macro-iface-new-slider", "New semantic slider"),
                ))
                .on_hover_text(tr(
                    ui,
                    Message::new(
                        "gui-macro-iface-new-slider-hint",
                        "One slider that moves several inner numbers, each along its curve",
                    ),
                ))
                .clicked()
        {
            self.new_slider();
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
        if self.exposed[i].is_driven() {
            let numbers = self.numbers();
            let x = &mut self.exposed[i];
            common_ui(ui, x);
            driven_ui(ui, x, &numbers);
            return;
        }
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
        common_ui(ui, x);
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

/// Its label, description, unit and group.
fn common_ui(ui: &mut Ui, x: &mut Exposed) {
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
}

/// A semantic slider's range, then each inner number it drives: which
/// one, its curve's points (`x` the slider, `y` the inner value) and the
/// curve drawn.
fn driven_ui(ui: &mut Ui, x: &mut Exposed, numbers: &[(String, String, f64)]) {
    let (lo, hi) = x
        .drives
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), d| {
            (
                d.curve.first().map_or(a, |p| a.min(p[0])),
                d.curve.last().map_or(b, |p| b.max(p[0])),
            )
        });
    let mut min = x.min.unwrap_or(lo);
    let mut max = x.max.unwrap_or(hi);
    ui.horizontal(|ui| {
        ui.label(tr(ui, Message::new("gui-macro-iface-range", "Range")));
        ui.add(egui::DragValue::new(&mut min).speed(0.01));
        ui.add(egui::DragValue::new(&mut max).speed(0.01));
    });
    x.min = Some(min);
    x.max = Some(max);
    let point_hint = tr(
        ui,
        Message::new(
            "gui-macro-iface-point",
            "x: the slider's value; y: the inner value there",
        ),
    );
    let mut remove_drive: Option<usize> = None;
    let many = x.drives.len() > 1;
    for (k, d) in x.drives.iter_mut().enumerate() {
        ui.separator();
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(("helios.macro.iface.drive", k))
                .selected_text(format!("{}.{}", d.node, d.param))
                .show_ui(ui, |ui| {
                    for (n, p, _) in numbers {
                        let on = d.node == *n && d.param == *p;
                        if ui.selectable_label(on, format!("{n}.{p}")).clicked() {
                            d.node.clone_from(n);
                            d.param.clone_from(p);
                        }
                    }
                });
            if many
                && ui
                    .small_button("×")
                    .on_hover_text(tr(
                        ui,
                        Message::new("gui-macro-iface-drive-remove", "Stop driving it"),
                    ))
                    .clicked()
            {
                remove_drive = Some(k);
            }
        });
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                let mut remove_point: Option<usize> = None;
                let len = d.curve.len();
                for (j, [px, py]) in d.curve.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(px).speed(0.01).prefix("x "))
                            .on_hover_text(&point_hint);
                        ui.add(egui::DragValue::new(py).speed(0.01).prefix("y "))
                            .on_hover_text(&point_hint);
                        if len > 2 && ui.small_button("×").clicked() {
                            remove_point = Some(j);
                        }
                    });
                }
                if let Some(j) = remove_point {
                    d.curve.remove(j);
                }
                if d.curve.len() < helios_core::curve::MAX_POINTS
                    && ui
                        .small_button(tr(
                            ui,
                            Message::new("gui-macro-iface-point-add", "Add a point"),
                        ))
                        .clicked()
                {
                    let [x0, y0] = d.curve[d.curve.len() - 1];
                    d.curve.push([x0 + 0.5, y0]);
                }
            });
            curve_ui(ui, &d.curve);
        });
    }
    if let Some(k) = remove_drive {
        x.drives.remove(k);
    }
    let driven: Vec<(String, String)> = x
        .drives
        .iter()
        .map(|d| (d.node.clone(), d.param.clone()))
        .collect();
    if let Some((n, p, v)) = numbers
        .iter()
        .find(|(n, p, _)| !driven.contains(&(n.clone(), p.clone())))
        && ui
            .button(tr(
                ui,
                Message::new("gui-macro-iface-drive-add", "Drive another parameter"),
            ))
            .clicked()
    {
        x.drives.push(Drive {
            node: n.clone(),
            param: p.clone(),
            curve: vec![[min, *v], [max, *v]],
        });
    }
}

/// A curve drawn in a small frame, its points marked.
fn curve_ui(ui: &mut Ui, curve: &[[f64; 2]]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(160.0, 80.0), egui::Sense::hover());
    let visuals = ui.visuals();
    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        rect,
        2.0,
        visuals.widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    if curve.len() < 2 {
        return;
    }
    let bounds = |i: usize| {
        curve
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
                (a.min(p[i]), b.max(p[i]))
            })
    };
    let (x0, x1) = bounds(0);
    let (y0, y1) = bounds(1);
    #[allow(clippy::cast_possible_truncation)]
    let at = |p: &[f64; 2]| {
        let fx = if x1 > x0 {
            (p[0] - x0) / (x1 - x0)
        } else {
            0.5
        };
        let fy = if y1 > y0 {
            (p[1] - y0) / (y1 - y0)
        } else {
            0.5
        };
        egui::pos2(
            rect.left() + 4.0 + (rect.width() - 8.0) * fx as f32,
            rect.bottom() - 4.0 - (rect.height() - 8.0) * fy as f32,
        )
    };
    let stroke = egui::Stroke::new(1.5, visuals.selection.bg_fill);
    let points: Vec<egui::Pos2> = curve.iter().map(at).collect();
    painter.line(points.clone(), stroke);
    for p in points {
        painter.circle_filled(p, 2.5, visuals.text_color());
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

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::ParamSpec;

    fn inner(node: &str, name: &str, value: Value) -> InnerParam {
        InnerParam {
            node: node.into(),
            spec: ParamSpec::float(name, "", -2.0, 2.0, 0.0),
            hint: None,
            value,
        }
    }

    #[test]
    fn a_new_semantic_slider_drives_the_first_free_number_flat() {
        let params = vec![
            inner("exp", "exposure", Value::Float(0.5)),
            inner("exp", "black", Value::Float(0.1)),
        ];
        let plain = Exposed {
            name: "exposure".into(),
            node: "exp".into(),
            param: "exposure".into(),
            ..Exposed::default()
        };
        let mut e = Editor::new(params, vec![plain], Box::new(|_| Ok(())));
        e.new_slider();
        let x = &e.exposed()[1];
        assert_eq!(x.name, "slider");
        assert!(x.is_driven());
        assert_eq!((x.min, x.max), (Some(0.0), Some(1.0)));
        assert_eq!(x.inner(), [("exp", "black")], "the exposed one is skipped");
        assert_eq!(x.drives[0].curve, [[0.0, 0.1], [1.0, 0.1]]);
        assert_eq!(e.selected, Some(1));
        // Nothing left to drive: no new slider.
        e.new_slider();
        assert_eq!(e.exposed().len(), 2);
        // Not a plain parameter for the inner list's check boxes.
        assert!(!e.is_exposed("", ""));
    }
}
