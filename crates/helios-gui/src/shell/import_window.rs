//! The import window (`docs/design/import-window.md`): File > Import…
//! (Ctrl+Shift+I), the empty Library's "Import photos…" and a card's
//! toast open it (`library.importWindow`).
//!
//! - **Sources**, the left column: the sources of the import connectors
//!   ([`helios_connect::registry`]): the cards and cameras mounted (the
//!   card connector, given the cards [`helios_storage`] polls off the
//!   interface thread), first; then the recent folders and the favourites
//!   (the folder connector's, added and removed here). One "Add…" button
//!   opens the wizard creating a source of any connector installed
//!   ([`super::import_wizard`]). As it opens, the window ticks the first
//!   card, else the last source used.
//! - **Contents**, the middle column: what the sources ticked hold, read
//!   off the interface thread ([`crate::import_scan`]), an offline
//!   volume's as the image map recorded it, shown as it comes in egui-pop's
//!   file browser ([`crate::import_browser`]): details, thumbnails,
//!   filmstrip, columns, carousel or agenda (by capture day), the mode
//!   kept for the window. Each photo is selected by default; those
//!   probably in the Library already are dimmed and marked while "New
//!   only" is on, a RAW+JPEG pair is one entry. Import takes the files
//!   selected (`library.importFiles` with `paths` and the choices).
//!   Folders or cards dropped on it from the system open as its sources.
//! - **Detail**, the right column ([`super::import_detail`]): the import
//!   profile in use on top (a menu: Everyday, the person's profiles,
//!   "Manage profiles…") with what it does in one line
//!   ([`super::import_profile_store`]), then the selected source's
//!   connector section under its name: its account and "Choose photos"
//!   for a service whose photos are chosen in its own page, the
//!   connector's own parameters. It follows the selected source, each
//!   pointing at its profile ([`super::import_sources`]); the profile's
//!   choices are set in the profile editor
//!   ([`super::import_profile_editor`]), which takes the window's body.
//! - **The footer**: the totals of the photos ticked, Cancel and Import.
//! - **A card inserted** while the window is closed: a toast whose action
//!   opens the window on it.
//! - **Files or folders dropped** on Helios, or pasted: the window opens
//!   with them as its sources, a file's folder with that file ticked
//!   (`paths`).
//!
//! The recent folders, the favourites and the last source are kept in a
//! small JSON file when the desktop gives one
//! ([`Shell::remember_import_sources_in`]).
//!
//! **The import focus** (docs/design/windows.md, slice 4): the window is
//! an OS window of its own, through egui-pop's application windows
//! (`AppHandle::open_window`, an egui deferred viewport: it repaints on
//! its own, and the main window sleeps while a card is read). Its state
//! ([`ImportWindow`]) is shared, behind an `Rc`, by the shell and the
//! window's content: the content draws it in the window's own pass, reads
//! the sources, ticks, chooses folders and presets there; what needs the
//! shell (Import, Forget, Storage…, sources dropped) waits in
//! `ImportWindow::events` for the main window's next pass. The window has
//! its own status bar ("Reading <path>" while the sources are read), and
//! egui-pop gives it its own toasts, dialogs and palette. It shares the
//! process's core: the thumbnails come from the library's one cache, so
//! no photo is read twice. Closing the window closes the import focus;
//! the main window stays. Where the backend has no windows of its own
//! (kittest, the web), egui embeds it in the main window.

use std::cell::{Ref, RefCell, RefMut};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use egui::Context as Egui;
use egui::ViewportId;
use egui_pop::api::files::{self, FILES, FileRequest};
use egui_pop::api::host::Pending;
use egui_pop::api::toasts::{TOASTS, Toast};
use egui_pop::api::windows::{WindowId, WindowSpec};
use egui_pop::core::overlay::DialogLayout;
use egui_pop::model::ids::CommandId;
use helios_core::Message;
use helios_shell::commands::Origin;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Shell;

mod source_list;
use super::import_auto::Trigger;
use super::import_browsers::Shown;
use super::import_choices::{self, ChoiceWords, Choices};
use super::import_detail::Part;
use super::import_sources::SourceKey;
use crate::import_browser::{ImportSource, ImportThumbnails, NOWHERE};
use crate::import_scan::{
    Connectors, Contents, Counts, Known, MapRecorder, Recorded, Scan, Scanned, Types,
};
use egui_pop::api::files::browser::{
    DropOrigin, EntryKind, EntryMarks, FileBrowser, SourceEntry, ViewModes,
};
use helios_storage::{Card, Volumes};

use crate::node_volumes::{self, Change, VolumeWatch};

/// The command that opens the window (`source`: the card or folder to
/// open it on).
pub const COMMAND: &str = "library.importWindow";

/// What is kept between runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Memory {
    #[serde(default)]
    pub(super) recent: Vec<PathBuf>,
    #[serde(default)]
    pub(super) favourites: Vec<PathBuf>,
    /// The accounts pinned among the Favorites, by id.
    #[serde(default)]
    pub(super) pinned: Vec<String>,
    /// The favourites that are cards, by path: the card's volume id. A
    /// card pinned is the same favourite wherever it is mounted, and
    /// another card mounted where it was is not pinned.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) cards: BTreeMap<PathBuf, String>,
    #[serde(default)]
    last: Option<PathBuf>,
}

impl Memory {
    /// The favourites follow the cards `mounted` now: a pinned card takes
    /// where it is mounted; a favourite at a card's root, pinned before
    /// cards were known by their volume, takes that card's. Whether any
    /// changed.
    pub(super) fn follow_cards(&mut self, mounted: &[super::import_sources::MountedCard]) -> bool {
        let mut changed = false;
        for m in mounted {
            let moved: Vec<PathBuf> = self
                .cards
                .iter()
                .filter(|(p, v)| **v == m.id && **p != m.root)
                .map(|(p, _)| p.clone())
                .collect();
            for old in moved {
                self.cards.remove(&old);
                self.cards.insert(m.root.clone(), m.id.clone());
                let had = self.favourites.contains(&m.root);
                self.favourites
                    .iter_mut()
                    .filter(|p| **p == old)
                    .for_each(|p| p.clone_from(&m.root));
                if had {
                    // Already listed at its new place: once.
                    let mut seen = false;
                    self.favourites.retain(|p| {
                        let first = *p != m.root || !seen;
                        seen |= *p == m.root;
                        first
                    });
                }
                changed = true;
            }
            if self.favourites.contains(&m.root) && !self.cards.contains_key(&m.root) {
                self.cards.insert(m.root.clone(), m.id.clone());
                changed = true;
            }
        }
        changed
    }

    /// Whether `path` is a favourite: for a card's, the card pinned is the
    /// one mounted there now (if any is).
    pub(super) fn favourite(
        &self,
        path: &Path,
        mounted: &[super::import_sources::MountedCard],
    ) -> bool {
        self.favourites.iter().any(|p| p == path)
            && self.cards.get(path).is_none_or(|v| {
                mounted
                    .iter()
                    .find(|m| m.root == path)
                    .is_none_or(|m| &m.id == v)
            })
    }
}

/// The command that forgets a folder in the image map (registered by
/// the desktop over its vault).
pub const FORGET_FOLDER: &str = "library.forgetFolder";
/// The command that forgets a volume in the image map.
pub const FORGET_VOLUME: &str = "library.forgetVolume";

/// The import focus's OS window: its id, under which egui-pop keeps its
/// size (`appWindow == 'import'` in `when` clauses, `window: "import"` for
/// agents).
pub const WINDOW: &str = "import";

/// The id of the window of the main one the import window is drawn in
/// where the backend has no windows of its own (kittest, the web).
const EMBEDDED_ID: &str = "helios.import.embedded";

/// What the window's pass reads of the window it is drawn in, and asks of
/// it: egui-pop's application window, or a window of the main one.
struct Frame {
    app: egui_pop::api::app::AppHandle,
    /// Drawn inside the main window: no panel of its own.
    embedded: bool,
    /// The person asked to close it (its close button).
    close_requested: bool,
    /// The title it shows.
    title: Option<String>,
    /// It closes.
    close: bool,
}

/// The shell's translator, given to the window's content.
pub(super) type Texts = Arc<dyn Fn(Message) -> String + Send + Sync>;

/// The import focus: the second focus of the process, of kind intake, in
/// an OS window of its own. Its state is shared with the window's content.
pub(super) struct ImportFocus {
    pub(super) state: Rc<RefCell<ImportWindow>>,
}

impl ImportFocus {
    pub(super) fn new() -> Self {
        Self {
            state: Rc::default(),
        }
    }

    pub(super) fn get(&self) -> Ref<'_, ImportWindow> {
        self.state.borrow()
    }

    pub(super) fn get_mut(&self) -> RefMut<'_, ImportWindow> {
        self.state.borrow_mut()
    }
}

/// What the window's words need of the shell's core, given at each of
/// the main window's passes while it is open.
#[derive(Clone, Default)]
struct ShellWords {
    /// The strategy line: what backups the photos will get, and whether
    /// none is set up.
    backup: (String, bool),
    /// Why nothing can be imported: no library open.
    no_library: Option<String>,
}

