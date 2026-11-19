//! The Calibration focus (`docs/design/calibration.md`, section 3): one
//! chart shot open, its four corner handles on the picture, the dry run's
//! report, the profile named and saved, and the saved profiles listed,
//! deleted, exported, imported and set as a camera's default.
//!
//! One window is one focus (ROADMAP, "One window is one focus"): the
//! focus opens in an OS window of its own (`calibrate`), as the import
//! focus does, with the same bars as every Helios window; where the
//! backend has no windows of its own (kittest, the web), in a window of
//! the main one.
//!
//! Node first: every action is a command of the desktop's node
//! (`calibrate.chart` for the dry run and the save, `calibrate.profiles.*`
//! for the list), run by the shell as the person's. The window's pass
//! only queues what was asked ([`Event`]); the main window's pass sends
//! it off the frame (`calibrate::with_landing`): the window says it works
//! while the node answers, and shows each answer at the frame it lands.

use std::cell::{Ref, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use egui::{Color32, Context as Egui, Pos2, Rect, Sense, Stroke};
use egui_pop::api::windows::WindowId;
use egui_pop::core::input::Input;
use egui_pop::core::tooltip::TooltipExt as _;
use egui_pop::core::ui::{Grid, Space, TextRole, Tone, Track, UiPop as _};
use egui_pop::core::widgets;
use helios_calibrate::{Layout, XRITE_CC24_2014};
use helios_core::Message;
use helios_shell::commands::Origin;
use helios_ui::{Action, Scroll};
use serde_json::{Value, json};

use super::Shell;
use super::focus_window::{FocusWindow, WindowFocus};

/// The command that opens the focus (`path`: the chart shot; `corners`:
/// four `[x, y]` in its pixels; `camera`: the profiles listed for it).
pub const COMMAND: &str = "calibrate.open";

/// The focus's OS window: its id, under which egui-pop keeps its size
/// (`appWindow == 'calibrate'`, `window: "calibrate"` for agents).
pub const WINDOW: &str = "calibrate";

/// The longest side of the picture shown, in pixels.
const SHOWN_SIDE: u32 = 2048;
/// A handle's side, in points.
const HANDLE: f32 = 16.0;

/// The focus's window (`focus_window`).
pub(super) static FRAME: FocusWindow = FocusWindow {
    window: WINDOW,
    embedded: "helios.calibrate.embedded",
    min_size: egui::vec2(720.0, 440.0),
    max_default_size: egui::vec2(1280.0, 820.0),
    share: 0.85,
};

/// What the window asked of the shell, run at the main window's pass.
#[derive(Clone, Debug, PartialEq)]
enum Event {
    /// Open the shot typed.
    Open,
    /// The dry run on the corners as they are.
    Solve,
    /// Save the profile, named, and maybe as the camera's default.
    Save,
    SetDefault(String),
    Delete(String),
    Export(String),
    Import,
}

/// A command sent off the frame: what lands is read as.
#[derive(Clone, Debug)]
enum Ask {
    /// An event's command (a dry run: the `n`th asked).
    Event(Event, u64),
    /// A save that also sets the camera's default: its answer.
    Save { as_default: bool },
    /// The default set after a save: the save's answer.
    SavedDefault(Value),
    /// The profiles listed, the `n`th time.
    List(u64),
    /// A camera's default profiles, read for the `n`th list.
    Defaults(String, u64),
}

/// An ask in flight and where its answer lands.
struct InFlight {
    ask: Ask,
    answer: std::sync::mpsc::Receiver<Result<Value, String>>,
}

/// The shot shown: its picture and the size of the image measured.
struct Picture {
    image: egui::ColorImage,
    texture: Option<egui::TextureHandle>,
    /// The measured image's size, in its pixels (a raw's mosaic).
    size: [f64; 2],
}

/// The focus's state, shared by the shell and the window's content.
#[derive(Default)]
pub(crate) struct Calibration {
    open: bool,
    /// The OS window opened for it, not seen closed since.
    shown: Option<WindowId>,
    /// The chart shot, as typed or given.
    path: String,
    /// The camera whose profiles are listed (all when none).
    camera: Option<String>,
    picture: Option<Picture>,
    /// The four handles, in the chart's corner order, in the measured
    /// image's pixels.
    corners: [[f64; 2]; 4],
    /// The corners before each finished drag: the focus's undo.
    undo: Vec<[[f64; 2]; 4]>,
    /// The handle dragged and the corners at the drag's start.
    drag: Option<(usize, [[f64; 2]; 4])>,
    /// The last dry run's answer.
    report: Option<Value>,
    /// Why the last command failed.
    error: Option<String>,
    /// What the last action did.
    said: Option<String>,
    name: String,
    as_default: bool,
    profiles: Vec<Value>,
    /// Each camera's default profiles.
    defaults: BTreeMap<String, Vec<String>>,
    /// Where exports go.
    folder: String,
    /// The file to import.
    import_path: String,
    events: Vec<Event>,
    texts: Option<Arc<dyn Fn(Message) -> String + Send + Sync>>,
    /// The asks whose answer has not landed.
    in_flight: Vec<InFlight>,
    /// The last dry run asked: an older one landing later is dropped.
    solves: u64,
    /// The last listing asked, likewise.
    lists: u64,
}

/// The focus: its state shared with the window's content.
#[derive(Default)]
pub(super) struct CalibrationFocus {
    pub(super) state: Rc<RefCell<Calibration>>,
}

impl CalibrationFocus {
    pub(super) fn get(&self) -> Ref<'_, Calibration> {
        self.state.borrow()
    }
}

