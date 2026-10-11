//! The shell's closed set of windows: confirmations, forms, pickers,
//! menus and notices, drawn the same way everywhere from the theme's
//! tokens. Panels, widgets and plugins never draw a window, a popup or a
//! menu of their own: they ask the shell (`Shell::confirm`, `Shell::form`,
//! `Shell::pick`, `Shell::notify`, or [`request`] from inside a panel), and
//! a test fails if one does (`tests/normalized.rs`).
//!
//! Every open dialog is also data: `dialog.current` describes it and
//! `dialog.answer` answers it, so an agent sees and answers what a human
//! would, its origin recorded like any write.
//!
//! A form's path field (a descriptor with a [`PathHint`]) has a Browse
//! button: the system's chooser when there is one, else Helios's own
//! picker of folders and files, drawn in the dialog (see [`Places`]).
//!
//! The rules each kind follows are in `docs/gui.md`, "Windows, dialogs and
//! menus".

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui::{Align, Align2, Color32, Id, Key, Layout, Order, RichText, Ui};
use helios_core::Value;
use helios_values::{Descriptor, PathHint, PathKind, Type, value_from_json, value_to_json};
use serde::Serialize;
use serde_json::{Value as Json, json};

/// How much a notice matters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warning,
    Error,
}

/// A notice: shown in the status bar and, unless it is plain information,
/// as a toast. It never takes the focus.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Notice {
    pub level: Level,
    pub text: String,
    /// A command offered with it (`(label, command id)`).
    pub action: Option<(String, String)>,
    /// When it was shown, in the GUI's seconds.
    #[serde(skip)]
    pub at: f64,
}

/// A question before an action: the action is named by its verb, never
/// "OK".
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Confirm {
    pub title: String,
    /// The consequences, in one line.
    pub detail: String,
    /// The primary button: "Delete 3 images".
    pub verb: String,
    /// Destructive: the danger style, and the focus starts on Cancel.
    pub danger: bool,
    /// What the primary action runs.
    pub command: String,
    pub args: Json,
}

/// One field of a form, typed by a value descriptor.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Field {
    pub key: String,
    pub descriptor: Descriptor,
    pub value: Value,
}

/// A form: fields from descriptors, the standard footer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Form {
    pub title: String,
    pub fields: Vec<Field>,
    pub verb: String,
    /// Run with `{ key: value, … }`.
    pub command: String,
}

/// One choice of a picker.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Choice {
    /// What the command receives.
    pub value: String,
    pub label: String,
    pub detail: Option<String>,
}

/// A list to choose from, with search: the palette's look.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Picker {
    pub title: String,
    pub choices: Vec<Choice>,
    /// Run with `{ arg: value }`.
    pub command: String,
    pub arg: String,
}

/// Information to read, closed by its one button ("About Helios").
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Info {
    pub title: String,
    pub lines: Vec<String>,
}

/// A dialog the shell shows, one at a time, modal.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Dialog {
    Confirm(Confirm),
    Form(Form),
    Picker(Picker),
    Info(Info),
}

/// A menu entry: a command, never anything else.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MenuItem {
    Command { id: String, args: Json },
    Separator,
    Submenu { label: String, items: Vec<MenuItem> },
}

impl MenuItem {
    pub fn command(id: &str) -> Self {
        MenuItem::Command {
            id: id.into(),
            args: Json::Null,
        }
    }
}

/// What a panel asks the shell for, from inside its content.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Confirm(Confirm),
    Form(Form),
    Picker(Picker),
    Menu {
        at: egui::Pos2,
        items: Vec<MenuItem>,
    },
    Notify(Level, String),
}

fn requests_id() -> Id {
    Id::new("helios.requests")
}

/// Asks the shell for a dialog, a menu or a notice from inside a panel;
/// the shell shows it at the end of the frame.
pub fn request(ctx: &egui::Context, r: Request) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<Request>>(requests_id())
            .push(r);
    });
}

/// Opens a context menu of commands on a widget's secondary click.
pub fn context_menu(response: &egui::Response, items: Vec<MenuItem>) {
    if response.secondary_clicked() {
        let at = response
            .interact_pointer_pos()
            .unwrap_or(response.rect.left_bottom());
        request(&response.ctx, Request::Menu { at, items });
    }
}

pub(crate) fn take_requests(ctx: &egui::Context) -> Vec<Request> {
    ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Vec<Request>>(requests_id())))
}