/// The import window's state: shared by the shell and the window's
/// content (see the module's docs).
// Its flags (open, touched, custom, camera_looks) are independent states of
// the window, not one mode spread over several bools.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
pub(crate) struct ImportWindow {
    pub(super) open: bool,
    /// The imports running and ended during this run (the node's
    /// promises, [`crate::activity::Activity::imports`]), shown above the
    /// footer with their progress, outcome, Cancel and Retry.
    promises: Vec<helios_vault::promises::Promise>,
    /// The OS window opened for it, not seen closed since.
    shown: Option<WindowId>,
    /// Its title as last drawn, for the window of the main one it is
    /// drawn in where the backend has no windows of its own.
    embedded_title: Option<String>,
    /// The shell's translator and what the words need of it.
    pub(super) texts: Option<Texts>,
    shell_words: ShellWords,
    /// What the window asked of the shell, taken at the main window's
    /// next pass.
    events: Vec<Event>,
    /// The node's volume watch the cards come from
    /// ([`Shell::follow_volumes`]).
    volumes: Option<VolumeWatch>,
    /// Its events, read at each frame of the main window.
    volume_events: Option<std::sync::mpsc::Receiver<helios_core::Envelope>>,
    /// Cards plugged in since the watch's first look, not toasted yet.
    inserted: Vec<Card>,
    /// The context the cards' and the scans' threads ask a frame of.
    pub(super) ctx: Arc<Mutex<Option<Egui>>>,
    /// The automatic imports ([`super::import_auto`]).
    pub(super) auto: super::import_auto::AutoImports,
    /// The library's import history, listed under the sources
    /// ([`super::import_history`]).
    pub(super) history: super::import_history::History,
    /// The viewport the window is drawn in (the main one while embedded):
    /// the one a scan's progress repaints.
    drawn_in: Arc<Mutex<Option<ViewportId>>>,
    /// Its OS window's viewport, closed but not yet dropped by egui: the
    /// main window passes again until it is.
    going: Option<ViewportId>,
    cards: Vec<Card>,
    pub(super) memory: Memory,
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
    /// The files the scans glance at first: the cells in view without a
    /// picture, then the next screen's.
    glance_first: crate::import_scan::Wanted,
    /// The files whose glance came from the glance cache as they were
    /// found: the scan's own glance of them is left out.
    glanced_early: std::collections::HashSet<PathBuf>,
    /// The files whose glances came, read or from the glance cache, in
    /// the order they came: the cells in view come first.
    glanced_in_order: Vec<PathBuf>,
    /// The files in view whose glance the glance cache was told shown
    /// (kept longest, never evicted by a prefetch).
    glances_shown: std::collections::HashSet<PathBuf>,
    pub(super) contents: Contents,
    /// The import connectors (`helios_connect`): the Sources column lists
    /// their sources, a scan lists a source through its connector.
    pub(super) connectors: Connectors,
    /// The accounts of the connectors that need one, and the connection
    /// running ([`super::import_accounts`]).
    pub(super) accounts: super::import_accounts::Accounts,
    /// The choices, the last used until changed, and where they are kept.
    pub(super) choices: Choices,
    pub(super) choices_file: Option<PathBuf>,
    /// The saved sources, each with its own choices
    /// ([`super::import_sources`]).
    pub(super) sources: super::import_sources::Sources,
    /// Copy or add as the person switched it since the window opened.
    pub(super) copy: Option<bool>,
    /// The custom rename pattern is being edited.
    pub(super) custom: bool,
    /// The Contents column: egui-pop's file browser over the photos.
    pub(super) browser: Option<FileBrowser>,
    source: ImportSource,
    thumbs: Option<Arc<ImportThumbnails>>,
    /// The library's thumbnail cache, for the photos it has (an offline
    /// volume's).
    thumb_cache: Option<Arc<helios_thumbs::ThumbCache>>,
    /// The image map's record, for a source whose volume is offline.
    recorded: Option<Recorded>,
    /// Each photo's marks (in your Library, RAW+JPG, offline), by path.
    marks: Arc<Mutex<std::collections::HashMap<PathBuf, EntryMarks>>>,
    /// What was last published to the browser: the contents' revision,
    /// New only, the place, and whether every source is read through.
    published: Option<(u64, bool, PathBuf, bool)>,
    /// Bumped when the ticks change outside the browser (all, none, a
    /// day).
    tick_revision: u64,
    /// The contents' revision, the entries listed and the ticks' revision
    /// the browser's selection was last set from.
    selection_synced: Option<(u64, usize, u64)>,
    /// The files dropped or pasted: in their folder, only they are ticked.
    picked: Vec<PathBuf>,
    /// The import profiles, with the filter presets, and where they are
    /// kept ([`super::import_profile_store`]).
    pub(super) profiles: super::import_profile_store::Profiles,
    /// The profile editor, while it is open: the window's body shows it
    /// ([`super::import_profile_editor`]).
    pub(super) manager: Option<super::import_profile_editor::Manager>,
    /// The Include and Apply During Import sections' typing.
    pub(super) profile_edit: super::import_profiles::Editing,
    /// The library keeps a camera's default look: the Choices offer to
    /// apply it. Read when the window opens.
    pub(super) camera_looks: bool,
    /// The "Add a source…" wizard, while it is open: the window's body
    /// shows its steps ([`super::import_wizard`]).
    pub(super) wizard: Option<super::import_wizard::Wizard>,
    /// Why it cannot import yet: a setup step it requires was left (the
    /// library folder), said in its footer until the step is done.
    pub(super) not_set_up: Option<Message>,
    /// The browsers of the source the Contents column shows, and the one
    /// shown ([`super::import_browsers`]).
    pub(super) browsing: super::import_browsers::Browsing,
    /// The photos chosen in a service's page, while the grid shows them
    /// ([`super::import_chosen`]).
    pub(super) chosen: Option<super::import_chosen::ChosenContents>,
    /// The services' albums, and the album selected as the source
    /// ([`super::import_albums`]).
    pub(super) albums: super::import_albums::Albums,
}

impl ImportWindow {
    pub(super) fn save(&self) {
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = serde_json::to_string_pretty(&self.memory).unwrap_or_default();
        if let Err(e) = std::fs::write(file, text) {
            log::warn!("import sources: {}: {e}", file.display());
        }
    }

    /// Closes the import focus: nothing more read, the main window drawn
    /// again.
    fn cancel(&mut self, ctx: &Egui) {
        let _stall = crate::stall::section("import window: closing");
        crate::start::trace_frame("import window closed");
        self.open = false;
        self.going_away();
        self.wizard = None;
        self.clear_contents();
        // The connection running gives up on its own thread; a picking
        // goes on there, detached, and says how it ended.
        self.accounts.window_closed();
        ctx.request_repaint_of(ViewportId::ROOT);
    }

    /// Its OS window goes: what still answers (a picking ending, a
    /// connection giving up) wakes the main window, never the window
    /// going, and the main window passes until egui has dropped it.
    fn going_away(&mut self) {
        let drawn = self
            .drawn_in
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        self.going = drawn.filter(|v| *v != ViewportId::ROOT);
    }

    /// Stops reading and forgets what was read.
    pub(super) fn clear_contents(&mut self) {
        self.chosen = None;
        self.scans.clear();
        self.glance_first
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.glanced_early.clear();
        self.glanced_in_order.clear();
        self.glances_shown.clear();
        self.contents = Contents::new();
        // The source browsed is looked at afresh.
        self.browsing.fresh = false;
        self.published = None;
        self.selection_synced = None;
        if let Some(t) = &self.thumbs {
            t.clear();
        }
        // The browser lists anew: never the last import's photos, drawn
        // without their thumbnails before the scans find them again.
        self.source.clear();
        if let Some(b) = &mut self.browser {
            b.reload();
        }
    }

    /// Reads the sources ticked (each on a thread of its own), forgets
    /// the ones unticked, and takes in what the reads sent.
    fn follow_scans(&mut self) {
        self.forget_unticked();
        let ticked = self.ticked.clone();
        for path in &ticked {
            if self.scans.iter().any(|(p, _, _)| p == path) {
                continue;
            }
            let seen_by = if self.dropped.contains(path) {
                "drop"
            } else {
                "import"
            };
            let map = self.map.clone().map(|m| (m, seen_by));
            let wake = self.waker();
            // A folder no volume holds now: as the image map recorded it.
            let scan = match &self.recorded {
                Some(recorded) if !path.exists() => {
                    Scan::recorded(path.clone(), recorded.clone(), wake)
                }
                _ => match self.connectors.for_path(path) {
                    Some(listed) => Scan::start(
                        listed,
                        path.clone(),
                        self.known.clone(),
                        map,
                        self.glance_first.clone(),
                        wake,
                    ),
                    None => continue,
                },
            };
            self.scans.push((path.clone(), scan, false));
        }
        let thumbs = self.thumbnails();
        for (_, scan, done) in &mut self.scans {
            for sent in scan.take() {
                match sent {
                    Scanned::Found(found) => {
                        let key = found.key.clone();
                        // A glance kept from before: the cell is drawn with
                        // its thumbnail, not empty until the scan's turn.
                        let early = (!found.offline)
                            .then(|| {
                                crate::import_scan::GlanceCache::global().cached(
                                    &found.path,
                                    found.bytes,
                                    found.modified,
                                )
                            })
                            .flatten();
                        let path = found.path.clone();
                        let at = self.contents.add(found);
                        if let (Some(key), Some(main)) = (key, self.contents.cells[at].main()) {
                            thumbs.key(main, key);
                        }
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
                        if let Some(g) = early {
                            let (taken, camera, thumb) =
                                (g.taken, g.camera.clone(), g.thumb.clone());
                            take_glance(&mut self.contents, &thumbs, &path, taken, camera, thumb);
                            self.glanced_in_order.push(path.clone());
                            self.glanced_early.insert(path);
                        }
                    }
                    Scanned::Glance {
                        path,
                        taken,
                        camera,
                        thumb,
                    } => {
                        if !self.glanced_early.remove(&path) {
                            take_glance(&mut self.contents, &thumbs, &path, taken, camera, thumb);
                            self.glanced_in_order.push(path);
                        }
                    }
                    Scanned::Done => *done = true,
                }
            }
        }
    }

    /// The sources no longer ticked are forgotten at once: their scans
    /// stopped (what they still send is dropped with them) and their
    /// photos gone from the contents. Run before anything reads the
    /// contents in a pass, the click that changed the source included:
    /// the grid, the count and Import never show another source's.
    fn forget_unticked(&mut self) {
        let ticked = &self.ticked;
        self.scans.retain(|(p, _, _)| ticked.contains(p));
        // The photos chosen in a service's page are no folder's.
        if self.chosen.is_none() {
            self.contents.keep_sources(ticked);
        }
    }

    /// Whether every source ticked is read through.
    fn read_through(&self) -> bool {
        self.ticked
            .iter()
            .all(|t| self.scans.iter().any(|(p, _, done)| p == t && *done))
    }

