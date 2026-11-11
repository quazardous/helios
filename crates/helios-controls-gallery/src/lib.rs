//! The controls gallery: one card per registered control, generated from
//! the registry (`helios_controls::Registry`), and a card of implicit
//! controls. Every part is drawn by the managed renderers of
//! `helios_controls::render`, from a resolution of its card
//! (`helios_controls::resolve`): placements in, parts drawn at their
//! rects, folded parts under "More (n)".
//!
//! - A control's card shows its [`VIEWS`]: each representation (full,
//!   compact, simplified), orientation (horizontal, vertical) and variant
//!   in a box of its own width, over one live value, so its gestures and
//!   detents can be tried; then its card data, the `controls.describe`
//!   output.
//! - The implicit card is a node of the gallery's store
//!   (`node/implicit/...`), one implicit control per parameter (a number,
//!   a toggle, a choice, a curve, a colour wheel, a text), resolved as
//!   the `controls.*` commands resolve it, in a wide box (the curve and
//!   the wheel drawn by egui-pop's editors, their own drawing) and in a
//!   narrow one where every part is compact (the curve and the wheel
//!   fall back to their value type's rows). Its "More (n)" row reveals a
//!   part through `controls.reveal`.
//!
//! Values live in a value store of the gallery's own, and every change
//! goes through the value flow (`helios_controls::flow`): a drag
//! previews inside one gesture and commits as one undo step, Escape
//! cancels with no trace.
//!
//! The gallery's second page is the **bench** ([`bench`], [`bench_ui`]):
//! controls dragged from a shelf onto a card laid out by the slot engine,
//! composed into cases, and tried live; the `controls.bench.*` commands
//! build it for agents and tests ([`Gallery::run_command`]).

pub mod bench;
pub mod bench_ui;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui_hud::commands::{Shared, SharedContexts};
use egui_pop::egui::{self, RichText, Ui};
use helios_controls::card::{Card, CardItem};
use helios_controls::commands::Contexts;
use helios_controls::context::CardBox;
use helios_controls::flow::{self, Outcome, Steps};
use helios_controls::model::{Representation, ValueSpec};
use helios_controls::render::{self, CardEvent, Edit};
use helios_controls::resolve::{CardPlacement, Layout, Layouts, Moment};
use helios_controls::values::{StoreMut, helios_value, value as framework_value};
use helios_controls::{ContextKeys, Control, HeliosRegistry, Orientation, Registry, Slot, Surface};
use helios_core::{ParamWidget, Value};
use helios_shell::commands::Origin;
use helios_values::{Descriptor, Store, Type};
use serde_json::json;

/// The window's title.
pub const TITLE: &str = "Helios controls gallery";

/// The node of the implicit card.
pub const IMPLICIT: &str = "implicit";

/// The store path of a control's live value.
pub fn path_of(control: &str) -> String {
    format!("node/gallery/{control}")
}

/// One way a control's card shows it: a variant, a representation and
/// an orientation, in a box `width` twelfths of a column wide (a column
/// is one unit across), at the slot given when one is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub name: &'static str,
    pub title: &'static str,
    pub variant: &'static str,
    pub representation: Representation,
    pub orientation: Orientation,
    pub width: u32,
    pub slot: Option<Slot>,
}

const fn view(
    name: &'static str,
    title: &'static str,
    variant: &'static str,
    representation: Representation,
    orientation: Orientation,
    width: u32,
) -> View {
    View {
        name,
        title,
        variant,
        representation,
        orientation,
        width,
        slot: None,
    }
}

/// The views of a control's card, in order.
pub const VIEWS: [View; 8] = [
    view(
        "full",
        "full · 1 unit",
        "slider",
        Representation::Full,
        Orientation::Horizontal,
        12,
    ),
    view(
        "full-wide",
        "full · 2 units, grown",
        "slider",
        Representation::Full,
        Orientation::Horizontal,
        24,
    ),
    view(
        "compact",
        "compact · ½ unit",
        "slider",
        Representation::Compact,
        Orientation::Horizontal,
        6,
    ),
    view(
        "simplified",
        "simplified · ⅓ unit",
        "slider",
        Representation::Simplified,
        Orientation::Horizontal,
        4,
    ),
    View {
        slot: Some(Slot::wide(4)),
        ..view(
            "vertical",
            "full · vertical, 4 rows",
            "slider",
            Representation::Full,
            Orientation::Vertical,
            12,
        )
    },
    view(
        "knob",
        "knob · ½ unit",
        "knob",
        Representation::Full,
        Orientation::Horizontal,
        6,
    ),
    view(
        "knob-simplified",
        "knob · simplified",
        "knob",
        Representation::Simplified,
        Orientation::Horizontal,
        4,
    ),
    view(
        "scrub",
        "scrub field · ½ unit",
        "scrub",
        Representation::Full,
        Orientation::Horizontal,
        6,
    ),
];

