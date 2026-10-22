//! `helios-gui [workspace] [--vault FILE] [--image FILE]`: opens a
//! workspace (a directory, the current one by default) in a window; with a
//! vault, its images fill the library, and what the user rates or labels
//! there is written back; with an image, the viewport shows it through an
//! exposure node whose parameters are values (`node/exposure/*`), applied
//! through a mask whose shapes the viewport draws (`node/mask/*`); with
//! both, the image is imported into the vault, opens with its saved edit,
//! and each edit step is saved there as a version.
//! `--snapshot <target> <file.png>` draws it offscreen instead and saves a
//! picture of the target. `--mask-display off|overlay|grey` starts with
//! the mask display in that mode (for pictures and benches).
//! `--node NAME=INSPECT[,CONTROL]` (repeatable) follows a running node's
//! jobs in the jobs centre, Cancel going to its control endpoint; a
//! `tcp://` address takes the node's session token from `--token-file
//! FILE` or `HELIOS_TOKEN` (see `helios_gui::relay`).
//!
//! Helios runs once per user: a later launch hands its files, workspaces
//! and `helios://` links to the running one and exits; `--new-instance`
//! opens a window of its own (see `helios_gui::platform`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui_pop::core::focus_mode::SystemFocus;
use egui_pop::model::layout::Layout;
use egui_pop::platform::instance::{self, Instance, Launch, Single};
use helios_core::Message;
use helios_gui::contexts::{self, Opened};
use helios_gui::platform::{self, Open};
use helios_gui::{Shell, headless, helios_panels};
use helios_gui::{quit, start};

/// Where a workspace's own files are kept (the profile it uses): in the
/// user's configuration, not in the workspace. Its layout is egui-pop's
/// document (`workspaces/<key>/layout` under the interface's settings).
fn workspace_dir(workspace: &Path) -> PathBuf {
    helios_gui::shell::config_dir()
        .join("workspaces")
        .join(helios_gui::host::workspace_key(workspace))
}

/// A layout an earlier Helios saved beside the workspace's profile, read
/// once when egui-pop has none for the workspace yet.
fn earlier_layout(workspace: &Path, ui_settings: &Path) -> Option<Layout> {
    let key = helios_gui::host::workspace_key(workspace);
    let current = ui_settings.join(format!("workspaces/{key}/layout.json"));
    if current.exists() {
        return None;
    }
    let text = std::fs::read_to_string(workspace_dir(workspace).join("layout.json")).ok()?;
    Layout::from_json(&text).ok()
}

type Persist = helios_gui::persist::Persist<helios_vault_sqlite::SqliteStore>;

/// A problem worth knowing: on the terminal, and in the log file once the
/// window's logs are set up.
fn warn(text: &str) {
    eprintln!("helios-gui: {text}");
    log::warn!(target: "helios", "{text}");
}

/// The library being read on a thread of its own as the window opens.
type LibraryReading = std::sync::mpsc::Receiver<Result<helios_gui::persist::LibraryRows, String>>;

/// Starts reading the library from `vault` on a thread of its own.
fn read_library(
    vault: Arc<helios_vault::Vault<helios_vault_sqlite::SqliteStore>>,
) -> LibraryReading {
    let (tx, reading) = std::sync::mpsc::channel();
    let sender = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("helios-library-read".into())
        .spawn(move || {
            let rows = helios_gui::persist::read_library(&vault);
            if let Ok(rows) = &rows {
                start::mark(&format!("library read ({} images, its thread)", rows.len()));
            }
            let _ = sender.send(rows);
        });
    if let Err(e) = spawned {
        let _ = tx.send(Err(format!("no thread to read the library: {e}")));
    }
    reading
}

/// Fills the library from the vault, and reads the thumbnails behind it,
/// one image at a time, from the first copy still present.
fn fill_library(shell: &mut Shell, persist: &Persist) -> Result<(), String> {
    let rows = helios_gui::persist::read_library(persist.vault())?;
    show_library(shell, persist, rows)
}

