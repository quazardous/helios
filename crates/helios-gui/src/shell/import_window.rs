//! The import window (`docs/design/import-window.md`): File > Import…
//! (Ctrl+Shift+I), the empty Library's "Import photos…" and a card's
//! toast open it (`library.importWindow`).
//!
//! - **Sources**, the left column: the cards and cameras mounted
//!   ([`crate::volumes`], polled off the interface thread), first; then
//!   the recent folders, the favourites (added and removed here) and
//!   "Choose a folder…" (egui-pop's folder chooser). Several can be
//!   ticked. As it opens, the window ticks the first card, else the last
//!   source used.
//! - **Contents** and **Choices**: to come; for now Import imports each
//!   source ticked through `library.importSource`, a card copied into the
//!   Library, a folder added where it is.
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
use crate::volumes::{self, Volume, Volumes, Watch};

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
    cards: Vec<Volume>,
    memory: Memory,
    file: Option<PathBuf>,
    /// The sources ticked, in the order ticked.
    ticked: Vec<PathBuf>,
    /// Whether the person ticked or unticked a source since it opened.
    touched: bool,
    /// The folder chooser, while it is open.
    choosing: Option<Pending>,
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
    contents_soon: String,
    choices: String,
    choices_card: String,
    choices_folder: String,
    cancel: String,
    import: String,
    nothing: String,
}

fn name_of(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl Shell {
    /// Watches the cards and cameras of `volumes` for the import window
    /// and its toasts (the desktop gives `volumes::system()`; tests a list
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
    pub fn import_cards(&self) -> Vec<Volume> {
        self.import_window.cards.clone()
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
            self.import_window_ui(ctx);
        }
    }

    /// "`EOS_DIGITAL` inserted": its action opens the window on the card.
    fn card_toast(&mut self, card: &Volume) {
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
            contents_soon: t(Message::new(
                "gui-import-window-contents-soon",
                "The photos of the sources ticked will show here.",
            )),
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
            import: t(Message::new("gui-import-verb", "Import")),
            nothing: t(Message::new(
                "gui-import-window-nothing",
                "Tick a card or a folder to import",
            )),
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
                        cols[1].weak(&words.contents_soon);
                        cols[2].heading(&words.choices);
                        cols[2].label(&words.choices_card);
                        cols[2].label(&words.choices_folder);
                    });
                });
                layout.footer(ui, |ui| {
                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can = !w.ticked.is_empty();
                        let import = ui
                            .add_enabled(can, egui::Button::new(&words.import))
                            .on_disabled_hover_text(&words.nothing);
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
            Some(Event::Cancel) => self.import_window.open = false,
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

    /// Imports each source ticked as `library.importSource` does, a card
    /// copied, a folder added where it is; the window closes at once.
    fn import_ticked_sources(&mut self) {
        let ticked = std::mem::take(&mut self.import_window.ticked);
        for path in &ticked {
            let card = self.import_window.is_card(path);
            let args = json!({ "path": path, "copy": card });
            if let Err(e) = self.import_from_source(&args, &Origin::User) {
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
                    Some(b) => format!("{} · {}", card.label, volumes::capacity_text(b)),
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
