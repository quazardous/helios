//! The import window (`docs/design/import-window.md`): File > Import…
//! (Ctrl+Shift+I), the empty Library's "Import photos…" and a card's
//! toast open it (`library.importWindow`).
//!
//! - **Sources**, the left column: the cards and cameras mounted
//!   ([`helios_storage`], polled off the interface thread), first; then
//!   the recent folders, the favourites (added and removed here) and
//!   "Choose a folder…" (egui-pop's folder chooser). Several can be
//!   ticked. As it opens, the window ticks the first card, else the last
//!   source used.
//! - **Contents**, the middle column: what the sources ticked hold, read
//!   off the interface thread ([`crate::import_scan`]) and shown as it
//!   comes, by capture day; each photo ticked by default, those probably
//!   in the Library already greyed while "New only" is on, a RAW+JPEG
//!   pair one cell. Import takes the files ticked (`library.importFiles`
//!   with `paths`), a card's copied into the Library, a folder's added
//!   where they are.
//! - **Choices**: to come.
//! - **A card inserted** while the window is closed: a toast whose action
//!   opens the window on it.
//!
//! The recent folders, the favourites and the last source are kept in a
//! small JSON file when the desktop gives one
//! ([`Shell::remember_import_sources_in`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use egui::Context as Egui;
use egui_pop::api::files::{self, FILES, FileRequest};
use egui_pop::api::host::Pending;
use egui_pop::api::toasts::{TOASTS, Toast};
use egui_pop::core::overlay::DialogLayout;
use egui_pop::model::ids::CommandId;
use helios_core::Message;
use helios_shell::commands::Origin;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Shell;
use crate::import_scan::{Contents, Counts, Known, Scan, Scanned, Types};
use helios_storage::{Card, Volumes, Watch};

/// The command that opens the window (`source`: the card or folder to
/// open it on).
pub const COMMAND: &str = "library.importWindow";

/// What is kept between runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Memory {
    #[serde(default)]
    recent: Vec<PathBuf>,
    #[serde(default)]
    favourites: Vec<PathBuf>,
    #[serde(default)]
    last: Option<PathBuf>,
}

/// The window's state, kept by the shell.
#[derive(Default)]
pub(crate) struct ImportWindow {
    open: bool,
    watch: Option<Watch>,
    /// The context the cards' thread asks a frame of.
    ctx: Arc<Mutex<Option<Egui>>>,
    cards: Vec<Card>,
    memory: Memory,
    file: Option<PathBuf>,
    /// The sources ticked, in the order ticked.
    ticked: Vec<PathBuf>,
    /// Whether the person ticked or unticked a source since it opened.
    touched: bool,
    /// The folder chooser, while it is open.
    choosing: Option<Pending>,
    /// What the vault holds, for the "in your Library" prefilter.
    known: Option<Known>,
    /// Each source ticked being read, and whether it is read through.
    scans: Vec<(PathBuf, Scan, bool)>,
    contents: Contents,
    /// The cell a Shift+click extends from, and the one Space toggles.
    anchor: Option<usize>,
    focus: Option<usize>,
}

impl ImportWindow {
    fn save(&self) {
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = serde_json::to_string_pretty(&self.memory).unwrap_or_default();
        if let Err(e) = std::fs::write(file, text) {
            log::warn!("import sources: {}: {e}", file.display());
        }
    }

    /// Stops reading and forgets what was read.
    fn clear_contents(&mut self) {
        self.scans.clear();
        self.contents = Contents::new();
        self.anchor = None;
        self.focus = None;
    }