/// Shows the library read (on a thread of its own at the start), and reads
/// the thumbnails behind it.
fn show_library(
    shell: &mut Shell,
    persist: &Persist,
    rows: helios_gui::persist::LibraryRows,
) -> Result<(), String> {
    let files = persist.show_library(shell, rows)?;
    // Quick open opens them from there.
    shell.set_image_files(files.iter().cloned());
    let sink = shell.library_thumbnails();
    std::thread::spawn(move || {
        for (id, path) in files {
            let image = match helios_load::Kind::of(&path) {
                helios_load::Kind::Picture => helios_load::load(&path).map(|l| Some(l.image)),
                helios_load::Kind::Raw => helios_load::preview(&path),
            };
            match image {
                Ok(Some(image)) => {
                    // Scaled here: the window only converts a small one.
                    let small = helios_gui::library::small(&image);
                    if sink.send((id, small)).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(e) => warn(&format!("no thumbnail for {}: {e}", path.display())),
            }
        }
    });
    Ok(())
}

/// Who ran a command, as the vault records it.
fn who(origin: &helios_shell::commands::Origin) -> String {
    use helios_shell::commands::Origin;
    match origin {
        Origin::User => "user".into(),
        Origin::Agent(id) => format!("agent:{id}"),
        Origin::Script(id) => format!("script:{id}"),
        Origin::Midi => "midi".into(),
        Origin::Remote(id) => format!("remote:{id}"),
    }
}

/// The time in seconds since the epoch, as the vault's link counts it.
fn seconds_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The library operations as commands (see `helios_library`): an import
/// runs as a job on the shell's jobs (`job.*`, in the jobs centre, where
/// it is cancelled), the library filled again when it ends, its
/// duplicates listed in the Duplicates panel; stack suggestions are
/// recorded, and applied once accepted (the Stacks panel answers them).
fn library_commands(
    shell: &mut Shell,
    persist: &Persist,
    refill: &Arc<AtomicBool>,
    ctx: &egui::Context,
) -> Result<(), String> {
    use helios_library::import::{Duplicates, Options, Placement};
    use helios_library::stacks;
    use helios_shell::commands::CommandSpec;
    use serde_json::{Value, json};

    let text = |args: &Value, key: &str| args.get(key).and_then(Value::as_str).map(String::from);
    let (vault, jobs) = (persist.vault().clone(), shell.jobs());
    let (done, ctx) = (refill.clone(), ctx.clone());
    // Each file's line goes to the Duplicates panel with the import's
    // arguments, so a duplicate kept is imported again the same way.
    let lines = shell.duplicates_sink();
    shell.register(
        CommandSpec::new(
            "library.import",
            "Import a folder or a card (`path`; `copy`, `into`, `rename`, `duplicates`)",
            "Library",
        ),
        move |args, _| {
            let path = text(args, "path").ok_or("`path` is missing")?;
            let mut options = Options::default();
            if args.get("copy").and_then(Value::as_bool).unwrap_or(false)
                || args.get("into").is_some()
            {
                options.placement = Placement::Copy {
                    into: text(args, "into").map(Into::into),
                };
            }
            if let Some(pattern) = text(args, "rename") {
                options.rename = Some(helios_library::rename::Pattern::parse(&pattern)?);
            }
            if let Some(policy) = text(args, "duplicates") {
                options.duplicates = Duplicates::parse(&policy)?;
            }
            let (vault, jobs, done, ctx) = (vault.clone(), jobs.clone(), done.clone(), ctx.clone());
            let (lines, asked) = (lines.clone(), args.clone());
            std::thread::spawn(move || {
                let sources = [std::path::PathBuf::from(path)];
                // The job's events say how it went; the lines feed the
                // Duplicates panel.
                let ended = helios_library::import::import(
                    vault.as_ref(),
                    &jobs,
                    &sources,
                    &options,
                    seconds(),
                    &mut |line: &Value| {
                        let _ = lines.send((asked.clone(), line.clone()));
                    },
                );
                if let Err(e) = ended {
                    warn(&format!("import: {e}"));
                }
                done.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            });
            Ok(json!({ "queued": true }))
        },
    )?;
    let (vault, bus) = (persist.vault().clone(), persist.bus().clone());
    shell.register(
        CommandSpec::new(
            "library.stacks.suggest",
            "Suggest stacks (`images`: ids, every image when absent; `resolver`)",
            "Library",
        ),
        move |args, _| {
            let images = args
                .get("images")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .map(|i| helios_vault::ImageId::from_hex(i.as_str().unwrap_or("")))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            let resolver = text(args, "resolver").unwrap_or_else(|| stacks::RULES.into());
            let resolvers = stacks::Resolvers::default();
            stacks::suggest(
                vault.as_ref(),
                Some(bus.as_ref()),
                &resolvers,
                &resolver,
                &images,
                seconds(),
            )
            .map(|d| stacks::to_json(&d))
        },
    )?;
    let (vault, bus) = (persist.vault().clone(), persist.bus().clone());
    shell.register(
        CommandSpec::new(
            "library.stacks.accept",
            "Accept a stack suggestion (`decision`; `groups`: the ones taken, all when absent)",
            "Library",
        ),
        move |args, origin| {
            let id = text(args, "decision").ok_or("`decision` is missing")?;
            let groups: Option<Vec<usize>> =
                args.get("groups").and_then(Value::as_array).map(|g| {
                    g.iter()
                        .filter_map(Value::as_u64)
                        .map(|n| n as usize)
                        .collect()
                });
            let who = who(origin);
            stacks::accept(
                vault.as_ref(),
                Some(bus.as_ref()),
                &id,
                groups.as_deref(),
                &who,
                seconds(),
            )
            .map(|d| stacks::to_json(&d))
        },
    )?;
    let (vault, bus) = (persist.vault().clone(), persist.bus().clone());
    shell.register(
        CommandSpec::new(
            "library.stacks.decline",
            "Decline a stack suggestion (`decision`)",
            "Library",
        ),
        move |args, origin| {
            let id = text(args, "decision").ok_or("`decision` is missing")?;
            let who = who(origin);
            stacks::decline(vault.as_ref(), Some(bus.as_ref()), &id, &who, seconds())
                .map(|d| stacks::to_json(&d))
        },
    )?;
    let vault = persist.vault().clone();
    shell.register(
        CommandSpec::new("library.stacks.list", "List the stacks", "Library"),
        move |_, _| {
            vault
                .stacks()
                .map(|s| serde_json::to_value(s).expect("stacks serialize"))
        },
    )?;
    let vault = persist.vault().clone();
    shell.register(
        CommandSpec::new(
            "library.stacks.dissolve",
            "Dissolve a stack, keeping its images (`stack`)",
            "Library",
        ),
        move |args, _| {
            let id = text(args, "stack").ok_or("`stack` is missing")?;
            vault
                .dissolve_stack(&id)
                .map(|s| serde_json::to_value(s).expect("stacks serialize"))
        },
    )?;
    Ok(())
}

/// A picture (or a raw file's embedded preview) through the
/// workspace's network (its template's, as edited; an exposure through a
/// mask for a workspace not made from one): the network and the node the
/// viewport shows.
fn image_graph(
    path: &Path,
    bundle: &helios_template::Bundle,
) -> Result<(helios_core::Graph, helios_core::NodeId), String> {
    let image = match helios_load::Kind::of(path) {
        helios_load::Kind::Picture => helios_load::load(path)?.image,
        helios_load::Kind::Raw => helios_load::preview(path)?
            .ok_or_else(|| format!("{}: no preview to show", path.display()))?,
    };
    let built = bundle.build_with(&helios_gui::templates::modules(), &image, None)?;
    Ok((built.graph, built.view.0))
}

/// What the viewport shows, to open again when the workspace's network
/// changes.
enum Shown {
    /// An image of the library, by id.
    Library(String),
    /// A file opened on its own.
    File(PathBuf),
}

/// An image made ready on a thread of its own: decoded through the
/// workspace's network and, with a vault, imported there.
struct Prepared {
    path: PathBuf,
    image: Option<helios_vault::ImageId>,
    graph: helios_core::Graph,
    node: helios_core::NodeId,
}

/// An image being made ready off the window's thread: the window opens it
/// in the first frame after it is (decoding a raw's preview takes long
/// enough to hold the window up).
struct Preparing {
    path: PathBuf,
    ready: std::sync::mpsc::Receiver<Result<Prepared, String>>,
}

/// Starts making `path` ready: decoded through `bundle`'s network, and
/// imported into `vault` when there is one.
fn prepare(
    path: PathBuf,
    bundle: helios_template::Bundle,
    vault: Option<Arc<helios_vault::Vault<helios_vault_sqlite::SqliteStore>>>,
) -> Preparing {
    let (tx, ready) = std::sync::mpsc::channel();
    let file = path.clone();
    let sender = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("helios-image-open".into())
        .spawn(move || {
            let prepared = image_graph(&file, &bundle).and_then(|(graph, node)| {
                let image = vault.map(|v| v.import_file(&file, seconds())).transpose()?;
                Ok(Prepared {
                    path: file,
                    image,
                    graph,
                    node,
                })
            });
            let _ = sender.send(prepared);
        });
    if let Err(e) = spawned {
        let _ = tx.send(Err(format!("no thread to open the image: {e}")));
    }
    Preparing { path, ready }
}

/// A file's name, as notices show it.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    )
}

