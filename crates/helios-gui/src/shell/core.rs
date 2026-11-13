//! The process core: what every window of the process shares.
//!
//! Helios runs one OS window per focus (a work intention: "edit the
//! wedding", "import the card"; see docs/design/windows.md). The shell is
//! split along that line: a [`Core`], one per process, holds what all
//! windows share, and a [`Focus`](super::focus::Focus), one per window,
//! holds what belongs to that window alone.
//!
//! The core holds egui-pop's application (one per process), the command
//! registry and its queue, the theme and the settings (profiles), the
//! value store and its undo, the viewport engine and its render cache
//! (shared by every window's viewport), the GPU render state (one device
//! per process), the clipboard, the jobs (the scheduler's reports, the
//! status line, the nodes followed), the imports and exports handed to
//! the host, the notices and the log, and the Settings app's pages
//! (Storage, Health check). The catalogue itself (the vault, the cache
//! service, the node connection) is the host's, outside the shell, and
//! one per process too.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use egui_pop::api::App;
use egui_pop::api::dialogs::Dialogs;
use helios_core::{Engine, Message};
use helios_shell::commands::Registry;
use serde_json::Value;

use super::{ExportAsk, ImportLines, Queue, health, storage};
use crate::dialogs::Notice;
use crate::plugins::{Declarations, ListSource};

