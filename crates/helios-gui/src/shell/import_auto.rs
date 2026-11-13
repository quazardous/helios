//! Automatic imports (`docs/design/import-connectors.md`, section 3.10):
//! a saved source may say "when this source has new photos, import them
//! with its choices", without the import window.
//!
//! - **Its trigger** (`auto` on the source, in `user/import-sources`):
//!   a card "when plugged in", a folder "when new files appear". It is
//!   set in the source's menu ("Import automatically") and in the
//!   wizard's choices step, the editor's included.
//! - **A card plugged in** (the node's volume watch, `volume.mounted`)
//!   whose saved source says so has its photos imported, copied, with
//!   the source's choices; the "inserted" toast gives way to the import's
//!   own notice.
//! - **A folder** is rescanned on a thread of its own every few seconds
//!   (no filesystem events: a network share raises none). A file is
//!   imported once its size and time have held still for the settling
//!   delay (a copy in progress is left alone), and once per watch.
//! - **Already imported** photos are skipped as duplicates by their
//!   content: an automatic import never keeps nor asks about one,
//!   whatever the source's choices say.
//! - The import is the host's job, as the window's: one undo step of the
//!   Library, its report in the Log panel and its end said.
//! - **Another client's change** to the sources (`settings.layerChanged`
//!   naming the document) is read again ([`Shell::follow_import_sources`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use egui::Context as Egui;
use helios_core::Message;
use helios_shell::commands::Origin;
use helios_storage::Card;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::Shell;
use super::import_sources::{DOCUMENT, SavedSource, SourceKey};
use crate::dialogs::{Level, Notice};

/// When a saved source imports on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trigger {
    /// A card: when it is plugged in.
    Plugged,
    /// A folder: when new files appear in it.
    NewFiles,
}

impl Trigger {
    /// The trigger a source of this kind is offered: a card's or a
    /// folder's.
    pub fn offered(card: bool) -> Self {
        if card { Self::Plugged } else { Self::NewFiles }
    }

    /// Its words in a source's menu and in the wizard.
    pub(super) fn message(self) -> Message {
        match self {
            Self::Plugged => Message::new("gui-import-auto-plugged", "When plugged in"),
            Self::NewFiles => Message::new("gui-import-auto-new-files", "When new files appear"),
        }
    }
}

/// "Off", in the same menus.
pub(super) fn off_message() -> Message {
    Message::new("gui-import-auto-off", "Off")
}

/// "Import automatically", the menus' title.
pub(super) fn title_message() -> Message {
    Message::new("gui-import-auto", "Import automatically")
}

/// How often the watched folders are rescanned, and how long a file must
/// hold still before it is imported.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    pub poll: Duration,
    pub settle: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(2),
            settle: Duration::from_secs(3),
        }
    }
}

/// The automatic imports' state, kept in the import window's.
#[derive(Default)]
pub(crate) struct AutoImports {
    /// The cards mounted since the last pass, at start included.
    pub(super) plugged: Vec<Card>,
    timing: Timing,
    /// The folders watched, shared with the watch's thread.
    watched: Option<Arc<Mutex<Vec<PathBuf>>>>,
    /// The files the watch found settled, by folder.
    ready: Option<Receiver<(PathBuf, Vec<PathBuf>)>>,
    /// The sources' document's text, read again after another client
    /// changed it.
    followed: Option<Receiver<String>>,
}

impl AutoImports {
    /// The folders to watch now: the watch's thread started for the first
    /// one, and told when they change.
    fn watch(&mut self, folders: Vec<PathBuf>, ctx: &Arc<Mutex<Option<Egui>>>) {
        if let Some(shared) = &self.watched {
            let mut held = shared.lock().unwrap_or_else(PoisonError::into_inner);
            if *held != folders {
                *held = folders;
            }
            return;
        }
        if folders.is_empty() {
            return;
        }
        let shared = Arc::new(Mutex::new(folders));
        let (tx, rx) = channel();
        let (theirs, ctx, timing) = (shared.clone(), ctx.clone(), self.timing);
        let spawned = std::thread::Builder::new()
            .name("helios-import-watch".into())
            .spawn(move || watch_folders(&theirs, &tx, timing, &ctx));
        match spawned {
            Ok(_) => {
                self.watched = Some(shared);
                self.ready = Some(rx);
            }
            Err(e) => log::warn!("automatic imports: no thread to watch folders on: {e}"),
        }
    }
}