/// What the status bar says while an image is made ready.
fn opening_notice(path: &Path) -> Message {
    Message::new("gui-image-opening", "Opening { $name }…").with("name", file_name(path))
}

/// What the status bar says when the image a workspace was left on
/// cannot be opened again.
fn not_reopened(error: &str) -> Message {
    Message::new(
        "gui-image-not-reopened",
        "The image left open could not be opened again: { $error }",
    )
    .with("error", error)
}

/// What the status bar says once it is open.
fn opened_notice(path: &Path) -> Message {
    Message::new("gui-image-opened", "{ $name } is open").with("name", file_name(path))
}

/// Shows an image made ready: opened with its saved edit (and saved
/// there) when it is in the vault, else on its own.
fn show_prepared(
    shell: &mut Shell,
    persist: Option<&mut Persist>,
    prepared: Prepared,
) -> Result<(), String> {
    let Prepared {
        image, graph, node, ..
    } = prepared;
    match (persist, image) {
        (Some(persist), Some(image)) => persist
            .open_image(shell, image, graph, node, "output")
            .map(|_| ()),
        _ => shell.set_view_source(graph, node, "output"),
    }
}

struct App {
    // Fields drop in this order; the marks time the quit's steps.
    persist: Option<Persist>,
    _persist_dropped: quit::MarkOnDrop,
    shell: Shell,
    _shell_dropped: quit::MarkOnDrop,
    /// What the viewport shows.
    shown: Option<Shown>,
    /// Set when an import ended: the library is filled again.
    refill: Arc<AtomicBool>,
    /// The start's last marks: the first frame, the first input answered.
    frames: start::Frames,
    /// The library being read, shown once it is.
    library_reading: Option<LibraryReading>,
    /// Images being made ready, opened in the viewport as each is.
    preparing: Vec<Preparing>,
}

