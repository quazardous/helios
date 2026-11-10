//! The bench page: the shelf on the left, the bench in the middle, the
//! live values on the right.
//!
//! - **The shelf** lists the shelf's groups, searchable, each entry with a
//!   small painted preview of its form (its preferred slot's width, its
//!   value type's look). Drag an entry onto the bench to place it; click
//!   it to put it at the end.
//! - **The bench** is the composition's card resolved through `resolve`
//!   and drawn by `render::card`, as a module's card is. While something
//!   is dragged over it, the slot it would take in the dense grid is drawn
//!   as a ghost. Its right edge is a handle that sets its width; the
//!   placements follow (with the slot engine's hysteresis).
//! - **Arrange** turns the bench into its layout mode: each control (and
//!   each case's legend) can be dragged to another place, selected to be
//!   grouped into a case, or removed. Off, every control is live.
//! - **The values** column reads each control's value, the last outcome of
//!   its change, and the undo steps, with undo and redo.
//!
//! - **The context bar** above the bench's tools sets every context key
//!   (task, detail, image flags, tool, device, density) through the
//!   context it shares with `controls.context`, and shows the space the
//!   box's width gives. A change re-resolves the bench at once, unless the
//!   pointer is over it: then it waits until the pointer leaves
//!   (anti-jump).
//! - **The mode switch** shows the composition on a panel, a dock, a
//!   toolbar, a float (each with its slot grid), or as HUD chips.
//! - **The placement and reason pane**, in the values column, says per
//!   control its slot, orientation, representation, visibility, why, and
//!   what decided it.
//! - **Save** and **Load** keep a bench as text in a field (and as a file,
//!   natively: `controls.bench.save`, `controls.bench.load`); a button per
//!   preset of the kit loads it.

use std::collections::{BTreeMap, BTreeSet};

use egui_pop::egui::{
    self, Align2, Color32, DragAndDrop, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, UiBuilder,
    WidgetInfo, pos2, vec2,
};
use serde::Serialize;

use super::{
    Bench, BenchMode, Composition, EditorDrawings, Entry, Origin, Piece, Saved, ShelfEntry, Spot,
};
use crate::context::{Density, Detail, Device, Task, Tier};
use crate::model::{Representation, ValueType};
use crate::render::{self, Drawings, Drawn};
use crate::slot::ChromeKind;
use crate::{CardPlacement, Control};
use serde_json::{Value as Json, json};

/// What is dragged: a shelf entry by name, or a bench item by key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Add(String),
    Move(String),
}

/// The shelf's width, in points.
const SHELF_WIDTH: f32 = 220.0;
/// The values column's width, in points.
const VALUES_WIDTH: f32 = 240.0;
/// The resize handle's width, in points.
const HANDLE: f32 = 10.0;
/// Room under the bench's card, to drop at the end.
const DROP_ROOM: f32 = 32.0;
/// The ghost's key in a trial composition.
const GHOST: &str = "\u{1}drop";

/// The bench page: the bench and how it is being handled.
pub struct BenchPage {
    pub bench: Bench,
    /// The layout mode: items move, group, go; values do not change.
    pub arranging: bool,
    /// Items selected in the layout mode (controls and cases), by key.
    pub selected: BTreeSet<String>,
    pub search: String,
    /// The legend a new case gets.
    pub legend: String,
    /// How each control was drawn in the last frame.
    pub drawn: BTreeMap<String, Drawn>,
    /// Where the pointer last was while something was dragged: the drop's
    /// place on the frame it is released.
    hover: Option<Pos2>,
    /// The bench's zone in the last frame: the pointer over it holds a
    /// context change (anti-jump).
    zone: Option<Rect>,
    /// The last resolution drawn: what the reason pane explains.
    pub last: Option<CardPlacement>,
    /// The file Save writes and Load reads (natively).
    pub file: String,
    /// The bench as text: what Save writes and Load reads.
    pub text: String,
    /// How the last save or load went.
    pub message: Option<String>,
    /// The host's drawings of its integrated controls.
    pub drawings: Box<dyn Drawings>,
}

/// Every input device (`ctl.device`).
pub const DEVICES: [Device; 5] = [
    Device::Pointer,
    Device::Pen,
    Device::Touch,
    Device::Keys,
    Device::Midi,
];