    /// Asks a frame of the window from another thread: of its own OS
    /// window, or of the main one while embedded or not drawn yet.
    pub(super) fn waker(&self) -> impl Fn() + Send + Sync + 'static {
        let (ctx, drawn_in) = (self.ctx.clone(), self.drawn_in.clone());
        move || {
            if let Some(ctx) = ctx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                let at = *drawn_in.lock().unwrap_or_else(PoisonError::into_inner);
                ctx.request_repaint_of(at.unwrap_or(ViewportId::ROOT));
            }
        }
    }

    /// What the scans read now, the first source not read through's:
    /// the folder it lists, else the file it glances at.
    fn reading(&self) -> Option<PathBuf> {
        self.scans
            .iter()
            .find(|(_, _, done)| !done)
            .map(|(_, scan, _)| scan.reading())
    }

    /// The thumbnails of the Contents, made the first time.
    pub(super) fn thumbnails(&mut self) -> Arc<ImportThumbnails> {
        let cache = self.thumb_cache.clone();
        self.thumbs
            .get_or_insert_with(|| Arc::new(ImportThumbnails::new(cache)))
            .clone()
    }

    pub(super) fn is_card(&self, path: &Path) -> bool {
        self.cards.iter().any(|c| c.root == path)
    }

    /// One source at a time: `path` selected alone, or none.
    fn tick(&mut self, path: &Path, on: bool) {
        self.ticked.retain(|p| p != path);
        if on {
            self.sources.account = None;
            self.ticked = vec![path.to_path_buf()];
        }
    }

    /// The cards as last polled; the first one ticked while the person
    /// has not chosen and no card is ticked.
    fn follow_cards(&mut self) {
        let Some(events) = &self.volume_events else {
            return;
        };
        let mut mounted = self.sources.mounted.clone();
        for change in events.try_iter().filter_map(|e| Change::of(&e)) {
            let Some(card) = change.card() else { continue };
            self.cards.retain(|c| c.root != card.root);
            mounted.retain(|m| m.root != card.root);
            if let Change::Mounted(m) = &change {
                mounted.push(mounted_card(&change, card));
                self.cards.push(card.clone());
                self.auto.plugged.push(card.clone());
                if !m.at_start {
                    self.inserted.push(card.clone());
                }
            } else {
                self.inserted.retain(|c| c.root != card.root);
            }
        }
        // The sources saved on a card follow it by its volume.
        if mounted != self.sources.mounted {
            if self.memory.follow_cards(&mounted) {
                self.save();
            }
            self.sources.see_cards(mounted);
        }
        self.connectors.set_cards(&self.cards);
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

/// The card of a volume event, by its volume.
fn mounted_card(change: &Change, card: &Card) -> super::import_sources::MountedCard {
    super::import_sources::MountedCard {
        root: card.root.clone(),
        id: change.id().0.clone(),
        label: card.label.clone(),
    }
}

/// A file's glance into the contents; a glance without a preview tells
/// the thumbnails there is none (a pair's other file may still give one).
pub(super) fn take_glance(
    contents: &mut Contents,
    thumbs: &ImportThumbnails,
    path: &Path,
    taken: Option<chrono::NaiveDateTime>,
    camera: Option<String>,
    thumb: Option<egui::ColorImage>,
) {
    let none = thumb.is_none();
    contents.glance(path, taken, camera, thumb);
    if none
        && let Some(main) = contents
            .cell_of(path)
            .and_then(|i| contents.cells[i].main())
    {
        thumbs.glanced(main, None);
    }
}

/// `text` shortened in its middle (an ellipsis for what is left out,
/// more of its end kept than of its start: a path's file name) to the
/// longest that `fits`; whole when it fits.
pub(crate) fn shorten_middle(text: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(text) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |kept: usize| {
        let head = kept / 3;
        let tail = kept - head;
        let mut s: String = chars[..head].iter().collect();
        s.push('…');
        s.extend(&chars[chars.len() - tail..]);
        s
    };
    // The most characters kept that fit, by halves.
    let (mut lo, mut hi) = (0, chars.len().saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&cut(mid)) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}

/// What the window's controls asked for this frame.
enum Event {
    Cancel,
    Import,
    /// An import profile picked in the profile menu (its id).
    Profile(String),
    /// "Manage profiles…": the profile editor.
    ManageProfiles,
    ChooseInto,
    Favourite(PathBuf, bool),
    /// An account pinned among the Favorites, or unpinned.
    Pin(String, bool),
    /// "Forget this folder" (`library.forgetFolder`).
    Forget(PathBuf),
    /// "Storage…": the Storage page, to set up a backup.
    Storage,
    /// Folders or cards dropped on the window: its new sources.
    Dropped(Vec<PathBuf>),
    /// An account asked of the Sources column or of its Contents: added,
    /// reconnected, photos chosen.
    Account(super::import_accounts::Asked),
    /// "Manage accounts…": Settings > Accounts.
    ManageAccounts,
    /// "Import again" on an import of the history (its id).
    Again(String),
    /// Cancel or Retry on an import's promise line (its id).
    Promise(String, crate::promise_line::Action),
}

/// The window's event for an account's action: Settings > Accounts is
/// the shell's.
fn account_event(asked: super::import_accounts::Asked) -> Event {
    match asked {
        super::import_accounts::Asked::Manage => Event::ManageAccounts,
        asked => Event::Account(asked),
    }
}

/// The Detail column's part for an account whose photos are chosen in
/// its service's page ([`super::import_accounts::choose_ui`]).
fn choose_contents_ui(ui: &mut egui::Ui, w: &mut ImportWindow, account: &str) -> Option<Event> {
    use super::import_accounts::translated;
    let texts = w.texts.clone()?;
    let connector = w.connector_of(&SourceKey::Account(account.to_owned()));
    let service = w.connectors.provider(&connector).map_or_else(
        || connector.clone(),
        |p| translated(Some(&texts), &p.manifest().label),
    );
    let header = account_header(w, account);
    let chosen = w.shows_chosen_of(account);
    super::import_accounts::choose_ui(
        ui,
        &mut w.accounts,
        &w.connectors,
        account,
        &header,
        &service,
        chosen,
        &|m| texts(m),
    )
    .map(account_event)
}

/// An account's header line: its saved source's name, else its own, with
/// its address.
fn account_header(w: &mut ImportWindow, account: &str) -> String {
    let saved = w
        .sources
        .named()
        .into_iter()
        .find(|(k, _)| matches!(k, SourceKey::Account(id) if id == account))
        .map(|(_, name)| name);
    w.accounts
        .listed()
        .iter()
        .find(|a| a.id == account)
        .map_or_else(
            || account.to_owned(),
            |a| super::import_accounts::header_of(a, saved.as_deref()),
        )
}

/// The Contents column's and the status bar's words.
struct GridWords {
    counts: String,
    /// "Reading <path>", the status bar's: the words before and after the
    /// path, and the path, while a source is read.
    reading: Option<(String, String, String)>,
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
    in_library: String,
    pair: String,
    offline: String,
    /// The browsers' switch and words.
    browse: super::import_browsers::BrowseWords,
    /// A source whose photos are chosen in its service's page: where to
    /// choose them.
    choose_on_the_right: String,
}

/// The window's words, in the shell's language.
struct Words {
    title: String,
    sources: String,
    cards: String,
    no_cards: String,
    folders: String,
    history: String,
    services: String,
    no_services: String,
    no_folders: String,
    /// The one "Add…" button, and what it does.
    add: String,
    add_hint: String,
    favourites: String,
    no_favourites: String,
    add_favourite: String,
    remove_favourite: String,
    forget: String,
    forget_hint: String,
    /// A source's menu: the editor, and the source taken off the column.
    edit_source: String,
    forget_source: String,
    forget_source_hint: String,
    /// "Import automatically" and its choices, in a source's menu.
    auto: String,
    auto_off: String,
    auto_plugged: String,
    auto_new_files: String,
    contents: String,
    grid: GridWords,
    choices: ChoiceWords,
    cancel: String,
    import_count: String,
    totals: String,
    /// The profile menu: its label, "Manage profiles…", and what the
    /// profile in use does.
    profile: String,
    manage_profiles: String,
    profile_summary: String,
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
    /// Follows the node's volume watch (`LocalNode::watch_volumes`): the
    /// import window lists the cards and cameras it publishes, and a card
    /// plugged in while the window is closed raises a toast. The shell
    /// reads the watch's events off the bus at each frame; the watch asks
    /// a frame of the windows when a volume comes or goes.
    pub fn follow_volumes(&mut self, watch: VolumeWatch) {
        let mut w = self.import.get_mut();
        let (ctx, drawn_in) = (w.ctx.clone(), w.drawn_in.clone());
        // Subscribed before the list is read: a card in both is listed
        // once (by its root).
        w.volume_events = Some(watch.subscribe());
        w.cards = watch.cards();
        let mounted = watch
            .mounted()
            .iter()
            .filter_map(|v| {
                let card = Card::of(v)?;
                Some(super::import_sources::MountedCard {
                    root: card.root,
                    id: v.id.0.clone(),
                    label: card.label,
                })
            })
            .collect::<Vec<_>>();
        if w.memory.follow_cards(&mounted) {
            w.save();
        }
        w.sources.see_cards(mounted);
        watch.on_change(move |_| {
            if let Some(ctx) = ctx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                // The main window's toasts, and the import window's list.
                ctx.request_repaint_of(ViewportId::ROOT);
                if let Some(at) = *drawn_in.lock().unwrap_or_else(PoisonError::into_inner) {
                    ctx.request_repaint_of(at);
                }
            }
        });
        w.volumes = Some(watch);
    }

    /// A volume watch of the shell's own on `volumes`, for a shell with
    /// no node (tests, a desktop without a library): [`Shell::follow_volumes`]
    /// on a watch publishing on a bus of its own.
    pub fn watch_cards(&mut self, volumes: Arc<dyn Volumes>) {
        self.follow_volumes(VolumeWatch::start(
            volumes,
            Arc::new(helios_core::Bus::new()),
            node_volumes::POLL,
        ));
    }

    /// The volume watch the shell follows ([`Shell::follow_volumes`]).
    pub(super) fn volume_watch(&self) -> Option<VolumeWatch> {
        self.import.get().volumes.clone()
    }

    /// Keeps the import window's recent folders, favourites and last
    /// source in `file` (JSON), read now.
    pub fn remember_import_sources_in(&mut self, file: PathBuf) {
        let mut w = self.import.get_mut();
        w.memory = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        w.file = Some(file);
        let mounted = w.sources.mounted.clone();
        if w.memory.follow_cards(&mounted) {
            w.save();
        }
    }

    /// The import window's favourite folders and cards, as kept.
    pub fn import_favourites(&self) -> Vec<PathBuf> {
        self.import.get().memory.favourites.clone()
    }

    /// Pins or unpins `path` among the import window's favourites, as the
    /// pin on its item does.
    pub fn set_import_favourite(&mut self, path: &Path, on: bool) {
        let mut w = self.import.get_mut();
        w.memory.favourites.retain(|p| p != path);
        w.memory.cards.remove(path);
        if on {
            if let Some(m) = w.sources.card_at(path) {
                let id = m.id.clone();
                w.memory.cards.insert(path.to_path_buf(), id);
            }
            w.memory.favourites.push(path.to_path_buf());
        }
        w.save();
    }

    /// Whether `path` shows as pinned in the import window's Favorites
    /// (for a card: the card pinned is the one mounted there).
    pub fn import_favourite(&self, path: &Path) -> bool {
        let w = self.import.get();
        w.memory.favourite(path, &w.sources.mounted)
    }

    /// The folders and cards the import window's History lists, newest
    /// first: a card once, where it is mounted now.
    pub fn import_history_folders(&mut self) -> Vec<PathBuf> {
        let mut w = self.import.get_mut();
        w.history.follow();
        w.history.recent_folders(&w.sources.mounted)
    }

    /// When the import window's History says `path` was last imported
    /// from (seconds since the epoch): a card's imports wherever it was
    /// mounted.
    pub fn import_last_import_of(&mut self, path: &Path) -> Option<u64> {
        let mut w = self.import.get_mut();
        w.history.follow();
        w.history.last_import_of(path, &w.sources.mounted)
    }

    /// Closes the import window as the system's close button does (its
    /// title bar's, past any dialog over it): the import focus closes.
    pub fn close_import_window(&mut self, ctx: &Egui) {
        let mut w = self.import.get_mut();
        if w.open {
            w.cancel(ctx);
        }
    }

    /// Whether the import window is open.
    pub fn import_window_open(&self) -> bool {
        self.import.get().open
    }

    /// The import window's cards, as last polled.
    pub fn import_cards(&self) -> Vec<Card> {
        self.import.get().cards.clone()
    }

    /// What the vault holds (size, partial hash), for the import window's
    /// "in your Library" prefilter; called once per source read, off the
    /// interface thread.
    pub fn set_import_known(&mut self, known: Known) {
        self.import.get_mut().known = Some(known);
    }

    /// The image map's recorder for what the import window's sources
    /// list (`docs/design/image-map.md`, section 4); called once per
    /// source read, off the interface thread.
    pub fn set_import_map(&mut self, map: MapRecorder) {
        self.import.get_mut().map = Some(map);
    }

    /// The image map's record of what Helios saw on every volume: the
    /// import window lists an offline folder's photos from it.
    pub fn set_import_recorded(&mut self, recorded: Recorded) {
        self.import.get_mut().recorded = Some(recorded);
    }

    /// The library's thumbnail cache: the import window shows the photos
    /// it has thumbnails of (an offline volume's) from it.
    pub fn set_import_thumbnail_cache(&mut self, cache: Arc<helios_thumbs::ThumbCache>) {
        let mut w = self.import.get_mut();
        w.thumb_cache = Some(cache);
        w.thumbs = None;
        w.browser = None;
    }

    /// The import window's file browser, once drawn: its mode, entries,
    /// selection and focus.
    pub fn import_browser(&self) -> Option<Ref<'_, FileBrowser>> {
        Ref::filter_map(self.import.get(), |w| w.browser.as_ref()).ok()
    }

    /// Whether the import window's thumbnail of the photo at `path` (a
    /// pair's raw) is made.
    pub fn import_thumbnail_ready(&self, path: &Path) -> bool {
        self.import
            .get()
            .thumbs
            .as_ref()
            .is_some_and(|t| t.ready(path))
    }

    /// What the import window's scans' glances did so far: files read,
    /// answered from the glance cache, and the most read at once.
    pub fn import_glances(&self) -> crate::import_scan::Glances {
        self.import
            .get()
            .scans
            .iter()
            .map(|(_, scan, _)| scan.glances())
            .fold(crate::import_scan::Glances::default(), |a, b| a + b)
    }

    /// The files whose glances the import window took, read or from the
    /// glance cache, in the order they came.
    pub fn import_glanced_in_order(&self) -> Vec<PathBuf> {
        self.import.get().glanced_in_order.clone()
    }

    /// The photos the import window's browser last asked thumbnails of as
    /// visible.
    pub fn import_thumbnails_on_screen(&self) -> Vec<PathBuf> {
        self.import
            .get()
            .thumbs
            .as_ref()
            .map(|t| t.on_screen())
            .unwrap_or_default()
    }

    /// The import window's file browser, to drive it.
    pub fn import_browser_mut(&mut self) -> Option<RefMut<'_, FileBrowser>> {
        RefMut::filter_map(self.import.get_mut(), |w| w.browser.as_mut()).ok()
    }

    /// The image map's job: the import window, the Library grid and
    /// Compare tell it the files in view.
    pub fn set_map_feed(&mut self, feed: helios_library::map_job::Feed) {
        self.focus.library.borrow_mut().set_map_feed(feed.clone());
        self.import.get_mut().map_feed = Some(feed);
    }

    /// The import window's counts: photos shown, new, their size, and the
    /// ones an Import takes.
    pub fn import_counts(&self) -> Counts {
        self.import.get().contents.counts()
    }

    /// Whether every source ticked in the import window is read through.
    pub fn import_read(&self) -> bool {
        self.import.get().read_through()
    }

    /// The import window's cells: name, day, probably in the Library, a
    /// RAW+JPEG pair, ticked.
    pub fn import_cells(&self) -> Vec<(String, Option<chrono::NaiveDate>, bool, bool, bool)> {
        self.import
            .get()
            .contents
            .cells
            .iter()
            .map(|c| (c.name(), c.day(), c.known(), c.is_pair(), c.ticked))
            .collect()
    }

    /// The sources ticked in the import window.
    pub fn import_ticked(&self) -> Vec<PathBuf> {
        self.import.get().ticked.clone()
    }

    /// The viewport the import window was last drawn in: its own OS
    /// window's, or the main window's while egui embeds it.
    pub fn import_window_viewport(&self) -> Option<ViewportId> {
        let w = self.import.get();
        *w.drawn_in.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `library.importWindow`: opens the import window, on `source` when
    /// given, or on the files and folders of `paths` (a folder a source,
    /// a file its folder with only the files given ticked; `copy` sets
    /// copy or add), else on the first card, else on the last source used.
    /// Open already, its window comes to the front.
    pub(super) fn open_import_window(&mut self, args: &Value) {
        // The strategy line reads the strategy and its disks as they are.
        self.refresh_volumes();
        let mut w = self.import.get_mut();
        if let Some(id) = &w.shown {
            self.core.app.handle().raise_window(id);
        }
        w.follow_cards();
        w.clear_contents();
        w.camera_looks = w
            .accounts
            .library
            .as_ref()
            .is_some_and(|l| l.has_camera_looks());
        w.open = true;
        w.touched = false;
        w.copy = None;
        w.custom = false;
        w.manager = None;
        w.ticked.clear();
        w.picked.clear();
        w.dropped.clear();
        w.sources.account = None;
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
                // One source at a time: the first given; the others are
                // listed under Folders, a click away.
                if w.ticked.is_empty() {
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
            let root = card.root.clone();
            w.ticked.push(root);
        } else if let Some(last) = w.memory.last.clone().filter(|p| p.exists()) {
            w.ticked.push(last);
        }
        // The folders given are listed with the recent ones, under
        // Folders: the one selected, and the others a click away.
        let given: Vec<PathBuf> = w
            .ticked
            .iter()
            .chain(w.dropped.iter())
            .filter(|p| !w.is_card(p) && !w.memory.recent.contains(p))
            .cloned()
            .collect();
        if !given.is_empty() {
            let mut given = given;
            given.dedup();
            for p in given.into_iter().rev() {
                w.memory.recent.insert(0, p);
            }
            w.memory.recent.truncate(8);
            w.save();
        }
    }

    /// Each frame of the main window: the cards polled, a toast for each
    /// card inserted while the window is closed, what the window asked of
    /// the shell (its toasts shown in the window), and its OS window
    /// opened or closed as the import focus is.
    pub(super) fn import_window_frame(&mut self, ctx: &Egui) {
        let _stall = crate::stall::section("import window: the main window's part");
        {
            let mut w = self.import.get_mut();
            {
                let mut held = w.ctx.lock().unwrap_or_else(PoisonError::into_inner);
                if held.is_none() {
                    *held = Some(ctx.clone());
                }
            }
            // The answers that come off the interface's thread (the
            // accounts read, a plugin's consent) ask for the window's pass.
            if w.accounts.wake.is_none() {
                w.accounts.wake = Some(Arc::new(w.waker()));
            }
            w.promises = self.core.activity.imports();
        }
        let (open, inserted) = {
            let mut w = self.import.get_mut();
            w.follow_cards();
            let inserted = std::mem::take(&mut w.inserted);
            (w.open, inserted)
        };
        // A card imported on its own is said by its import, not toasted.
        let automatic = self.run_automatic_imports();
        if !open {
            for card in inserted
                .into_iter()
                .filter(|c| !automatic.contains(&c.root))
            {
                self.card_toast(&card);
            }
        }
        self.receive_chosen_folder();
        if open {
            // What the window's words need of the shell.
            let words = ShellWords {
                backup: self.import_backup_line(),
                no_library: self
                    .core
                    .no_library
                    .clone()
                    .or_else(|| self.import.get().not_set_up.clone())
                    .map(|m| self.text(m)),
            };
            let texts: Texts = Arc::new(self.texts());
            let mut w = self.import.get_mut();
            w.texts = Some(texts);
            w.shell_words = words;
        }
        let events = std::mem::take(&mut self.import.get_mut().events);
        if !events.is_empty() {
            // Asked in the import window, answered in the main window's
            // pass: what they say (a toast) shows in the import window
            // while it is open (the main window's once it closes).
            let app = self.core.app.handle();
            let group = app
                .window_viewport(&WindowId::new(WINDOW))
                .map(|v| app.window_group(v));
            let before = std::mem::replace(&mut self.toasts_in, group);
            for event in events {
                self.import_event(event);
            }
            self.toasts_in = before;
        }
        self.follow_import_focus(ctx);
    }

    /// The import focus's OS window, opened while the focus is open and
    /// closed with it; closed by itself (its close button, `window.close`),
    /// the focus closes, and the main window stays. Where the backend has
    /// no windows of its own (kittest, the web), a window of the main one
    /// instead ([`Shell::embedded_import_window`]).
    fn follow_import_focus(&mut self, ctx: &Egui) {
        if ctx.embed_viewports() {
            self.embedded_import_window(ctx);
            return;
        }
        let app = self.core.app.handle().clone();
        let id = WindowId::new(WINDOW);
        let there = app.window_viewport(&id).is_some();
        let mut w = self.import.get_mut();
        // A window closed is dropped by egui at a pass of the main window
        // that no longer declares it; a pass may still hold it (its last
        // commands), and the main window may then sit idle: it passes
        // again, shortly, until the window is gone.
        if w.open {
            w.going = None;
        }
        if let Some(going) = w.going {
            if ctx.input(|i| i.raw.viewports.contains_key(&going)) {
                ctx.request_repaint_after(GONE_CHECK);
            } else {
                w.going = None;
            }
        }
        match (w.open, there) {
            (true, false) if w.shown.is_some() => {
                w.open = false;
                w.shown = None;
                w.clear_contents();
            }
            (true, false) => {
                let title = w
                    .texts
                    .as_ref()
                    .map_or_else(|| "Import photos".to_owned(), |t| t(window_title()));
                // The main window's title bar start and menus, as every
                // Helios window has (docs/design/windows.md, section 7).
                let spec = WindowSpec::new(WINDOW, title)
                    .size(default_size(ctx.content_rect().size()))
                    .min_size(MIN_SIZE)
                    .shell_bars(true);
                w.shown = Some(id.clone());
                drop(w);
                let state = self.import.state.clone();
                app.open_window(spec, move |ui, window| {
                    let _stall = crate::stall::frame("the import window's pass");
                    // Never drawn while the shell holds it (it does not, between
                    // its own passes).
                    if let Ok(mut w) = state.try_borrow_mut() {
                        let mut frame = Frame {
                            app: window.app().clone(),
                            embedded: window.embedded(),
                            close_requested: window.close_requested(),
                            title: None,
                            close: false,
                        };
                        w.window_pass(ui, &mut frame);
                        if let Some(title) = frame.title {
                            window.set_title(title);
                        }
                        if frame.close {
                            let app = window.app().clone();
                            if holds_main_window(&app) {
                                // Moved into the main window (closed while
                                // it was open): the shell comes back there,
                                // and the window closes at the next pass.
                                app.raise_window(&WindowId::main());
                            } else {
                                window.close();
                                hide_closing_window(ui.ctx());
                            }
                        }
                    }
                });
            }
            // Moved into the main window (closed while it was open): the
            // main window draws the shell again, rather than closing as
            // the last window would (Helios quitting); its own window is
            // closed at the next pass.
            (false, true) if holds_main_window(&app) => {
                drop(w);
                app.raise_window(&WindowId::main());
            }
            (false, true) => {
                w.shown = None;
                drop(w);
                app.close_window(&id);
            }
            (false, false) => w.shown = None,
            (true, true) => {}
        }
    }

    /// The import window inside the main window, where the backend has no
    /// windows of its own (kittest, the web): a window of the main one,
    /// centred and closable, as egui's own embedding of an application
    /// window is neither (it sits at the top left, over the menus, with a
    /// collapse arrow and no close button).
    fn embedded_import_window(&mut self, ctx: &Egui) {
        let state = self.import.state.clone();
        let Ok(mut w) = state.try_borrow_mut() else {
            return;
        };
        if !w.open {
            return;
        }
        let title = w.embedded_title.clone().unwrap_or_else(|| {
            w.texts
                .as_ref()
                .map_or_else(|| "Import photos".to_owned(), |t| t(window_title()))
        });
        let main = ctx.content_rect().size();
        let max = (main - egui::vec2(32.0, 32.0)).max(egui::Vec2::ZERO);
        let mut frame = Frame {
            app: self.core.app.handle().clone(),
            embedded: true,
            close_requested: false,
            title: None,
            close: false,
        };
        let mut open = true;
        egui::Window::new(&title)
            .id(egui::Id::unique(EMBEDDED_ID))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(default_size(main))
            .min_size(MIN_SIZE.min(max))
            .max_size(max)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| w.window_pass(ui, &mut frame));
        if frame.title.is_some() {
            w.embedded_title = frame.title;
        }
        // Its close button closes the import focus, as the OS window's does.
        if !open && w.open {
            w.cancel(ctx);
        }
    }

    /// "`EOS_DIGITAL` inserted": its action opens the window on the card.
    fn card_toast(&mut self, card: &Card) {
        let text = self.text(
            Message::new("gui-import-window-card-inserted", "{ $card } inserted")
                .with("card", card.label.clone()),
        );
        let action = self.text(Message::new("gui-import-window-open", "Import photos…"));
        if let Some(toasts) = self.core.app.handle().service(&TOASTS) {
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
        self.import.get_mut().receive_chosen_folder();
    }

    /// What the window asked of the shell.
    fn import_event(&mut self, event: Event) {
        match event {
            Event::Forget(path) => {
                let args = serde_json::json!({ "path": path.display().to_string() });
                if let Err(e) = self.run(FORGET_FOLDER, args, Origin::User) {
                    self.warn(format!("forget: {e}"));
                }
            }
            Event::Import => self.import_ticked_sources(),
            Event::Promise(id, action) => self.core.activity.ask(action, &id),
            Event::Again(id) => {
                if let Err(e) = self.import_again(&json!({ "id": id }), &Origin::User) {
                    self.warn(e);
                }
            }
            Event::Dropped(paths) => self.open_import_window(&json!({ "paths": paths })),
            Event::Storage => {
                if let Err(e) = self.run(super::storage::COMMAND, Value::Null, Origin::User) {
                    self.warn(format!("storage: {e}"));
                }
            }
            Event::ManageAccounts => {
                let open = egui_pop::api::admin::CMD_OPEN;
                let path = json!({ "path": "accounts/list" });
                if let Err(e) = self.run(open.as_str(), path, Origin::User) {
                    self.warn(format!("accounts: {e}"));
                }
            }
            // Handled in the window's own pass.
            Event::Cancel
            | Event::ChooseInto
            | Event::Favourite(..)
            | Event::Pin(..)
            | Event::Account(_)
            | Event::Profile(_)
            | Event::ManageProfiles => {}
        }
    }

    /// Imports the photos ticked of each source ticked, as
    /// `library.importFiles` does with their `paths` and the choices
    /// (copied or added where they are, filed, named, duplicates,
    /// darktable edits), kept as the last used. Handed to the node, each
    /// import answers at once with its promise: the window stays, showing
    /// its line, and may close while it runs; queued for the host (a
    /// shell with no node), the window closes at once. Without a library
    /// open, nothing is imported: a toast says why and the window stays.
    fn import_ticked_sources(&mut self) {
        // No library to import into: said, the window left open.
        if let Err(e) = self.can_import() {
            self.warn(e);
            return;
        }
        // The grid shows the photos chosen in a service's page.
        if self.import.get().chosen.is_some() {
            self.import_chosen();
            return;
        }
        let (choices, copy, ticked) = {
            let mut w = self.import.get_mut();
            let copy = w.copies();
            let any_card = w.ticked.iter().any(|p| w.is_card(p));
            // The window's choices are kept as the last used; a saved
            // source's are kept on it already.
            if !any_card && w.sources.showing.is_none() {
                w.choices.copy_folders = copy;
            }
            if w.sources.showing.is_none() {
                w.save_choices();
            }
            (w.choices.clone(), copy, std::mem::take(&mut w.ticked))
        };
        // Whether an import went to the node: the window then stays, its
        // promise line showing the import, and may be closed any time.
        let mut handed = false;
        for path in &ticked {
            let (files, card, own) = {
                let mut w = self.import.get_mut();
                // With saved sources, each folder imports with its own
                // choices; copied or not as it says, unless switched.
                let own = w.sources.kept().then(|| {
                    let c = w.choices_for(path);
                    let copy = if w.is_card(path) {
                        w.copy.unwrap_or(true)
                    } else {
                        w.copy.unwrap_or(c.copy_folders)
                    };
                    (c, copy)
                });
                (w.contents.selected_files(path), w.is_card(path), own)
            };
            if files.is_empty() {
                continue;
            }
            let args = match &own {
                Some((c, copy)) => c.args(&files, *copy),
                None => choices.args(&files, copy),
            };
            let started = self.import_files(&args, &Origin::User);
            if let Ok(answer) = &started {
                handed |= answer.get("promise").is_some();
            }
            if let Err(e) = started {
                self.warn(e);
            } else if self
                .import
                .get_mut()
                .sources
                .confirm(&SourceKey::Folder(path.clone()))
            {
                // Import pressed on a source set to import on its own:
                // its automatic import is confirmed.
                self.note(Message::new(
                    "gui-import-auto-confirmed",
                    "Automatic import confirmed: from now on it runs on its own",
                ));
            }
            let mut w = self.import.get_mut();
            if !card {
                w.memory.recent.retain(|r| r != path);
                w.memory.recent.insert(0, path.clone());
                w.memory.recent.truncate(8);
            }
            w.memory.last = Some(path.clone());
        }
        if handed {
            self.import.get_mut().save();
        } else {
            self.import.get_mut().close_after_import();
        }
    }
}