/// Where path fields browse.
#[derive(Clone, Debug, Default)]
pub struct Places {
    /// Browse opens the system's chooser (through the XDG desktop portal
    /// on Linux), on a thread of its own so the window keeps drawing. When
    /// off, or when no chooser shows up, Helios's own picker opens instead.
    /// Off in headless runs, and when `HELIOS_NO_NATIVE_DIALOGS` is set.
    pub native: bool,
    /// The recent workspaces: offered first when choosing a directory.
    pub recent: Vec<PathBuf>,
}

/// A chooser that answers nothing this soon was never shown (no portal,
/// no fallback): Helios's own picker takes over.
const NO_CHOOSER: Duration = Duration::from_millis(250);

/// The system's chooser, open on its own thread for a field.
pub(crate) struct Native {
    field: usize,
    hint: PathHint,
    answer: mpsc::Receiver<(Option<PathBuf>, Duration)>,
}

/// A row of Helios's own path picker.
#[derive(Clone, Debug, PartialEq)]
enum Place {
    Recent(PathBuf),
    Up(PathBuf),
    Dir(PathBuf),
    File(PathBuf),
}

/// Helios's own path picker, in the picker's look: the folder shown, its
/// subfolders (and files, for a file), a path to type.
pub(crate) struct Browse {
    field: usize,
    hint: PathHint,
    dir: PathBuf,
    rows: Vec<Place>,
    typed: String,
    selected: usize,
    error: Option<String>,
}

impl Browse {
    fn new(field: usize, hint: PathHint, start: &Path, recent: &[PathBuf]) -> Self {
        let mut b = Browse {
            field,
            hint,
            dir: PathBuf::new(),
            rows: Vec::new(),
            typed: String::new(),
            selected: 0,
            error: None,
        };
        if b.go(start, recent).is_err() {
            let fallback = home();
            if let Err(e) = b.go(&fallback, recent) {
                b.dir = fallback;
                b.typed = b.dir.display().to_string();
                b.error = Some(e);
            }
        }
        b
    }

    /// Shows a folder; the folder left is selected when going up.
    fn go(&mut self, dir: &Path, recent: &[PathBuf]) -> Result<(), String> {
        let read = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let (mut dirs, mut files) = (Vec::new(), Vec::new());
        for e in read.flatten() {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = e.path();
            if path.is_dir() {
                dirs.push(path);
            } else if self.hint.kind == PathKind::File && self.hint.accepts(&path) {
                files.push(path);
            }
        }
        let by_name = |p: &PathBuf| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        };
        dirs.sort_by_key(by_name);
        files.sort_by_key(by_name);
        let mut rows = Vec::new();
        if self.hint.kind == PathKind::Dir {
            rows.extend(
                recent
                    .iter()
                    .filter(|r| r.as_path() != dir && r.is_dir())
                    .map(|r| Place::Recent(r.clone())),
            );
        }
        if let Some(up) = dir.parent() {
            rows.push(Place::Up(up.to_path_buf()));
        }
        rows.extend(dirs.into_iter().map(Place::Dir));
        rows.extend(files.into_iter().map(Place::File));
        let left = self.dir.clone();
        self.selected = rows
            .iter()
            .position(|r| matches!(r, Place::Dir(d) if *d == left))
            .unwrap_or(0);
        self.rows = rows;
        self.dir = dir.to_path_buf();
        self.typed = dir.display().to_string();
        self.error = None;
        Ok(())
    }

    /// The path typed, when it is not the folder shown.
    fn typed_path(&self) -> Option<PathBuf> {
        let t = self.typed.trim();
        (!t.is_empty() && t != self.dir.display().to_string()).then(|| expand(t))
    }

    /// What taking a path means: going into a folder, or the answer.
    fn take(&mut self, path: PathBuf, recent: &[PathBuf]) -> Option<PathBuf> {
        if path.is_dir() {
            if let Err(e) = self.go(&path, recent) {
                self.error = Some(e);
            }
            None
        } else if self.hint.kind == PathKind::File && path.is_file() && self.hint.accepts(&path) {
            Some(path)
        } else {
            self.error = Some(format!("Nothing to open at {}", path.display()));
            None
        }
    }

    /// The answer of the primary button: the folder shown (or typed), or
    /// the file selected (or typed).
    fn choose(&mut self, recent: &[PathBuf]) -> Option<PathBuf> {
        match self.hint.kind {
            PathKind::Dir => match self.typed_path() {
                Some(p) if p.is_dir() => Some(p),
                Some(p) => {
                    self.error = Some(format!("No such folder: {}", p.display()));
                    None
                }
                None => Some(self.dir.clone()),
            },
            PathKind::File => {
                if let Some(p) = self.typed_path() {
                    return self.take(p, recent);
                }
                if let Some(Place::File(p)) = self.rows.get(self.selected) {
                    Some(p.clone())
                } else {
                    self.error = Some("Choose a file".into());
                    None
                }
            }
        }
    }
}