/// The handles on a centred rectangle of an image of `size`, in the
/// chart's corner order (top left, top right, bottom right, bottom left).
fn centred(size: [f64; 2]) -> [[f64; 2]; 4] {
    let [w, h] = size;
    [
        [0.2 * w, 0.2 * h],
        [0.8 * w, 0.2 * h],
        [0.8 * w, 0.8 * h],
        [0.2 * w, 0.8 * h],
    ]
}

/// The shot at `path`: its picture (a raw's embedded preview, stretched
/// over its mosaic) and the measured image's size.
fn load(path: &Path) -> Result<Picture, String> {
    let loaded = helios_load::load(path)?;
    let r = loaded.image.region();
    let size = [f64::from(r.width), f64::from(r.height)];
    let shown = match loaded.camera {
        None => loaded.image,
        Some(_) => helios_load::preview(path)?
            .ok_or_else(|| format!("{}: the raw carries no preview", path.display()))?,
    };
    let r = shown.region();
    let scale = (f64::from(SHOWN_SIDE) / f64::from(r.width.max(r.height).max(1))).min(1.0);
    let small = shown.sample_at(
        helios_core::Region::sized(
            ((f64::from(r.width) * scale).round() as u32).max(1),
            ((f64::from(r.height) * scale).round() as u32).max(1),
        ),
        scale,
    );
    Ok(Picture {
        image: crate::library::srgb(&small),
        texture: None,
        size,
    })
}

fn as_corners(v: &Value) -> Option<[[f64; 2]; 4]> {
    let list = v.as_array()?;
    if list.len() != 4 {
        return None;
    }
    let mut out = [[0.0; 2]; 4];
    for (o, p) in out.iter_mut().zip(list) {
        *o = [p.get(0)?.as_f64()?, p.get(1)?.as_f64()?];
    }
    Some(out)
}

impl Calibration {
    fn text(&self, m: Message) -> String {
        match &self.texts {
            Some(t) => t(m),
            None => m.english,
        }
    }

    /// The shot at [`Self::path`] opened, its corners placed (or kept
    /// when given), its report to come.
    fn open_shot(&mut self, corners: Option<[[f64; 2]; 4]>) -> bool {
        self.report = None;
        self.undo.clear();
        self.drag = None;
        let path = PathBuf::from(self.path.trim());
        match load(&path) {
            Ok(p) => {
                self.corners = corners.unwrap_or_else(|| centred(p.size));
                self.picture = Some(p);
                if self.folder.is_empty()
                    && let Some(dir) = path.parent()
                {
                    self.folder = dir.display().to_string();
                }
                self.error = None;
                true
            }
            Err(e) => {
                self.picture = None;
                self.error = Some(e);
                false
            }
        }
    }

    /// The window's content.
    fn window_ui(&mut self, ui: &mut egui::Ui) {
        let full = ui.available_rect_before_wrap();
        let left_w = (full.width() * 0.22).clamp(180.0, 280.0);
        let right_w = (full.width() * 0.3).clamp(240.0, 360.0);
        let left = Rect::from_min_size(full.min, egui::vec2(left_w, full.height()));
        let right = Rect::from_min_max(egui::pos2(full.max.x - right_w, full.min.y), full.max);
        let centre = Rect::from_min_max(
            egui::pos2(left.max.x + 8.0, full.min.y),
            egui::pos2(right.min.x - 8.0, full.max.y),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
            helios_ui::scroll(ui, "helios.calibrate.profiles", Scroll::Region, |ui| {
                self.profiles_ui(ui);
            });
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(centre), |ui| {
            self.shot_ui(ui);
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
            helios_ui::scroll(ui, "helios.calibrate.report", Scroll::Region, |ui| {
                if !self.in_flight.is_empty() {
                    // Words, not a spinner: no frame drawn for nothing.
                    ui.muted(self.text(Message::new(
                        "gui-calibrate-working",
                        "Waiting for the library…",
                    )));
                }
                self.report_ui(ui);
                ui.divider();
                self.save_ui(ui);
                self.patches_ui(ui);
            });
        });
        ui.allocate_rect(full, Sense::hover());
    }