impl ImportWindow {
    /// An import started: the window closes, what it showed forgotten,
    /// the photos chosen and not imported given up.
    pub(super) fn close_after_import(&mut self) {
        self.save();
        crate::start::trace_frame("import window closed");
        self.open = false;
        self.going_away();
        self.clear_contents();
        self.accounts.window_closed();
    }
}

/// How soon the main window passes again while a closed window is not
/// dropped yet.
const GONE_CHECK: std::time::Duration = std::time::Duration::from_millis(50);

/// Hides an OS window closing from its own pass, and brings the main
/// window forward. egui drops a window's viewport at a pass of the main
/// window, and a compositor gives a hidden window no frames (behind a
/// browser opened for a sign-in or a picking): until the main window
/// shows again, the closed window would stay on screen, drawn empty
/// (black) whenever something asks it for a frame. Hidden at once, it
/// waits out of sight; the main window, asked to the front, gets its
/// frames back. Wayland cannot hide a window: it is minimised there.
pub(super) fn hide_closing_window(ctx: &Egui) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
    ctx.send_viewport_cmd_to(ViewportId::ROOT, egui::ViewportCommand::Focus);
    ctx.request_repaint_of(ViewportId::ROOT);
}

/// Whether the import window's content holds the main window: moved
/// into it when the main window was closed while it was open.
fn holds_main_window(app: &egui_pop::api::app::AppHandle) -> bool {
    app.window_viewport(&WindowId::new(WINDOW)) == Some(ViewportId::ROOT)
}