impl Drop for App {
    fn drop(&mut self) {
        quit::mark("application dropping");
    }
}

impl App {
    /// Opens an image the library activated (Enter, a double click) in
    /// the viewport, from its first copy still present.
    fn open_from_library(&mut self, id: &str) -> Result<(), String> {
        let bundle = self.bundle();
        let persist = self.persist.as_mut().ok_or("no vault open")?;
        let image = helios_vault::ImageId::from_hex(id)?;
        let path = persist
            .vault()
            .locations(&image)?
            .into_iter()
            .find(|l| l.present)
            .map(|l| l.path)
            .ok_or_else(|| format!("no copy of image {id} is present"))?;
        let (graph, node) = image_graph(&path, &bundle)?;
        persist.open_image(&mut self.shell, image, graph, node, "output")?;
        self.shown = Some(Shown::Library(id.to_string()));
        contexts::open(
            self.shell.app().handle(),
            &Opened::Image(id.to_string()),
            &egui_pop::model::commands::Origin::User,
        );
        Ok(())
    }

    /// The darkroom the workspace was left in, its image opened again;
    /// left, with a word, when the image cannot be.
    fn restore_darkroom(&mut self) {
        let Some(opened) = contexts::darkroom(self.shell.app().handle()) else {
            return;
        };
        let opening = match &opened {
            Opened::Image(id) => self.open_from_library(id),
            Opened::File(path) => self.open(Open::Image(path.clone())),
        };
        if let Err(e) = opening {
            contexts::leave_darkroom(
                self.shell.app().handle(),
                &egui_pop::model::commands::Origin::User,
            );
            self.shell
                .notify(helios_gui::dialogs::Level::Warning, not_reopened(&e));
        }
    }

    /// The workspace's network and the rest of its state.
    fn bundle(&self) -> helios_template::Bundle {
        self.shell
            .workspace_bundle()
            .unwrap_or_else(helios_template::workspace::blank)
    }

    /// Opens what the viewport shows again, through the workspace's
    /// network as it is now.
    fn reopen(&mut self) -> Result<(), String> {
        match self.shown.take() {
            Some(Shown::Library(id)) => self.open_from_library(&id),
            Some(Shown::File(path)) => self.open(Open::Image(path)),
            None => Ok(()),
        }
    }

    /// Shows the library once it is read; whether it is still being read.
    fn show_library_read(&mut self) -> bool {
        use std::sync::mpsc::TryRecvError;
        let Some(reading) = &self.library_reading else {
            return false;
        };
        let read = match reading.try_recv() {
            Ok(read) => read,
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => {
                Err("the library's reading stopped (a panic)".to_string())
            }
        };
        self.library_reading = None;
        let shown = read.and_then(|rows| {
            let count = rows.len();
            match &self.persist {
                Some(persist) => show_library(&mut self.shell, persist, rows).map(|()| count),
                None => Ok(count),
            }
        });
        match shown {
            Ok(count) => {
                start::mark("library shown");
                // The status bar said it was being read.
                self.shell.notify(
                    helios_gui::dialogs::Level::Info,
                    Message::new("gui-library-read", "{ $count } images in the library")
                        .with("count", count),
                );
            }
            Err(e) => self.shell.notify(
                helios_gui::dialogs::Level::Error,
                format!("cannot fill the library: {e}"),
            ),
        }
        false
    }