    /// Reads the sources ticked (each on a thread of its own), forgets
    /// the ones unticked, and takes in what the reads sent.
    fn follow_scans(&mut self) {
        let ticked = self.ticked.clone();
        let before = self.scans.len();
        self.scans.retain(|(p, _, _)| ticked.contains(p));
        if self.scans.len() != before {
            self.contents.keep_sources(&ticked);
            self.anchor = None;
            self.focus = None;
        }
        for path in &ticked {
            if self.scans.iter().any(|(p, _, _)| p == path) {
                continue;
            }
            let ctx = self.ctx.clone();
            let scan = Scan::start(path.clone(), self.known.clone(), move || {
                if let Some(ctx) = ctx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                    ctx.request_repaint();
                }
            });
            self.scans.push((path.clone(), scan, false));
        }
        for (_, scan, done) in &mut self.scans {
            for sent in scan.take() {
                match sent {
                    Scanned::Found(found) => self.contents.add(found),
                    Scanned::Glance { path, taken, thumb } => {
                        self.contents.glance(&path, taken, thumb);
                    }
                    Scanned::Done => *done = true,
                }
            }
        }
    }

    fn is_card(&self, path: &Path) -> bool {
        self.cards.iter().any(|c| c.root == path)
    }

    fn tick(&mut self, path: &Path, on: bool) {
        self.ticked.retain(|p| p != path);
        if on {
            self.ticked.push(path.to_path_buf());
        }
    }

    /// The cards as last polled; the first one ticked while the person
    /// has not chosen and no card is ticked.
    fn follow_cards(&mut self) {
        let Some(watch) = &self.watch else { return };
        self.cards = watch.cards();
        let cards = &self.cards;
        if self.open
            && !self.touched
            && let Some(first) = self.cards.first()
            && !self
                .ticked
                .iter()
                .any(|p| cards.iter().any(|c| &c.root == p))
        {
            self.ticked = vec![first.root.clone()];
        }
    }
}

/// What the window's controls asked for this frame.
enum Event {
    Cancel,
    Import,
    Choose,
    Favourite(PathBuf, bool),
}

/// The Contents column's words.
struct GridWords {
    counts: String,
    reading: String,
    empty: String,
    new_only: String,
    types: String,
    all_types: String,
    raw_only: String,
    jpeg_only: String,
    select: String,
    all: String,
    none: String,
    day: String,
    no_date: String,
    in_library: String,
    pair: String,
}

/// The window's words, in the shell's language.
struct Words {
    title: String,
    sources: String,
    cards: String,
    no_cards: String,
    recent: String,
    favourites: String,
    no_favourites: String,
    choose: String,
    add_favourite: String,
    remove_favourite: String,
    contents: String,
    grid: GridWords,
    choices: String,
    choices_card: String,
    choices_folder: String,
    cancel: String,
    import_count: String,
    nothing: String,
    none_ticked: String,
}