/// A file as last seen by the watch.
struct Seen {
    size: u64,
    modified: Option<SystemTime>,
    since: Instant,
    sent: bool,
}

/// The watch's thread: each folder rescanned every `timing.poll`; the
/// files settled for `timing.settle` and not sent yet sent. Ends once
/// the window dropped its end of `shared`.
fn watch_folders(
    shared: &Arc<Mutex<Vec<PathBuf>>>,
    tx: &Sender<(PathBuf, Vec<PathBuf>)>,
    timing: Timing,
    ctx: &Arc<Mutex<Option<Egui>>>,
) {
    let mut states: HashMap<PathBuf, HashMap<PathBuf, Seen>> = HashMap::new();
    while Arc::strong_count(shared) > 1 {
        let folders = shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        states.retain(|f, _| folders.contains(f));
        for folder in folders {
            let Ok(files) = helios_connect::local::files_of(&folder) else {
                continue;
            };
            let state = states.entry(folder.clone()).or_default();
            state.retain(|p, _| files.binary_search(p).is_ok());
            let now = Instant::now();
            let mut ready = Vec::new();
            for file in files {
                let Ok(meta) = std::fs::metadata(&file) else {
                    continue;
                };
                let (size, modified) = (meta.len(), meta.modified().ok());
                match state.get_mut(&file) {
                    None => {
                        state.insert(
                            file,
                            Seen {
                                size,
                                modified,
                                since: now,
                                sent: false,
                            },
                        );
                    }
                    Some(seen) if seen.sent => {}
                    Some(seen) if (seen.size, seen.modified) != (size, modified) => {
                        (seen.size, seen.modified, seen.since) = (size, modified, now);
                    }
                    Some(seen) if now.duration_since(seen.since) >= timing.settle => {
                        seen.sent = true;
                        ready.push(file);
                    }
                    Some(_) => {}
                }
            }
            if !ready.is_empty() {
                if tx.send((folder, ready)).is_err() {
                    return;
                }
                if let Some(ctx) = ctx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                    ctx.request_repaint();
                }
            }
        }
        std::thread::sleep(timing.poll);
    }
}

