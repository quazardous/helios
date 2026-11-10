//! The `egui_hud` demo: a neutral catalogue of controls, a mixer channel and
//! a light, declared as data ([`KIT`]), put on `egui_hud`'s bench and laid
//! out by its slot engine (`egui_gridflow`).
//!
//! - [`kit`] reads the catalogue: the controls, their shelf groups, the
//!   parameters offered as implicit controls (an EQ curve, a colour
//!   wheel), the presets and the tools of the context bar.
//! - [`DemoDrawings`] draws the integrated controls: the gain's fader,
//!   the level meter, and (through `egui_hud`'s editor drawings) the curve
//!   and the wheel. Where a drawing cannot go, `egui_hud` draws the
//!   control's declared fallback: the gain's knob, the curve's rows of
//!   points, the wheel's swatch and channels.
//! - [`Demo`] is the page: a header, then `egui_hud`'s bench page (shelf,
//!   bench, context bar, modes, presets, values with undo and redo, the
//!   placement and reason pane). The level meter follows a simulated
//!   signal of the gain and mute on the bench.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::rc::Rc;

use egui_hud::bench::ui::BenchPage;
use egui_hud::bench::{Bench, EditorDrawings, Kit, Preset, ShelfParam};
use egui_hud::model::{Control, Representation};
use egui_hud::render::{self, Drawings, Edit, Look};
use egui_hud::slot::{Integration, Orientation};
use egui_hud::{Registry, Value};
use egui_pop::egui::{self, Color32, Rect, Role, Sense, Stroke, StrokeKind, Ui, WidgetInfo};
use serde::Deserialize;

/// The window's and the page's title.
pub const TITLE: &str = "egui_hud demo";

/// The catalogue, as data.
pub const KIT: &str = include_str!("../kit.json");

/// The gain control's id: drawn as a fader where it has room.
pub const GAIN: &str = "gain";
/// The mute control's id.
pub const MUTE: &str = "mute";
/// The level meter's id: an indicator, drawn as a bar.
pub const METER: &str = "meter";

/// The source code, linked from the header.
pub const SOURCE: &str = "https://github.com/quazardous/helios/tree/main/crates/egui_hud_demo";

/// The catalogue file's shape.
#[derive(Deserialize)]
struct KitData {
    controls: Vec<Control>,
    groups: BTreeMap<String, String>,
    #[serde(default)]
    parameters: Vec<ShelfParam>,
    #[serde(default)]
    implicit: bool,
    #[serde(default)]
    presets: Vec<Preset>,
    #[serde(default)]
    tools: Vec<String>,
}

/// The kit of a catalogue written as [`KIT`] is: every control checked
/// as it is registered.
pub fn kit_of(text: &str) -> Result<Kit, String> {
    let data: KitData = serde_json::from_str(text).map_err(|e| format!("the catalogue: {e}"))?;
    Ok(Kit {
        registry: Registry::with(data.controls)?,
        groups: data.groups,
        parameters: data.parameters,
        implicit: data.implicit,
        presets: data.presets,
        tools: data.tools,
    })
}

/// The demo's kit, from [`KIT`].
pub fn kit() -> Result<Kit, String> {
    kit_of(KIT)
}

/// The demo's bench, its first preset loaded; an empty bench with the
/// reason when the catalogue does not load.
pub fn bench() -> Result<Bench, String> {
    let mut b = Bench::new(kit()?);
    b.load_sample();
    Ok(b)
}

/// The meter's lowest reading, in decibels: silence.
pub const SILENCE: f64 = -60.0;
/// The meter's highest reading, in decibels.
pub const PEAK: f64 = 6.0;

/// The simulated signal's level at time `t` (seconds), in decibels:
/// the gain's level moving like music, or silence when muted.
pub fn signal(gain: f64, muted: bool, t: f64) -> f64 {
    if muted {
        return SILENCE;
    }
    let beat = (t * 4.1).sin().abs() * (t * 0.73).sin().abs();
    (gain - 18.0 + 16.0 * beat).clamp(SILENCE, PEAK)
}

/// The demo's drawings: the gain's fader and the level meter, then
/// `egui_hud`'s curve editor and colour wheel.
pub struct DemoDrawings {
    /// The meter's reading, in decibels, set each frame by the page.
    pub level: Rc<Cell<f64>>,
}

impl DemoDrawings {
    /// The gain as a fader (vertical when its slot is taller than wide).
    fn fader(
        ui: &mut Ui,
        key: &str,
        control: &Control,
        rect: Rect,
        value: &mut Value,
    ) -> Vec<Edit> {
        let mut v = value.as_f64().unwrap_or(0.0);
        let look = Look {
            representation: Representation::Full,
            orientation: if rect.height() > rect.width() {
                Orientation::Vertical
            } else {
                Orientation::Horizontal
            },
            variant: Some("fader".into()),
            integration: Integration::Managed,
        };
        let out = render::number_at(ui, control, &look, rect, &mut v);
        *value = Value::Float(v);
        out.changes
            .into_iter()
            .map(|change| Edit::Number {
                key: key.to_owned(),
                change,
            })
            .collect()
    }