fn name_of(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl Shell {
    /// Watches the cards and cameras of `volumes` for the import window
    /// and its toasts (the desktop gives `helios_storage::system()`; tests a list
    /// of their own).
    pub fn watch_cards(&mut self, volumes: Arc<dyn Volumes>) {
        let ctx = self.import_window.ctx.clone();
        self.import_window.watch = Some(Watch::start(volumes, move || {
            if let Some(ctx) = ctx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                ctx.request_repaint();
            }
        }));
    }

    /// Keeps the import window's recent folders, favourites and last
    /// source in `file` (JSON), read now.
    pub fn remember_import_sources_in(&mut self, file: PathBuf) {
        self.import_window.memory = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        self.import_window.file = Some(file);
    }

    /// Whether the import window is open.
    pub fn import_window_open(&self) -> bool {
        self.import_window.open
    }

    /// The import window's cards, as last polled.
    pub fn import_cards(&self) -> Vec<Card> {
        self.import_window.cards.clone()
    }

    /// What the vault holds (size, partial hash), for the import window's
    /// "in your Library" prefilter; called once per source read, off the
    /// interface thread.
    pub fn set_import_known(&mut self, known: Known) {
        self.import_window.known = Some(known);
    }

    /// The import window's counts: photos shown, new, their size, and the
    /// ones an Import takes.
    pub fn import_counts(&self) -> Counts {
        self.import_window.contents.counts()
    }

    /// Whether every source ticked in the import window is read through.
    pub fn import_read(&self) -> bool {
        let w = &self.import_window;
        w.scans.len() == w.ticked.len() && w.scans.iter().all(|(_, _, done)| *done)
    }

    /// The import window's cells: name, day, probably in the Library, a
    /// RAW+JPEG pair, ticked.
    pub fn import_cells(&self) -> Vec<(String, Option<chrono::NaiveDate>, bool, bool, bool)> {
        self.import_window
            .contents
            .cells
            .iter()
            .map(|c| (c.name(), c.day(), c.known(), c.is_pair(), c.ticked))
            .collect()
    }

    /// The sources ticked in the import window.
    pub fn import_ticked(&self) -> Vec<PathBuf> {
        self.import_window.ticked.clone()
    }

    /// `library.importWindow`: opens the import window, on `source` when
    /// given, else on the first card, else on the last source used.
    pub(super) fn open_import_window(&mut self, args: &Value) {
        let w = &mut self.import_window;
        w.follow_cards();
        w.clear_contents();
        w.open = true;
        w.touched = false;
        w.ticked.clear();
        if let Some(source) = args
            .get("source")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            w.ticked.push(PathBuf::from(source));
            w.touched = true;
        } else if let Some(card) = w.cards.first() {
            w.ticked.push(card.root.clone());
        } else if let Some(last) = w.memory.last.clone().filter(|p| p.exists()) {
            w.ticked.push(last);
        }
    }

    /// Each frame: the cards polled, a toast for each card inserted while
    /// the window is closed, and the window when open.
    pub(super) fn import_window_frame(&mut self, ctx: &Egui) {
        {
            let mut held = self
                .import_window
                .ctx
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if held.is_none() {
                *held = Some(ctx.clone());
            }
        }
        self.import_window.follow_cards();
        let inserted = self
            .import_window
            .watch
            .as_ref()
            .map(Watch::take_inserted)
            .unwrap_or_default();
        if !self.import_window.open {
            for card in inserted {
                self.card_toast(&card);
            }
        }
        self.receive_chosen_folder();
        if self.import_window.open {
            self.import_window.follow_scans();
            self.import_window_ui(ctx);
        }
    }

    /// "`EOS_DIGITAL` inserted": its action opens the window on the card.
    fn card_toast(&mut self, card: &Card) {
        let text = self.text(
            Message::new("gui-import-window-card-inserted", "{ $card } inserted")
                .with("card", card.label.clone()),
        );
        let action = self.text(Message::new("gui-import-window-open", "Import photos…"));
        if let Some(toasts) = self.app.handle().service(&TOASTS) {
            toasts.notify(Toast::info(&text).action(
                &action,
                CommandId::from_static(COMMAND),
                json!({ "source": card.root }),
            ));
        }
        self.notes.push(text);
    }

    /// The folder the chooser gave: a recent source, ticked.
    fn receive_chosen_folder(&mut self) {
        let Some(result) = self.import_window.choosing.as_ref().and_then(Pending::take) else {
            return;
        };
        self.import_window.choosing = None;
        if let Ok(files::Chosen::Paths(paths)) = files::chosen(&result) {
            let w = &mut self.import_window;
            for p in paths {
                w.memory.recent.retain(|r| r != &p);
                w.memory.recent.insert(0, p.clone());
                w.memory.recent.truncate(8);
                w.tick(&p, true);
                w.touched = true;
            }
            w.save();
        }
    }

    fn import_words(&self) -> Words {
        let t = |m: Message| self.text(m);
        Words {
            title: t(Message::new("gui-import-window-title", "Import photos")),
            sources: t(Message::new("gui-import-window-sources", "Sources")),
            cards: t(Message::new("gui-import-window-cards", "Cards and cameras")),
            no_cards: t(Message::new(
                "gui-import-window-no-cards",
                "No card inserted",
            )),
            recent: t(Message::new("gui-import-window-recent", "Recent")),
            favourites: t(Message::new("gui-import-window-favourites", "Favourites")),
            no_favourites: t(Message::new("gui-import-window-no-favourites", "None yet")),
            choose: t(Message::new("gui-import-window-choose", "Choose a folder…")),
            add_favourite: t(Message::new(
                "gui-import-window-add-favourite",
                "Add to favourites",
            )),
            remove_favourite: t(Message::new(
                "gui-import-window-remove-favourite",
                "Remove from favourites",
            )),
            contents: t(Message::new("gui-import-window-contents", "Contents")),
            grid: self.grid_words(),
            choices: t(Message::new("gui-import-window-choices", "Choices")),
            choices_card: t(Message::new(
                "gui-import-window-choices-card",
                "A card is copied into the Library.",
            )),
            choices_folder: t(Message::new(
                "gui-import-window-choices-folder",
                "A folder is added where it is.",
            )),
            cancel: t(Message::new("gui-import-window-cancel", "Cancel")),
            import_count: t(
                Message::new("gui-import-window-import", "Import { $count }")
                    .with("count", self.import_window.contents.counts().selected),
            ),
            nothing: t(Message::new(
                "gui-import-window-nothing",
                "Tick a card or a folder to import",
            )),
            none_ticked: t(Message::new(
                "gui-import-window-none-ticked",
                "Tick the photos to import",
            )),
        }
    }

    fn grid_words(&self) -> GridWords {
        let t = |m: Message| self.text(m);
        let counts = self.import_window.contents.counts();
        let size = self
            .import_window
            .ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or_else(
                || helios_storage::capacity_text(counts.bytes),
                |ctx| crate::library::size_text(ctx, counts.bytes),
            );
        GridWords {
            counts: t(Message::new(
                "gui-import-window-counts",
                "{ $photos } photos, { $new } new, { $size }",
            )
            .with("photos", counts.photos)
            .with("new", counts.new)
            .with("size", size)),
            reading: t(Message::new("gui-import-window-reading", "Reading…")),
            empty: t(Message::new(
                "gui-import-window-empty",
                "No photos in the sources ticked",
            )),
            new_only: t(Message::new("gui-import-window-new-only", "New only")),
            types: t(Message::new("gui-import-window-types", "Types")),
            all_types: t(Message::new("gui-import-window-all-types", "All types")),
            raw_only: t(Message::new("gui-import-window-raw-only", "Raw only")),
            jpeg_only: t(Message::new("gui-import-window-jpeg-only", "JPEG only")),
            select: t(Message::new("gui-import-window-select", "Select:")),
            all: t(Message::new("gui-import-window-select-all", "all")),
            none: t(Message::new("gui-import-window-select-none", "none")),
            day: t(Message::new("gui-import-window-select-day", "day")),
            no_date: t(Message::new("gui-import-window-no-date", "No date")),
            in_library: t(Message::new(
                "gui-import-window-in-library",
                "in your Library",
            )),
            pair: t(Message::new("gui-import-window-pair", "RAW+JPG")),
        }
    }

    fn import_window_ui(&mut self, ctx: &Egui) {
        let words = self.import_words();
        let mut open = true;
        let mut event = None;
        let w = &mut self.import_window;
        let max = ctx.content_rect().size() - egui::vec2(32.0, 32.0);
        egui::Window::new(&words.title)
            .id(egui::Id::unique("helios.import.window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(960.0, 560.0).min(max))
            .max_size(max)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                let layout = DialogLayout::new(ui, "helios.import.window").fill(true);
                layout.body(ui, |ui, height| {
                    ui.set_min_height(height);
                    ui.columns(3, |cols| {
                        sources_ui(&mut cols[0], w, &words, &mut event);
                        cols[1].heading(&words.contents);
                        let reading = w.scans.iter().any(|(_, _, done)| !done);
                        contents_ui(&mut cols[1], w, &words.grid, reading);
                        cols[2].heading(&words.choices);
                        cols[2].label(&words.choices_card);
                        cols[2].label(&words.choices_folder);
                    });
                });
                layout.footer(ui, |ui| {
                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let count = w.contents.counts().selected;
                        let why = if w.ticked.is_empty() {
                            &words.nothing
                        } else {
                            &words.none_ticked
                        };
                        let import = ui
                            .add_enabled(count > 0, egui::Button::new(&words.import_count))
                            .on_disabled_hover_text(why);
                        if import.clicked() {
                            event = Some(Event::Import);
                        }
                        if ui.button(&words.cancel).clicked() {
                            event = Some(Event::Cancel);
                        }
                    });
                });
            });
        if !open {
            event = Some(Event::Cancel);
        }
        match event {
            None => {}
            Some(Event::Cancel) => {
                self.import_window.open = false;
                self.import_window.clear_contents();
            }
            Some(Event::Choose) => {
                if let Some(f) = self.app.handle().service(&FILES) {
                    let request = FileRequest::folder(&words.choose).purpose("helios.import");
                    self.import_window.choosing = Some(f.choose(self.app.handle(), request));
                }
            }
            Some(Event::Favourite(path, on)) => {
                let w = &mut self.import_window;
                w.memory.favourites.retain(|p| p != &path);
                if on {
                    w.memory.favourites.push(path);
                }
                w.save();
            }
            Some(Event::Import) => self.import_ticked_sources(),
        }
    }

    /// Imports the photos ticked of each source ticked, as
    /// `library.importFiles` does with their `paths`: a card's copied, a
    /// folder's added where they are; the window closes at once.
    fn import_ticked_sources(&mut self) {
        let ticked = std::mem::take(&mut self.import_window.ticked);
        for path in &ticked {
            let files = self.import_window.contents.selected_files(path);
            if files.is_empty() {
                continue;
            }
            let card = self.import_window.is_card(path);
            let args = json!({ "paths": files, "copy": card });
            if let Err(e) = self.import_files(&args, &Origin::User) {
                self.warn(e);
            }
            let w = &mut self.import_window;
            if !card {
                w.memory.recent.retain(|r| r != path);
                w.memory.recent.insert(0, path.clone());
                w.memory.recent.truncate(8);
            }
            w.memory.last = Some(path.clone());
        }
        self.import_window.save();
        self.import_window.open = false;
        self.import_window.clear_contents();
    }
}