/// `~` and `~/…` as the home directory.
fn expand(typed: &str) -> PathBuf {
    match typed.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            home().join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(typed),
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Where browsing for a field starts: the folder its value names (or the
/// file's folder), else home.
fn start_of(value: &str) -> PathBuf {
    let value = value.trim();
    if value.is_empty() {
        return home();
    }
    let p = expand(value);
    if p.is_dir() {
        p
    } else {
        p.parent()
            .filter(|d| d.is_dir())
            .map_or_else(home, Path::to_path_buf)
    }
}

/// Opens the system's chooser on a thread; `None` when it cannot start.
fn native(ctx: &egui::Context, field: usize, hint: &PathHint, start: PathBuf) -> Option<Native> {
    let (tx, answer) = mpsc::channel();
    let ctx = ctx.clone();
    let h = hint.clone();
    std::thread::Builder::new()
        .name("helios-chooser".into())
        .spawn(move || {
            let began = Instant::now();
            let d = rfd::FileDialog::new().set_directory(&start);
            let picked = match h.kind {
                PathKind::Dir => d.set_title("Choose a folder").pick_folder(),
                PathKind::File if h.extensions.is_empty() => {
                    d.set_title("Choose a file").pick_file()
                }
                PathKind::File => d
                    .set_title("Choose a file")
                    .add_filter("Accepted files", &h.extensions)
                    .pick_file(),
            };
            let _ = tx.send((picked, began.elapsed()));
            ctx.request_repaint();
        })
        .ok()?;
    Some(Native {
        field,
        hint: hint.clone(),
        answer,
    })
}

/// How the user (or an agent) answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Primary,
    Cancel,
    /// A form's values, or a picker's choice.
    Values(Json),
}

/// A dialog being shown, with what the user typed so far.
pub(crate) struct Open {
    pub dialog: Dialog,
    /// A form's values, a picker's query.
    pub values: Vec<Value>,
    pub query: String,
    pub selected: usize,
    pub focused: bool,
    /// Helios's own path picker, open for a field.
    pub browse: Option<Browse>,
    /// The system's chooser, open for a field.
    pub native: Option<Native>,
}

impl Open {
    pub fn new(dialog: Dialog) -> Self {
        let values = match &dialog {
            Dialog::Form(f) => f.fields.iter().map(|f| f.value.clone()).collect(),
            _ => Vec::new(),
        };
        Open {
            dialog,
            values,
            query: String::new(),
            selected: 0,
            focused: false,
            browse: None,
            native: None,
        }
    }

    /// Browses for a form's path field: the system's chooser if asked
    /// for and it starts, else Helios's own picker.
    fn start_browse(&mut self, ctx: &egui::Context, field: usize, places: &Places) {
        let Dialog::Form(f) = &self.dialog else {
            return;
        };
        let Some(hint) = f.fields.get(field).and_then(|f| f.descriptor.path.clone()) else {
            return;
        };
        let start = start_of(&self.text(field));
        if places.native {
            self.native = native(ctx, field, &hint, start.clone());
            if self.native.is_some() {
                return;
            }
        }
        self.browse = Some(Browse::new(field, hint, &start, &places.recent));
    }

    /// A text field's value.
    fn text(&self, field: usize) -> String {
        match self.values.get(field) {
            Some(Value::Text(t)) => t.clone(),
            _ => String::new(),
        }
    }

    /// Takes the system chooser's answer when it comes; opens Helios's own
    /// picker when no chooser showed up.
    fn poll_native(&mut self, places: &Places) {
        let Some(n) = &self.native else {
            return;
        };
        let fallback = match n.answer.try_recv() {
            Err(mpsc::TryRecvError::Empty) => return,
            Ok((Some(path), _)) => {
                if let Some(v) = self.values.get_mut(n.field) {
                    *v = Value::Text(path.display().to_string());
                }
                false
            }
            Ok((None, took)) => took < NO_CHOOSER,
            Err(mpsc::TryRecvError::Disconnected) => true,
        };
        let n = self.native.take().expect("checked above");
        if fallback {
            let start = start_of(&self.text(n.field));
            self.browse = Some(Browse::new(n.field, n.hint, &start, &places.recent));
        }
    }

    /// As agents read it.
    pub fn describe(&self) -> Json {
        let mut j = serde_json::to_value(&self.dialog).expect("dialogs serialize");
        if let Dialog::Form(f) = &self.dialog {
            j["values"] = f
                .fields
                .iter()
                .zip(&self.values)
                .map(|(f, v)| (f.key.clone(), value_to_json(v)))
                .collect::<serde_json::Map<_, _>>()
                .into();
            if let Some(b) = &self.browse {
                j["browsing"] = json!({
                    "field": f.fields[b.field].key,
                    "dir": b.dir.display().to_string(),
                });
            }
        }
        j
    }

    /// The command an answer runs, with its arguments; `None` to close
    /// without running anything.
    pub fn resolve(&self, answer: &Answer) -> Result<Option<(String, Json)>, String> {
        match (&self.dialog, answer) {
            (_, Answer::Cancel) | (Dialog::Info(_), _) => Ok(None),
            (Dialog::Confirm(c), Answer::Primary) => Ok(Some((c.command.clone(), c.args.clone()))),
            (Dialog::Confirm(_), Answer::Values(_)) => {
                Err("a confirmation takes primary or cancel".into())
            }
            (Dialog::Form(f), answer) => {
                let mut args = serde_json::Map::new();
                for (field, value) in f.fields.iter().zip(&self.values) {
                    args.insert(field.key.clone(), value_to_json(value));
                }
                if let Answer::Values(Json::Object(given)) = answer {
                    for (k, v) in given {
                        let field = f
                            .fields
                            .iter()
                            .find(|x| x.key == *k)
                            .ok_or_else(|| format!("no field `{k}`"))?;
                        let typed = value_from_json(field.descriptor.ty, v)?;
                        args.insert(k.clone(), value_to_json(&typed));
                    }
                }
                Ok(Some((f.command.clone(), Json::Object(args))))
            }
            (Dialog::Picker(p), Answer::Values(v)) => {
                let value = v
                    .as_str()
                    .or_else(|| v.get(&p.arg).and_then(Json::as_str))
                    .ok_or("say which choice")?;
                if !p.choices.iter().any(|c| c.value == value) {
                    return Err(format!("`{value}` is not a choice"));
                }
                Ok(Some((p.command.clone(), json!({ p.arg.as_str(): value }))))
            }
            (Dialog::Picker(p), Answer::Primary) => {
                let shown = self.filtered(p);
                let c = shown.get(self.selected).ok_or("nothing to choose")?;
                Ok(Some((
                    p.command.clone(),
                    json!({ p.arg.as_str(): c.value }),
                )))
            }
        }
    }

    fn filtered<'a>(&self, p: &'a Picker) -> Vec<&'a Choice> {
        let words: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        p.choices
            .iter()
            .filter(|c| {
                let hay =
                    format!("{} {}", c.label, c.detail.as_deref().unwrap_or("")).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect()
    }
}

/// Sizes and colors the kinds share, from the theme.
#[derive(Clone)]
pub(crate) struct Look {
    pub width: f32,
    pub pad: f32,
    pub toast: f32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub danger: Color32,
    pub warning: Color32,
}

impl Look {
    pub fn of(r: &helios_theme::Resolved) -> Self {
        let c = |t: &str| crate::theme::color(r, t);
        Look {
            width: r.size("size.dialog"),
            pad: r.size("space.dialog"),
            toast: r.size("size.toast"),
            accent: c("accent.base"),
            accent_text: c("accent.text"),
            danger: c("danger"),
            warning: c("warning"),
        }
    }
}

/// A button of the standard footer.
fn footer_button(ui: &mut Ui, look: &Look, label: &str, kind: Option<bool>) -> egui::Response {
    // `Some(danger)`: the primary button; `None`: the secondary one.
    let text = RichText::new(label);
    let button = match kind {
        Some(true) => egui::Button::new(text.color(Color32::WHITE)).fill(look.danger),
        Some(false) => egui::Button::new(text.color(look.accent_text)).fill(look.accent),
        None => egui::Button::new(text),
    };
    ui.add(button.min_size(egui::vec2(88.0, 0.0)))
}

/// The footer every dialog has: Cancel then the primary action, on the
/// right. Returns the answer clicked.
fn footer(ui: &mut Ui, look: &Look, verb: &str, danger: bool, name: &str) -> Option<Answer> {
    footer_of(ui, look, verb, "Cancel", danger, name)
}

/// The footer with its secondary button named (Cancel, Back).
fn footer_of(
    ui: &mut Ui,
    look: &Look,
    verb: &str,
    secondary: &str,
    danger: bool,
    name: &str,
) -> Option<Answer> {
    let mut answer = None;
    ui.add_space(look.pad * 0.5);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let primary = footer_button(ui, look, verb, Some(danger));
        crate::name_rect(ui.ctx(), &format!("{name}.primary"), primary.rect);
        if primary.clicked() {
            answer = Some(Answer::Primary);
        }
        let cancel = footer_button(ui, look, secondary, None);
        crate::name_rect(ui.ctx(), &format!("{name}.cancel"), cancel.rect);
        if cancel.clicked() {
            answer = Some(Answer::Cancel);
        }
        // Destructive: the focus starts on the safe action.
        if danger && !ui.memory(|m| m.focused().is_some()) {
            cancel.request_focus();
        }
    });
    answer
}