/// The window's title.
fn window_title() -> Message {
    Message::new("gui-import-window-title", "Import photos")
}

impl ImportWindow {
    /// The folder the chooser gave: a recent source, ticked; or where
    /// copies go; or the library folder a picking asked for, the picking
    /// then started.
    fn receive_chosen_folder(&mut self) {
        if let Some(result) = self.choosing_into.as_ref().and_then(Pending::take) {
            self.choosing_into = None;
            if let Ok(files::Chosen::Paths(paths)) = files::chosen(&result)
                && let Some(p) = paths.into_iter().next()
            {
                self.choices.into = Some(p);
            }
        }
        let Some(result) = self.choosing.as_ref().and_then(Pending::take) else {
            return;
        };
        self.choosing = None;
        if let Ok(files::Chosen::Paths(paths)) = files::chosen(&result) {
            for p in paths {
                self.memory.recent.retain(|r| r != &p);
                self.memory.recent.insert(0, p.clone());
                self.memory.recent.truncate(8);
                self.tick(&p, true);
                self.touched = true;
            }
            self.save();
        }
    }

    /// The window's words, in the shell's language; none before the shell
    /// gave its translator.
    fn words(&self, ctx: &Egui, sample: Option<&import_choices::Sample>) -> Option<Words> {
        let texts = self.texts.clone()?;
        let t = |m: Message| texts(m);
        let (backup, no_backup) = self.shell_words.backup.clone();
        let counts = self.contents.counts();
        Some(Words {
            title: t(window_title()),
            sources: t(Message::new("gui-import-window-sources", "Sources")),
            cards: t(Message::new("gui-import-window-cards", "Cards and cameras")),
            no_cards: t(Message::new(
                "gui-import-window-no-cards",
                "No card inserted",
            )),
            folders: t(Message::new("gui-import-window-folders", "Folders")),
            history: t(Message::new("gui-import-history", "History")),
            services: t(Message::new("gui-import-window-services", "Services")),
            no_services: t(Message::new(
                "gui-import-window-no-services",
                "No account yet",
            )),
            no_folders: t(Message::new(
                "gui-import-window-no-folders",
                "No folder yet",
            )),
            add: t(Message::new("gui-import-window-add", "Add…")),
            add_hint: t(Message::new(
                "gui-import-window-add-hint",
                "Add a source: a folder, a card, an account…",
            )),
            favourites: t(Message::new("gui-import-window-favourites", "Favourites")),
            no_favourites: t(Message::new("gui-import-window-no-favourites", "None yet")),
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
            edit_source: t(Message::new(
                "gui-import-window-edit-source",
                "Edit source…",
            )),
            forget_source: t(Message::new(
                "gui-import-window-forget-source",
                "Forget this source",
            )),
            forget_source_hint: t(Message::new(
                "gui-import-window-forget-source-hint",
                "Takes it off this column, its name and choices with it; the photos and the library are untouched",
            )),
            auto: t(super::import_auto::title_message()),
            auto_off: t(super::import_auto::off_message()),
            auto_plugged: t(Trigger::Plugged.message()),
            auto_new_files: t(Trigger::NewFiles.message()),
            forget_hint: t(Message::new(
                "gui-import-window-forget-hint",
                "Drops what the image map knows of this folder's files not in the Library; it is not recorded again until you list it",
            )),
            contents: t(Message::new("gui-import-window-contents", "Contents")),
            grid: self.grid_words(ctx, &t),
            choices: self.choice_words(&t, sample),
            cancel: t(Message::new("gui-import-window-cancel", "Cancel")),
            import_count: t(
                Message::new("gui-import-window-import", "Import { $count }")
                    .with("count", counts.selected),
            ),
            totals: t(
                Message::new("gui-import-window-totals", "{ $photos } photos, { $size }")
                    .with("photos", counts.selected)
                    .with(
                        "size",
                        crate::library::size_text(ctx, counts.selected_bytes),
                    ),
            ),
            profile: t(Message::new("gui-import-window-profile", "Import profile")),
            manage_profiles: t(Message::new(
                "gui-import-window-manage-profiles",
                "Manage profiles…",
            )),
            profile_summary: super::import_profile_store::summary(
                &self.choices,
                self.copies(),
                sample.and_then(import_choices::Sample::date),
                &t,
            ),
            nothing: t(Message::new(
                "gui-import-window-nothing",
                "Tick a card or a folder to import",
            )),
            none_ticked: t(Message::new(
                "gui-import-window-none-ticked",
                "Tick the photos to import",
            )),
            no_library: self.shell_words.no_library.clone(),
            backup,
            no_backup,
            storage: t(Message::new("gui-import-backup-storage", "Storage…")),
        })
    }