/// A key's value as data (`develop`, `pointer`).
fn name_of<T: Serialize>(v: T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// A drop-down of `all`, labelled `label`; answers the value picked.
fn pick<T: Copy + PartialEq + Serialize>(ui: &mut Ui, label: &str, now: T, all: &[T]) -> Option<T> {
    let mut chosen = None;
    ui.label(label);
    let r = egui::ComboBox::from_id_salt(("bench-pick", label))
        .selected_text(name_of(now))
        .show_ui(ui, |ui| {
            for v in all {
                if ui.selectable_label(*v == now, name_of(*v)).clicked() && *v != now {
                    chosen = Some(*v);
                }
            }
        })
        .response;
    r.widget_info(|| WidgetInfo::labeled(Role::ComboBox, true, label));
    chosen
}

impl BenchPage {
    pub fn new(bench: Bench) -> Self {
        Self {
            bench,
            arranging: false,
            selected: BTreeSet::new(),
            search: String::new(),
            legend: "Case".into(),
            drawn: BTreeMap::new(),
            hover: None,
            zone: None,
            last: None,
            file: "bench.controls.json".into(),
            text: String::new(),
            message: None,
            drawings: Box::new(EditorDrawings),
        }
    }

    /// The page with the host's own drawings.
    #[must_use]
    pub fn with_drawings(mut self, drawings: Box<dyn Drawings>) -> Self {
        self.drawings = drawings;
        self
    }

    /// The whole page: shelf, bench, values.
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                vec2(SHELF_WIDTH, ui.available_height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(SHELF_WIDTH);
                    self.shelf(ui);
                },
            );
            ui.separator();
            let middle =
                (ui.available_width() - VALUES_WIDTH - 16.0).max(self.bench.width + HANDLE);
            ui.allocate_ui_with_layout(
                vec2(middle, ui.available_height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(middle);
                    self.context_bar(ui);
                    ui.separator();
                    self.toolbar(ui);
                    ui.add_space(4.0);
                    self.bench_zone(ui);
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                vec2(VALUES_WIDTH, ui.available_height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(VALUES_WIDTH);
                    self.values(ui);
                },
            );
        });
        self.drag_label(ui);
    }

    /// The shelf: a search, then each group's entries.
    pub fn shelf(&mut self, ui: &mut Ui) {
        ui.heading("Shelf");
        ui.weak("Drag a control onto the bench, or click it to add it at the end.");
        let r = ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search controls")
                .desired_width(f32::INFINITY),
        );
        r.widget_info(|| WidgetInfo::labeled(Role::TextInput, true, "Search the shelf"));
        ui.add_space(4.0);
        let groups: Vec<(String, Vec<ShelfEntry>)> = self
            .bench
            .shelf
            .groups(&self.search)
            .into_iter()
            .map(|(g, list)| (g, list.into_iter().cloned().collect()))
            .collect();
        if groups.is_empty() {
            ui.weak("Nothing on the shelf matches.");
        }
        egui::ScrollArea::vertical()
            .id_salt("bench-shelf")
            .show(ui, |ui| {
                for (group, entries) in groups {
                    ui.add_space(4.0);
                    ui.strong(group);
                    for e in entries {
                        if self.shelf_entry(ui, &e) {
                            let _ = self.bench.add(&e.name, None);
                        }
                    }
                }
            });
    }

    /// One shelf entry: its preview, label and hint. Dragged, it carries
    /// its name; answers whether it was clicked.
    fn shelf_entry(&self, ui: &mut Ui, e: &ShelfEntry) -> bool {
        let (rect, r) =
            ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click_and_drag());
        r.dnd_set_drag_payload(Payload::Add(e.name.clone()));
        let name = format!("Shelf: {}", e.label);
        r.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
        let visuals = ui.style().interact(&r);
        ui.painter().rect(
            rect,
            3.0,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            StrokeKind::Inside,
        );
        let preview = Rect::from_min_size(rect.min + vec2(6.0, 8.0), vec2(56.0, 24.0));
        if let Some(c) = self.bench.registry.get(&e.control) {
            paint_preview(ui, c, preview);
        }
        let text_x = preview.right() + 8.0;
        let painter = ui.painter().with_clip_rect(rect.shrink(2.0));
        painter.text(
            pos2(text_x, rect.top() + 4.0),
            Align2::LEFT_TOP,
            &e.label,
            egui::TextStyle::Body.resolve(ui.style()),
            visuals.text_color(),
        );
        painter.text(
            pos2(text_x, rect.bottom() - 4.0),
            Align2::LEFT_BOTTOM,
            &e.hint,
            egui::TextStyle::Small.resolve(ui.style()),
            ui.visuals().weak_text_color(),
        );
        let r = r.on_hover_text(&e.hint);
        r.clicked()
    }

    /// The context bar: every context key, set through the context the
    /// bench shares with `controls.context`; the space the box gives.
    pub fn context_bar(&mut self, ui: &mut Ui) {
        let k = self.bench.keys();
        let mut args = serde_json::Map::new();
        ui.horizontal_wrapped(|ui| {
            ui.strong("Context");
            ui.separator();
            if let Some(t) = pick(ui, "Task", k.task, &Task::ALL) {
                args.insert("task".into(), json!(t));
            }
            if let Some(d) = pick(ui, "Detail", k.detail, &Detail::ALL) {
                args.insert("detail".into(), json!(d));
            }
            let mut image = k.image;
            ui.label("Image");
            let mut flags = false;
            flags |= ui.checkbox(&mut image.raw, "raw").changed();
            flags |= ui.checkbox(&mut image.edited, "edited").changed();
            flags |= ui.checkbox(&mut image.masked, "masked").changed();
            if flags {
                args.insert("image".into(), json!(image));
            }
            let mut tool = None;
            ui.label("Tool");
            let r = egui::ComboBox::from_id_salt("bench-tool")
                .selected_text(if k.tool.is_empty() { "none" } else { &k.tool })
                .show_ui(ui, |ui| {
                    for t in &self.bench.tools {
                        let name = if t.is_empty() { "none" } else { t };
                        if ui.selectable_label(k.tool == *t, name).clicked() && k.tool != *t {
                            tool = Some(t.clone());
                        }
                    }
                })
                .response;
            r.widget_info(|| WidgetInfo::labeled(Role::ComboBox, true, "Tool"));
            if let Some(t) = tool {
                args.insert("tool".into(), json!(t));
            }
            if let Some(d) = pick(ui, "Device", k.device, &DEVICES) {
                args.insert("device".into(), json!(d));
            }
            if let Some(d) = pick(ui, "Density", k.density, &Density::ALL) {
                args.insert("density".into(), json!(d));
            }
            let space = format!(
                "Space: {} ({:.1} columns)",
                name_of(k.space),
                k.card_box.columns()
            );
            ui.label(&space)
                .on_hover_text("Derived from the bench's width in slot columns at the density, with half a unit of hysteresis");
        });
        if self.last.as_ref().is_some_and(|p| p.pending) {
            ui.weak("A context change waits until the pointer leaves the bench.");
        }
        if !args.is_empty()
            && let Err(e) = self.bench.set_context(&Json::Object(args))
        {
            self.message = Some(e);
        }
    }

    /// The bench's tools: the mode, the layout mode, case tools, clear,
    /// the presets, save and load.
    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Bench");
            for m in BenchMode::ALL {
                if ui
                    .selectable_label(self.bench.mode == m, m.label())
                    .on_hover_text(format!("Show the bench as a {}", m.label()))
                    .clicked()
                {
                    self.bench.set_mode(m);
                }
            }
            ui.separator();
            ui.toggle_value(&mut self.arranging, "Arrange")
                .on_hover_text("Move, group and remove the controls; off, they are live");
            if ui.button("Clear").clicked() {
                self.bench.clear();
                self.selected.clear();
            }
            let presets: Vec<(String, String)> = self
                .bench
                .presets
                .iter()
                .map(|p| (p.name.clone(), p.help.clone()))
                .collect();
            for (name, help) in presets {
                if ui.button(&name).on_hover_text(help).clicked() {
                    if let Err(e) = self.bench.load_preset(&name) {
                        self.message = Some(e);
                    }
                    self.selected.clear();
                }
            }
            let k = self.bench.keys();
            ui.weak(format!(
                "{:.0} pt · {} columns",
                self.bench.width,
                k.card_box.columns(),
            ));
        });
        ui.horizontal(|ui| {
            #[cfg(not(target_arch = "wasm32"))]
            self.files(ui);
            if let Some(m) = &self.message {
                ui.weak(m);
            }
        });
        egui::CollapsingHeader::new("Bench as text")
            .id_salt("bench-as-text")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("Save as text")
                        .on_hover_text("Write the bench as text below")
                        .clicked()
                    {
                        self.save_text();
                    }
                    if ui
                        .button("Load text")
                        .on_hover_text("Load the bench written below")
                        .clicked()
                    {
                        self.load_text();
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("bench-text")
                    .max_height(160.0)
                    .show(ui, |ui| self.text_field(ui));
            });
        if self.bench.mode == BenchMode::Toolbar {
            ui.weak("A toolbar is one line of slots, the rest under More tools; the slot engine has no height limit yet, so this one grows down.");
        }
        if !self.arranging {
            return;
        }
        self.selected.retain(|k| self.bench.composition.holds(k));
        let controls = self.bench.controls_among(&self.selected);
        let case = self
            .selected
            .iter()
            .find(|k| self.bench.composition.case(k).is_some())
            .cloned();
        ui.horizontal_wrapped(|ui| {
            if self.selected.is_empty() {
                ui.weak("Click controls to select them; drag them to move them.");
            }
            if let [one] = controls.as_slice()
                && let Some(p) = self.bench.composition.piece_mut(one)
            {
                let tiers = [Tier::Essential, Tier::Standard, Tier::Expert];
                if let Some(t) = pick(ui, "Tier", p.tier, &tiers) {
                    p.tier = t;
                }
                let pinned = self.bench.layout.parts.get(one).and_then(|l| l.visibility);
                let mut hidden = pinned == Some(crate::resolve::Pinned::Hidden);
                if ui.checkbox(&mut hidden, "Hidden in the layout").changed() {
                    let v = if hidden { json!("hidden") } else { Json::Null };
                    let _ = self.bench.part(one, &json!({"visibility": v}));
                }
            }
            if !controls.is_empty() {
                let r = ui.add(egui::TextEdit::singleline(&mut self.legend).desired_width(120.0));
                r.widget_info(|| WidgetInfo::labeled(Role::TextInput, true, "Case legend"));
                if ui.button("Group into case").clicked()
                    && let Ok(key) = self.bench.group(&controls, &self.legend)
                {
                    self.selected = BTreeSet::from([key]);
                }
            }
            if let Some(key) = case
                && let Some(c) = self.bench.composition.case_mut(&key)
            {
                ui.separator();
                let r = ui.add(egui::TextEdit::singleline(&mut c.legend).desired_width(120.0));
                r.widget_info(|| WidgetInfo::labeled(Role::TextInput, true, "Legend"));
                ui.checkbox(&mut c.separator, "Separator");
                let mut help = c.help.is_some();
                if ui.checkbox(&mut help, "Help").changed() {
                    c.help = help.then(|| "Help text".to_owned());
                }
                if let Some(h) = &mut c.help {
                    let r = ui.add(egui::TextEdit::singleline(h).desired_width(140.0));
                    r.widget_info(|| WidgetInfo::labeled(Role::TextInput, true, "Help text"));
                }
                if ui.button("Ungroup").clicked() {
                    let _ = self.bench.composition.ungroup(&key);
                    self.selected.remove(&key);
                }
            }
        });
    }

    /// Save and Load as a file (natively: a browser has no file system;
    /// the bench as text serves both).
    #[cfg(not(target_arch = "wasm32"))]
    fn files(&mut self, ui: &mut Ui) {
        let r = ui.add(egui::TextEdit::singleline(&mut self.file).desired_width(180.0));
        r.widget_info(|| WidgetInfo::labeled(Role::TextInput, true, "Bench file"));
        if ui.button("Save").clicked() {
            let out = self
                .bench
                .run("controls.bench.save", &json!({"path": self.file}));
            self.message = Some(match out {
                Ok(_) => format!("Saved to {}", self.file),
                Err(e) => e,
            });
        }
        if ui.button("Load").clicked() {
            let out = self
                .bench
                .run("controls.bench.load", &json!({"path": self.file}));
            self.selected.clear();
            self.message = Some(match out {
                Ok(_) => format!("Loaded {}", self.file),
                Err(e) => e,
            });
        }
    }

    /// Writes the bench as text in [`BenchPage::text`].
    pub fn save_text(&mut self) {
        match serde_json::to_string_pretty(&self.bench.save()) {
            Ok(t) => {
                self.text = t;
                self.message = Some("Saved as text".into());
            }
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    /// Loads the bench written in [`BenchPage::text`]; refused, the
    /// bench stays as it was.
    pub fn load_text(&mut self) {
        let out = serde_json::from_str::<Saved>(&self.text)
            .map_err(|e| format!("not a saved bench: {e}"))
            .and_then(|saved| self.bench.load(saved));
        self.selected.clear();
        self.message = Some(match out {
            Ok(()) => "Loaded from text".into(),
            Err(e) => e,
        });
    }

    /// The bench as text, editable: what Save as text writes and Load
    /// text reads.
    pub fn text_field(&mut self, ui: &mut Ui) {
        let r = ui.add(
            egui::TextEdit::multiline(&mut self.text)
                .code_editor()
                .desired_rows(6)
                .desired_width(f32::INFINITY)
                .hint_text("A saved bench, as text"),
        );
        r.widget_info(|| WidgetInfo::labeled(Role::TextInput, true, "Saved bench as text"));
    }

    /// The bench: its card drawn through the managed renderers, the cases'
    /// frames, the drop ghost, the layout mode's handles, and the width
    /// handle.
    pub fn bench_zone(&mut self, ui: &mut Ui) {
        // The hand over the bench holds a context change (anti-jump).
        let pointer = ui.input(|i| i.pointer.hover_pos());
        self.bench.pointer_in = self.zone.zip(pointer).is_some_and(|(z, p)| z.contains(p));
        if !self.bench.mode.boxed() {
            self.last = None;
            self.hud_zone(ui);
            return;
        }
        let (placement, origins) = match self.bench.resolve() {
            Ok(p) => p,
            Err(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
                return;
            }
        };
        let card_height = placement
            .layout
            .as_ref()
            .map_or(0.0, crate::card::CardLayout::height);
        let origin = ui.cursor().min;
        let zone = Rect::from_min_size(
            origin,
            vec2(self.bench.width, card_height.max(72.0) + DROP_ROOM),
        );
        ui.painter().rect(
            zone.expand(2.0),
            4.0,
            ui.visuals().extreme_bg_color,
            ui.visuals().widgets.noninteractive.bg_stroke,
            StrokeKind::Outside,
        );
        if self.bench.composition.entries.is_empty() {
            ui.painter().text(
                zone.center(),
                Align2::CENTER_CENTER,
                "Drag controls here from the shelf",
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().weak_text_color(),
            );
        }
        let at =
            |r: &crate::card::Rect| Rect::from_min_size(origin + vec2(r.x, r.y), vec2(r.w, r.h));
        self.slot_grid(ui, zone, &placement);
        self.case_frames(ui, &placement, &at);

        // The card, as a module's card is drawn.
        let mut values: BTreeMap<String, crate::Value> = self
            .bench
            .composition
            .keys()
            .into_iter()
            .filter_map(|k| Some((k.to_owned(), self.bench.value(k)?)))
            .collect();
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(zone)
                .id_salt("bench-card")
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        if self.arranging {
            // The layout mode: the controls show, the handles take the input.
            child.disable();
        }
        let out = render::card(
            &mut child,
            &self.bench.registry,
            &placement,
            &mut values,
            self.drawings.as_mut(),
        );
        self.drawn = out.drawn;
        if !self.arranging {
            self.bench.apply(out.edits);
        }
        for e in out.events {
            match e {
                render::CardEvent::Reveal(key) => self.bench.reveal(&key),
                render::CardEvent::RevealAll => self.bench.reveal_all(),
                // No marks and no picker on the bench.
                render::CardEvent::Reset(_) | render::CardEvent::Quad(_) => {}
            }
        }
        if self.arranging {
            self.handles(ui, &placement, &origins, &at);
        }
        self.drop(ui, zone, &placement, &origins, &at);
        // The "More" row, below the card, is part of the bench's zone.
        let zone = zone.union(child.min_rect());
        ui.allocate_rect(zone, Sense::hover());
        self.width_handle(ui, zone);
        self.zone = Some(zone);
        self.last = Some(placement);
    }

    /// The surface's slot grid behind the card: a line between slot
    /// columns at the density; on a toolbar, its one line of slots.
    fn slot_grid(&self, ui: &Ui, zone: Rect, placement: &CardPlacement) {
        let keys = &placement.keys;
        let column = keys.density.column();
        let row = keys.density.row();
        let v = ui.visuals();
        let stroke = Stroke::new(
            1.0,
            v.widgets.noninteractive.bg_stroke.color.gamma_multiply(0.6),
        );
        let painter = ui.painter();
        let columns = keys.card_box.columns().floor() as usize;
        for c in 1..=columns {
            let x = zone.left() + column * c as f32;
            if x < zone.right() - 0.5 {
                painter.vline(x, zone.y_range(), stroke);
            }
        }
        if self.bench.mode == BenchMode::Toolbar {
            let line = Rect::from_min_size(zone.min, vec2(zone.width(), row));
            painter.rect_stroke(
                line,
                2.0,
                Stroke::new(1.0, v.selection.stroke.color.gamma_multiply(0.6)),
                StrokeKind::Inside,
            );
        }
    }

    /// The HUD: no slots, each control as its chip (a read-out) when its
    /// variant declares one, else a note that it stays in the panel; and
    /// the gizmos the controls declare on the image.
    fn hud_zone(&mut self, ui: &mut Ui) {
        let forms = self.bench.hud();
        let origin = ui.cursor().min;
        let width = self.bench.width;
        let inner = ui
            .allocate_ui_with_layout(
                vec2(width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(width);
                    ui.weak("The HUD has no slots: a control shows there as its chip.");
                    ui.horizontal_wrapped(|ui| {
                        for f in forms.iter().filter(|f| f.chip) {
                            let text = format!("{} {}", f.label, f.readout);
                            let name = format!("HUD chip: {text}");
                            let r = egui::Frame::new()
                                .fill(ui.visuals().widgets.inactive.weak_bg_fill)
                                .stroke(ui.visuals().widgets.inactive.bg_stroke)
                                .corner_radius(10.0)
                                .inner_margin(egui::Margin::symmetric(8, 3))
                                .show(ui, |ui| ui.label(&text))
                                .response;
                            r.widget_info(|| WidgetInfo::labeled(Role::Label, true, &name));
                        }
                    });
                    for f in forms.iter().filter(|f| !f.chip) {
                        ui.weak(format!(
                            "{}: no HUD chip declared; it stays in the panel",
                            f.label
                        ));
                    }
                    let gizmos: Vec<&str> = forms
                        .iter()
                        .filter(|f| f.gizmo)
                        .map(|f| f.label.as_str())
                        .collect();
                    if gizmos.is_empty() {
                        ui.weak("No control here declares a gizmo on the image.");
                    } else {
                        ui.weak(format!(
                            "Gizmos on the image, drawn by the host, not on the bench yet: {}",
                            gizmos.join(", ")
                        ));
                    }
                },
            )
            .response
            .rect;
        let zone = Rect::from_min_size(origin, vec2(width, inner.height().max(72.0)));
        ui.painter().rect_stroke(
            zone.expand(2.0),
            4.0,
            ui.visuals().widgets.noninteractive.bg_stroke,
            StrokeKind::Outside,
        );
        self.zone = Some(zone);
    }

    /// A frame around each case: the band the slot engine gave its group,
    /// across the card, from its legend to its lowest part.
    fn case_frames(
        &self,
        ui: &Ui,
        placement: &CardPlacement,
        at: &dyn Fn(&crate::card::Rect) -> Rect,
    ) {
        let Some(layout) = &placement.layout else {
            return;
        };
        for g in &layout.groups {
            let Some(r) = &g.rect else { continue };
            let stroke = if self.selected.contains(&g.key) {
                Stroke::new(2.0, ui.visuals().selection.stroke.color)
            } else {
                ui.visuals().widgets.noninteractive.bg_stroke
            };
            ui.painter()
                .rect_stroke(at(r).expand(1.0), 4.0, stroke, StrokeKind::Outside);
        }
    }

    /// The layout mode's handles: each control and each case's legend can
    /// be dragged, clicked to select, and removed.
    fn handles(
        &mut self,
        ui: &mut Ui,
        placement: &CardPlacement,
        origins: &[Origin],
        at: &dyn Fn(&crate::card::Rect) -> Rect,
    ) {
        for (p, o) in placement.items.iter().zip(origins) {
            let Some(r) = p.rect.as_ref().filter(|_| p.visibility.is_shown()) else {
                continue;
            };
            let key = match o {
                Origin::Control { key, .. } => key.clone(),
                Origin::Chrome {
                    case,
                    kind: ChromeKind::Legend,
                } => case.clone(),
                Origin::Chrome { .. } => continue,
            };
            let rect = at(r).shrink(1.0);
            let r = ui.interact(
                rect,
                ui.scope_id().with(("bench-handle", &key)),
                Sense::click_and_drag(),
            );
            r.dnd_set_drag_payload(Payload::Move(key.clone()));
            let name = format!("Move {key}");
            r.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
            let selected = self.selected.contains(&key);
            let v = ui.visuals();
            let fill = if selected {
                v.selection.bg_fill.gamma_multiply(0.35)
            } else if r.hovered() {
                v.widgets.hovered.weak_bg_fill.gamma_multiply(0.5)
            } else {
                Color32::TRANSPARENT
            };
            let stroke = if selected {
                Stroke::new(2.0, v.selection.stroke.color)
            } else {
                Stroke::new(1.0, v.widgets.inactive.bg_stroke.color)
            };
            ui.painter()
                .rect(rect, 2.0, fill, stroke, StrokeKind::Inside);
            if r.clicked() && !self.selected.remove(&key) {
                self.selected.insert(key.clone());
            }
            // Remove: a small cross at the top right.
            let side = rect.height().min(16.0);
            let cross =
                Rect::from_min_size(pos2(rect.right() - side, rect.top()), vec2(side, side));
            let x = ui.interact(
                cross,
                ui.scope_id().with(("bench-remove", &key)),
                Sense::click(),
            );
            let name = format!("Remove {key}");
            x.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
            let colour = if x.hovered() {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().weak_text_color()
            };
            let c = cross.shrink(side * 0.3);
            let painter = ui.painter();
            painter.line_segment([c.left_top(), c.right_bottom()], Stroke::new(1.5, colour));
            painter.line_segment([c.right_top(), c.left_bottom()], Stroke::new(1.5, colour));
            if x.clicked() {
                let _ = self.bench.remove(&key);
                self.selected.remove(&key);
            }
        }
    }

    /// A drag over the bench: the ghost of where it lands; released over
    /// it, the control is added or moved there.
    fn drop(
        &mut self,
        ui: &Ui,
        zone: Rect,
        placement: &CardPlacement,
        origins: &[Origin],
        at: &dyn Fn(&crate::card::Rect) -> Rect,
    ) {
        let Some(payload) = DragAndDrop::payload::<Payload>(ui.ctx()) else {
            self.hover = None;
            return;
        };
        if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) {
            self.hover = Some(pos);
        }
        let Some(pos) = self.hover.filter(|p| zone.contains(*p)) else {
            return;
        };
        let target = target_spot(&self.bench.composition, placement, origins, at, pos);
        let (trial, ghost) = self.trial(&payload, &target);
        if let Some(trial) = trial
            && let Ok(p) = self.bench.resolve_trial(&trial)
        {
            let rects: Vec<Rect> = p
                .items
                .iter()
                .zip(trial.card().1)
                .filter(|(_, o)| match o {
                    Origin::Control { key, spot } => {
                        *key == ghost || spot.case.as_deref() == Some(ghost.as_str())
                    }
                    Origin::Chrome { case, .. } => *case == ghost,
                })
                .filter_map(|(i, _)| i.rect.as_ref().map(at))
                .collect();
            if let Some(first) = rects.first() {
                let r = rects.iter().fold(*first, |a, b| a.union(*b));
                let v = ui.visuals();
                ui.painter().rect(
                    r.shrink(1.0),
                    3.0,
                    v.selection.bg_fill.gamma_multiply(0.4),
                    Stroke::new(2.0, v.selection.stroke.color),
                    StrokeKind::Inside,
                );
            }
        }
        if ui.input(|i| i.pointer.any_released()) {
            match payload.as_ref() {
                Payload::Add(name) => {
                    let _ = self.bench.add(name, Some(target));
                }
                Payload::Move(key) => {
                    let to = self.landing(key, &target);
                    let _ = self.bench.composition.place(key, &to);
                }
            }
            DragAndDrop::clear_payload(ui.ctx());
            self.hover = None;
        }
    }

    /// Where an item dropped at an insertion point lands: a case only at
    /// the top level.
    fn landing(&self, key: &str, target: &Spot) -> Spot {
        let c = &self.bench.composition;
        let target = match (&target.case, c.case(key)) {
            (Some(case), Some(_)) => {
                let i = c.find(case).map_or(0, |s| s.index);
                Spot::top(if case == key { i } else { i + 1 })
            }
            _ => target.clone(),
        };
        c.landing(key, &target)
    }

    /// The composition with the drop applied, and the ghost's key.
    fn trial(&self, payload: &Payload, target: &Spot) -> (Option<Composition>, String) {
        let mut trial = self.bench.composition.clone();
        match payload {
            Payload::Add(name) => {
                let Some(e) = self.bench.shelf.get(name) else {
                    return (None, String::new());
                };
                let piece = Entry::Control(Piece::new(GHOST, &e.control));
                (
                    trial.insert(piece, target).ok().map(|()| trial),
                    GHOST.into(),
                )
            }
            Payload::Move(key) => {
                let to = self.landing(key, target);
                (trial.place(key, &to).ok().map(|()| trial), key.clone())
            }
        }
    }

    /// The handle on the bench's right edge: dragged, it sets the width.
    fn width_handle(&mut self, ui: &mut Ui, zone: Rect) {
        let rect = Rect::from_min_size(
            pos2(zone.right() + 2.0, zone.top()),
            vec2(HANDLE, zone.height()),
        );
        let r = ui.interact(rect, ui.scope_id().with("bench-width"), Sense::drag());
        let name = format!("Bench width, {:.0} points", self.bench.width);
        r.widget_info(|| WidgetInfo::labeled(Role::Splitter, true, &name));
        if r.dragged() {
            self.bench.set_width(self.bench.width + r.drag_delta().x);
        }
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let colour = if r.hovered() || r.dragged() {
            ui.visuals().widgets.hovered.fg_stroke.color
        } else {
            ui.visuals().widgets.noninteractive.fg_stroke.color
        };
        let x = rect.center().x;
        let mid = rect.center().y;
        for dy in [-6.0, 0.0, 6.0] {
            ui.painter().circle_filled(pos2(x, mid + dy), 1.5, colour);
        }
    }

    /// The live values: each control's read-out and last outcome, the
    /// undo steps, undo and redo.
    pub fn values(&mut self, ui: &mut Ui) {
        ui.heading("Values");
        let pieces: Vec<(String, String, Option<String>)> = self
            .bench
            .composition
            .pieces()
            .into_iter()
            .map(|(p, c)| {
                (
                    p.key.clone(),
                    p.control.clone(),
                    c.map(|c| c.legend.clone()),
                )
            })
            .collect();
        if pieces.is_empty() {
            ui.weak("Nothing on the bench.");
        }
        egui::Grid::new("bench-values")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for (key, control, case) in &pieces {
                    let label = self
                        .bench
                        .registry
                        .get(control)
                        .map_or_else(|| key.clone(), |c| c.label.clone());
                    let name = match case {
                        Some(case) => format!("{case} › {label}"),
                        None => label,
                    };
                    ui.label(name).on_hover_text(key);
                    ui.label(format!("{key}: {}", self.bench.readout(key)));
                    ui.end_row();
                    if let Some(last) = self.bench.last.get(key) {
                        ui.label("");
                        ui.weak(last);
                        ui.end_row();
                    }
                }
            });
        ui.add_space(6.0);
        let steps = self.bench.store.undo_steps().len();
        ui.label(format!("Undo steps: {steps}"));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(steps > 0, egui::Button::new("Undo"))
                .clicked()
            {
                self.bench.undo();
            }
            let redo = !self.bench.store.redo_steps().is_empty();
            if ui.add_enabled(redo, egui::Button::new("Redo")).clicked() {
                self.bench.redo();
            }
        });
        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .id_salt("bench-reasons")
            .show(ui, |ui| self.reasons(ui));
    }

    /// The placement and reason pane: per control, its slot, orientation
    /// and representation, its visibility, why, and what decided it.
    pub fn reasons(&mut self, ui: &mut Ui) {
        ui.heading("Placements");
        let Some(placement) = &self.last else {
            for f in self.bench.hud() {
                ui.label(egui::RichText::new(&f.label).strong());
                ui.weak(if f.chip {
                    "HUD: its chip"
                } else {
                    "HUD: no chip; stays in the panel"
                });
            }
            return;
        };
        if let Some(m) = &placement.more {
            ui.weak(format!(
                "{} · {} not applicable",
                m.title(),
                m.inapplicable.len()
            ));
        }
        for e in self.bench.explain(placement) {
            ui.add_space(4.0);
            let mut head = egui::RichText::new(format!("{} · {}", e.label, e.visibility)).strong();
            if e.visibility != "shown" {
                head = head.color(ui.visuals().warn_fg_color);
            }
            ui.label(head).on_hover_text(&e.key);
            if let Some(slot) = &e.slot {
                let orientation = e
                    .orientation
                    .map_or_else(String::new, |o| format!(" · {o}"));
                let rep = e
                    .representation
                    .map_or("", crate::model::representation_name);
                ui.weak(format!("{slot}{orientation} · {rep}"));
            }
            let mut marks = Vec::new();
            if e.changed {
                marks.push("changed");
            }
            if e.emphasised {
                marks.push("emphasised");
            }
            if e.deferred {
                marks.push("deferred");
            }
            if !marks.is_empty() {
                ui.weak(marks.join(" · "));
            }
            ui.add(egui::Label::new(egui::RichText::new(format!("Why: {}", e.why)).small()).wrap());
            ui.add(
                egui::Label::new(egui::RichText::new(format!("By: {}", e.decided_by)).small())
                    .wrap(),
            );
            for n in &e.notes {
                ui.add(egui::Label::new(egui::RichText::new(n).small().weak()).wrap());
            }
        }
    }

    /// While something is dragged, its name by the pointer.
    fn drag_label(&self, ui: &Ui) {
        let Some(payload) = DragAndDrop::payload::<Payload>(ui.ctx()) else {
            return;
        };
        let Some(pos) = ui.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let text = match payload.as_ref() {
            Payload::Add(name) => self
                .bench
                .shelf
                .get(name)
                .map_or_else(|| name.clone(), |e| e.label.clone()),
            Payload::Move(key) => key.clone(),
        };
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            ui.scope_id().with("bench-drag"),
        ));
        let galley = painter.layout_no_wrap(
            text,
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().strong_text_color(),
        );
        let rect = Rect::from_min_size(pos + vec2(12.0, 8.0), galley.size()).expand(4.0);
        painter.rect(
            rect,
            3.0,
            ui.visuals().window_fill,
            ui.visuals().window_stroke,
            StrokeKind::Outside,
        );
        painter.galley(rect.min + vec2(4.0, 4.0), galley, Color32::PLACEHOLDER);
    }
}

