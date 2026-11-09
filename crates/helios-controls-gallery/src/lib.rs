//! The controls gallery: one card per registered control, generated from
//! the registry (`helios_controls::Registry`). Each card shows the
//! control's representations side by side (each variant in full, the
//! compact field, the HUD chip) over one live value, so its gestures and
//! detents can be tried, and its card data: the `controls.describe`
//! output.
//!
//! The value lives in a value store of the gallery's own, at
//! `node/gallery/<control>`, and every change goes through the value flow
//! (`helios_controls::flow`): a drag previews inside one gesture and
//! commits as one undo step, Escape cancels with no trace.

use std::collections::BTreeMap;

use egui_pop::egui::{self, RichText, Ui};
use helios_controls::flow::{self, Outcome, Steps};
use helios_controls::model::{Representation, ValueSpec};
use helios_controls::render;
use helios_controls::{Control, Registry};
use helios_core::Value;
use helios_shell::commands::Origin;
use helios_values::{Descriptor, Store};

/// The window's title.
pub const TITLE: &str = "Helios controls gallery";

/// The store path of a control's live value.
pub fn path_of(control: &str) -> String {
    format!("node/gallery/{control}")
}

/// The gallery's state: the registry, the store of live values, the
/// gestures open, the last outcome of each control.
pub struct Gallery {
    pub registry: Registry,
    pub store: Store,
    steps: Steps,
    gestures: BTreeMap<String, flow::Gesture>,
    last: BTreeMap<String, String>,
}

impl Default for Gallery {
    fn default() -> Self {
        Self::new(Registry::standard())
    }
}

impl Gallery {
    /// A gallery of a registry's controls, each with its value at its
    /// default.
    pub fn new(registry: Registry) -> Self {
        let mut store = Store::new();
        for c in registry.controls() {
            if let ValueSpec::Number(n) = &c.value {
                let mut d = Descriptor::float(&c.label, n.hard.0, n.hard.1, n.default);
                d.soft = Some(n.soft);
                d.step = n.step;
                d.coarse = n.coarse;
                d.unit.clone_from(&n.unit);
                d.precision = n.precision;
                d.display_scale = n.display_scale;
                store.declare(&path_of(&c.id), d);
            }
        }
        Self {
            registry,
            store,
            steps: Steps::new(),
            gestures: BTreeMap::new(),
            last: BTreeMap::new(),
        }
    }

    /// A control's live value.
    pub fn value(&self, id: &str) -> f64 {
        self.store
            .get(&path_of(id))
            .and_then(|e| e.value.as_f64())
            .unwrap_or(0.0)
    }

    /// Writes a renderer's changes through the value flow.
    pub fn apply(&mut self, control: &Control, changes: &[render::Change]) {
        let path = path_of(&control.id);
        for c in changes {
            let open = self.gestures.remove(&control.id);
            let outcome = if !c.done {
                let g = match open {
                    Some(g) => g,
                    None => match flow::begin(&mut self.store, control, &path, Origin::User) {
                        Ok(g) => g,
                        Err(_) => continue,
                    },
                };
                let _ = g.preview(&mut self.store, c.to, false);
                self.gestures.insert(control.id.clone(), g);
                continue;
            } else if c.cancelled {
                if let Some(g) = open {
                    g.cancel(&mut self.store);
                }
                "cancelled".to_owned()
            } else {
                let result = match open {
                    Some(g) => {
                        let _ = g.preview(&mut self.store, c.to, c.cause == flow::Cause::Typed);
                        Ok(g.commit(&mut self.store, &self.steps, c.cause, "pointer"))
                    }
                    None => flow::apply(
                        &mut self.store,
                        &self.steps,
                        control,
                        &path,
                        flow::OneShot {
                            to: c.to,
                            force: c.cause == flow::Cause::Typed,
                            origin: Origin::User,
                            cause: c.cause,
                            device: "pointer",
                        },
                    ),
                };
                match result {
                    Ok(Outcome::Committed { .. }) => "committed: one undo step".to_owned(),
                    Ok(Outcome::Vetoed) => "vetoed".to_owned(),
                    Ok(Outcome::Unchanged) => "unchanged".to_owned(),
                    Err(e) => e,
                }
            };
            self.last.insert(control.id.clone(), outcome);
        }
    }

    /// Every card, one per registered control.
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.heading(TITLE);
        ui.label(format!(
            "{} controls registered. Drag, scrub (Ctrl: fine, Alt: no detents, Escape: cancel), double click to reset, arrows and the wheel to step.",
            self.registry.controls().count()
        ));
        ui.separator();
        let ids: Vec<String> = self.registry.controls().map(|c| c.id.clone()).collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for id in ids {
                self.card(ui, &id);
                ui.add_space(12.0);
            }
        });
    }

    /// One control's card.
    pub fn card(&mut self, ui: &mut Ui, id: &str) {
        let Some(control) = self.registry.get(id).cloned() else {
            return;
        };
        let description = self.registry.describe(id).unwrap_or_default();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.heading(&control.label);
                ui.weak(format!("`{}`", control.id));
            });
            ui.label(&control.purpose);
            ui.add_space(6.0);
            let mut value = self.value(id);
            let mut changes = Vec::new();
            ui.horizontal_wrapped(|ui| {
                for variant in &control.variants {
                    ui.vertical(|ui| {
                        ui.set_width(240.0);
                        ui.weak(format!("full · {}", variant.id));
                        let out = ui
                            .push_id(("full", &variant.id), |ui| {
                                render::show(
                                    ui,
                                    &control,
                                    Representation::Full,
                                    Some(&variant.id),
                                    &mut value,
                                )
                            })
                            .inner;
                        changes.extend(out.changes);
                    });
                }
                ui.vertical(|ui| {
                    ui.weak("compact");
                    let out = render::show(ui, &control, Representation::Compact, None, &mut value);
                    changes.extend(out.changes);
                });
                ui.vertical(|ui| {
                    ui.weak("simplified · HUD chip");
                    render::show(ui, &control, Representation::Simplified, None, &mut value);
                });
            });
            self.apply(&control, &changes);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let shown = control
                    .value
                    .number()
                    .map_or_else(String::new, |n| n.format(self.value(id)));
                ui.label(format!("Value: {shown}"));
                ui.weak(format!("Undo steps: {}", self.store.undo_steps().len()));
                if let Some(last) = self.last.get(id) {
                    ui.weak(format!("Last: {last}"));
                }
                if ui.button("Undo").clicked() {
                    let _ = self.store.undo(&Origin::User);
                }
                if ui.button("Redo").clicked() {
                    let _ = self.store.redo(&Origin::User);
                }
            });
            ui.collapsing("Card data (controls.describe)", |ui| {
                let text = serde_json::to_string_pretty(&description).unwrap_or_default();
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .id_salt(("describe", id))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).monospace().small());
                    });
            });
        });
    }
}

/// The gallery as an eframe application.
#[derive(Default)]
pub struct App {
    pub gallery: Gallery,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| self.gallery.ui(ui));
    }
}

/// Set so a test can write a value as an agent would.
pub fn set_value(gallery: &mut Gallery, id: &str, v: f64) {
    let _ = gallery.store.set(helios_values::Set::new(
        &path_of(id),
        helios_values::Change::To(Value::Float(v)),
        Origin::User,
    ));
}