/// A field's widget, from its descriptor; true when its Browse button
/// was clicked.
fn field_ui(ui: &mut Ui, field: &Field, value: &mut Value, places: &Places, waiting: bool) -> bool {
    let d = &field.descriptor;
    ui.label(&d.label);
    let mut browse = false;
    match (d.ty, &mut *value) {
        (Type::Text, Value::Text(v)) if d.path.is_some() => {
            ui.horizontal(|ui| {
                let room = ui.available_width() - ui.spacing().item_spacing.x - 80.0;
                ui.add(egui::TextEdit::singleline(v).desired_width(room.max(120.0)));
                let b = ui.add_enabled(!waiting, egui::Button::new("Browse…"));
                crate::name_rect(
                    ui.ctx(),
                    &format!("helios.dialog.browse.{}", field.key),
                    b.rect,
                );
                browse = b.clicked();
            });
            // A directory: the recent workspaces, one click away.
            if d.path.as_ref().is_some_and(|h| h.kind == PathKind::Dir) && !places.recent.is_empty()
            {
                ui.end_row();
                ui.weak("recent");
                ui.vertical(|ui| {
                    for p in places.recent.iter().take(5) {
                        let shown = p.display().to_string();
                        if list_row(ui, *v == shown, &shown, None).clicked() {
                            *v = shown;
                        }
                    }
                });
            }
        }
        (Type::Float, Value::Float(v)) => {
            let (lo, hi) = d.soft.or(d.hard).unwrap_or((f64::MIN, f64::MAX));
            let mut shown = *v * d.display_scale;
            let mut drag = egui::DragValue::new(&mut shown)
                .range(lo * d.display_scale..=hi * d.display_scale)
                .speed(d.step * d.display_scale)
                .max_decimals(d.precision as usize);
            if let Some(u) = &d.unit {
                drag = drag.suffix(format!(" {u}"));
            }
            if ui.add(drag).changed() {
                *v = shown / d.display_scale;
            }
        }
        (Type::Int, Value::Int(v)) => {
            let (lo, hi) = d.hard.unwrap_or((i64::MIN as f64, i64::MAX as f64));
            ui.add(egui::DragValue::new(v).range(lo as i64..=hi as i64));
        }
        (Type::Bool, Value::Bool(v)) => {
            ui.checkbox(v, "");
        }
        (Type::Choice, Value::Choice(v)) => {
            egui::ComboBox::from_id_salt(("helios.form", field.key.as_str()))
                .selected_text(v.clone())
                .show_ui(ui, |ui| {
                    for c in &d.choices {
                        ui.selectable_value(v, c.clone(), c);
                    }
                });
        }
        (Type::Text, Value::Text(v)) => {
            ui.text_edit_singleline(v);
        }
        _ => {
            ui.weak(format!("{value:?}"));
        }
    }
    ui.end_row();
    browse
}