/// The insertion point under `pos`: before or after the nearest item, by
/// which half the pointer is in; past the last row, the end.
fn target_spot(
    composition: &Composition,
    placement: &CardPlacement,
    origins: &[Origin],
    at: &dyn Fn(&crate::card::Rect) -> Rect,
    pos: Pos2,
) -> Spot {
    let end = Spot::top(composition.entries.len());
    let shown: Vec<(Rect, &Origin)> = placement
        .items
        .iter()
        .zip(origins)
        .filter(|(p, _)| p.visibility.is_shown())
        .filter_map(|(p, o)| Some((at(p.rect.as_ref()?), o)))
        .collect();
    let bottom = shown
        .iter()
        .map(|(r, _)| r.bottom())
        .fold(f32::MIN, f32::max);
    if shown.is_empty() || pos.y > bottom {
        return end;
    }
    let Some((rect, origin)) = shown.iter().min_by(|a, b| {
        a.0.distance_sq_to_pos(pos)
            .total_cmp(&b.0.distance_sq_to_pos(pos))
    }) else {
        return end;
    };
    let case_index = |case: &str| composition.find(case).map_or(0, |s| s.index);
    match origin {
        Origin::Control { spot, .. } => {
            let after = if rect.width() >= rect.height() {
                pos.x > rect.center().x
            } else {
                pos.y > rect.center().y
            };
            Spot {
                case: spot.case.clone(),
                index: spot.index + usize::from(after),
            }
        }
        Origin::Chrome {
            case,
            kind: ChromeKind::Legend,
        } if pos.y < rect.center().y => Spot::top(case_index(case)),
        Origin::Chrome {
            case,
            kind: ChromeKind::Separator,
        } => Spot::top(case_index(case) + 1),
        Origin::Chrome { case, .. } => Spot::in_case(case, 0),
    }
}