    /// The meter: a bar from silence to the peak, green, then yellow over
    /// -12 dB, red over 0 dB; its reading beside it.
    fn meter(ui: &mut Ui, control: &Control, rect: Rect, level: f64) {
        let r = ui.allocate_rect(rect, Sense::hover());
        let reading = if level <= SILENCE {
            "silent".to_owned()
        } else {
            format!("{level:+.1} dB")
        };
        let name = format!("{}: {reading}", control.label);
        r.widget_info(|| WidgetInfo::labeled(Role::ProgressIndicator, true, &name));
        let _ = r.on_hover_text(&name);
        let vertical = rect.height() > rect.width();
        let bar = if vertical {
            Rect::from_center_size(
                rect.center(),
                egui::vec2(rect.width().min(18.0), rect.height()),
            )
        } else {
            Rect::from_center_size(
                rect.center(),
                egui::vec2(rect.width(), rect.height().min(14.0)),
            )
        };
        let v = ui.visuals();
        let painter = ui.painter();
        painter.rect(
            bar,
            2.0,
            v.extreme_bg_color,
            v.widgets.noninteractive.bg_stroke,
            StrokeKind::Inside,
        );
        let share = ((level - SILENCE) / (PEAK - SILENCE)).clamp(0.0, 1.0) as f32;
        let colour = if level > 0.0 {
            Color32::from_rgb(220, 70, 60)
        } else if level > -12.0 {
            Color32::from_rgb(220, 190, 70)
        } else {
            Color32::from_rgb(90, 190, 110)
        };
        let inner = bar.shrink(2.0);
        let fill = if vertical {
            Rect::from_min_max(
                egui::pos2(inner.left(), inner.bottom() - inner.height() * share),
                inner.max,
            )
        } else {
            Rect::from_min_max(
                inner.min,
                egui::pos2(inner.left() + inner.width() * share, inner.bottom()),
            )
        };
        if share > 0.0 {
            painter.rect_filled(fill, 1.0, colour);
        }
        // The 0 dB mark.
        let zero = ((0.0 - SILENCE) / (PEAK - SILENCE)) as f32;
        let mark = Stroke::new(1.0, v.text_color());
        if vertical {
            let y = inner.bottom() - inner.height() * zero;
            painter.hline(bar.x_range(), y, mark);
        } else {
            let x = inner.left() + inner.width() * zero;
            painter.vline(x, bar.y_range(), mark);
        }
    }
}

impl Drawings for DemoDrawings {
    fn draw(
        &mut self,
        ui: &mut Ui,
        key: &str,
        control: &Control,
        rect: Rect,
        value: &mut Value,
    ) -> Option<Vec<Edit>> {
        match control.id.as_str() {
            GAIN => Some(Self::fader(ui, key, control, rect, value)),
            METER => {
                Self::meter(ui, control, rect, self.level.get());
                Some(Vec::new())
            }
            _ => EditorDrawings.draw(ui, key, control, rect, value),
        }
    }
}

/// The demo's page: a header, then the bench page.
pub struct Demo {
    pub page: BenchPage,
    /// Why the catalogue did not load, when it did not.
    pub error: Option<String>,
    level: Rc<Cell<f64>>,
}

impl Default for Demo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo {
    /// The demo with its first preset on the bench.
    pub fn new() -> Self {
        let (bench, error) = match bench() {
            Ok(b) => (b, None),
            Err(e) => (Bench::new(Kit::default()), Some(e)),
        };
        let level = Rc::new(Cell::new(SILENCE));
        let page = BenchPage::new(bench).with_drawings(Box::new(DemoDrawings {
            level: level.clone(),
        }));
        Self { page, error, level }
    }

    /// The first value on the bench of a control, by its id.
    fn value_of(&self, control: &str) -> Option<Value> {
        let b = &self.page.bench;
        b.composition
            .pieces()
            .into_iter()
            .find(|(p, _)| p.control == control)
            .and_then(|(p, _)| b.value(&p.key))
    }

    /// Whether a control is on the bench, by its id.
    pub fn holds(&self, control: &str) -> bool {
        self.page
            .bench
            .composition
            .pieces()
            .iter()
            .any(|(p, _)| p.control == control)
    }

    /// The meter's reading at time `t`: the first gain and mute on the
    /// bench (0 dB and not muted without them).
    pub fn level_at(&self, t: f64) -> f64 {
        let gain = self.value_of(GAIN).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let muted = self.value_of(MUTE) == Some(Value::Bool(true));
        signal(gain, muted, t)
    }

    /// The whole page.
    pub fn ui(&mut self, ui: &mut Ui) {
        let t = ui.input(|i| i.time);
        self.level.set(self.level_at(t));
        if self.holds(METER) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            self.header(ui);
            ui.separator();
            egui::ScrollArea::both()
                .id_salt("demo-page")
                .auto_shrink([false, false])
                .show(ui, |ui| self.page.ui(ui));
        });
    }

    /// The title, what the demo shows, the source, and (in a browser) a
    /// download of the bench.
    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(TITLE);
            ui.label("Controls as data: a mixer channel and a light, composed on a bench, laid out by a slot engine and resolved in their context.");
            ui.hyperlink_to("Source", SOURCE);
            #[cfg(target_arch = "wasm32")]
            if ui
                .button("Download the bench")
                .on_hover_text("Save the bench as a JSON file")
                .clicked()
                && let Ok(text) = serde_json::to_string_pretty(&self.page.bench.save())
            {
                download("bench.controls.json", &text);
            }
        });
        ui.weak("Drag controls from the shelf onto the bench, or pick a preset. Arrange moves, groups and removes them; the handle on the bench's right edge sets its width; the context bar and the modes change what each control becomes, and the Placements pane says why.");
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }
}

/// Percent-encodes `text` for a `data:` URL.
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Offers `text` to the browser as a file named `name`.
#[cfg(target_arch = "wasm32")]
fn download(name: &str, text: &str) {
    use egui_pop::eframe::wasm_bindgen::JsCast as _;
    use egui_pop::eframe::web_sys;

    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Ok(a) = document.create_element("a") else {
        return;
    };
    let url = format!(
        "data:application/json;charset=utf-8,{}",
        percent_encode(text)
    );
    let _ = a.set_attribute("href", &url);
    let _ = a.set_attribute("download", name);
    if let Ok(a) = a.dyn_into::<web_sys::HtmlElement>() {
        a.click();
    }
}