/// Helios's own path picker, drawn in the dialog: `Some(Some(path))` when
/// one is chosen, `Some(None)` to go back to the form.
#[allow(clippy::option_option)]
fn browse_ui(
    ui: &mut Ui,
    look: &Look,
    b: &mut Browse,
    recent: &[PathBuf],
    (enter, down, up): (bool, bool, bool),
) -> Option<Option<PathBuf>> {
    let edit = ui.add(
        egui::TextEdit::singleline(&mut b.typed)
            .hint_text("Type a path")
            .desired_width(f32::INFINITY),
    );
    crate::name_rect(ui.ctx(), "helios.dialog.path", edit.rect);
    // The path keeps the keys: arrows move in the list, Enter opens.
    edit.request_focus();
    if let Some(e) = &b.error {
        ui.colored_label(look.warning, e);
    }
    let n = b.rows.len();
    if n > 0 {
        if down {
            b.selected = (b.selected + 1) % n;
        }
        if up {
            b.selected = (b.selected + n - 1) % n;
        }
    }
    ui.separator();
    let mut taken = None;
    egui::ScrollArea::vertical()
        .max_height(look.width * 0.75)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (i, row) in b.rows.iter().enumerate() {
                let (label, detail, path) = match row {
                    Place::Recent(p) => (p.display().to_string(), Some("recent"), p),
                    Place::Up(p) => ("..".to_string(), Some("up"), p),
                    Place::Dir(p) | Place::File(p) => (
                        p.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        matches!(row, Place::Dir(_)).then_some("folder"),
                        p,
                    ),
                };
                let r = list_row(ui, i == b.selected, &label, detail);
                if i == b.selected && (down || up) {
                    r.scroll_to_me(None);
                }
                if r.clicked() {
                    b.selected = i;
                    taken = Some(path.clone());
                }
            }
            if n == 0 {
                ui.weak(match b.hint.kind {
                    PathKind::Dir => "No folder here",
                    PathKind::File => "Nothing to choose here",
                });
            }
        });
    if taken.is_none() && enter {
        taken = b.typed_path().or_else(|| {
            b.rows.get(b.selected).map(|r| match r {
                Place::Recent(p) | Place::Up(p) | Place::Dir(p) | Place::File(p) => p.clone(),
            })
        });
    }
    let verb = match b.hint.kind {
        PathKind::Dir => "Choose this folder",
        PathKind::File => "Choose",
    };
    let clicked = footer_of(ui, look, verb, "Back", false, "helios.dialog");
    if let Some(p) = taken
        && let Some(chosen) = b.take(p, recent)
    {
        return Some(Some(chosen));
    }
    match clicked {
        Some(Answer::Primary) => b.choose(recent).map(Some),
        Some(_) => Some(None),
        None => None,
    }
}