    /// Opens the images made ready since the last frame; whether some are
    /// still being made ready.
    fn open_prepared(&mut self) -> bool {
        use std::sync::mpsc::TryRecvError;
        let mut i = 0;
        while i < self.preparing.len() {
            let done = match self.preparing[i].ready.try_recv() {
                Ok(Ok(prepared)) => {
                    let path = prepared.path.clone();
                    show_prepared(&mut self.shell, self.persist.as_mut(), prepared)
                        .map_err(|e| format!("{}: {e}", path.display()))
                }
                Ok(Err(e)) => Err(e),
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(TryRecvError::Disconnected) => Err(format!(
                    "{}: its opening stopped (a panic)",
                    self.preparing[i].path.display()
                )),
            };
            let path = self.preparing.remove(i).path;
            match done {
                Ok(()) => {
                    start::mark("image opened");
                    // The status bar said it was being opened.
                    self.shell
                        .notify(helios_gui::dialogs::Level::Info, opened_notice(&path));
                    // The darkroom on it, under the place the user was in.
                    contexts::open(
                        self.shell.app().handle(),
                        &Opened::File(path.clone()),
                        &egui_pop::model::commands::Origin::User,
                    );
                    self.shown = Some(Shown::File(path));
                }
                Err(e) => {
                    // The darkroom restored on it is left: back where the
                    // user came from.
                    let handle = self.shell.app().handle();
                    if contexts::darkroom(handle) == Some(Opened::File(path)) {
                        contexts::leave_darkroom(handle, &egui_pop::model::commands::Origin::User);
                        self.shell
                            .notify(helios_gui::dialogs::Level::Warning, not_reopened(&e));
                    } else {
                        self.shell.notify(helios_gui::dialogs::Level::Error, e);
                    }
                }
            }
        }
        !self.preparing.is_empty()
    }

    /// Opens what a later launch or a link asked for: an image in the
    /// viewport, a workspace in a window of its own (none for the one
    /// this window shows).
    fn open(&mut self, open: Open) -> Result<(), String> {
        match open {
            Open::Image(path) => {
                // Made ready off the window's thread, opened once it is.
                self.shell
                    .notify(helios_gui::dialogs::Level::Info, opening_notice(&path));
                let vault = self.persist.as_ref().map(|p| p.vault().clone());
                self.preparing.push(prepare(path, self.bundle(), vault));
                Ok(())
            }
            Open::Workspace(dir) => {
                let dir = dir.canonicalize().unwrap_or(dir);
                if self.shell.workspace() == Some(helios_gui::host::workspace_key(&dir)) {
                    Ok(())
                } else {
                    self.shell
                        .run(
                            "workspace.open",
                            serde_json::json!({ "path": dir }),
                            helios_shell::commands::Origin::User,
                        )
                        .map(|_| ())
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            // The window closes after this frame: nothing to draw (work
            // waiting for a frame, thumbnails or tiles, would hold the
            // window up), only what the user changed to write.
            quit::requested();
            if let Some(persist) = &mut self.persist {
                persist.frame(&mut self.shell, seconds_f64());
                quit::mark("vault written (closing frame)");
            }
            return;
        }
        start::mark_in_first_frame("egui's pass begun (fonts ready)");
        self.shell.ui(ui);
        // The GPU path is installed by now: the image's own surface
        // follows the Wide-gamut preview setting.
        let frame: &eframe::Frame = frame;
        if let Some(rs) = frame.wgpu_render_state() {
            self.shell.follow_image_surface(ui.ctx(), rs, frame);
        }
        for open in self.shell.take_opens() {
            if let Err(e) = self.open(open) {
                self.shell.notify(helios_gui::dialogs::Level::Error, e);
            }
        }
        // The workspace's network changed (a reset, an import, an undo):
        // the image goes through it again.
        if self.shell.take_network_changed()
            && let Err(e) = self.reopen()
        {
            self.shell.notify(helios_gui::dialogs::Level::Error, e);
        }
        let reading = self.show_library_read();
        if self.open_prepared() || reading {
            // Back soon for what is still being read or made ready.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(30));
        }
        // Imports from the workspace's source, and duplicates the user
        // chose to keep, imported again.
        let mut imports = self.shell.take_imports();
        imports.extend(self.shell.duplicates_mut().take_imports());
        for args in imports {
            let _ = self
                .shell
                .run("library.import", args, helios_shell::commands::Origin::User);
        }
        let activated = self.shell.library_mut().take_activated();
        if let Some(id) = activated
            && let Err(e) = self.open_from_library(&id)
        {
            self.shell.notify(helios_gui::dialogs::Level::Error, e);
        }
        if self.refill.swap(false, Ordering::Relaxed)
            && let Some(persist) = &self.persist
        {
            if let Err(e) = fill_library(&mut self.shell, persist) {
                warn(&format!("cannot fill the library: {e}"));
            }
            // The workspace proposes stacks after each import, to review.
            if self.shell.suggests_stacks() {
                self.shell.stack_review_mut().suggest(Vec::new());
            }
        }
        if let Some(persist) = &mut self.persist {
            persist.frame(&mut self.shell, seconds_f64());
            // While a gesture goes on, come back to save it in time.
            if self.shell.values.lock().is_ok_and(|v| v.uncommitted()) {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(
                        helios_gui::persist::FLUSH_SECONDS,
                    ));
            }
        }
        if !self.frames.done() {
            let ctx = ui.ctx().clone();
            let saw_input = ctx.input(start::saw_input);
            self.frames.end(saw_input, || ctx.request_repaint());
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, _raw_input: &mut egui::RawInput) {
        self.frames.begin();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        helios_gui::display::CLEAR_COLOR.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        quit::mark("on_exit");
        // egui-pop's exit: the workspace saved (`save_workspace`), then
        // the `app.exit` step saves the interface's settings and keys.
        if let Err(e) = self.shell.exit() {
            warn(&format!("cannot save the interface's settings: {e}"));
        }
        quit::mark("interface settings, keys and layout saved");
    }
}

/// Saves the profile the workspace uses as the application closes
/// (egui-pop's `on_exit`), keeping what else the file says (the template
/// it was made from); egui-pop's `app.exit` step saves its layout.
fn save_workspace(dir: &Path, app: &egui_pop::api::AppHandle) {
    let profile = app.context().get("heliosProfile").to_string();
    let file = dir.join("workspace.json");
    let mut pointer = helios_gui::templates::read_json(&file);
    pointer["profile"] = serde_json::json!(profile);
    let saved =
        std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&file, pointer.to_string()));
    if let Err(e) = saved {
        warn(&format!("cannot save {}: {e}", file.display()));
    }
    quit::mark("workspace profile saved");
}