/// What every window of the process shares (see the module's docs).
pub(super) struct Core {
    /// egui-pop's application: one per process. Its window scope (a
    /// layout, overlays and toasts per window) comes with egui-pop's
    /// multi-window API.
    pub(super) app: App,
    pub(super) registry: Arc<Mutex<Registry>>,
    pub(super) lists: Rc<RefCell<ListSource>>,
    /// Keys, menu entries and ticks of Helios's commands.
    pub(super) declarations: Rc<RefCell<Declarations>>,
    /// The panels declared: every window may show any of them.
    pub(super) panels: Vec<egui_pop::model::panels::PanelSpec>,
    /// The commands' actions, applied at the next frame.
    pub(super) queue: Queue,
    /// The journal read up to here, for the workbench's refusals.
    pub(super) seen: u64,
    pub(super) themes: crate::theme::Themes,
    pub(super) theme: crate::theme::Tokens,
    pub(super) theme_name: String,
    pub(super) theme_overrides: BTreeMap<String, serde_json::Value>,
    /// The theme egui-pop last installed, as seen by the shell.
    pub(super) installed: Option<crate::theme::ThemeSpec>,
    /// Where profiles are kept (`<name>.helios-profile`).
    pub(super) profiles_dir: PathBuf,
    pub(super) profile_name: String,
    /// A profile read and shown as a diff, waiting for `profile.import.apply`.
    pub(super) pending_import: Option<helios_profile::Profile>,
    /// This machine, as profile variants select it.
    pub(super) machine: helios_profile::Machine,
    /// The profile in use as read, with its variants, and as resolved for
    /// this machine.
    pub(super) loaded: Option<(helios_profile::Profile, helios_profile::Resolved)>,
    /// What `profile.resolved` answers.
    pub(super) resolved_view: Arc<Mutex<Value>>,
    /// The Stacks panel's proposals.
    pub(super) stack_review: Rc<RefCell<crate::stack_review::Review>>,
    /// The duplicates imports found (the Duplicates panel).
    pub(super) duplicates: Rc<RefCell<crate::duplicates::Duplicates>>,
    /// Imports asked for (`library.import`'s arguments), for the host.
    pub(super) imports: Vec<Value>,
    /// Why the host has no library to import into (no `library.import`
    /// registered), else none: imports then say so instead of being
    /// queued.
    pub(super) no_library: Option<Message>,
    /// The Health check page (a page of the Settings app).
    pub(super) health: health::HealthPage,
    /// The Storage page (a page of the Settings app).
    pub(super) storage: storage::StoragePage,
    /// The lines of the imports the host runs (`import_sink`).
    pub(super) import_lines: (ImportLines, std::sync::mpsc::Receiver<(Value, Value)>),
    /// Exports asked for, for the host.
    pub(super) exports: Vec<ExportAsk>,
    /// Where `workshop.run` hands a run: the host's runner, which holds
    /// the library (none without one).
    pub(super) workshop_runner: Option<super::workshop_run::RunFn>,
    /// Where `workshop.review` reads and changes runs (the host's runner).
    pub(super) workshop_reviewer: Option<super::workshop_run::ReviewFn>,
    /// What the exports the host runs say (`export_sink`).
    pub(super) export_lines: (
        std::sync::mpsc::Sender<helios_export::batch::Line>,
        std::sync::mpsc::Receiver<helios_export::batch::Line>,
    ),
    /// The Log panel's entries.
    pub(super) log: Rc<RefCell<crate::log_panel::Log>>,
    /// The library's images on disk, by id, for quick open.
    pub(super) image_files: BTreeMap<String, PathBuf>,
    /// egui-pop's dialogs, once the application is built (`dialog.current`).
    pub(super) dialogs: Arc<OnceLock<Dialogs>>,
    /// Confirmations are answered yes by policy (a script's `--yes`).
    pub(super) assume_yes: bool,
    /// Every notice so far; the status bar shows the last, toasts the
    /// warnings and errors.
    pub(super) notices: Vec<Notice>,
    /// The GUI's clock, in seconds, as of this frame.
    pub(super) now: f64,
    /// The viewport engine and its render cache, shared by every window's
    /// viewport ([`crate::viewport::Viewport::on_engine`]).
    pub(super) engine: Arc<Engine>,
    /// The render state the viewport's GPU path waits to be installed on
    /// (the process's one device).
    pub(super) gpu: Option<eframe::egui_wgpu::RenderState>,
    /// What installing it found.
    pub(super) gpu_info: Option<egui_pop::viewport::gpu::GpuInfo>,
    /// The clipboard's endpoints (`crate::clipboard`).
    pub(super) clip: crate::clipboard::Endpoints,
    /// The system clipboard beyond text: files and images
    /// (`crate::clipboard::system`).
    pub(super) system: crate::clipboard::system::Shared,
    /// The image being rendered for the system clipboard.
    pub(super) copying: Option<std::sync::mpsc::Receiver<Result<image::RgbaImage, String>>>,
    /// Every value widgets and agents edit (`param.*` commands), and what
    /// Edit > Undo walks back through the host.
    pub(super) values: Arc<Mutex<helios_values::Store>>,
    /// The open image's history snapshots (`crate::edit_snapshots`).
    pub(super) snapshots: crate::edit_snapshots::Shared,
    /// Headless runs: no cursor blink, no animation and no repaint asked
    /// by the workspace's watcher, so a frame settles and a test's `run`
    /// ends.
    pub(super) still: bool,
    /// The catalogues the shell's text is shown through.
    pub(super) i18n: egui_pop::i18n::I18n,
    /// What later launches and links asked to open (`platform`).
    pub(super) opens: crate::platform::Opens,
    /// The viewport engine's events read up to here, and the modules whose
    /// GPU fallback was already said.
    pub(super) engine_seen: u64,
    pub(super) fell_back: BTreeSet<String>,
    /// The catalogues' generation About's description was last
    /// translated for (`follow_language`).
    pub(super) about_generation: Option<u64>,
    /// Where background work reports (`job.*` on the shell's bus).
    pub(super) jobs: helios_core::jobs::Jobs,
    /// The library's undo steps (imports), shared with the history owner.
    pub(super) library_history: crate::library_history::LibraryHistory,
    /// Helios's status line, drawn in egui-pop's status bar.
    pub(super) status: crate::jobs::Status,
    /// The jobs' ends read up to here, for the desktop notification.
    pub(super) ended: crate::jobs::Ended,
    /// The nodes followed, whose jobs show in the jobs centre.
    pub(super) relays: crate::jobs::Relays,
    /// The lost nodes' jobs to take out of the jobs centre.
    pub(super) forgotten: crate::jobs::Forgotten,
    /// The first run's "Coming from darktable?" is offered (the desktop
    /// only, `offer_pointer_choice`).
    pub(super) pointer_choice: bool,
}