    /// The chart shot's line, then the picture with its handles and grid.
    #[allow(
        clippy::too_many_lines,
        reason = "the picture, its grid and its handles in one place"
    )]
    fn shot_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let name = self.text(Message::new("gui-calibrate-shot", "Chart shot"));
            let label = ui.text_role(TextRole::Body, &name);
            let width = (ui.available_width() - 120.0).max(120.0);
            let field = Input::path(&mut self.path, &name)
                .id_salt("helios.calibrate.shot")
                .width(width)
                .show(ui);
            // Named by the words beside it, which tests and agents find.
            field.response.labelled_by(label.id);
            let open = self.text(Message::new("gui-calibrate-open-shot", "Open"));
            if helios_ui::action(ui, Action::Secondary, &open).clicked() || field.submitted {
                self.events.push(Event::Open);
            }
        });
        if let Some(e) = &self.error {
            ui.text_tone(Tone::Danger, e.clone());
        }
        if !self.undo.is_empty() {
            let undo = self.text(Message::new(
                "gui-calibrate-corners-undo",
                "Put the corner back",
            ));
            if helios_ui::action(ui, Action::Secondary, &undo).clicked()
                && let Some(c) = self.undo.pop()
            {
                self.corners = c;
                self.events.push(Event::Solve);
            }
        }
        let Some(picture) = self.picture.as_mut() else {
            ui.muted(self.text(Message::new(
                "gui-calibrate-no-shot",
                "Open a shot of a ColorChecker chart to make a profile",
            )));
            return;
        };
        let texture = picture.texture.get_or_insert_with(|| {
            ui.ctx().load_texture(
                "helios.calibrate.shot",
                picture.image.clone(),
                egui::TextureOptions::LINEAR,
            )
        });
        let id = texture.id();
        let size = picture.size;
        let avail = ui.available_size().max(egui::vec2(64.0, 64.0));
        let (area, _) = ui.allocate_exact_size(avail, Sense::hover());
        let scale = (f64::from(area.width()) / size[0]).min(f64::from(area.height()) / size[1]);
        let shown = Rect::from_center_size(
            area.center(),
            egui::vec2((size[0] * scale) as f32, (size[1] * scale) as f32),
        );
        let painter = ui.painter_at(area);
        painter.image(
            id,
            shown,
            Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let to_screen =
            |p: [f64; 2]| shown.min + egui::vec2((p[0] * scale) as f32, (p[1] * scale) as f32);
        // The grid: each patch's sampling square, coloured by its status.
        if let Some(layout) = Layout::new(&XRITE_CC24_2014, self.corners) {
            for k in 0..XRITE_CC24_2014.patches.len() {
                let status = self
                    .report
                    .as_ref()
                    .and_then(|r| r["patches"][k]["status"].as_str())
                    .unwrap_or_default();
                let colour = match status {
                    "clipped" => Color32::from_rgb(230, 60, 60),
                    "excluded" => Color32::GRAY,
                    "" | "accepted" | "ok" => Color32::from_rgb(80, 220, 120),
                    _ => Color32::from_rgb(240, 190, 40),
                };
                let quad: Vec<Pos2> = layout
                    .sampling_quad(k)
                    .iter()
                    .map(|p| to_screen(*p))
                    .collect();
                painter.add(egui::Shape::closed_line(quad, Stroke::new(1.5, colour)));
            }
        }
        // The handles: a drag is one gesture; Ctrl moves a tenth of the
        // pointer, Escape puts the corner back.
        let fine = ui.input(|i| i.modifiers.command);
        for i in 0..4 {
            let at = to_screen(self.corners[i]);
            let rect = Rect::from_center_size(at, egui::vec2(HANDLE, HANDLE));
            let r = ui.interact(
                rect,
                ui.scope_id().with(("helios.calibrate.corner", i)),
                Sense::drag(),
            );
            let name = self.text(
                Message::new("gui-calibrate-corner", "Chart corner { $n }")
                    .with("n", (i + 1).to_string()),
            );
            egui_pop::core::a11y::name(&r, egui::Role::Button, &name);
            let r = r.tip(self.text(Message::new(
                "gui-calibrate-corner-tip",
                "Drag onto the centre of the corner patch",
            )));
            if r.drag_started() {
                self.drag = Some((i, self.corners));
            }
            if r.dragged()
                && let Some((at, _)) = self.drag
                && at == i
            {
                let d = r.drag_delta();
                let k = if fine {
                    helios_controls::gizmo::FINE
                } else {
                    1.0
                } / scale;
                let c = &mut self.corners[i];
                c[0] = (c[0] + f64::from(d.x) * k).clamp(0.0, size[0]);
                c[1] = (c[1] + f64::from(d.y) * k).clamp(0.0, size[1]);
                if ui.input(|inp| inp.key_pressed(egui::Key::Escape))
                    && let Some((_, start)) = self.drag.take()
                {
                    self.corners = start;
                }
            }
            if r.drag_stopped()
                && let Some((at, start)) = self.drag.take()
                && at == i
                && start != self.corners
            {
                self.undo.push(start);
                self.events.push(Event::Solve);
            }
            let hot = r.hovered() || r.dragged();
            painter.circle(
                at,
                HANDLE * 0.5,
                Color32::from_black_alpha(if hot { 40 } else { 90 }),
                Stroke::new(2.0, if hot { Color32::YELLOW } else { Color32::WHITE }),
            );
            painter.text(
                at + egui::vec2(HANDLE * 0.7, -HANDLE * 0.7),
                egui::Align2::LEFT_BOTTOM,
                (i + 1).to_string(),
                egui::FontId::proportional(12.0),
                Color32::WHITE,
            );
        }
    }

    /// The report of the last dry run.
    #[allow(clippy::too_many_lines, reason = "the report's lines in one place")]
    fn report_ui(&self, ui: &mut egui::Ui) {
        ui.section_title(self.text(Message::new("gui-calibrate-report", "Report")));
        let Some(r) = &self.report else {
            ui.muted(self.text(Message::new(
                "gui-calibrate-report-none",
                "Place the four corners on the centres of the corner patches",
            )));
            return;
        };
        let num = |v: &Value| v.as_f64().map_or_else(|| "-".into(), |x| format!("{x:.2}"));
        ui.text_role(
            TextRole::Body,
            self.text(
                Message::new("gui-calibrate-quality", "Quality: { $quality }")
                    .with("quality", r["quality"].as_str().unwrap_or_default()),
            ),
        );
        ui.text_role(
            TextRole::Body,
            self.text(
                Message::new(
                    "gui-calibrate-before",
                    "Delta E before: mean { $mean }, max { $max }",
                )
                .with("mean", num(&r["before"]["mean"]))
                .with("max", num(&r["before"]["max"])),
            ),
        );
        ui.text_role(
            TextRole::Body,
            self.text(
                Message::new(
                    "gui-calibrate-after",
                    "Delta E after: mean { $mean }, max { $max }",
                )
                .with("mean", num(&r["after"]["mean"]))
                .with("max", num(&r["after"]["max"])),
            ),
        );
        if r["residual"].is_object() {
            self.residual_ui(ui, &r["residual"]);
        }
        let worst: Vec<&str> = r["after"]["worst"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !worst.is_empty() {
            ui.text_role(
                TextRole::Body,
                self.text(
                    Message::new("gui-calibrate-worst", "Worst patches: { $patches }")
                        .with("patches", worst.join(", ")),
                ),
            );
        }
        if let Some(k) = r["illuminant"]["cct"].as_f64() {
            ui.text_role(
                TextRole::Body,
                self.text(
                    Message::new("gui-calibrate-light", "Light: { $kelvin } K")
                        .with("kelvin", format!("{k:.0}")),
                ),
            );
        }
        let turns = r["orientation"]["quarter_turns"].as_u64().unwrap_or(0);
        let mirrored = r["orientation"]["mirrored"].as_bool().unwrap_or(false);
        if turns != 0 || mirrored {
            ui.text_role(
                TextRole::Body,
                self.text(
                    Message::new(
                        "gui-calibrate-orientation",
                        "Chart seen turned by { $turns } quarter turns",
                    )
                    .with("turns", turns.to_string()),
                ),
            );
        }
        let clipped: Vec<&str> = r["patches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["status"] == "clipped")
            .filter_map(|p| p["name"].as_str())
            .collect();
        if !clipped.is_empty() {
            ui.text_tone(
                Tone::Warning,
                self.text(
                    Message::new("gui-calibrate-clipped", "Clipped, left out: { $patches }")
                        .with("patches", clipped.join(", ")),
                ),
            );
        }
        for w in r["warnings"].as_array().into_iter().flatten() {
            let text = w["message"]
                .as_str()
                .or_else(|| w["kind"].as_str())
                .map_or_else(|| w.to_string(), String::from);
            ui.text_tone(Tone::Warning, text);
        }
    }

    /// Each patch's Delta E before and after, and its status: a table at
    /// the foot of the report's scrolling column, under "Make a profile".
    fn patches_ui(&self, ui: &mut egui::Ui) {
        let Some(r) = &self.report else {
            return;
        };
        let num = |v: &Value| v.as_f64().map_or_else(|| "-".into(), |x| format!("{x:.2}"));
        ui.space_role(Space::Group);
        ui.section_title(self.text(Message::new("gui-calibrate-patches", "Patches")));
        // Columns as wide as their words: the scrolling column does not
        // grow the window, and its bar does not cover the status.
        let tracks = [Track::CONTENT; 4];
        ui.grid(Grid::new("helios.calibrate.patch-table", tracks), |g| {
            for p in r["patches"].as_array().into_iter().flatten() {
                g.cell(|ui| ui.text_role(TextRole::Body, p["name"].as_str().unwrap_or_default()));
                g.cell(|ui| ui.text_role(TextRole::Body, num(&p["before"])));
                g.cell(|ui| ui.text_role(TextRole::Body, num(&p["after"])));
                g.cell(|ui| ui.muted(p["status"].as_str().unwrap_or_default()));
            }
        });
    }

    /// The residual table's line: kept or dropped, and Delta E without and
    /// with it.
    fn residual_ui(&self, ui: &mut egui::Ui, residual: &Value) {
        let num = |v: &Value| v.as_f64().map_or_else(|| "-".into(), |x| format!("{x:.2}"));
        let status = if residual["status"] == "kept" {
            Message::new("gui-calibrate-residual-kept", "Residual table: kept")
        } else {
            Message::new("gui-calibrate-residual-dropped", "Residual table: dropped")
        };
        ui.text_role(TextRole::Body, self.text(status));
        ui.text_role(
            TextRole::Body,
            self.text(
                Message::new(
                    "gui-calibrate-residual-held-out",
                    "Held-out Delta E: { $without } without the table, { $with } with it",
                )
                .with("without", num(&residual["held_out_without"]))
                .with("with", num(&residual["held_out_with"])),
            ),
        );
        if residual["status"] == "kept" {
            ui.text_role(
                TextRole::Body,
                self.text(
                    Message::new(
                        "gui-calibrate-residual-after",
                        "Delta E with the table: mean { $mean }, max { $max }",
                    )
                    .with("mean", num(&residual["after"]["mean"]))
                    .with("max", num(&residual["after"]["max"])),
                ),
            );
        }
    }

    /// The profile's name, "Use as the default", Save.
    fn save_ui(&mut self, ui: &mut egui::Ui) {
        ui.section_title(self.text(Message::new("gui-calibrate-save-heading", "Make a profile")));
        let name = self.text(Message::new("gui-calibrate-name", "Profile name"));
        labelled_field(ui, &name, Input::text(&mut self.name, &name));
        let default = self.text(Message::new(
            "gui-calibrate-as-default",
            "Use as the default for this camera",
        ));
        widgets::checkbox(ui, &mut self.as_default, default.as_str());
        let save = self.text(Message::new("gui-calibrate-save", "Save the profile"));
        if ui
            .add_enabled_ui(self.report.is_some(), |ui| {
                helios_ui::action(ui, Action::Primary, &save)
            })
            .inner
            .clicked()
        {
            self.events.push(Event::Save);
        }
        if let Some(said) = &self.said {
            ui.muted(said.clone());
        }
    }

    /// The saved profiles, by camera, each with its actions.
    #[allow(
        clippy::too_many_lines,
        reason = "the list and its actions in one place"
    )]
    fn profiles_ui(&mut self, ui: &mut egui::Ui) {
        ui.section_title(self.text(Message::new("gui-calibrate-profiles", "Profiles")));
        if self.profiles.is_empty() {
            ui.muted(self.text(Message::new(
                "gui-calibrate-profiles-none",
                "No calibration profile yet",
            )));
        }
        let mut cameras: Vec<String> = self
            .profiles
            .iter()
            .map(|p| p["camera"]["key"].as_str().unwrap_or_default().to_string())
            .collect();
        cameras.sort();
        cameras.dedup();
        for camera in cameras {
            ui.text_role(TextRole::Section, &camera);
            let defaults = self.defaults.get(&camera).cloned().unwrap_or_default();
            let profiles: Vec<Value> = self
                .profiles
                .iter()
                .filter(|p| p["camera"]["key"] == camera.as_str())
                .cloned()
                .collect();
            for p in profiles {
                let id = p["id"].as_str().unwrap_or_default().to_string();
                let pname = p["name"].as_str().unwrap_or_default().to_string();
                let is_default = defaults.contains(&id);
                ui.card(Tone::Neutral, |ui| {
                    let light = p["illuminant"]["cct"]
                        .as_f64()
                        .map_or_else(String::new, |k| format!(", {k:.0} K"));
                    ui.text_role(
                        TextRole::Body,
                        format!(
                            "{pname}{light}, {}",
                            p["quality"].as_str().unwrap_or_default()
                        ),
                    );
                    if is_default {
                        ui.meta(self.text(Message::new(
                            "gui-calibrate-is-default",
                            "The camera's default",
                        )));
                    }
                    ui.horizontal_wrapped(|ui| {
                        let mut action =
                            |ui: &mut egui::Ui, key: &str, english: &str, event: Event| {
                                let word = self.text(Message::new(key, english));
                                let r = helios_ui::row_action(ui, Action::Secondary, &word);
                                egui_pop::core::a11y::name(
                                    &r,
                                    egui::Role::Button,
                                    &format!("{word}: {pname}"),
                                );
                                if r.clicked() {
                                    self.events.push(event);
                                }
                            };
                        if !is_default {
                            action(
                                ui,
                                "gui-calibrate-set-default",
                                "Set as default",
                                Event::SetDefault(id.clone()),
                            );
                        }
                        action(
                            ui,
                            "gui-calibrate-export",
                            "Export",
                            Event::Export(id.clone()),
                        );
                        action(
                            ui,
                            "gui-calibrate-delete",
                            "Delete",
                            Event::Delete(id.clone()),
                        );
                    });
                });
            }
        }
        ui.divider();
        let folder = self.text(Message::new(
            "gui-calibrate-export-folder",
            "Export to folder",
        ));
        labelled_field(ui, &folder, Input::path(&mut self.folder, &folder));
        let file = self.text(Message::new(
            "gui-calibrate-import-file",
            "Profile file to import",
        ));
        labelled_field(ui, &file, Input::path(&mut self.import_path, &file));
        let import = self.text(Message::new("gui-calibrate-import", "Import the profile"));
        let ready = !self.import_path.trim().is_empty();
        if ui
            .add_enabled_ui(ready, |ui| {
                helios_ui::action(ui, Action::Secondary, &import)
            })
            .inner
            .clicked()
        {
            self.events.push(Event::Import);
        }
    }
}