/// How long the window waits for the window manager's focus mode: past
/// it, the window opens as click to focus.
const FOCUS_MODE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(250);

/// The window manager's focus mode, being read on a thread of its own (a
/// child process on Linux: `gsettings`, `kreadconfig`) from the start on.
struct FocusReading {
    started: std::time::Instant,
    read: std::sync::mpsc::Receiver<SystemFocus>,
}

fn read_focus_mode() -> FocusReading {
    let (tx, read) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("helios-focus-mode".into())
        .spawn(move || {
            let _ = tx.send(egui_pop::core::focus_mode::read_system());
        });
    FocusReading {
        started: std::time::Instant::now(),
        read,
    }
}

/// The focus mode read, waited for until [`FOCUS_MODE_DEADLINE`] after
/// the reading started; click to focus past it (said in the log).
fn focus_mode(reading: &FocusReading) -> SystemFocus {
    let left = FOCUS_MODE_DEADLINE.saturating_sub(reading.started.elapsed());
    reading.read.recv_timeout(left).unwrap_or_else(|_| {
        warn("the window manager's focus mode was not read in time: click to focus");
        SystemFocus::default()
    })
}

/// The nodes `--node NAME=INSPECT[,CONTROL]` names, their `tcp://`
/// addresses with the token of `--token-file FILE`, else `HELIOS_TOKEN`.
fn node_links(args: &[String]) -> Result<Vec<helios_gui::relay::NodeLink>, String> {
    let value = |flag: &str| -> Result<Vec<&String>, String> {
        let mut values = Vec::new();
        for (i, a) in args.iter().enumerate() {
            if a == flag {
                values.push(
                    args.get(i + 1)
                        .ok_or_else(|| format!("{flag} takes a value"))?,
                );
            }
        }
        Ok(values)
    };
    let token = match value("--token-file")?.last() {
        Some(file) => Some(
            std::fs::read_to_string(file.as_str())
                .map_err(|e| format!("--token-file {file}: {e}"))?
                .trim()
                .to_string(),
        ),
        None => std::env::var("HELIOS_TOKEN").ok(),
    };
    value("--node")?
        .into_iter()
        .map(|link| helios_gui::relay::NodeLink::parse(link, token.as_deref()))
        .collect()
}