/// The implicit card's boxes: two units wide with every part in full,
/// one unit wide with every part compact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Wide,
    Compact,
}

/// The gallery's pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    /// A card per registered control, then the implicit card.
    #[default]
    Catalogue,
    /// The shelf and the bench.
    Bench,
}

/// The gallery's state: the registry, the store of live values, the
/// gestures open, the last outcome of each control, what the
/// `controls.*` commands share, and the bench.
pub struct Gallery {
    pub registry: Registry,
    pub store: Store,
    steps: Steps,
    gestures: BTreeMap<String, flow::Gesture>,
    last: BTreeMap<String, String>,
    commands: Shared,
    pub page: Page,
    pub bench: bench_ui::BenchPage,
}

impl Default for Gallery {
    fn default() -> Self {
        Self::new(Registry::standard())
    }
}

/// The implicit card's parameters, as a module would declare them.
fn implicit_params() -> Vec<(&'static str, Descriptor)> {
    let list = |label: &str, default: Vec<f64>, widget: ParamWidget| {
        let mut d = Descriptor::text(label, "");
        d.ty = Type::Floats;
        d.default = Value::List(default);
        d.widget = Some(widget);
        d
    };
    let mut amount = Descriptor::float("Amount", 0.0, 1.0, 0.5);
    amount.unit = Some("%".into());
    amount.display_scale = 100.0;
    amount.precision = 0;
    vec![
        ("amount", amount),
        ("preserve", Descriptor::bool("Preserve colours", true)),
        (
            "method",
            Descriptor::choice("Method", &["Linear", "Filmic", "Sigmoid"], "Filmic"),
        ),
        (
            "curve",
            list(
                "Curve",
                vec![0.0, 0.0, 0.25, 0.2, 0.75, 0.85, 1.0, 1.0],
                ParamWidget::Curve,
            ),
        ),
        (
            "lift",
            list("Lift", vec![0.08, 0.3, 0.1], ParamWidget::Wheel),
        ),
        ("notes", Descriptor::text("Notes", "")),
    ]
}