    fn grid_words(&self, ctx: &Egui, t: &dyn Fn(Message) -> String) -> GridWords {
        let counts = self.contents.counts();
        let size = crate::library::size_text(ctx, counts.bytes);
        GridWords {
            counts: t(Message::new(
                "gui-import-window-counts",
                "{ $photos } photos, { $new } new, { $size }",
            )
            .with("photos", counts.photos)
            .with("new", counts.new)
            .with("size", size)),
            reading: self.reading().map(|path| {
                // Translated around a mark, so the path alone is shortened
                // to the line's width.
                const MARK: &str = "\u{E000}";
                let text = t(
                    Message::new("gui-import-window-reading", "Reading { $path }")
                        .with("path", MARK),
                );
                let (before, after) = text.split_once(MARK).unwrap_or((text.as_str(), ""));
                (
                    before.to_owned(),
                    after.to_owned(),
                    path.display().to_string(),
                )
            }),
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
            in_library: t(Message::new(
                "gui-import-window-in-library",
                "in your Library",
            )),
            pair: t(Message::new("gui-import-window-pair", "RAW+JPG")),
            offline: t(Message::new("gui-import-window-offline", "offline")),
            browse: super::import_browsers::BrowseWords::new(t),
            choose_on_the_right: t(Message::new(
                "gui-import-window-choose-on-the-right",
                "Choose photos in the panel on the right: they show here.",
            )),
        }
    }