/// A field under the words that name it, as a form's field: egui-pop's
/// `Input` (named `name` already), labelled by those words too.
fn labelled_field(ui: &mut egui::Ui, name: &str, field: Input<'_>) {
    let label = ui.text_role(TextRole::Body, name);
    field.show(ui).response.labelled_by(label.id);
}

/// The window's title.
fn window_title() -> Message {
    Message::new("gui-calibrate-title", "Calibrate camera colour")
}

impl WindowFocus for Calibration {
    fn is_open(&self) -> bool {
        self.open
    }

    fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    fn shown(&mut self) -> &mut Option<WindowId> {
        &mut self.shown
    }

    fn title(&self, _: &Egui) -> String {
        self.text(window_title())
    }

    fn window_ui(&mut self, ui: &mut egui::Ui) {
        Calibration::window_ui(self, ui);
    }

    fn asks_main_pass(&self) -> bool {
        !self.events.is_empty()
    }
}

impl Shell {
    /// `calibrate.open`: opens the Calibration focus, on the shot at
    /// `path` with its `corners` when given, its profiles listed (of
    /// `camera` when given). Open already, its window comes forward.
    pub(super) fn open_calibration(&mut self, args: &Value) {
        super::focus_window::open(self.core.app.handle(), &self.calibration.state);
        {
            let mut c = self.calibration.state.borrow_mut();
            c.error = None;
            c.said = None;
            if let Some(camera) = args["camera"].as_str() {
                c.camera = Some(camera.to_string());
            }
            if let Some(path) = args["path"].as_str().filter(|p| !p.is_empty()) {
                c.path = path.to_string();
                let corners = as_corners(&args["corners"]);
                if c.open_shot(corners) {
                    c.events.push(Event::Solve);
                }
            }
        }
        self.refresh_profiles();
    }