/// Draws the open dialog, modal; returns the answer given this frame.
pub(crate) fn dialog_ui(
    ctx: &egui::Context,
    look: &Look,
    open: &mut Open,
    places: &Places,
) -> Option<Answer> {
    open.poll_native(places);
    // The keys of this frame belong to the path picker when it is open.
    let browsing = open.browse.is_some();
    let (enter, escape, down, up) = ctx.input(|i| {
        (
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::Escape),
            i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::ArrowUp),
        )
    });
    let mut answer = None;
    let title = match &open.dialog {
        Dialog::Confirm(c) => c.title.clone(),
        Dialog::Form(f) => match &open.browse {
            Some(b) if b.hint.kind == PathKind::Dir => "Choose a folder".into(),
            Some(_) => "Choose a file".into(),
            None => f.title.clone(),
        },
        Dialog::Picker(p) => p.title.clone(),
        Dialog::Info(i) => i.title.clone(),
    };
    let modal = egui::Modal::new(Id::new("helios.dialog")).show(ctx, |ui| {
        ui.set_width(look.width);
        ui.add_space(look.pad * 0.25);
        ui.label(RichText::new(&title).heading().strong());
        ui.add_space(look.pad * 0.5);
        match &mut open.dialog {
            Dialog::Confirm(c) => {
                ui.label(&c.detail);
                ui.add_space(look.pad * 0.5);
                answer = footer(ui, look, &c.verb, c.danger, "helios.dialog");
            }
            Dialog::Form(f) => {
                if let Some(b) = &mut open.browse {
                    match browse_ui(ui, look, b, &places.recent, (enter, down, up)) {
                        Some(Some(path)) => {
                            open.values[b.field] = Value::Text(path.display().to_string());
                            open.browse = None;
                        }
                        Some(None) => open.browse = None,
                        None => {}
                    }
                } else {
                    let waiting = open.native.is_some();
                    let mut asked = None;
                    egui::Grid::new("helios.form")
                        .num_columns(2)
                        .spacing([look.pad, look.pad * 0.5])
                        .show(ui, |ui| {
                            for (i, (field, value)) in
                                f.fields.iter().zip(open.values.iter_mut()).enumerate()
                            {
                                if field_ui(ui, field, value, places, waiting) {
                                    asked = Some(i);
                                }
                            }
                        });
                    if waiting {
                        ui.weak("Waiting for the system's file chooser…");
                    }
                    answer = footer(ui, look, &f.verb, false, "helios.dialog");
                    if let Some(i) = asked {
                        open.start_browse(ui.ctx(), i, places);
                    }
                }
            }
            Dialog::Info(i) => {
                for line in &i.lines {
                    ui.label(line);
                }
                ui.add_space(look.pad * 0.5);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let close = footer_button(ui, look, "Close", Some(false));
                    crate::name_rect(ui.ctx(), "helios.dialog.primary", close.rect);
                    if close.clicked() {
                        answer = Some(Answer::Cancel);
                    }
                });
            }
            Dialog::Picker(p) => {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut open.query)
                        .hint_text("Type to search")
                        .desired_width(f32::INFINITY),
                );
                if !open.focused {
                    edit.request_focus();
                    open.focused = true;
                }
                if edit.changed() {
                    open.selected = 0;
                }
                let words: Vec<String> = open
                    .query
                    .split_whitespace()
                    .map(str::to_lowercase)
                    .collect();
                let shown: Vec<&Choice> = p
                    .choices
                    .iter()
                    .filter(|c| {
                        let hay = format!("{} {}", c.label, c.detail.as_deref().unwrap_or(""))
                            .to_lowercase();
                        words.iter().all(|w| hay.contains(w))
                    })
                    .collect();
                let n = shown.len();
                if n > 0 {
                    if down {
                        open.selected = (open.selected + 1) % n;
                    }
                    if up {
                        open.selected = (open.selected + n - 1) % n;
                    }
                }
                ui.separator();
                for (i, c) in shown.iter().enumerate() {
                    let r = list_row(ui, i == open.selected, &c.label, c.detail.as_deref());
                    if r.clicked() {
                        answer = Some(Answer::Values(json!(c.value)));
                    }
                }
                if n == 0 {
                    ui.weak("Nothing matches");
                }
            }
        }
    });
    crate::name_rect(ctx, "helios.dialog", modal.response.rect);
    if browsing {
        // Escape leaves the path picker, not the form.
        if open.browse.is_some() && (escape || modal.should_close()) {
            open.browse = None;
        }
        return answer;
    }
    if answer.is_none() {
        if escape || modal.should_close() {
            answer = Some(Answer::Cancel);
        } else if enter {
            // Enter means the primary action, except on a destructive
            // confirmation whose focus is on Cancel.
            let safe_focused = matches!(&open.dialog, Dialog::Confirm(c) if c.danger);
            answer = Some(if safe_focused {
                Answer::Cancel
            } else {
                Answer::Primary
            });
        }
    }
    answer
}