    /// The window's own pass (its content, given to egui-pop's
    /// `open_window`): the cards and the scans followed, the window drawn
    /// with its status bar, and what its controls asked done, here or by
    /// the shell at the main window's next pass.
    fn window_pass(&mut self, ui: &mut egui::Ui, window: &mut Frame) {
        let ctx = ui.ctx().clone();
        *self.drawn_in.lock().unwrap_or_else(PoisonError::into_inner) = Some(ctx.viewport_id());
        if !self.open {
            window.close = true;
            return;
        }
        self.follow_cards();
        // The photos chosen in a service's page, else the sources ticked.
        if !self.follow_chosen() {
            self.follow_scans();
        }
        self.history.follow();
        // The Choices column shows the selected source's; a change made
        // in this pass is saved on it at its end.
        self.follow_selection();
        // The Contents column shows the source browsed in its browser.
        self.follow_browser();
        let shown = self.sources.showing.clone();
        let before = self.choices.clone();
        // The profile editor edits a profile, not the source shown.
        let managing = self.manager.is_some();
        let sample = self.sample();
        let Some(words) = self.words(&ctx, sample.as_ref()) else {
            return;
        };
        window.title = Some(words.title.clone());
        let app = window.app.clone();
        // Its title bar and menus are egui-pop's (`WindowSpec::shell_bars`).
        let mut event = if window.embedded {
            self.window_ui(ui, &words, &app)
        } else {
            egui::CentralPanel::default()
                .show(ui, |ui| self.window_ui(ui, &words, &app))
                .inner
        };
        // Dropped anywhere else on the window: its sources too.
        if event.is_none() {
            let dropped: Vec<PathBuf> = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect()
            });
            if !dropped.is_empty() {
                event = Some(Event::Dropped(dropped));
            }
        }
        // Its close button closes the import focus.
        if window.close_requested {
            event = Some(Event::Cancel);
        }
        match event {
            None => {}
            Some(Event::Cancel) => {
                self.cancel(&ctx);
                window.close = true;
                // Closed: nothing more of this pass (no dialog, no choice
                // kept), the window gone at once.
                return;
            }
            Some(Event::ChooseInto) => {
                if let Some(f) = app.service(&FILES) {
                    let request =
                        FileRequest::folder(&words.choices.change).purpose("helios.import.into");
                    self.choosing_into = Some(f.choose(&app, request));
                }
            }
            Some(Event::Favourite(path, on)) => {
                self.memory.favourites.retain(|p| p != &path);
                self.memory.cards.remove(&path);
                if on {
                    // A card is pinned by its volume.
                    if let Some(m) = self.sources.card_at(&path) {
                        self.memory.cards.insert(path.clone(), m.id.clone());
                    }
                    self.memory.favourites.push(path);
                }
                self.save();
            }
            Some(Event::Pin(id, on)) => {
                self.memory.pinned.retain(|p| p != &id);
                if on {
                    self.memory.pinned.push(id);
                }
                self.save();
            }
            Some(Event::Account(asked)) => self.account_asked(asked),
            Some(Event::Profile(id)) => self.use_profile(&id),
            Some(Event::ManageProfiles) => self.open_manager(),
            // The shell's, at the main window's next pass.
            Some(
                e @ (Event::Import
                | Event::Forget(_)
                | Event::Storage
                | Event::ManageAccounts
                | Event::Dropped(_)
                | Event::Again(_)
                | Event::Promise(..)),
            ) => {
                self.events.push(e);
                ctx.request_repaint_of(ViewportId::ROOT);
            }
        }
        // The connection running, in a dialog over the window; before
        // it, a plugin's capabilities asked for, its action run again
        // once allowed.
        if let Some(texts) = self.texts.clone() {
            if let Some(again) =
                super::import_accounts::consent_dialog(&ctx, &mut self.accounts, &|m| texts(m))
            {
                self.account_run(again);
            }
            super::import_accounts::connect_dialog(&ctx, &mut self.accounts, &|m| texts(m));
        }
        // The chooser's answer comes in this window too.
        self.receive_chosen_folder();
        if managing || self.manager.is_some() {
            self.keep_profile_edits();
        } else {
            self.keep_source_choices(shown, &before);
        }
        self.link_sources();
    }

    /// Adds, reconnects or signs out an account, as the Sources column
    /// asked; what fails is said in the window's toasts.
    pub(super) fn account_asked(&mut self, asked: super::import_accounts::Asked) {
        // A plugin's capabilities not granted yet are asked for first.
        if self.accounts.ask_consent(&asked).is_some() {
            return;
        }
        self.account_run(asked);
    }

    /// Runs what the Sources column asked, its plugin's consent settled.
    pub(super) fn account_run(&mut self, asked: super::import_accounts::Asked) {
        use super::import_accounts::Asked;
        let done = match asked {
            Asked::Add(connector) => self.connect_account(&connector, None),
            Asked::Reconnect(id) => {
                let connector = self
                    .accounts
                    .listed()
                    .iter()
                    .find(|a| a.id == id)
                    .map(|a| a.connector.clone());
                match connector {
                    Some(c) => self.connect_account(&c, Some(&id)),
                    None => Err(format!("no account {id}")),
                }
            }
            // Turned into the shell's event where it is asked.
            Asked::Manage => Ok(()),
            Asked::Pick(id) => {
                // Where they go is never asked: the source's `download`
                // folder, at the footer's Import.
                let wake = self.waker();
                self.accounts.pick(&self.connectors, &id, wake)
            }
        };
        if let Err(e) = done {
            log::warn!("import accounts: {e}");
        }
    }

    /// Starts connecting a new account of `connector`, or `account` again.
    pub(super) fn connect_account(
        &mut self,
        connector: &str,
        account: Option<&str>,
    ) -> Result<(), String> {
        let wake = self.waker();
        self.accounts.connect(
            &self.connectors,
            connector,
            account,
            self.texts.as_ref(),
            wake,
        )
    }

    /// The window's body: its status bar at the bottom, the three columns
    /// and the footer above.
    fn window_ui(
        &mut self,
        outer: &mut egui::Ui,
        words: &Words,
        app: &egui_pop::api::app::AppHandle,
    ) -> Option<Event> {
        let mut event = None;
        // The window has the size the person gives it, never its
        // content's: an `egui::Window` (the window embedded) grows to what
        // its content took the frame before, so a content a point too wide
        // grew it frame after frame. The content lays out in the room it
        // is given, apart, and the window holds that room.
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
        let reading = self.scans.iter().any(|(_, _, done)| !done);
        status_bar_ui(ui, &words.grid, reading);
        // The profile editor takes the columns' and the footer's room,
        // over the wizard when its "Customise…" opened it.
        if self.manager.is_some() {
            if self.manager_ui(ui, &words.choices) {
                event = Some(Event::ChooseInto);
            }
            return event;
        }
        // So does the wizard.
        if self.wizard.is_some() {
            self.wizard_ui(ui, app);
            return event;
        }
        let w = self;
        let layout = DialogLayout::new(ui, "helios.import.window").fill(true);
        // The body is the room above the footer, exactly: no scroll area
        // around it, each column scrolls on its own (Sources and Choices
        // when taller than the window, Contents its grid).
        let height = layout.body_height(ui);
        // Sources and Choices keep their widths (a quarter of the room at
        // most); Contents takes the rest, each column in its own rect, a
        // line between them.
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
        // A column's `Ui` is the body's child, not a scope it allocates:
        // a column taller than its rect is clipped there, and never pushes
        // the footer down nor the body past the window.
        let column = |ui: &mut egui::Ui,
                      salt: &str,
                      rect: egui::Rect,
                      add: &mut dyn FnMut(&mut egui::Ui)| {
            let mut ui = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(salt)
                    .max_rect(rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            add(&mut ui);
        };
        column(ui, "helios.import.sources", left, &mut |ui| {
            source_list::sources_ui(ui, w, words, &mut event);
        });
        column(ui, "helios.import.contents", middle, &mut |ui| {
            ui.heading(&words.contents);
            let browse = &words.grid.browse;
            match super::import_browsers::switch_ui(ui, w, browse) {
                Shown::Grid => {
                    if let Some(paths) = contents_ui(ui, w, &words.grid, reading, app) {
                        event = Some(Event::Dropped(paths));
                    }
                }
                // Its connection and "Choose photos" are in the Detail
                // column; the photos chosen fill the ordinary grid here.
                Shown::Picker(account) if w.shows_chosen_of(&account) => {
                    if let Some(paths) = contents_ui(ui, w, &words.grid, reading, app) {
                        event = Some(Event::Dropped(paths));
                    }
                }
                Shown::Picker(_) => {
                    ui.separator();
                    ui.weak(&words.grid.choose_on_the_right);
                }
                // The album selected under the account, in the grid once
                // its photos are listed.
                Shown::Albums if w.shows_album() => {
                    if let Some(paths) = contents_ui(ui, w, &words.grid, reading, app) {
                        event = Some(Event::Dropped(paths));
                    }
                }
                Shown::Albums => {
                    ui.separator();
                    match w.album_shown().map(|(_, _, p)| p) {
                        Some(super::import_albums::Photos::Failed(why)) => {
                            ui.weak(format!("{} {why}", browse.album_failed));
                        }
                        Some(_) => {
                            ui.weak(&browse.album_reading);
                        }
                        None => {
                            ui.weak(&browse.album_choose);
                        }
                    }
                }
                Shown::NotListed => {
                    ui.separator();
                    ui.weak(&browse.not_listed);
                }
            }
        });
        // The Detail column: the choices every source shares on top, the
        // selected source's connector section below
        // ([`super::import_detail`]).
        column(ui, "helios.import.choices", right, &mut |ui| {
            let mut storage = false;
            let detail = w.detail();
            if let Some(e) = profile_ui(ui, w, words) {
                event = Some(e);
            }
            egui::ScrollArea::vertical()
                .id_salt("helios.import.choices")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.separator();
                    ui.heading(&detail.label);
                    for part in &detail.parts {
                        match part {
                            Part::Account { id, picker: true } => {
                                if let Some(e) = choose_contents_ui(ui, w, id) {
                                    event = Some(e);
                                }
                                ui.separator();
                            }
                            Part::Account { id, picker: false } => {
                                ui.label(account_header(w, id));
                            }
                            // The profile's: in the profile editor.
                            Part::Copy => {}
                            Part::Params => {
                                ui.separator();
                                w.detail_params_ui(ui, &detail);
                            }
                        }
                    }
                    ui.separator();
                    storage = backup_line_ui(ui, words);
                });
            if storage {
                event = Some(Event::Storage);
            }
        });
        layout.footer(ui, |ui| {
            // The imports running, and those ended during this run: they
            // go on when the window closes.
            if !w.promises.is_empty() {
                ui.separator();
            }
            for p in &w.promises {
                if let Some(action) = crate::promise_line::show(ui, p) {
                    event = Some(Event::Promise(p.id.clone(), action));
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let count = w.contents.counts().selected;
                    let bad_name = words.choices.bad_name.as_ref().filter(|_| w.copies());
                    let why = if let Some(e) = &words.no_library {
                        e
                    } else if w.ticked.is_empty() && w.chosen.is_none() {
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
        event
    }
}

/// The window's status bar: "Reading <path>" while the sources are read,
/// the path shortened in its middle to the bar's width.
fn status_bar_ui(ui: &mut egui::Ui, words: &GridWords, reading: bool) {
    egui::Panel::bottom("helios.import.status")
        .resizable(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // The bar keeps its height, reading or not.
                ui.set_min_height(ui.spacing().interact_size.y);
                let Some((before, after, path)) = words.reading.as_ref().filter(|_| reading) else {
                    return;
                };
                ui.spinner();
                let width = ui.available_width();
                let font = egui::TextStyle::Body.resolve(ui.style());
                let color = ui.visuals().weak_text_color();
                let fits = |p: &str| {
                    let text = format!("{before}{p}{after}");
                    ui.painter()
                        .layout_no_wrap(text, font.clone(), color)
                        .size()
                        .x
                        <= width
                };
                let path = shorten_middle(path, fits);
                ui.add(
                    egui::Label::new(egui::RichText::new(format!("{before}{path}{after}")).weak())
                        .truncate(),
                );
            });
        });
}

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

/// What a source's menu asked.
pub(super) enum MenuAsked {
    Edit,
    Forget,
    /// "Import automatically": off, or this trigger.
    Auto(Option<Trigger>),
}

/// "Edit source…", "Import automatically" (when `auto` gives the trigger
/// offered and the one set) and "Forget this source", in a source's menu.
fn menu_items(
    ui: &mut egui::Ui,
    words: &Words,
    auto: Option<(Trigger, Option<Trigger>)>,
) -> Option<MenuAsked> {
    let mut asked = None;
    if ui.button(&words.edit_source).clicked() {
        asked = Some(MenuAsked::Edit);
        ui.close();
    }
    if let Some((offered, set)) = auto {
        ui.menu_button(&words.auto, |ui| {
            if ui.radio(set.is_none(), &words.auto_off).clicked() {
                asked = Some(MenuAsked::Auto(None));
                ui.close();
            }
            let label = match offered {
                Trigger::Plugged => &words.auto_plugged,
                Trigger::NewFiles => &words.auto_new_files,
            };
            if ui.radio(set == Some(offered), label).clicked() {
                asked = Some(MenuAsked::Auto(Some(offered)));
                ui.close();
            }
        });
    }
    if ui
        .button(&words.forget_source)
        .on_hover_text(&words.forget_source_hint)
        .clicked()
    {
        asked = Some(MenuAsked::Forget);
        ui.close();
    }
    asked
}

impl ImportWindow {
    /// What a source's menu asked: its editor opened, or it forgotten.
    fn source_menu_asked(&mut self, asked: MenuAsked, key: SourceKey) {
        match asked {
            MenuAsked::Edit => self.open_editor(key),
            MenuAsked::Forget => self.forget_source(&key),
            MenuAsked::Auto(auto) => self.set_auto(&key, auto),
        }
    }
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

/// The top of the Detail column: the import profile in use (a menu of
/// the profiles, then "Manage profiles…"), whose source it is for, and
/// what it does in one line.
fn profile_ui(ui: &mut egui::Ui, w: &mut ImportWindow, words: &Words) -> Option<Event> {
    let mut event = None;
    ui.heading(&words.choices.heading);
    let in_use = w.profile_in_use();
    let shown = w
        .profiles
        .get(&in_use)
        .map_or_else(|| in_use.clone(), |p| p.name.clone());
    egui::ComboBox::new("helios.import.profile", &words.profile)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for p in w.profiles.list() {
                if ui.selectable_label(p.id == in_use, &p.name).clicked() {
                    event = Some(Event::Profile(p.id.clone()));
                }
            }
            ui.separator();
            if ui.selectable_label(false, &words.manage_profiles).clicked() {
                event = Some(Event::ManageProfiles);
            }
        });
    if let Some(shown_for) = &words.choices.shown_for {
        ui.weak(shown_for);
    }
    ui.add(egui::Label::new(egui::RichText::new(&words.profile_summary).weak()).wrap());
    event
}