    /// The focus's corners, report and profiles, for tests and agents.
    pub fn calibration_state(&self) -> Value {
        let c = self.calibration.get();
        json!({
            "open": c.open,
            "path": c.path,
            "corners": c.corners,
            "report": c.report,
            "error": c.error,
            "profiles": c.profiles,
            "defaults": c.defaults,
            "busy": !c.in_flight.is_empty(),
        })
    }

    /// The profiles listed again, then each camera's default, off the
    /// frame.
    fn refresh_profiles(&mut self) {
        let camera = self.calibration.get().camera.clone();
        let args = camera.map_or_else(|| json!({}), |c| json!({ "camera": c }));
        let n = {
            let mut c = self.calibration.state.borrow_mut();
            c.lists += 1;
            c.lists
        };
        self.send(helios_node_core::calibrate::LIST, args, Ask::List(n));
    }

    /// Runs command `id` as the person's off the frame: its answer lands
    /// as `ask` at a coming frame (at once when it answered at once).
    fn send(&mut self, id: &str, args: Value, ask: Ask) {
        let (landing, answer) = std::sync::mpsc::channel();
        // Its landing is said on the bus: the window's listener wakes it
        // (`super::window_news`).
        let bus = std::sync::Arc::downgrade(&self.core.app.handle().bus());
        let landing = crate::calibrate::Landing::new(landing, move || {
            super::window_news::say::<super::window_news::CalibrateNews>(&bus);
        });
        let (result, taken) =
            crate::calibrate::with_landing(landing, || CommandId(id).run(self, args));
        if taken && result.is_ok() {
            self.calibration
                .state
                .borrow_mut()
                .in_flight
                .push(InFlight { ask, answer });
        } else {
            self.landed(ask, result);
        }
    }