/// A small picture of a control's form: its preferred slot's width (a
/// row is the preview's full width), and its value type's look.
fn paint_preview(ui: &Ui, control: &Control, rect: Rect) {
    let v = ui.visuals();
    let width = control
        .variants
        .first()
        .and_then(|x| x.form(Representation::Full))
        .map_or(12, |f| f.policy.size.width.twelfths().min(12)) as f32;
    let r = Rect::from_min_size(rect.min, vec2(rect.width() * width / 12.0, rect.height()));
    let painter = ui.painter();
    let stroke = Stroke::new(1.0, v.widgets.inactive.fg_stroke.color);
    let accent = v.selection.bg_fill;
    painter.rect(r, 3.0, v.extreme_bg_color, stroke, StrokeKind::Inside);
    match control.value.value_type() {
        ValueType::Number => {
            let fill = Rect::from_min_max(r.min, pos2(r.left() + r.width() * 0.45, r.bottom()));
            painter.rect_filled(fill.shrink(2.0), 2.0, accent);
        }
        ValueType::Bool => {
            let b = Rect::from_center_size(pos2(r.left() + 10.0, r.center().y), vec2(10.0, 10.0));
            painter.rect(b, 2.0, accent, stroke, StrokeKind::Inside);
        }
        ValueType::OneOf => {
            let c = pos2(r.right() - 8.0, r.center().y);
            painter.line_segment([c + vec2(-3.0, -2.0), c + vec2(0.0, 2.0)], stroke);
            painter.line_segment([c + vec2(0.0, 2.0), c + vec2(3.0, -2.0)], stroke);
        }
        ValueType::Points => {
            let s = r.shrink(3.0);
            let pts = [0.0_f32, 0.15, 0.5, 0.85, 1.0]
                .iter()
                .enumerate()
                .map(|(i, y)| {
                    pos2(
                        s.left() + s.width() * i as f32 / 4.0,
                        s.bottom() - s.height() * y,
                    )
                })
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(1.5, accent)));
        }
        ValueType::Colour => {
            painter.circle(r.center(), r.height() * 0.35, accent, stroke);
        }
        _ => {
            painter.text(
                r.left_center() + vec2(4.0, 0.0),
                Align2::LEFT_CENTER,
                "Aa",
                egui::TextStyle::Small.resolve(ui.style()),
                stroke.color,
            );
        }
    }
}
