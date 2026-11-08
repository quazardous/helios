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
//!   with `paths` and the choices).
//! - **Choices**, the right column ([`super::import_choices`]): copy or
//!   add where they are, where copies go and how they are named, with
//!   examples from the first photo ticked; duplicates; darktable edits.
//! - **The footer**: the preset menu (Default, the presets saved, "Save
//!   as preset…", "Delete preset"), the totals of the photos ticked,
//!   Cancel and Import.
//! - **A card inserted** while the window is closed: a toast whose action
//!   opens the window on it.
//! - **Files or folders dropped** on Helios, or pasted: the window opens
//!   with them as its sources, a file's folder with that file ticked
//!   (`paths`).
//!
//! The recent folders, the favourites and the last source are kept in a
//! small JSON file when the desktop gives one
//! ([`Shell::remember_import_sources_in`]).

use std::collections::BTreeMap;
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
use super::import_choices::{self, Asked, ChoiceWords, Choices};
use crate::import_scan::{Contents, Counts, Known, MapRecorder, Scan, Scanned, Types};
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
/// The command that forgets a folder in the image map (registered by
/// the desktop over its vault).
pub const FORGET_FOLDER: &str = "library.forgetFolder";
/// The command that forgets a volume in the image map.
pub const FORGET_VOLUME: &str = "library.forgetVolume";

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
    pub(super) ticked: Vec<PathBuf>,
    /// Whether the person ticked or unticked a source since it opened.
    touched: bool,
    /// The folder chooser, while it is open: for a source, or for Into.
    choosing: Option<Pending>,
    choosing_into: Option<Pending>,
    /// What the vault holds, for the "in your Library" prefilter.
    known: Option<Known>,
    /// The image map's recorder: what the sources list is kept.
    map: Option<MapRecorder>,
    /// The image map's job, told which files are in view (read first).
    map_feed: Option<helios_library::map_job::Feed>,
    /// The files in view last told to it.
    on_screen: Vec<PathBuf>,
    /// The sources given by a drop or a paste (recorded as `drop`).
    dropped: Vec<PathBuf>,
    /// Each source ticked being read, and whether it is read through.
    scans: Vec<(PathBuf, Scan, bool)>,
    pub(super) contents: Contents,
    /// The choices, the last used until changed, and where they are kept.
    pub(super) choices: Choices,
    pub(super) choices_file: Option<PathBuf>,
    /// Copy or add as the person switched it since the window opened.
    pub(super) copy: Option<bool>,
    /// The custom rename pattern is being edited.
    pub(super) custom: bool,
    /// The cell a Shift+click extends from, and the one Space toggles.
    anchor: Option<usize>,
    focus: Option<usize>,
    /// The files dropped or pasted: in their folder, only they are ticked.
    picked: Vec<PathBuf>,
    /// The presets, by name, and where they are kept.
    pub(super) presets: BTreeMap<String, Choices>,
    pub(super) presets_file: Option<PathBuf>,
    /// The name of the preset being saved, while it is typed, and
    /// whether its field is to take the focus.
    naming: Option<(String, bool)>,
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
            let seen_by = if self.dropped.contains(path) {
                "drop"
            } else {
                "import"
            };
            let map = self.map.clone().map(|m| (m, seen_by));
            let scan = Scan::start(path.clone(), self.known.clone(), map, move || {
                if let Some(ctx) = ctx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                    ctx.request_repaint();
                }
            });
            self.scans.push((path.clone(), scan, false));
        }
        for (_, scan, done) in &mut self.scans {
            for sent in scan.take() {
                match sent {
                    Scanned::Found(found) => {
                        let at = self.contents.add(found);
                        let cell = &mut self.contents.cells[at];
                        let picked = &self.picked;
                        if picked
                            .iter()
                            .any(|p| p.parent() == Some(cell.source.as_path()))
                        {
                            cell.ticked = cell
                                .raw
                                .iter()
                                .chain(&cell.picture)
                                .any(|f| picked.contains(f));
                        }
                    }
                    Scanned::Glance {
                        path,
                        taken,
                        camera,
                        thumb,
                    } => {
                        self.contents.glance(&path, taken, camera, thumb);
                    }
                    Scanned::Done => *done = true,
                }
            }
        }
    }

    pub(super) fn is_card(&self, path: &Path) -> bool {
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
    /// A preset applied (`None`: Default).
    Preset(Option<String>),
    SavePreset(String),
    DeletePreset(String),
    Choose,
    ChooseInto,
    Favourite(PathBuf, bool),
    /// "Forget this folder" (`library.forgetFolder`).
    Forget(PathBuf),
    /// "Storage…": the Storage page, to set up a backup.
    Storage,
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
    forget: String,
    forget_hint: String,
    contents: String,
    grid: GridWords,
    choices: ChoiceWords,
    cancel: String,
    import_count: String,
    totals: String,
    preset: String,
    default_preset: String,
    last_used: String,
    save_preset: String,
    preset_name: String,
    save: String,
    delete_preset: String,
    nothing: String,
    none_ticked: String,
    /// Why nothing can be imported: no library open.
    no_library: Option<String>,
    /// The strategy line: what backups the photos will get.
    backup: String,
    /// No strategy: the line offers the Storage page.
    no_backup: bool,
    storage: String,
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

    /// Calls `f` (off the interface thread) when a volume appears or goes,
    /// once [`Shell::watch_cards`] watches them: the background volume
    /// scan's cue.
    pub fn on_volumes_changed(&self, f: impl Fn() + Send + 'static) {
        if let Some(watch) = &self.import_window.watch {
            watch.on_volumes(f);
        }
    }

    /// Whether the volumes are watched ([`Shell::watch_cards`]).
    pub(super) fn watching_volumes(&self) -> bool {
        self.import_window.watch.is_some()
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

    /// The image map's recorder for what the import window's sources
    /// list (`docs/design/image-map.md`, section 4); called once per
    /// source read, off the interface thread.
    pub fn set_import_map(&mut self, map: MapRecorder) {
        self.import_window.map = Some(map);
    }

    /// The image map's job: the import window, the Library grid and
    /// Compare tell it the files in view.
    pub fn set_map_feed(&mut self, feed: helios_library::map_job::Feed) {
        self.library.borrow_mut().set_map_feed(feed.clone());
        self.import_window.map_feed = Some(feed);
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
    /// given, or on the files and folders of `paths` (a folder a source,
    /// a file its folder with only the files given ticked; `copy` sets
    /// copy or add), else on the first card, else on the last source used.
    pub(super) fn open_import_window(&mut self, args: &Value) {
        // The strategy line reads the strategy and its disks as they are.
        self.refresh_volumes();
        let w = &mut self.import_window;
        w.follow_cards();
        w.clear_contents();
        w.open = true;
        w.touched = false;
        w.copy = None;
        w.custom = false;
        w.naming = None;
        w.ticked.clear();
        w.picked.clear();
        w.dropped.clear();
        let paths: Vec<PathBuf> = args
            .get("paths")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        if let Some(copy) = args.get("copy").and_then(Value::as_bool) {
            w.copy = Some(copy);
        }
        if !paths.is_empty() {
            for path in paths {
                let source = if path.is_dir() {
                    path
                } else {
                    let Some(dir) = path.parent().map(Path::to_path_buf) else {
                        continue;
                    };
                    w.picked.push(path);
                    dir
                };
                if !w.dropped.contains(&source) {
                    w.dropped.push(source.clone());
                }
                if !w.ticked.contains(&source) {
                    w.ticked.push(source);
                }
            }
            w.touched = true;
        } else if let Some(source) = args
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
        // A folder given is listed with the recent ones, to show it ticked.
        let given: Vec<PathBuf> = w
            .ticked
            .iter()
            .filter(|p| !w.is_card(p) && !w.memory.recent.contains(p))
            .cloned()
            .collect();
        if !given.is_empty() {
            for p in given.into_iter().rev() {
                w.memory.recent.insert(0, p);
            }
            w.memory.recent.truncate(8);
            w.save();
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
        if let Some(result) = self
            .import_window
            .choosing_into
            .as_ref()
            .and_then(Pending::take)
        {
            self.import_window.choosing_into = None;
            if let Ok(files::Chosen::Paths(paths)) = files::chosen(&result)
                && let Some(p) = paths.into_iter().next()
            {
                self.import_window.choices.into = Some(p);
            }
        }
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

    fn import_words(&self, sample: Option<&import_choices::Sample>) -> Words {
        let t = |m: Message| self.text(m);
        let backup = self.import_backup_line();
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
            forget: t(Message::new(
                "gui-import-window-forget",
                "Forget this folder",
            )),
            forget_hint: t(Message::new(
                "gui-import-window-forget-hint",
                "Drops what the image map knows of this folder's files not in the Library; it is not recorded again until you list it",
            )),
            contents: t(Message::new("gui-import-window-contents", "Contents")),
            grid: self.grid_words(),
            choices: self.choice_words(sample),
            cancel: t(Message::new("gui-import-window-cancel", "Cancel")),
            import_count: t(
                Message::new("gui-import-window-import", "Import { $count }")
                    .with("count", self.import_window.contents.counts().selected),
            ),
            totals: {
                let counts = self.import_window.contents.counts();
                t(
                    Message::new("gui-import-window-totals", "{ $photos } photos, { $size }")
                        .with("photos", counts.selected)
                        .with("size", self.import_size_text(counts.selected_bytes)),
                )
            },
            preset: t(Message::new("gui-import-window-preset", "Preset")),
            default_preset: t(Message::new("gui-import-window-preset-default", "Default")),
            last_used: t(Message::new(
                "gui-import-window-preset-last-used",
                "Last used",
            )),
            save_preset: t(Message::new(
                "gui-import-window-save-preset",
                "Save as preset…",
            )),
            preset_name: t(Message::new("gui-import-window-preset-name", "Preset name")),
            save: t(Message::new("gui-import-window-preset-save", "Save")),
            delete_preset: t(Message::new(
                "gui-import-window-delete-preset",
                "Delete preset",
            )),
            nothing: t(Message::new(
                "gui-import-window-nothing",
                "Tick a card or a folder to import",
            )),
            none_ticked: t(Message::new(
                "gui-import-window-none-ticked",
                "Tick the photos to import",
            )),
            no_library: self.no_library.clone().map(t),
            backup: backup.0,
            no_backup: backup.1,
            storage: t(Message::new("gui-import-backup-storage", "Storage…")),
        }
    }

    /// A size, as the Library says it.
    fn import_size_text(&self, bytes: u64) -> String {
        self.import_window
            .ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or_else(
                || helios_storage::capacity_text(bytes),
                |ctx| crate::library::size_text(ctx, bytes),
            )
    }

    fn grid_words(&self) -> GridWords {
        let t = |m: Message| self.text(m);
        let counts = self.import_window.contents.counts();
        let size = self.import_size_text(counts.bytes);
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
        let sample = self.import_window.sample();
        let words = self.import_words(sample.as_ref());
        let mut open = true;
        let mut event = None;
        let w = &mut self.import_window;
        let main = ctx.content_rect().size();
        let max = main - egui::vec2(32.0, 32.0);
        egui::Window::new(&words.title)
            .id(egui::Id::unique("helios.import.window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(default_size(main))
            .min_size(MIN_SIZE.min(max))
            .max_size(max)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |outer| {
                // The window has the size the person gives it, never its
                // content's: an `egui::Window` grows to what its content
                // took the frame before, so a content a point too wide
                // grew it frame after frame. The content lays out in the
                // room it is given, apart, and the window holds that room.
                let room = outer.available_rect_before_wrap();
                outer.advance_cursor_after_rect(room);
                let mut ui = outer.new_child(
                    egui::UiBuilder::new()
                        .id_salt("helios.import.room")
                        .max_rect(room)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                ui.set_clip_rect(room.intersect(ui.clip_rect()));
                let ui = &mut ui;
                let layout = DialogLayout::new(ui, "helios.import.window").fill(true);
                layout.body(ui, |ui, height| {
                    // Sources and Choices keep their widths (a quarter of
                    // the room at most); Contents takes the rest, each
                    // column in its own rect, a line between them.
                    let gap = ui.spacing().item_spacing.x;
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), height),
                        egui::Sense::hover(),
                    );
                    let [left, middle, right] = columns(rect, gap);
                    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
                    for x in [left.max.x + gap, right.min.x - gap] {
                        ui.painter().vline(x, rect.y_range(), stroke);
                    }
                    let column =
                        |ui: &mut egui::Ui,
                         rect: egui::Rect,
                         add: &mut dyn FnMut(&mut egui::Ui)| {
                            ui.scope_builder(
                                egui::UiBuilder::new()
                                    .max_rect(rect)
                                    .layout(egui::Layout::top_down(egui::Align::Min)),
                                |ui| {
                                    ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                                    add(ui);
                                },
                            );
                        };
                    column(ui, left, &mut |ui| sources_ui(ui, w, &words, &mut event));
                    column(ui, middle, &mut |ui| {
                        ui.heading(&words.contents);
                        let reading = w.scans.iter().any(|(_, _, done)| !done);
                        contents_ui(ui, w, &words.grid, reading);
                    });
                    column(ui, right, &mut |ui| {
                        if let Some(Asked::Into) = import_choices::choices_ui(ui, w, &words.choices)
                        {
                            event = Some(Event::ChooseInto);
                        }
                        ui.separator();
                        if backup_line_ui(ui, &words) {
                            event = Some(Event::Storage);
                        }
                    });
                });
                layout.footer(ui, |ui| {
                    ui.separator();
                    ui.horizontal(|ui| {
                        if let Some(e) = presets_ui(ui, w, &words) {
                            event = Some(e);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let count = w.contents.counts().selected;
                            let bad_name = words.choices.bad_name.as_ref().filter(|_| w.copies());
                            let why = if let Some(e) = &words.no_library {
                                e
                            } else if w.ticked.is_empty() {
                                &words.nothing
                            } else if let Some(e) = bad_name {
                                e
                            } else {
                                &words.none_ticked
                            };
                            let import = ui
                                .add_enabled(
                                    count > 0 && bad_name.is_none() && words.no_library.is_none(),
                                    egui::Button::new(&words.import_count),
                                )
                                .on_disabled_hover_text(why);
                            if import.clicked() {
                                event = Some(Event::Import);
                            }
                            if ui.button(&words.cancel).clicked() {
                                event = Some(Event::Cancel);
                            }
                            if let Some(e) = &words.no_library {
                                ui.colored_label(ui.visuals().warn_fg_color, e);
                            } else if count > 0 {
                                ui.label(&words.totals);
                            }
                        });
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
            Some(Event::ChooseInto) => {
                if let Some(f) = self.app.handle().service(&FILES) {
                    let request =
                        FileRequest::folder(&words.choices.change).purpose("helios.import.into");
                    self.import_window.choosing_into = Some(f.choose(self.app.handle(), request));
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
            Some(Event::Forget(path)) => {
                let args = serde_json::json!({ "path": path.display().to_string() });
                if let Err(e) = self.run(FORGET_FOLDER, args, Origin::User) {
                    self.warn(format!("forget: {e}"));
                }
            }
            Some(Event::Import) => self.import_ticked_sources(),
            Some(Event::Storage) => {
                if let Err(e) = self.run(super::storage::COMMAND, Value::Null, Origin::User) {
                    self.warn(format!("storage: {e}"));
                }
            }
            Some(Event::Preset(name)) => self.apply_import_preset(name.as_deref()),
            Some(Event::SavePreset(name)) => self.save_import_preset(&name),
            Some(Event::DeletePreset(name)) => self.delete_import_preset(&name),
        }
    }

    /// Imports the photos ticked of each source ticked, as
    /// `library.importFiles` does with their `paths` and the choices
    /// (copied or added where they are, filed, named, duplicates,
    /// darktable edits), kept as the last used; the window closes at
    /// once. Without a library open, nothing is imported: a toast says
    /// why and the window stays.
    fn import_ticked_sources(&mut self) {
        // No library to import into: said, the window left open.
        if let Err(e) = self.can_import() {
            self.warn(e);
            return;
        }
        let copy = self.import_window.copies();
        let any_card = self
            .import_window
            .ticked
            .iter()
            .any(|p| self.import_window.is_card(p));
        if !any_card {
            self.import_window.choices.copy_folders = copy;
        }
        self.import_window.save_choices();
        let choices = self.import_window.choices.clone();
        let ticked = std::mem::take(&mut self.import_window.ticked);
        for path in &ticked {
            let files = self.import_window.contents.selected_files(path);
            if files.is_empty() {
                continue;
            }
            let card = self.import_window.is_card(path);
            let args = choices.args(&files, copy);
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
/// The strategy line under the choices: "2 copies: Backup disk (plugged
/// in), NAS (not seen for 3 days)", or "No backup set up:" with a button
/// to the Storage page; whether it was pressed.
fn backup_line_ui(ui: &mut egui::Ui, words: &Words) -> bool {
    ui.horizontal_wrapped(|ui| {
        if words.no_backup {
            ui.colored_label(ui.visuals().warn_fg_color, &words.backup);
            ui.button(&words.storage).clicked()
        } else {
            ui.add(egui::Label::new(&words.backup).wrap());
            false
        }
    })
    .inner
}

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
                if let Some(e) = source_row(ui, w, words, &card.root, label, None) {
                    *event = Some(e);
                }
            }
            ui.separator();
            ui.strong(&words.recent);
            let favourites = w.memory.favourites.clone();
            for path in w.memory.recent.clone() {
                let star = (!favourites.contains(&path)).then_some((&words.add_favourite, true));
                if let Some(e) = source_row(ui, w, words, &path, name_of(&path), star) {
                    *event = Some(e);
                }
            }
            ui.strong(&words.favourites);
            if favourites.is_empty() {
                ui.weak(&words.no_favourites);
            }
            for path in favourites {
                let remove = Some((&words.remove_favourite, false));
                if let Some(e) = source_row(ui, w, words, &path, name_of(&path), remove) {
                    *event = Some(e);
                }
            }
            ui.separator();
            if ui.button(&words.choose).clicked() {
                *event = Some(Event::Choose);
            }
        });
}

/// A source's row: its tick, a favourite's button when given, and
/// "Forget this folder" in its menu.
fn source_row(
    ui: &mut egui::Ui,
    w: &mut ImportWindow,
    words: &Words,
    path: &Path,
    label: String,
    favourite: Option<(&String, bool)>,
) -> Option<Event> {
    let mut event = None;
    ui.horizontal(|ui| {
        let mut on = w.ticked.iter().any(|p| p == path);
        let tick = ui
            .checkbox(&mut on, label)
            .on_hover_text(path.display().to_string());
        if tick.changed() {
            w.tick(path, on);
            w.touched = true;
        }
        tick.context_menu(|ui| {
            if ui
                .button(&words.forget)
                .on_hover_text(&words.forget_hint)
                .clicked()
            {
                event = Some(Event::Forget(path.to_path_buf()));
                ui.close();
            }
        });
        if let Some((text, add)) = favourite {
            let icon = if add { "☆" } else { "✕" };
            if ui.small_button(icon).on_hover_text(text).clicked() {
                event = Some(Event::Favourite(path.to_path_buf(), add));
            }
        }
    });
    event
}

/// The widths of the Sources and Choices columns, in points (at most a
/// quarter of the window each); Contents takes the rest.
const SOURCES_WIDTH: f32 = 220.0;
const CHOICES_WIDTH: f32 = 300.0;

/// The import window's least size and the largest it opens at, in points.
const MIN_SIZE: egui::Vec2 = egui::vec2(560.0, 360.0);
const MAX_DEFAULT_SIZE: egui::Vec2 = egui::vec2(1200.0, 800.0);

/// The size the import window opens at in a main window of `main`: four
/// fifths of it, at most [`MAX_DEFAULT_SIZE`], at least [`MIN_SIZE`] while
/// the main window holds it.
fn default_size(main: egui::Vec2) -> egui::Vec2 {
    let max = (main - egui::vec2(32.0, 32.0)).max(egui::Vec2::ZERO);
    (main * 0.8).min(MAX_DEFAULT_SIZE).max(MIN_SIZE).min(max)
}

/// The rects of the Sources, Contents and Choices columns in `rect`, a
/// line between two columns with `gap` on each side of it: the three
/// columns and the two gaps fill `rect` exactly, never more.
fn columns(rect: egui::Rect, gap: f32) -> [egui::Rect; 3] {
    let total = rect.width();
    let lines = 4.0 * gap;
    let side = |w: f32| w.min((total - lines) / 4.0).max(0.0);
    let (left, right) = (side(SOURCES_WIDTH), side(CHOICES_WIDTH));
    let middle = (total - left - right - lines).max(0.0);
    let at = |x: f32, w: f32| {
        egui::Rect::from_min_size(egui::pos2(x, rect.min.y), egui::vec2(w, rect.height()))
    };
    let middle_x = rect.min.x + left + 2.0 * gap;
    [
        at(rect.min.x, left),
        at(middle_x, middle),
        at(middle_x + middle + 2.0 * gap, right),
    ]
}

/// The footer's preset menu: Default, the presets saved, "Save as
/// preset…" (a name asked in place) and "Delete preset" (the one the
/// choices are).
fn presets_ui(ui: &mut egui::Ui, w: &mut ImportWindow, words: &Words) -> Option<Event> {
    let mut event = None;
    let current = w.preset_of_choices();
    let shown = match &current {
        Some(name) => name.clone(),
        None if w.choices == Choices::default() => words.default_preset.clone(),
        None => words.last_used.clone(),
    };
    egui::ComboBox::new("helios.import.preset", &words.preset)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            let default = current.is_none() && w.choices == Choices::default();
            if ui
                .selectable_label(default, &words.default_preset)
                .clicked()
            {
                event = Some(Event::Preset(None));
            }
            for name in w.presets.keys() {
                if ui
                    .selectable_label(current.as_ref() == Some(name), name)
                    .clicked()
                {
                    event = Some(Event::Preset(Some(name.clone())));
                }
            }
            ui.separator();
            if ui.selectable_label(false, &words.save_preset).clicked() {
                w.naming = Some((current.clone().unwrap_or_default(), true));
            }
            if ui
                .add_enabled(
                    current.is_some(),
                    egui::Button::selectable(false, &words.delete_preset),
                )
                .clicked()
                && let Some(name) = &current
            {
                event = Some(Event::DeletePreset(name.clone()));
            }
        });
    if let Some((name, focus)) = &mut w.naming {
        let edit = ui.add(
            egui::TextEdit::singleline(name)
                .hint_text(&words.preset_name)
                .desired_width(140.0),
        );
        if std::mem::take(focus) {
            edit.request_focus();
        }
        let entered = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let name = name.trim().to_string();
        let save = ui.add_enabled(!name.is_empty(), egui::Button::new(&words.save));
        if (save.clicked() || entered) && !name.is_empty() {
            event = Some(Event::SavePreset(name));
            w.naming = None;
        } else if ui.small_button("×").on_hover_text(&words.cancel).clicked() {
            w.naming = None;
        }
    }
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
    let mut in_view = Vec::new();
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
                        if cell_ui(ui, c, i, w.focus == Some(i), words, &mut in_view) {
                            clicked = Some(i);
                        }
                    }
                });
            }
        });
    if let Some(feed) = &w.map_feed
        && in_view != w.on_screen
    {
        feed.on_screen(in_view.clone());
        w.on_screen = in_view;
    }
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
/// hover. Whether it was clicked; its files go to `in_view` when it is
/// in view.
fn cell_ui(
    ui: &mut egui::Ui,
    c: &Contents,
    i: usize,
    focused: bool,
    words: &GridWords,
    in_view: &mut Vec<PathBuf>,
) -> bool {
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
        in_view.extend(cell.raw.iter().chain(&cell.picture).cloned());
        if cell.raw.is_none() && cell.picture.is_none() {
            in_view.push(cell.source.clone());
        }
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