    /// The answers landed since the last frame, read.
    fn calibration_landings(&mut self) {
        let mut landed = Vec::new();
        self.calibration.state.borrow_mut().in_flight.retain(|f| {
            match f.answer.try_recv() {
                Ok(r) => landed.push((f.ask.clone(), r)),
                Err(std::sync::mpsc::TryRecvError::Empty) => return true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => landed.push((
                    f.ask.clone(),
                    Err("the calibration's work was dropped".into()),
                )),
            }
            false
        });
        if landed.is_empty() {
            return;
        }
        // The focus's own window shows what landed: its listener of
        // `helios.calibrate.window.news`, said as each landed, woke it
        // (`super::window_news`).
        for (ask, result) in landed {
            self.landed(ask, result);
        }
    }

    /// What an ask's answer changes in the focus.
    fn landed(&mut self, ask: Ask, result: Result<Value, String>) {
        use helios_node_core::calibrate::SET_DEFAULT;
        match ask {
            Ask::Event(event, n) => {
                if event == Event::Solve {
                    let mut c = self.calibration.state.borrow_mut();
                    if n == c.solves {
                        match result {
                            Ok(answer) => {
                                c.error = None;
                                c.report = Some(answer);
                            }
                            Err(e) => {
                                c.report = None;
                                c.error = Some(e);
                            }
                        }
                    }
                    return;
                }
                self.finished(&event, result);
            }
            Ask::Save { as_default } => match result {
                Ok(saved) if as_default => {
                    let id = saved["profile"]["id"].clone();
                    self.send(
                        SET_DEFAULT,
                        json!({ "profiles": [id] }),
                        Ask::SavedDefault(saved),
                    );
                }
                r => self.finished(&Event::Save, r),
            },
            Ask::SavedDefault(saved) => self.finished(&Event::Save, result.map(|_| saved)),
            Ask::List(n) => {
                if n != self.calibration.get().lists {
                    return;
                }
                let Ok(listed) = result else {
                    return;
                };
                let profiles: Vec<Value> =
                    listed["profiles"].as_array().cloned().unwrap_or_default();
                let mut cameras: Vec<String> = profiles
                    .iter()
                    .filter_map(|p| p["camera"]["key"].as_str().map(String::from))
                    .collect();
                cameras.sort_unstable();
                cameras.dedup();
                {
                    let mut c = self.calibration.state.borrow_mut();
                    c.profiles = profiles;
                    c.defaults.retain(|k, _| cameras.contains(k));
                }
                for camera in cameras {
                    // A dry run of a clear answers the rule as it is.
                    let asked = json!({ "camera": camera, "profiles": [], "dry_run": true });
                    self.send(SET_DEFAULT, asked, Ask::Defaults(camera, n));
                }
            }
            Ask::Defaults(camera, n) => {
                let mut c = self.calibration.state.borrow_mut();
                if n != c.lists {
                    return;
                }
                if let Ok(a) = result {
                    let ids = a["before"]["profiles"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    c.defaults.insert(camera, ids);
                }
            }
        }
    }

    /// Each frame of the main window: what the window asked, run, and its
    /// OS window opened or closed as the focus is.
    pub(super) fn calibration_frame(&mut self, ctx: &Egui) {
        let texts: Arc<dyn Fn(Message) -> String + Send + Sync> = Arc::new(self.texts());
        self.calibration.state.borrow_mut().texts = Some(texts);
        let events = std::mem::take(&mut self.calibration.state.borrow_mut().events);
        if !events.is_empty() || !self.calibration.get().in_flight.is_empty() {
            crate::calibrate::drawn_in(ctx, self.values());
        }
        for event in events {
            self.calibration_event(event);
        }
        self.calibration_landings();
        let app = self.core.app.handle().clone();
        FRAME.follow(&app, ctx, &self.calibration.state);
    }

    /// Sends what the window asked off the frame.
    fn calibration_event(&mut self, event: Event) {
        use helios_node_core::calibrate::{CHART, DELETE, EXPORT, IMPORT, SET_DEFAULT};
        let (path, corners) = {
            let c = self.calibration.get();
            (c.path.trim().to_string(), c.corners)
        };
        let ask = |e: &Event| Ask::Event(e.clone(), 0);
        match &event {
            Event::Open => {
                let opened = self.calibration.state.borrow_mut().open_shot(None);
                if opened {
                    self.calibration
                        .state
                        .borrow_mut()
                        .events
                        .push(Event::Solve);
                }
            }
            Event::Solve => {
                let n = {
                    let mut c = self.calibration.state.borrow_mut();
                    c.solves += 1;
                    c.solves
                };
                self.send(
                    CHART,
                    json!({ "path": path, "corners": corners, "dry_run": true }),
                    Ask::Event(Event::Solve, n),
                );
            }
            Event::Save => {
                let (name, as_default) = {
                    let c = self.calibration.get();
                    (c.name.trim().to_string(), c.as_default)
                };
                let mut args = json!({ "path": path, "corners": corners, "save": true });
                if !name.is_empty() {
                    args["name"] = json!(name);
                }
                self.send(CHART, args, Ask::Save { as_default });
            }
            Event::SetDefault(id) => {
                self.send(SET_DEFAULT, json!({ "profiles": [id] }), ask(&event));
            }
            Event::Delete(id) => self.send(DELETE, json!({ "profile": id }), ask(&event)),
            Event::Export(id) => {
                let folder = self.calibration.get().folder.trim().to_string();
                self.send(
                    EXPORT,
                    json!({ "profile": id, "path": folder }),
                    ask(&event),
                );
            }
            Event::Import => {
                let file = self.calibration.get().import_path.trim().to_string();
                self.send(
                    IMPORT,
                    json!({ "path": file, "dry_run": false }),
                    ask(&event),
                );
            }
        }
    }

    /// An action's answer landed: what it did said, or why it failed;
    /// the profiles listed again.
    fn finished(&mut self, event: &Event, result: Result<Value, String>) {
        let said = match (event, &result) {
            (_, Err(_)) | (Event::Solve | Event::Open, _) => None,
            (Event::Save, Ok(a)) => Some(
                Message::new("gui-calibrate-saved", "Profile “{ $name }” saved")
                    .with("name", a["profile"]["name"].as_str().unwrap_or_default()),
            ),
            (Event::Export(_), Ok(a)) => Some(
                Message::new("gui-calibrate-exported", "Exported to { $path }")
                    .with("path", a["path"].as_str().unwrap_or_default()),
            ),
            (Event::Import, Ok(_)) => {
                Some(Message::new("gui-calibrate-imported", "Profile imported"))
            }
            (Event::Delete(_), Ok(_)) => {
                Some(Message::new("gui-calibrate-deleted", "Profile deleted"))
            }
            (Event::SetDefault(_), Ok(_)) => Some(Message::new(
                "gui-calibrate-default-set",
                "Default set for the camera",
            )),
        };
        let said = said.map(|m| self.text(m));
        {
            let mut c = self.calibration.state.borrow_mut();
            match result {
                Ok(_) => {
                    c.error = None;
                    c.said = said;
                }
                Err(e) => c.error = Some(e),
            }
        }
        self.refresh_profiles();
    }
}

/// A command run by the shell as the person's, not toasted on failure.
struct CommandId<'a>(&'a str);

impl CommandId<'_> {
    fn run(&self, shell: &mut Shell, args: Value) -> Result<Value, String> {
        use egui_pop::api::host::Reply;
        let id = egui_pop::model::ids::CommandId::new(self.0).map_err(|e| e.to_string())?;
        match shell.core.app.run(&id, args, super::to_pop(&Origin::User)) {
            Reply::Done(r) => r,
            Reply::Later(_) => Ok(Value::Null),
        }
    }
}