impl Shell {
    /// Each frame of the main window: the sources changed by another
    /// client read again, the folders to watch told to the watch, and the
    /// automatic imports of the cards plugged in and of the files settled
    /// in watched folders queued. The roots of the cards imported so.
    pub(super) fn run_automatic_imports(&mut self) -> Vec<PathBuf> {
        let _stall = crate::stall::section("import window: automatic imports");
        let (plugged, ready, sources) = {
            let mut w = self.import.get_mut();
            w.follow_kept_sources();
            let folders: Vec<PathBuf> = w
                .sources
                .list
                .iter()
                .filter(|s| s.auto == Some(Trigger::NewFiles))
                .filter_map(|s| s.path.clone())
                .collect();
            let ctx = w.ctx.clone();
            w.auto.watch(folders, &ctx);
            let plugged = std::mem::take(&mut w.auto.plugged);
            let ready: Vec<_> = w
                .auto
                .ready
                .as_ref()
                .map(|r| r.try_iter().collect())
                .unwrap_or_default();
            (plugged, ready, w.sources.list.clone())
        };
        let source = |path: &PathBuf, trigger: Trigger| {
            sources
                .iter()
                .find(|s| {
                    s.auto == Some(trigger) && s.key() == Some(SourceKey::Folder(path.clone()))
                })
                .cloned()
        };
        let mut imported = Vec::new();
        for card in plugged {
            let Some(saved) = source(&card.root, Trigger::Plugged) else {
                continue;
            };
            let files = helios_connect::local::files_of(&card.root).unwrap_or_default();
            imported.push(card.root.clone());
            self.import_automatically(&saved, &card.label, files, true);
        }
        for (folder, files) in ready {
            // Turned off since the watch found them: left alone.
            let Some(saved) = source(&folder, Trigger::NewFiles) else {
                continue;
            };
            let label = folder.file_name().map_or_else(
                || folder.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            self.import_automatically(&saved, &label, files, false);
        }
        imported
    }

    /// Queues the import of `files` from `saved`, with its own choices
    /// (else the window's), a card's copied, duplicates skipped; said in
    /// the Log panel and the notices.
    fn import_automatically(
        &mut self,
        saved: &SavedSource,
        label: &str,
        files: Vec<PathBuf>,
        card: bool,
    ) {
        if files.is_empty() {
            return;
        }
        let name = saved.name.clone().unwrap_or_else(|| label.to_owned());
        let choices = {
            let w = self.import.get();
            saved
                .choices
                .clone()
                .unwrap_or_else(|| w.window_choices().clone())
        };
        let copy = card || choices.copy_folders;
        let mut args = choices.args(&files, copy);
        args["duplicates"] = json!("skip");
        let count = files.len();
        if let Err(e) = self.import_files(&args, &Origin::User) {
            self.core.log.borrow_mut().push(
                Level::Warning,
                Message::new(
                    "gui-import-auto-failed",
                    "Automatic import from { $source }: { $error }",
                )
                .with("source", name)
                .with("error", e),
            );
            return;
        }
        let message = Message::new(
            "gui-import-auto-started",
            "Automatic import from { $source }: { $files } files",
        )
        .with("source", name)
        .with("files", count);
        self.core
            .log
            .borrow_mut()
            .push(Level::Info, message.clone());
        let text = self.text(message.clone());
        self.notes.push(text.clone());
        self.core.notices.push(Notice {
            level: Level::Info,
            text,
            key: message.key,
            action: None,
            at: self.core.now,
            report: None,
        });
    }

    /// Reads the import sources again each time the node says another
    /// client changed them: `heard` is the node's bus (`Bus::subscribe`),
    /// `read` the document's text as the node keeps it. Read off the
    /// interface's thread; the window takes the text at its next pass.
    pub fn follow_import_sources(
        &mut self,
        heard: Receiver<helios_core::Envelope>,
        read: impl Fn() -> Result<Option<String>, String> + Send + 'static,
    ) {
        let (tx, rx) = channel();
        let mut w = self.import.get_mut();
        let ctx = w.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("helios-import-sources-follow".into())
            .spawn(move || {
                while let Ok(e) = heard.recv() {
                    if e.kind
                        != <helios_node_core::settings::LayerChanged as helios_core::Event>::KIND
                        || e.payload["layer"].as_str() != Some(DOCUMENT)
                    {
                        continue;
                    }
                    match read() {
                        Ok(text) => {
                            if tx.send(text.unwrap_or_default()).is_err() {
                                return;
                            }
                        }
                        Err(e) => log::warn!("import sources: {DOCUMENT}: {e}"),
                    }
                    if let Some(ctx) = ctx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                        ctx.request_repaint();
                    }
                }
            });
        match spawned {
            Ok(_) => w.auto.followed = Some(rx),
            Err(e) => log::warn!("import sources: not followed: {e}"),
        }
    }

    /// How often watched folders are rescanned and how long a file must
    /// hold still before it is imported automatically (tests shorten
    /// them); before the first folder is watched.
    pub fn set_automatic_import_timing(&mut self, poll: Duration, settle: Duration) {
        self.import.get_mut().auto.timing = Timing { poll, settle };
    }

    /// Sets when the import source of `path` (a folder or a card) imports
    /// on its own, as its menu's "Import automatically" does.
    pub fn set_import_auto(&mut self, path: &std::path::Path, auto: Option<Trigger>) {
        self.import
            .get_mut()
            .set_auto(&SourceKey::Folder(path.to_path_buf()), auto);
    }
}

impl super::import_window::ImportWindow {
    /// The sources' document as another client left it: taken when it
    /// differs from the list.
    fn follow_kept_sources(&mut self) {
        let Some(texts) = &self.auto.followed else {
            return;
        };
        let Some(text) = texts.try_iter().last() else {
            return;
        };
        let list = super::import_sources::parse(&text);
        if list != self.sources.list {
            self.sources.list = list;
            if self.sources.showing.is_some() {
                self.choices = self.sources.window.clone();
                self.sources.showing = None;
            }
        }
    }

    /// Sets when the source of `key` imports on its own; one step of the
    /// column's Undo, a source not saved saved by it.
    pub(super) fn set_auto(&mut self, key: &SourceKey, auto: Option<Trigger>) {
        let mut source = match self.sources.get(key) {
            Some(s) => s.clone(),
            None if auto.is_none() => return,
            None => {
                let connector = self.connector_of(key);
                SavedSource::new(connector, key)
            }
        };
        if source.auto == auto {
            return;
        }
        source.auto = auto;
        self.replace_source(key, source);
    }
}