/// The Sources column: cards first, then recent folders, favourites and
/// "Choose a folder…".
fn sources_ui(ui: &mut egui::Ui, w: &mut ImportWindow, words: &Words, event: &mut Option<Event>) {
    ui.heading(&words.sources);
    egui::ScrollArea::vertical()
        .id_salt("helios.import.sources")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.strong(&words.cards);
            if w.cards.is_empty() {
                ui.weak(&words.no_cards);
            }
            let cards = w.cards.clone();
            for card in &cards {
                let label = match card.capacity {
                    Some(b) => format!("{} · {}", card.label, helios_storage::capacity_text(b)),
                    None => card.label.clone(),
                };
                source_row(ui, w, &card.root, label, None);
            }
            ui.separator();
            ui.strong(&words.recent);
            let favourites = w.memory.favourites.clone();
            for path in w.memory.recent.clone() {
                let star = (!favourites.contains(&path)).then_some((&words.add_favourite, true));
                if let Some(e) = source_row(ui, w, &path, name_of(&path), star) {
                    *event = Some(e);
                }
            }
            ui.strong(&words.favourites);
            if favourites.is_empty() {
                ui.weak(&words.no_favourites);
            }
            for path in favourites {
                let remove = Some((&words.remove_favourite, false));
                if let Some(e) = source_row(ui, w, &path, name_of(&path), remove) {
                    *event = Some(e);
                }
            }
            ui.separator();
            if ui.button(&words.choose).clicked() {
                *event = Some(Event::Choose);
            }
        });
}