/// A row of a list (the palette, a picker): its label, its detail on the
/// right.
pub(crate) fn list_row(
    ui: &mut Ui,
    selected: bool,
    label: &str,
    detail: Option<&str>,
) -> egui::Response {
    ui.horizontal(|ui| {
        let r = ui.selectable_label(selected, label);
        if let Some(d) = detail {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.weak(d);
            });
        }
        r
    })
    .inner
}

/// One entry of a menu, as drawn.
pub(crate) struct Entry {
    pub id: String,
    pub args: Json,
    pub title: String,
    pub keys: Option<String>,
    /// Why it does not apply here, if it does not.
    pub disabled: Option<String>,
    /// Shown ticked (the theme in use).
    pub checked: bool,
}

/// A menu resolved against the registry, the keymap and the context.
pub(crate) enum Row {
    Entry(Entry),
    Separator,
    Submenu(String, Vec<Row>),
}

/// The width of menus.
const MENU_WIDTH: f32 = 260.0;

/// Draws an open menu at a position; returns the command picked, or
/// `Some(None)` when it should close without one.
#[allow(clippy::option_option)]
pub(crate) fn menu_ui(
    ctx: &egui::Context,
    at: egui::Pos2,
    rows: &[Row],
    selected: &mut usize,
) -> Option<Option<(String, Json)>> {
    let entries: Vec<&Entry> = rows
        .iter()
        .filter_map(|r| match r {
            Row::Entry(e) if e.disabled.is_none() => Some(e),
            _ => None,
        })
        .collect();
    let (enter, escape, down, up) = ctx.input(|i| {
        (
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::Escape),
            i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::ArrowUp),
        )
    });
    let n = entries.len();
    if n > 0 {
        if down {
            *selected = (*selected + 1) % n;
        }
        if up {
            *selected = (*selected + n - 1) % n;
        }
    }
    let mut picked = None;
    let area = egui::Area::new(Id::new("helios.menu"))
        .order(Order::Foreground)
        .fixed_pos(at)
        .pivot(Align2::LEFT_TOP)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(MENU_WIDTH);
                ui.set_max_width(MENU_WIDTH);
                let mut k = 0;
                draw_rows(ui, rows, *selected, &mut k, &mut picked);
            })
        });
    crate::name_rect(ctx, "helios.menu", area.response.rect);
    if let Some(p) = picked {
        return Some(Some(p));
    }
    if enter && let Some(e) = entries.get(*selected) {
        return Some(Some((e.id.clone(), e.args.clone())));
    }
    let clicked_away = ctx.input(|i| i.pointer.any_pressed())
        && ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| !area.response.rect.contains(p));
    (escape || clicked_away).then_some(None)
}