impl Gallery {
    /// A gallery of a registry's controls, each with its value at its
    /// default, and the implicit card's node.
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
        for (param, d) in implicit_params() {
            store.declare(&format!("node/{IMPLICIT}/{param}"), d);
        }
        let contexts = SharedContexts::default();
        // The notes are hidden by the person's layout: under "More".
        contexts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .layouts = Layouts::person(Layout::default().hide("notes"));
        // The bench resolves in the same context: `controls.context` and
        // its context bar read and set the same keys.
        let bench = bench_ui::BenchPage::new(bench::Bench::with_contexts(
            bench::Kit {
                registry: registry.clone(),
                ..bench::kit()
            },
            contexts.clone(),
        ));
        let commands = Shared {
            registry: Arc::new(registry.clone()),
            steps: Arc::new(Mutex::new(Steps::new())),
            contexts,
        };
        Self {
            registry,
            store,
            steps: Steps::new(),
            gestures: BTreeMap::new(),
            last: BTreeMap::new(),
            commands,
            page: Page::default(),
            bench,
        }
    }

    /// Runs a command as an agent would: `controls.bench.*` on the bench,
    /// the other `controls.*` on the gallery's store.
    pub fn run_command(
        &mut self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        if name.starts_with("controls.bench.") {
            return self.bench.bench.run(name, args);
        }
        egui_hud::commands::run(
            name,
            args,
            &Origin::Agent("gallery".into()),
            &mut StoreMut(&mut self.store),
            &self.commands,
        )
    }

    /// A control's live value.
    pub fn value(&self, id: &str) -> f64 {
        self.store
            .get(&path_of(id))
            .and_then(|e| e.value.as_f64())
            .unwrap_or(0.0)
    }

    /// A parameter of the implicit card, as stored.
    pub fn implicit_value(&self, param: &str) -> Option<Value> {
        self.store
            .get(&format!("node/{IMPLICIT}/{param}"))
            .map(|e| e.value.clone())
    }

    /// The contexts the gallery's `controls.*` commands share.
    pub fn contexts(&self) -> MutexGuard<'_, Contexts> {
        self.commands
            .contexts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes a renderer's changes through the value flow.
    pub fn apply(&mut self, control: &Control, changes: &[render::Change]) {
        self.apply_at(control, &path_of(&control.id), changes);
    }

    /// Writes a renderer's changes to the number at `path` through the
    /// value flow: a gesture's frames preview, its end commits one undo
    /// step, a cancel leaves no trace.
    pub fn apply_at(&mut self, control: &Control, path: &str, changes: &[render::Change]) {
        for c in changes {
            let open = self.gestures.remove(path);
            let outcome = if !c.done {
                let g = match open {
                    Some(g) => g,
                    None => match flow::begin(&mut self.store, control, path, Origin::User) {
                        Ok(g) => g,
                        Err(_) => continue,
                    },
                };
                let _ = g.preview(&mut self.store, c.to, false);
                self.gestures.insert(path.to_owned(), g);
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
                        path,
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

    /// The page chosen: the catalogue or the bench.
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.page, Page::Catalogue, "Catalogue");
            ui.selectable_value(&mut self.page, Page::Bench, "Bench");
        });
        ui.separator();
        match self.page {
            Page::Catalogue => self.catalogue(ui),
            Page::Bench => self.bench.ui(ui),
        }
    }

    /// Every card, one per registered control, then the implicit card.
    pub fn catalogue(&mut self, ui: &mut Ui) {
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
            self.implicit_card(ui);
        });
    }

    /// A one-part card of control `id` resolved for a view.
    pub fn resolve_view(&self, id: &str, view: &View) -> Result<CardPlacement, String> {
        let mut layout = Layout::default()
            .represent(id, view.representation)
            .orient(id, view.orientation);
        if let Some(s) = view.slot {
            layout = layout.slot(id, s);
        }
        let card = Card::new(vec![CardItem::control(id, id).variant(view.variant)]);
        let keys = ContextKeys::new(CardBox::new(Surface::Panel, view.width, 240));
        helios_controls::resolve::resolve(
            &self.registry,
            &card,
            &keys,
            &Layouts::person(layout),
            &BTreeMap::new(),
            &Moment::default(),
            None,
        )
    }

    /// The views a control has: those of its variants.
    pub fn views_of(control: &Control) -> Vec<&'static View> {
        VIEWS
            .iter()
            .filter(|v| control.faces.iter().any(|x| x.id == v.variant))
            .collect()
    }

    /// One view of control `id`, drawn through the managed renderers, its
    /// changes written through the value flow.
    pub fn view(&mut self, ui: &mut Ui, id: &str, view: &View) {
        let Some(control) = self.registry.get(id).cloned() else {
            return;
        };
        let placement = match self.resolve_view(id, view) {
            Ok(p) => p,
            Err(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
                return;
            }
        };
        let mut values = BTreeMap::from([(id.to_owned(), egui_hud::Value::Float(self.value(id)))]);
        let out = ui
            .push_id(("view", view.name), |ui| {
                render::card(
                    ui,
                    &self.registry,
                    &placement,
                    &mut values,
                    &mut render::NoDrawings,
                )
            })
            .inner;
        let changes: Vec<render::Change> = out
            .edits
            .into_iter()
            .filter_map(|e| match e {
                Edit::Number { change, .. } => Some(change),
                Edit::Set { .. } => None,
            })
            .collect();
        self.apply(&control, &changes);
    }

    /// One control's card: its views side by side, its value, its undo
    /// steps and its card data.
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
            ui.horizontal_wrapped(|ui| {
                for v in Self::views_of(&control) {
                    // Each view in a cell its box's width, so the row wraps.
                    let width = v.width as f32 / 12.0 * 240.0;
                    let rows = v.slot.map_or(1.0, |s| s.width.twelfths() as f32 / 12.0);
                    let size = egui::vec2(width, 20.0 + rows * 24.0);
                    ui.allocate_ui_with_layout(
                        size,
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_size(size);
                            ui.add(egui::Label::new(RichText::new(v.title).weak()).truncate());
                            self.view(ui, id, v);
                        },
                    );
                }
            });
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

    /// The implicit card resolved in a fit, with what `controls.reveal`
    /// left in the shared contexts; and the registry with its implicit
    /// controls.
    pub fn resolve_implicit(&self, fit: Fit) -> Result<(CardPlacement, Registry), String> {
        let (card, registry, values) =
            helios_controls::commands::node_card(&self.store, &self.registry, IMPLICIT)?;
        let c = self.contexts();
        let mut layouts = c.layouts.clone();
        let width = match fit {
            Fit::Wide => 24,
            Fit::Compact => {
                let mut layout = Layout::default();
                for k in card.keys() {
                    layout = layout.represent(k, Representation::Compact);
                }
                layouts.person.push(layout);
                12
            }
        };
        let keys = ContextKeys::new(CardBox::new(Surface::Panel, width, 240));
        let moment = c.moments.get(IMPLICIT).cloned().unwrap_or_default();
        let p = helios_controls::resolve::resolve(
            &registry, &card, &keys, &layouts, &values, &moment, None,
        )?;
        Ok((p, registry))
    }

    /// The implicit card in one fit: drawn, its edits written, its
    /// reveals run as `controls.reveal`. Says how each part was drawn.
    pub fn implicit_fit(&mut self, ui: &mut Ui, fit: Fit) -> BTreeMap<String, render::Drawn> {
        let (placement, registry) = match self.resolve_implicit(fit) {
            Ok(p) => p,
            Err(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
                return BTreeMap::new();
            }
        };
        let mut values: BTreeMap<String, egui_hud::Value> = placement
            .controls()
            .filter_map(|p| {
                let v = self.implicit_value(&p.key)?;
                Some((p.key.clone(), framework_value(&v)))
            })
            .collect();
        let out = ui
            .push_id(("implicit", fit == Fit::Wide), |ui| {
                render::card(ui, &registry, &placement, &mut values, &mut EditorDrawings)
            })
            .inner;
        for e in out.edits {
            let path = format!("node/{IMPLICIT}/{}", e.key());
            match e {
                Edit::Number { key, change } => {
                    let Some(control) = placement
                        .get(&key)
                        .and_then(|p| p.control.as_deref())
                        .and_then(|id| registry.get(id))
                        .cloned()
                    else {
                        continue;
                    };
                    self.apply_at(&control, &path, &[change]);
                }
                Edit::Set { to, done: true, .. } => {
                    let _ = self.store.set(helios_values::Set::new(
                        &path,
                        helios_values::Change::To(helios_value(to)),
                        Origin::User,
                    ));
                }
                Edit::Set { .. } => {}
            }
        }
        for event in out.events {
            let (name, args) = match event {
                CardEvent::Reveal(key) => {
                    ("controls.reveal", json!({"node": IMPLICIT, "key": key}))
                }
                CardEvent::RevealAll => ("controls.reveal_all", json!({"node": IMPLICIT})),
                // No marks and no picker on the gallery's card.
                CardEvent::Reset(_) | CardEvent::Quad(_) => continue,
            };
            let _ = egui_hud::commands::run(
                name,
                &args,
                &Origin::User,
                &mut StoreMut(&mut self.store),
                &self.commands,
            );
        }
        out.drawn
    }

    /// The implicit card: a node of implicit controls, wide then compact.
    pub fn implicit_card(&mut self, ui: &mut Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading("Implicit controls");
            ui.label("A node's parameters with no control patched to them, each with an implicit control from its descriptor. Wide: every part in full, the curve and the wheel drawn by egui-pop's editors. Compact: every part in a row, the curve and the wheel as their values' rows. The notes are hidden by the layout, under More.");
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.weak("wide · 2 units");
                    let _ = self.implicit_fit(ui, Fit::Wide);
                });
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.weak("compact · 1 unit");
                    let _ = self.implicit_fit(ui, Fit::Compact);
                });
            });
        });
    }
}

/// The drawings of the integrated controls: egui-pop's curve editor and
/// colour wheel, as the bench draws them.
pub use egui_hud::bench::EditorDrawings;

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