/// A source's row: its tick, and a favourite's button when given.
fn source_row(
    ui: &mut egui::Ui,
    w: &mut ImportWindow,
    path: &Path,
    label: String,
    favourite: Option<(&String, bool)>,
) -> Option<Event> {
    let mut event = None;
    ui.horizontal(|ui| {
        let mut on = w.ticked.iter().any(|p| p == path);
        if ui
            .checkbox(&mut on, label)
            .on_hover_text(path.display().to_string())
            .changed()
        {
            w.tick(path, on);
            w.touched = true;
        }
        if let Some((text, add)) = favourite {
            let icon = if add { "☆" } else { "✕" };
            if ui.small_button(icon).on_hover_text(text).clicked() {
                event = Some(Event::Favourite(path.to_path_buf(), add));
            }
        }
    });
    event
}

/// The side of a cell of the grid, in points.
const CELL: f32 = 88.0;

/// The Contents column: the count line, the filters and selection, then
/// the grid by day.
fn contents_ui(ui: &mut egui::Ui, w: &mut ImportWindow, words: &GridWords, reading: bool) {
    let c = &mut w.contents;
    ui.horizontal(|ui| {
        ui.label(&words.counts);
        if reading {
            ui.spinner();
            ui.weak(&words.reading);
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut c.new_only, &words.new_only);
        let mut types = c.types;
        let name = |t: Types| match t {
            Types::All => &words.all_types,
            Types::Raw => &words.raw_only,
            Types::Jpeg => &words.jpeg_only,
        };
        egui::ComboBox::new("helios.import.types", &words.types)
            .selected_text(name(types))
            .show_ui(ui, |ui| {
                for t in [Types::All, Types::Raw, Types::Jpeg] {
                    ui.selectable_value(&mut types, t, name(t));
                }
            });
        c.set_types(types);
        ui.label(&words.select);
        if ui.small_button(&words.all).clicked() {
            c.tick_all(true);
        }
        if ui.small_button(&words.none).clicked() {
            c.tick_all(false);
        }
        // The focused photo's day, alone.
        let day = w.focus.map(|i| c.cells[i].day());
        if ui
            .add_enabled(day.is_some(), egui::Button::new(&words.day).small())
            .clicked()
            && let Some(day) = day
        {
            let types = c.types;
            for cell in &mut c.cells {
                cell.ticked = cell.day() == day || !cell.passes(types);
            }
        }
    });
    ui.separator();
    // The previews read become textures, a few each frame.
    let mut made = 0;
    for (i, cell) in c.cells.iter_mut().enumerate() {
        if made == 48 {
            ui.ctx().request_repaint();
            break;
        }
        if let Some(thumb) = cell.thumb.take() {
            cell.texture = Some(ui.ctx().load_texture(
                format!("helios.import.thumb.{i}"),
                crate::library::thumbnail(&thumb),
                egui::TextureOptions::LINEAR,
            ));
            made += 1;
        }
    }
    if c.cells.is_empty() && !reading {
        ui.weak(&words.empty);
        return;
    }
    let shown = c.shown();
    let days = c.days().to_vec();
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("helios.import.grid")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (day, cells) in &days {
                let day_text = day.map_or_else(|| words.no_date.clone(), |d| d.to_string());
                let mut all = cells.iter().all(|&i| c.cells[i].ticked || !c.importable(i));
                if ui
                    .checkbox(&mut all, format!("{day_text} · {}", cells.len()))
                    .changed()
                {
                    for &i in cells {
                        c.cells[i].ticked = all;
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    for &i in cells {
                        if cell_ui(ui, c, i, w.focus == Some(i), words) {
                            clicked = Some(i);
                        }
                    }
                });
            }
        });
    let shift = ui.input(|i| i.modifiers.shift);
    if let Some(i) = clicked {
        // Shift extends the anchor's tick to every cell between.
        if let Some(anchor) = w.anchor.filter(|_| shift) {
            let on = c.cells[anchor].ticked;
            let (a, b) = (
                shown.iter().position(|&x| x == anchor),
                shown.iter().position(|&x| x == i),
            );
            if let (Some(a), Some(b)) = (a, b) {
                for &x in &shown[a.min(b)..=a.max(b)] {
                    c.cells[x].ticked = on;
                }
            }
        } else {
            c.cells[i].ticked = !c.cells[i].ticked;
            w.anchor = Some(i);
        }
        w.focus = Some(i);
    }
    if let Some(i) = w.focus
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space))
    {
        c.cells[i].ticked = !c.cells[i].ticked;
        w.anchor = Some(i);
    }
}