fn main() -> Result<(), String> {
    start::begin();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, target, out] = args.as_slice()
        && flag == "--snapshot"
    {
        let shell = Shell::new(helios_panels(), None);
        let mut harness = headless::harness(shell, egui::vec2(1280.0, 800.0));
        let image = headless::snapshot(&mut harness, target)?;
        return helios_gui::snapshot::save(&image, Path::new(out));
    }
    // One Helios per user: a later launch hands over and exits.
    let instance = if args.iter().any(|a| a == platform::NEW_INSTANCE) {
        Instance::detached(Launch::current())
    } else {
        match instance::single(platform::APP_ID) {
            Ok(Single::Handed) => {
                eprintln!("helios-gui: handed over to the running Helios");
                return Ok(());
            }
            Ok(Single::First(first)) => first,
            Err(e) => {
                eprintln!("helios-gui: no single instance ({e}), running on its own");
                Instance::detached(Launch::current())
            }
        }
    };
    start::mark("instance check");
    // The window draws with the process's one device, which the engine
    // computes on (see `helios_gui::shell::wgpu_setup`): opened on a
    // thread while the settings, the vault and the shell are read, as
    // nothing else uses the GPU before the window.
    let gpu_opening = std::thread::Builder::new()
        .name("helios-gpu-open".into())
        .spawn(|| {
            let gpu = helios_gpu::shared::gpu();
            start::mark("GPU device opened (its thread)");
            gpu
        })
        .map_err(|e| format!("no thread to open the GPU: {e}"))?;
    let focus_reading = read_focus_mode();
    if args.last().is_some_and(|a| a == "--image") {
        return Err("--image takes a FILE".into());
    }
    // The mask display from the start, for pictures and benches.
    let mask_display = helios_gui::mask_display::flag(&args)?;
    let cwd = std::env::current_dir().ok();
    let asked = platform::opens(&args, cwd.as_deref());
    // The last image named, the first workspace (else the current
    // directory).
    let image_file = asked.iter().rev().find_map(|o| match o {
        Open::Image(p) => Some(p.clone()),
        Open::Workspace(_) => None,
    });
    let nodes = node_links(&args)?;
    let vault_file = args
        .iter()
        .position(|a| a == "--vault")
        .map(|i| {
            args.get(i + 1)
                .map(PathBuf::from)
                .ok_or("--vault takes a FILE")
        })
        .transpose()?;
    let workspace = asked
        .iter()
        .find_map(|o| match o {
            Open::Workspace(p) => Some(p.clone()),
            Open::Image(_) => None,
        })
        .or(cwd)
        .and_then(|p| p.canonicalize().ok())
        .ok_or("no workspace directory")?;
    if let Err(e) = helios_gui::shell::remember_workspace(&workspace) {
        eprintln!("helios-gui: cannot remember the workspace: {e}");
    }
    let dir = workspace_dir(&workspace);
    // The interface's settings (window place, focus mode, the user's
    // keys) are the user's, beside Helios's other settings.
    let ui_settings = helios_gui::shell::config_dir().join("ui");
    let name = workspace.file_name().map_or_else(
        || workspace.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    // The workspace's profile, else the Default one; the workspace's
    // saved layout wins over the profile's.
    let profile = std::fs::read_to_string(dir.join("workspace.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["profile"].as_str().map(String::from))
        .unwrap_or_else(|| "default".into());
    start::mark("workspace and settings read");
    let bus = std::sync::Arc::new(helios_core::Bus::new());
    let persist = match vault_file {
        Some(path) => Some(Persist::new(
            helios_vault_sqlite::SqliteStore::open(&path)?,
            bus,
        )),
        None => None,
    };
    start::mark("vault opened (and migrated)");
    // Held here, the vault outlives the window: it closes once the
    // window is gone (see `helios_gui::quit`).
    let vault = persist.as_ref().map(|p| p.vault().clone());
    // The library is read while the window opens.
    let library_reading = vault.clone().map(read_library);
    // The workspace as it stands: its template's network, as edited.
    let config = helios_gui::shell::config_dir();
    let bundle = match helios_template::workspace::Files::new(&config, &workspace).load() {
        Ok(state) => state.bundle,
        Err(e) => {
            warn(&format!("the workspace: {e}"));
            helios_template::workspace::blank()
        }
    };
    start::mark("workspace read");
    // The image asked for is made ready while the window opens.
    let preparing = image_file.map(|path| prepare(path, bundle, vault.clone()));
    let earlier = earlier_layout(&workspace, &ui_settings);
    let mut builder = Shell::builder(helios_panels(), Some(ui_settings))
        .workspace(&workspace)
        .on_exit(move |app| save_workspace(&dir, app))
        .desktop(instance)
        .system_focus(focus_mode(&focus_reading));
    // A darkroom left on an image the vault no longer has is not
    // restored.
    if let Some(vault) = vault.clone() {
        builder = builder.images(Arc::new(move |id: &str| {
            helios_vault::ImageId::from_hex(id)
                .ok()
                .and_then(|image| vault.locations(&image).ok())
                .is_some_and(|l| l.iter().any(|l| l.present))
        }));
    }
    start::mark("shell declared (commands, themes, translations)");
    let gpu = gpu_opening
        .join()
        .map_err(|_| "the GPU's opening stopped (a panic)".to_string())?
        .map_err(|e| format!("no GPU to draw with: {e}"))?;
    start::mark("GPU device ready");
    let title = format!("{name} — Helios");
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title.clone())
            .with_app_id("helios-gui")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0])
            // egui-pop's frame is Helios's by default (its `window.frame`
            // setting gives the system's back, and the window follows).
            .with_decorations(false)
            // The image's own surface on Wayland shows through the
            // interface; the interface itself stays opaque (`clear_color`).
            .with_transparent(helios_gui::display::TRANSPARENT_WINDOW),
        ..Default::default()
    };
    options.wgpu_options.wgpu_setup = helios_gui::shell::wgpu_setup(&gpu);
    options.window_builder = Some(Box::new(|viewport| {
        start::mark("window being created");
        viewport
    }));
    // egui-pop opens the window where it was last (repaired) and builds
    // the application on its context.
    let ran = builder
        .run_native("helios-gui", options, move |mut shell, cc| {
            start::mark("window open, application built (plugins, fonts)");
            if let Some(rs) = cc.wgpu_render_state.as_ref() {
                shell.attach_gpu(rs);
            }
            start::mark("GPU attached to the viewport");
            shell.set_title(&name);
            // The first run asks "Coming from darktable?" (once).
            shell.offer_pointer_choice();
            if let Err(e) = shell.use_profile(&profile) {
                shell.notes.push(format!("profile {profile}: {e}"));
            }
            start::mark("profile applied");
            // The workspace's layout wins over the profile's: egui-pop's
            // document is loaded again (or an earlier Helios's, once).
            if let Some(saved) = earlier {
                shell.set_layout(saved);
            } else if let Err(e) = shell.load_layout() {
                shell.notes.push(format!("layout: {e}"));
            }
            start::mark("workspace layout restored");
            for link in nodes {
                if let Err(e) = shell.follow_node(link) {
                    shell.notes.push(e);
                }
            }
            start::mark("node relays started");
            // The workspace's state, edited from here: its keys in the
            // workspace's keymap layer, its library settings.
            if let Err(e) = shell.open_workspace(&config, &workspace) {
                shell.notes.push(format!("workspace: {e}"));
            }
            start::mark("workspace opened (keys, library settings)");
            if let Some(mode) = mask_display
                && let Err(e) = shell.run(
                    helios_gui::mask_display::COMMAND,
                    serde_json::json!({ "mode": mode.name() }),
                    helios_shell::commands::Origin::User,
                )
            {
                shell
                    .notes
                    .push(format!("{}: {e}", helios_gui::mask_display::FLAG));
            }
            if library_reading.is_some() {
                shell.notify(
                    helios_gui::dialogs::Level::Info,
                    Message::new("gui-library-reading", "Reading the library…"),
                );
            }
            if let Some(p) = &preparing {
                shell.notify(helios_gui::dialogs::Level::Info, opening_notice(&p.path));
            }
            let refill = Arc::new(AtomicBool::new(false));
            if let Some(p) = &persist
                && let Err(e) = library_commands(&mut shell, p, &refill, &cc.egui_ctx)
            {
                shell.notes.push(format!("library commands: {e}"));
            }
            let mut app = App {
                persist,
                _persist_dropped: quit::MarkOnDrop("vault link dropped"),
                shell,
                _shell_dropped: quit::MarkOnDrop("shell dropped"),
                shown: None,
                refill,
                frames: start::Frames::default(),
                library_reading,
                preparing: preparing.into_iter().collect(),
            };
            // Back where the workspace was left: the image named on the
            // command line (being made ready, the darkroom on it once it
            // is) wins over the darkroom restored.
            if app.preparing.is_empty() {
                app.restore_darkroom();
            }
            start::mark("application ready");
            Box::new(app)
        })
        .map_err(|e| e.to_string());
    quit::mark("window closed (eframe returned)");
    // The window is gone: what is left runs against its deadline.
    let mut finisher = quit::Finisher::default();
    if let Some(vault) = vault {
        finisher.spawn("vault closed", quit::VAULT_DEADLINE, move || drop(vault));
    }
    for left in finisher.finish() {
        warn(&format!("quit: {left} was left unfinished"));
    }
    ran
}