/// The Contents column: the count line, the filters and the selection,
/// then the file browser. Gives the paths a drop on it offered.
fn contents_ui(
    ui: &mut egui::Ui,
    w: &mut ImportWindow,
    words: &GridWords,
    reading: bool,
    app: &egui_pop::api::app::AppHandle,
) -> Option<Vec<PathBuf>> {
    let c = &mut w.contents;
    // What is read now is said in the window's status bar.
    ui.label(&words.counts);
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
            w.tick_revision += 1;
        }
        if ui.small_button(&words.none).clicked() {
            c.tick_all(false);
            w.tick_revision += 1;
        }
        // The focused photo's day, alone.
        let day = w
            .browser
            .as_ref()
            .and_then(FileBrowser::focus)
            .and_then(|f| c.cell_of(f))
            .map(|i| c.cells[i].day());
        if ui
            .add_enabled(day.is_some(), egui::Button::new(&words.day).small())
            .clicked()
            && let Some(day) = day
        {
            let types = c.types;
            for cell in &mut c.cells {
                cell.ticked = cell.day() == day || !cell.passes(types);
            }
            w.tick_revision += 1;
        }
    });
    ui.separator();
    if w.contents.cells.is_empty() && !reading {
        ui.weak(&words.empty);
    }
    w.sync_browser(words);
    // The folder tree, when shown, takes a strip on the grid's left.
    let tree = w.browsing.current == Some(helios_connect::Browser::Tree);
    let room = ui.available_rect_before_wrap();
    let grid_rect = if tree {
        let width = (room.width() * 0.3).clamp(80.0, 220.0);
        let strip = egui::Rect::from_min_size(room.min, egui::vec2(width, room.height()));
        let mut strip_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("helios.import.tree.strip")
                .max_rect(strip)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        strip_ui.set_clip_rect(strip.intersect(ui.clip_rect()));
        super::import_browsers::tree_ui(&mut strip_ui, w, &words.browse);
        let gap = ui.spacing().item_spacing.x;
        egui::Rect::from_min_max(egui::pos2(strip.max.x + gap, room.min.y), room.max)
    } else {
        room
    };
    let mut grid_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("helios.import.grid")
            .max_rect(grid_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    ui.advance_cursor_after_rect(room);
    let ui = &mut grid_ui;
    let browser = w.browser.as_mut()?;
    let out = browser.show_in(ui, app, "helios.import");
    if out.selection_changed {
        w.take_selection();
    }
    // The scans glance at the cells waiting for a picture first, a pair's
    // files both.
    if let Some(thumbs) = &w.thumbs {
        let c = &w.contents;
        let first: Vec<PathBuf> = thumbs
            .waiting()
            .iter()
            .filter_map(|p| c.cell_of(p))
            .flat_map(|i| c.cells[i].raw.iter().chain(&c.cells[i].picture).cloned())
            .collect();
        *w.glance_first
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = first;
    }
    if let Some(thumbs) = &w.thumbs {
        let c = &w.contents;
        let in_view: Vec<PathBuf> = thumbs
            .on_screen()
            .iter()
            .filter_map(|p| c.cell_of(p))
            .flat_map(|i| c.cells[i].raw.iter().chain(&c.cells[i].picture).cloned())
            .collect();
        show_glances(&mut w.glances_shown, c, &in_view);
        // The image map reads the photos in view first (a service's
        // photos chosen have no file to read).
        if let Some(feed) = &w.map_feed
            && w.chosen.is_none()
            && in_view != w.on_screen
        {
            feed.on_screen(in_view.clone());
            w.on_screen = in_view;
        }
    }
    out.dropped.map(|d| d.paths).filter(|p| !p.is_empty())
}

/// Tells the glance cache the glances of the files in view are shown (once
/// each): they outlive every prefetched glance, in RAM and, marked off the
/// interface thread, on disk.
fn show_glances(
    shown: &mut std::collections::HashSet<PathBuf>,
    contents: &Contents,
    in_view: &[PathBuf],
) {
    let cache = crate::import_scan::GlanceCache::global();
    let mut warm = Vec::new();
    for path in in_view {
        if shown.contains(path) {
            continue;
        }
        let Some((bytes, modified)) = contents.file_facts(path) else {
            continue;
        };
        // Not kept yet: asked again on a later frame, once glanced.
        if let Some(on_disk) = cache.show(path, bytes, modified) {
            shown.insert(path.clone());
            warm.extend(on_disk);
        }
    }
    if !warm.is_empty() {
        let cache = cache.clone();
        let spawned = std::thread::Builder::new()
            .name("helios-glance-shown".into())
            .spawn(move || cache.warm(&warm));
        if let Err(e) = spawned {
            log::debug!("import glances shown: no thread: {e}");
        }
    }
}

/// A photo as the browser lists it: by its raw (a pair's) or its picture,
/// named after both, with its capture time.
fn entry_of(cell: &crate::import_scan::Cell) -> Option<SourceEntry> {
    let secs = |s: i64| {
        u64::try_from(s)
            .ok()
            .map(|s| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s))
    };
    Some(SourceEntry {
        path: cell.main()?.clone(),
        name: cell.name(),
        kind: EntryKind::File,
        size: Some(cell.bytes),
        modified: cell.modified.and_then(secs),
        // The camera's local time, kept as if it were UTC (as the image
        // map keeps it): the agenda's days are the camera's.
        taken: cell.taken.and_then(|t| secs(t.and_utc().timestamp())),
        app_id: None,
    })
}

impl ImportWindow {
    /// The browser over the contents: made the first time, given what the
    /// scans found since (entries, marks, previews), on the first source
    /// ticked, its selection the photos ticked.
    fn sync_browser(&mut self, words: &GridWords) {
        // The source clicked in this pass: the one before it is gone from
        // the contents before they are published.
        self.forget_unticked();
        let thumbs = self.thumbnails();
        // The previews read (sRGB pictures at the cell's size already) go
        // to the thumbnails, which make a few textures a frame.
        for cell in &mut self.contents.cells {
            if let Some(thumb) = cell.thumb.take() {
                cell.previewed = true;
                if let Some(main) = cell.main() {
                    thumbs.glanced(main, Some(thumb));
                }
            }
        }
        let place = self
            .chosen_root()
            .map(Path::to_path_buf)
            .or_else(|| self.ticked.first().cloned())
            .unwrap_or_else(|| PathBuf::from(NOWHERE));
        let done = self.chosen.is_some() || self.read_through();
        let sig = (
            self.contents.revision(),
            self.contents.new_only,
            place.clone(),
            done,
        );
        let republished = self.published.as_ref() != Some(&sig);
        if republished {
            let c = &mut self.contents;
            let shown = c.shown();
            let mut marks = std::collections::HashMap::with_capacity(shown.len());
            let mut entries = Vec::with_capacity(shown.len());
            for i in shown {
                let cell = &c.cells[i];
                let Some(entry) = entry_of(cell) else {
                    continue;
                };
                let mut badges = Vec::new();
                if cell.offline {
                    badges.push(words.offline.clone());
                }
                if c.new_only && cell.known() {
                    badges.push(words.in_library.clone());
                }
                if cell.is_pair() {
                    badges.push(words.pair.clone());
                }
                marks.insert(
                    entry.path.clone(),
                    EntryMarks {
                        badges,
                        dimmed: !c.importable(i),
                    },
                );
                entries.push(entry);
            }
            *self.marks.lock().unwrap_or_else(PoisonError::into_inner) = marks;
            self.source.publish(&place, entries, done);
            self.published = Some(sig);
        }
        let made = self.browser.is_none();
        let browser = self.browser.get_or_insert_with(|| {
            let marks = self.marks.clone();
            let mut b = FileBrowser::new(
                "helios.import",
                Arc::new(self.source.clone()),
                place.clone(),
            )
            .with_modes(ViewModes::built_in())
            // The photos alone: no folder above them in Finder's columns.
            .with_root(NOWHERE)
            .with_thumbnails(thumbs)
            .with_marks(move |e| {
                marks
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&e.path)
                    .cloned()
                    .unwrap_or_default()
            })
            // Folders and cards from the system open as the sources.
            .with_drops(|offer| offer.origin == DropOrigin::System);
            // An import starts on its thumbnails, until another mode is
            // kept for the window.
            b.set_mode("thumbnails");
            b
        });
        if made {
            Self::browser_made(browser, &mut self.browsing);
        }
        if browser.place() != place {
            browser.open(place);
        }
        browser.pump();
        // A listing that ended is told nothing more: one that missed what
        // was just published (another source's photos gone, a filter's)
        // lists again, never keeping photos the contents no longer show.
        if republished && !browser.loading() {
            let published = self.source.entries();
            let listed: std::collections::HashSet<&Path> =
                browser.entries().iter().map(|e| e.path.as_path()).collect();
            if listed.len() != published.len()
                || published.iter().any(|e| !listed.contains(e.path.as_path()))
            {
                browser.reload();
            }
        }
        // The selection is the photos ticked: the new ones come ticked.
        let key = (
            self.contents.revision(),
            browser.entries().len(),
            self.tick_revision,
        );
        if self.selection_synced != Some(key) {
            let c = &self.contents;
            let ticked = browser
                .entries()
                .iter()
                .filter(|e| c.cell_of(&e.path).is_some_and(|i| c.cells[i].ticked))
                .map(|e| e.path.clone())
                .collect::<Vec<_>>();
            browser.set_selection(ticked);
            self.selection_synced = Some(key);
        }
    }

    /// The photos the person selected in the browser are the ones ticked
    /// (a photo the browser does not list keeps its tick).
    fn take_selection(&mut self) {
        let Some(browser) = &self.browser else { return };
        let selected = &browser.selection().paths;
        let listed: std::collections::HashSet<&Path> =
            browser.entries().iter().map(|e| e.path.as_path()).collect();
        for cell in &mut self.contents.cells {
            if let Some(main) = cell.main().filter(|m| listed.contains(m.as_path())) {
                cell.ticked = selected.contains(main);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::shorten_middle;

    #[test]
    fn a_long_path_is_shortened_in_its_middle_keeping_its_file_name() {
        let path = "/media/someone/EOS_DIGITAL/DCIM/100CANON/IMG_0042.CR3";
        assert_eq!(shorten_middle(path, |_| true), path);
        let short = shorten_middle(path, |s| s.chars().count() <= 24);
        assert_eq!(short.chars().count(), 24);
        assert!(short.contains('…'));
        assert!(short.starts_with("/media"));
        assert!(short.ends_with("IMG_0042.CR3"));
    }
}