/// A cell: its preview, its tick, greyed with "in your Library" when New
/// only leaves it out, "RAW+JPG" on a pair; its name, time and size on
/// hover. Whether it was clicked.
fn cell_ui(ui: &mut egui::Ui, c: &Contents, i: usize, focused: bool, words: &GridWords) -> bool {
    let cell = &c.cells[i];
    let importable = c.importable(i);
    let greyed = c.new_only && cell.known();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(CELL, CELL), egui::Sense::click());
    let mut label = cell.name();
    if greyed {
        label = format!("{label} · {}", words.in_library);
    }
    if cell.is_pair() {
        label = format!("{label} · {}", words.pair);
    }
    let ticked = cell.ticked && importable;
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::CheckBox, importable, ticked, &label)
    });
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
        if let Some(texture) = &cell.texture {
            let size = texture.size_vec2();
            let inner = rect.shrink(4.0);
            let scale = (inner.width() / size.x).min(inner.height() / size.y);
            let image = egui::Rect::from_center_size(inner.center(), size * scale);
            let tint = if greyed {
                egui::Color32::from_gray(90)
            } else {
                egui::Color32::WHITE
            };
            painter.image(
                texture.id(),
                image,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                tint,
            );
        }
        let small = egui::FontId::proportional(10.0);
        if greyed {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &words.in_library,
                small.clone(),
                visuals.strong_text_color(),
            );
        }
        if cell.is_pair() {
            let text =
                painter.layout_no_wrap(words.pair.clone(), small, visuals.strong_text_color());
            let at = rect.left_bottom() + egui::vec2(4.0, -4.0 - text.size().y);
            painter.rect_filled(
                egui::Rect::from_min_size(at, text.size()).expand(2.0),
                2.0,
                visuals.widgets.inactive.bg_fill,
            );
            painter.galley(at, text, visuals.strong_text_color());
        }
        if importable {
            let tick =
                egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(14.0, 14.0));
            let fill = if ticked {
                visuals.selection.bg_fill
            } else {
                visuals.widgets.inactive.bg_fill
            };
            painter.rect_filled(tick, 2.0, fill);
            if ticked {
                painter.text(
                    tick.center(),
                    egui::Align2::CENTER_CENTER,
                    "✔",
                    egui::FontId::proportional(11.0),
                    visuals.selection.stroke.color,
                );
            }
        }
        let stroke = if focused {
            visuals.selection.stroke
        } else if response.hovered() {
            visuals.widgets.hovered.bg_stroke
        } else {
            egui::Stroke::NONE
        };
        painter.rect_stroke(rect.shrink(0.5), 4.0, stroke, egui::StrokeKind::Inside);
    }
    let size = crate::library::size_text(ui.ctx(), cell.bytes);
    let time = cell.time().map_or_else(
        || words.no_date.clone(),
        |t| t.format("%Y-%m-%d %H:%M:%S").to_string(),
    );
    let response = response.on_hover_text(format!("{}\n{time}\n{size}", cell.name()));
    response.clicked() && importable
}