fn draw_rows(
    ui: &mut Ui,
    rows: &[Row],
    selected: usize,
    k: &mut usize,
    picked: &mut Option<(String, Json)>,
) {
    for row in rows {
        match row {
            Row::Separator => {
                ui.separator();
            }
            Row::Submenu(label, sub) => {
                ui.horizontal(|ui| {
                    // Aligned with the entries (their tick's room), no
                    // frame, an arrow on the right.
                    // The tick's room, plus the padding a selectable label
                    // has and a frameless button lacks.
                    let pad = ui.spacing().button_padding.x;
                    ui.allocate_exact_size(egui::vec2(14.0 + pad, 14.0), egui::Sense::hover());
                    ui.style_mut().visuals.button_frame = false;
                    let r = ui
                        .menu_button(label, |ui| draw_rows(ui, sub, usize::MAX, &mut 0, picked))
                        .response;
                    let arrow = egui::pos2(ui.max_rect().right() - 8.0, r.rect.center().y);
                    let ink = ui.visuals().weak_text_color();
                    ui.painter().line_segment(
                        [arrow + egui::vec2(-3.0, -4.0), arrow],
                        egui::Stroke::new(1.5, ink),
                    );
                    ui.painter().line_segment(
                        [arrow, arrow + egui::vec2(-3.0, 4.0)],
                        egui::Stroke::new(1.5, ink),
                    );
                });
            }
            Row::Entry(e) => {
                let enabled = e.disabled.is_none();
                let is_selected = enabled && *k == selected;
                if enabled {
                    *k += 1;
                }
                let r = ui
                    .add_enabled_ui(enabled, |ui| {
                        ui.horizontal(|ui| {
                            // A tick, painted (not every font has one).
                            let (tick, _) = ui
                                .allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                            if e.checked {
                                let c = tick.center();
                                let stroke = egui::Stroke::new(1.5, ui.visuals().text_color());
                                ui.painter().line_segment(
                                    [c + egui::vec2(-4.0, 0.0), c + egui::vec2(-1.0, 3.0)],
                                    stroke,
                                );
                                ui.painter().line_segment(
                                    [c + egui::vec2(-1.0, 3.0), c + egui::vec2(4.5, -3.5)],
                                    stroke,
                                );
                            }
                            let r = ui.selectable_label(is_selected, &e.title);
                            if let Some(keys) = &e.keys {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.weak(keys);
                                });
                            }
                            r
                        })
                        .inner
                    })
                    .inner;
                let r = match &e.disabled {
                    Some(why) => r.on_disabled_hover_text(why),
                    None => r,
                };
                if r.clicked() {
                    *picked = Some((e.id.clone(), e.args.clone()));
                }
            }
        }
    }
}

/// How long a notice stays as a toast, in seconds; errors stay until
/// dismissed.
const TOAST_SECONDS: f64 = 6.0;

/// Draws the toasts of recent notices, bottom right, without taking the
/// focus; returns a command an action button asked for, and the notices
/// dismissed.
pub(crate) fn toasts_ui(
    ctx: &egui::Context,
    look: &Look,
    notices: &[Notice],
    dismissed: &mut Vec<usize>,
) -> Option<String> {
    let now = ctx.input(|i| i.time);
    let shown: Vec<(usize, &Notice)> = notices
        .iter()
        .enumerate()
        .filter(|(i, n)| {
            n.level != Level::Info
                && !dismissed.contains(i)
                && (n.level == Level::Error || now - n.at < TOAST_SECONDS)
        })
        .rev()
        .take(3)
        .collect();
    if shown.is_empty() {
        return None;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    let mut run = None;
    let area = egui::Area::new(Id::new("helios.toasts"))
        .order(Order::Foreground)
        .anchor(Align2::RIGHT_BOTTOM, [-look.pad, -(look.pad + 24.0)])
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_width(look.toast);
            for (i, n) in shown {
                let mark = match n.level {
                    Level::Error => look.danger,
                    Level::Warning => look.warning,
                    Level::Info => look.accent,
                };
                egui::Frame::popup(ui.style())
                    .stroke(egui::Stroke::new(1.0, mark))
                    .show(ui, |ui| {
                        ui.set_width(look.toast);
                        ui.horizontal_top(|ui| {
                            // A mark of the level's color (painted: not
                            // every font has a dot).
                            let (dot, _) = ui.allocate_exact_size(
                                egui::vec2(10.0, ui.text_style_height(&egui::TextStyle::Body)),
                                egui::Sense::hover(),
                            );
                            ui.painter().circle_filled(dot.center(), 4.0, mark);
                            ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                                if ui.small_button("×").on_hover_text("Dismiss").clicked() {
                                    dismissed.push(i);
                                }
                                if let Some((label, command)) = &n.action
                                    && ui.small_button(label).clicked()
                                {
                                    run = Some(command.clone());
                                    dismissed.push(i);
                                }
                                ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
                                    ui.add(egui::Label::new(&n.text).wrap());
                                });
                            });
                        });
                    });
            }
        });
    crate::name_rect(ctx, "helios.toasts", area.response.rect);
    run
}
