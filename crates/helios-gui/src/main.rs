//! `helios-gui [workspace] [--vault FILE] [--image FILE]`: opens a
//! workspace (a directory, the current one by default) in a window; the
//! vault (`--vault FILE`, else the default library in Helios's data
//! directory, see `platform::library_file`) fills the library with its
//! images, and what the user rates or labels
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

// A window program on Windows, in release: started from Explorer or the
// Start menu, no console opens beside the window. Started from a terminal,
// it writes to that terminal's console (`attach_parent_console`).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui_pop::core::focus_mode::SystemFocus;
use egui_pop::model::layout::Layout;
use egui_pop::platform::instance::{self, Instance, Launch, Single};
use helios_core::Message;
use helios_gui::contexts::{self, Opened};
use helios_gui::platform::{self, Open};
use helios_gui::shell::Closing;
use helios_gui::{Shell, headless, helios_panels, helios_panels_with};
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
type LocalNode = helios_gui::node::LocalNode<helios_vault_sqlite::SqliteStore>;
type Backups = helios_gui::node_jobs::Backups<helios_vault_sqlite::SqliteStore>;

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
            // In the default sort: a workspace sorted otherwise reads it
            // again once shown.
            let rows = helios_gui::persist::read_library(&vault, helios_gui::library::DEFAULT_SORT);
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

/// Fills the library from the vault, and reads the thumbnails behind it:
/// from the thumbnail cache, else from the first copy still present.
fn fill_library(shell: &mut Shell, persist: &Persist) -> Result<(), String> {
    let rows = helios_gui::persist::read_library(persist.vault(), shell.library().sort())?;
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
    // A new pool replaces the last (an import filled the library again):
    // the thumbnails already shown are not read again.
    let cache = THUMB_CACHE.get().cloned().flatten();
    let loader = helios_gui::thumbnails::start(shell, cache, &files, warn);
    *THUMB_LOADER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(loader);
    Ok(())
}

/// The thumbnail cache of the library opened (none without a vault, or
/// when its folder cannot be made).
static THUMB_CACHE: std::sync::OnceLock<Option<Arc<helios_thumbs::ThumbCache>>> =
    std::sync::OnceLock::new();

/// The pool filling the library's thumbnails.
static THUMB_LOADER: std::sync::Mutex<Option<helios_thumbs::Loader>> = std::sync::Mutex::new(None);

/// The time in seconds since the epoch, as the vault's link counts it.
fn seconds_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The health check's runner: Helios's checks, over the node's inputs
/// (the volumes mounted, the vault and its file, its backups), filled from
/// the desktop: the cards an import left short, still inserted, the
/// `health.destinationDays` setting read from `settings`, the GPU in use
/// and its kernels, the plugins installed, each probed, and the
/// catalogue's snapshots; the ignored findings kept beside Helios's
/// settings.
fn health_runner(
    vault: Option<Arc<helios_vault::Vault<helios_vault_sqlite::SqliteStore>>>,
    file: Option<PathBuf>,
    cards: Arc<std::sync::Mutex<Vec<helios_library::import::Card>>>,
    settings: egui_pop::core::binding::SharedStore,
    plugins: Arc<helios_plugin::Broker>,
) -> helios_health::runner::Runner {
    use helios_health::backup::CardShort;
    use helios_health::{Findings, Ignored, Registry, runner};
    let context =
        helios_node_core::health::HealthInputs::new(helios_storage::system(), vault, file)
            .with_destination_days(move || helios_gui::platform::destination_days(&settings))
            .with_cards(move || {
                cards
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .filter(|c| c.path.is_dir())
                    .map(|c| CardShort {
                        label: c.label.clone(),
                        short: c.short,
                    })
                    .collect()
            })
            .with_gpu(health_gpu)
            .with_plugins(move || health_plugins(&plugins))
            .with_snapshots(helios_gui::catalogue::health)
            .into_context();
    let ignored = Ignored::load(&helios_gui::shell::config_dir().join("health-ignored.json"));
    let mut registry = Registry::builtin();
    registry.add(Arc::new(helios_health::gpu::Kernels::new(health_kernels())));
    runner::Runner::new(
        registry,
        context,
        Findings::with_ignored(ignored),
        runner::Schedule::desktop(std::time::Instant::now()),
    )
}

/// The adapters wgpu enumerated and the one the process's device is open
/// on; `None` before the GPU was tried (never enumerates from here).
fn health_gpu() -> Option<helios_health::gpu::Gpu> {
    use helios_health::gpu::{Adapter, AdapterKind, Gpu};
    let adapters = helios_gpu::shared::enumerated()?;
    let opened = helios_gpu::shared::opened();
    let adapters: Vec<Adapter> = adapters
        .iter()
        .map(|a| {
            let info = a.get_info();
            Adapter {
                name: info.name.clone(),
                driver: format!("{} {}", info.driver, info.driver_info)
                    .trim()
                    .to_string(),
                backend: format!("{:?}", info.backend),
                kind: match info.device_type {
                    eframe::wgpu::DeviceType::DiscreteGpu => AdapterKind::Discrete,
                    eframe::wgpu::DeviceType::IntegratedGpu => AdapterKind::Integrated,
                    eframe::wgpu::DeviceType::VirtualGpu => AdapterKind::Virtual,
                    eframe::wgpu::DeviceType::Other => AdapterKind::Other,
                    eframe::wgpu::DeviceType::Cpu => AdapterKind::Software,
                },
            }
        })
        .collect();
    let used = opened.as_ref().map(|(i, _)| *i);
    Some(Gpu {
        adapters,
        used,
        why_cpu: used
            .is_none()
            .then(|| "the GPU device did not open (see the log)".to_string()),
    })
}

/// Compiles every GPU kernel of the modules Helios links on the device
/// open (none without one); run by the health check's thread.
fn health_kernels() -> impl Fn() -> Vec<helios_health::gpu::Kernel> + Send + Sync + 'static {
    let catalogue = helios_gui::templates::modules();
    let kernels: Vec<(String, helios_core::GpuKernel)> = catalogue
        .module_names()
        .filter_map(|n| Some((n.to_string(), catalogue.get_module(n)?.gpu_kernel()?)))
        .collect();
    move || {
        let Some((_, gpu)) = helios_gpu::shared::opened() else {
            return Vec::new();
        };
        kernels
            .iter()
            .map(|(module, kernel)| helios_health::gpu::Kernel {
                module: module.clone(),
                error: gpu.compile(module, *kernel).err(),
            })
            .collect()
    }
}

/// Every plugin installed, probed (started and its handshake answered,
/// then stopped), and those whose manifest was refused.
fn health_plugins(broker: &helios_plugin::Broker) -> Vec<helios_health::plugins::Plugin> {
    use helios_health::plugins::{Plugin, Status};
    use helios_plugin::Probed;
    let mut out: Vec<Plugin> = broker
        .ids()
        .filter_map(|id| {
            let m = broker.manifest(id)?;
            let (status, log) = match broker.probe(id) {
                Ok((Probed::Answers, log)) => (Status::Answers, log),
                Ok((Probed::Disabled, log)) => (Status::Disabled, log),
                Ok((Probed::NotGranted(c), log)) => (Status::NotGranted(c), log),
                Ok((Probed::Failed(e), log)) => (Status::Crashed(e), log),
                Err(e) => (Status::Crashed(e), Vec::new()),
            };
            Some(Plugin {
                id: id.to_string(),
                name: m.title().to_string(),
                version: m.version.clone(),
                dir: broker.dir(id).map(Path::to_path_buf).unwrap_or_default(),
                status,
                log,
            })
        })
        .collect();
    for r in broker.rejected() {
        let manifest = helios_plugin::Manifest::read(&r.dir).ok();
        let id = manifest.as_ref().map_or_else(
            || {
                r.dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            },
            |m| m.id.clone(),
        );
        // A copy shadowed by another of the same id is not a plugin of
        // its own.
        if broker.manifest(&id).is_some() {
            continue;
        }
        out.push(Plugin {
            name: manifest
                .as_ref()
                .map_or_else(|| id.clone(), |m| m.title().to_string()),
            version: manifest.map(|m| m.version).unwrap_or_default(),
            id,
            dir: r.dir.clone(),
            status: Status::Unsupported(r.problems.clone()),
            log: Vec::new(),
        });
    }
    out
}

/// Fills, on a thread of its own, the capture fields (time, camera, lens,
/// settings) of the photos imported before Helios kept them
/// (`helios_library::capture::backfill`): a job in the jobs centre that
/// resumes where the last one stopped, and does nothing once finished.
fn backfill_captures<S: helios_vault::Store + 'static>(
    vault: Arc<helios_vault::Vault<S>>,
    jobs: helios_core::jobs::Jobs,
) {
    std::thread::spawn(move || {
        if let Err(e) =
            helios_library::capture::backfill(&vault, &jobs, &helios_library::capture::Files, None)
        {
            warn(&format!("capture backfill: {e}"));
        }
    });
}

fn seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Set by a catalogue restore: Helios starts again once its window has
/// closed and its vault is released, the restore applied at the opening.
static RESTART: AtomicBool = AtomicBool::new(false);

/// The window's context, once it is open: a request to end from the
/// system (`helios_gui::ending`) closes it as Ctrl+Q does.
static WINDOW: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

/// The system asked Helios to end (`helios_gui::ending`): the main
/// window's close quits whatever windows are open, without a question.
static SESSION_ENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// SIGTERM, SIGINT and SIGHUP (on Windows, the console's events and the
/// session's end) close Helios as Ctrl+Q does; ended before the window
/// opens or past the deadline, the session is marked clean all the same.
fn listen_for_ending() -> Arc<helios_gui::ending::Ending> {
    use helios_gui::ending::{DEADLINE, Ending};
    let crashes = platform::crash_dir();
    let ending = Ending::new(
        Box::new(|| {
            let Some(ctx) = WINDOW.get() else {
                return false;
            };
            SESSION_ENDING.store(true, std::sync::atomic::Ordering::SeqCst);
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            ctx.request_repaint();
            true
        }),
        Box::new(move |code| {
            if let Some(dir) = &crashes {
                helios_gui::ending::mark_clean(dir);
            }
            std::process::exit(code);
        }),
        DEADLINE,
    );
    if let Err(e) = helios_gui::ending::listen(ending.clone()) {
        warn(&format!("the system's requests to end are not heard: {e}"));
    }
    ending
}

/// Catalogue snapshots (`helios_gui::catalogue`): one taken by hand
/// (copied to the backup disks on a thread of its own), the list, and a
/// restore (asked of the vault, then Helios closes and starts again).
/// Then the schedule: a thread looks every ten minutes whether the
/// newest snapshot is older than `catalogue.snapshotEvery`.
fn catalogue_commands(
    shell: &mut Shell,
    persist: &Persist,
    file: PathBuf,
    ctx: &egui::Context,
) -> Result<(), String> {
    use helios_gui::catalogue;
    use helios_shell::commands::CommandSpec;
    use serde_json::{Value, json};

    let settings = shell.app().handle().store();
    let (vault, at, store) = (persist.vault().clone(), file.clone(), settings.clone());
    shell.register(
        CommandSpec::new(
            catalogue::SNAPSHOT_COMMAND,
            "Snapshot the catalogue now: a verified copy next to it, then copied to the backup disks plugged in",
            "Library",
        ),
        move |_, _| {
            let options = helios_vault_sqlite::Options {
                keep_snapshots: catalogue::kept(&store),
                ..helios_vault_sqlite::Options::default()
            };
            let taken = helios_vault_sqlite::snapshot::take_file(&at, "by-hand", &options)?;
            let (vault, at, copy, keep) = (vault.clone(), at.clone(), taken.clone(), options.keep_snapshots);
            std::thread::spawn(move || {
                let mounted = helios_storage::system().volumes();
                let (_, errors) = catalogue::copy_to_backups(&vault, &mounted, &at, &copy, keep);
                for e in errors {
                    warn(&format!("catalogue snapshot copy: {e}"));
                }
            });
            Ok(json!({ "file": taken }))
        },
    )?;
    let (at, t) = (file.clone(), shell.texts());
    shell.register(
        CommandSpec::new(
            catalogue::LIST_COMMAND,
            "The catalogue's snapshots, next to it and on the backup disks plugged in, newest first, each with how old it is and where",
            "Library",
        )
        .read_only(),
        move |_, _| {
            let mounted = helios_storage::system().volumes();
            serde_json::to_value(catalogue::list(&at, &mounted))
                .map(|list| helios_gui::shell::snapshot_rows(&t, list))
                .map_err(|e| e.to_string())
        },
    )?;
    let (at, egui) = (file.clone(), ctx.clone());
    shell.register(
        CommandSpec::new(
            catalogue::RESTORE_COMMAND,
            "Restore the catalogue from a snapshot (`file`): Helios closes and starts again, the current catalogue kept aside",
            "Library",
        ),
        move |args, _| {
            let from = args
                .get("file")
                .and_then(Value::as_str)
                .ok_or("`file` is missing")?;
            helios_vault_sqlite::snapshot::request_restore(&at, Path::new(from))?;
            RESTART.store(true, Ordering::Relaxed);
            egui.send_viewport_cmd(egui::ViewportCommand::Close);
            Ok(json!({ "restarting": true }))
        },
    )?;
    // The schedule. The thread holds the vault only while it works, so
    // quitting releases it.
    let vault = Arc::downgrade(persist.vault());
    std::thread::Builder::new()
        .name("catalogue-snapshots".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_mins(2));
            loop {
                let Some(vault) = vault.upgrade() else { return };
                if helios_vault_sqlite::snapshot::due(&file, catalogue::every(&settings), seconds())
                {
                    let mounted = helios_storage::system().volumes();
                    if let Err(e) = catalogue::take(
                        &vault,
                        &mounted,
                        &file,
                        "daily",
                        catalogue::kept(&settings),
                    ) {
                        warn(&format!("catalogue snapshot: {e}"));
                    }
                }
                drop(vault);
                std::thread::sleep(std::time::Duration::from_mins(10));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// The library operations as commands (see `helios_library`): an import
/// runs as a job on the shell's jobs (`job.*`, in the jobs centre, where
/// it is cancelled), the library filled again when it ends, its
/// duplicates listed in the Duplicates panel, its photos' darktable edits
/// kept as versions (unless `darktable` is false), the darktable edits
/// not kept whole in the Log panel and its end said; stack suggestions are
/// recorded, and applied once accepted (the Stacks panel answers them).
fn library_commands(
    shell: &mut Shell,
    persist: &Persist,
    backups: Option<&Arc<Backups>>,
    plugins: &Arc<helios_plugin::Broker>,
    refill: &Arc<AtomicBool>,
    ctx: &egui::Context,
) -> Result<(), String> {
    use helios_library::stacks;
    use helios_shell::commands::CommandSpec;
    use serde_json::{Value, json};

    let text = |args: &Value, key: &str| args.get(key).and_then(Value::as_str).map(String::from);
    let (vault, jobs) = (persist.vault().clone(), shell.jobs());
    // The node's backups, for the strategy's "after each import".
    let after_import = backups.cloned();
    let (done, ctx) = (refill.clone(), ctx.clone());
    // Each file's line goes to the shell with the import's arguments (the
    // Duplicates panel, so a duplicate kept is imported again the same
    // way; the Log panel), then the import's end.
    let lines = shell.import_sink();
    // Each import that changed the library is one undo step of its own
    // scope; undo and redo move the vault's rows, never a file, then the
    // Library is filled again.
    let history = shell.library_history();
    {
        let (vault, done, ctx) = (vault.clone(), done.clone(), ctx.clone());
        history.set_apply(Arc::new(move |changes, forward| {
            let r = if forward {
                vault.replay(changes)
            } else {
                vault.revert(changes)
            };
            done.store(true, Ordering::Relaxed);
            ctx.request_repaint();
            r
        }));
    }
    // The source plugins (`import.source`): their accounts in the import
    // window, "Pick photos…" importing what was picked as fetched copies.
    for provider in helios_plugin::source::providers(plugins) {
        shell.add_import_connector(Arc::new(provider));
    }
    {
        let (vault, jobs, done, ctx) = (vault.clone(), jobs.clone(), done.clone(), ctx.clone());
        let history = history.clone();
        let backed = after_import.clone();
        shell.set_import_remote(Arc::new(move |connector, source, items| {
            let remote = helios_library::fetch::Remote {
                connector,
                source,
                items,
                new_only: false,
            };
            let options = helios_library::import::Options::default();
            let mut report = helios_library::fetch::import_remote(
                vault.as_ref(),
                &jobs,
                &remote,
                &options,
                seconds(),
                &mut |_| {},
            )?;
            let label = helios_gui::library_history::import_label(report.added + report.located);
            history.record(label, std::mem::take(&mut report.changes), seconds_f64());
            // The strategy's "after each import", run by the node.
            if let Some(backups) = &backed {
                backups.start(
                    helios_storage::strategy::Trigger::OnImport,
                    Some(report.clone()),
                );
            }
            done.store(true, Ordering::Relaxed);
            ctx.request_repaint();
            Ok(report.added)
        }));
    }
    let plugins = plugins.clone();
    shell.register(
        CommandSpec::new(
            helios_gui::shell::IMPORT_COMMAND,
            "Import a folder or a card (`path`, or `paths` for several files and folders; `importer`: `plugin`, `importer` and `source` to import through an importer plugin; `copy`, `into`, `rename`, `layout`, `duplicates`; `darktable`: false leaves the .xmp sidecars out)",
            "Library",
        ),
        move |args, _| {
            let options = helios_gui::importers::options(args)?;
            let through = args.get("importer").is_some().then(|| plugins.clone());
            let mut sources: Vec<std::path::PathBuf> = args
                .get("paths")
                .and_then(Value::as_array)
                .map(|p| p.iter().filter_map(Value::as_str).map(Into::into).collect())
                .unwrap_or_default();
            sources.extend(text(args, "path").map(Into::into));
            if sources.is_empty() && through.is_none() {
                return Err("`path` is missing".into());
            }
            let (vault, jobs, done, ctx) = (vault.clone(), jobs.clone(), done.clone(), ctx.clone());
            let (lines, asked, history) = (lines.clone(), args.clone(), history.clone());
            let backed = after_import.clone();
            std::thread::spawn(move || {
                // The job's events say how it went; the lines feed the
                // Duplicates panel.
                let mut emit = |line: &Value| {
                    let _ = lines.send((asked.clone(), line.clone()));
                };
                let ended = match &through {
                    // The plugin lists, Helios imports.
                    Some(plugins) => helios_gui::importers::run(
                        plugins,
                        vault.as_ref(),
                        &jobs,
                        &asked,
                        seconds(),
                        &mut emit,
                    ),
                    None => helios_library::import::import(
                        vault.as_ref(),
                        &jobs,
                        &sources,
                        &options,
                        seconds(),
                        &mut emit,
                    ),
                };
                let end = match ended {
                    Ok(mut report) => {
                        let label = helios_gui::library_history::import_label(
                            report.added + report.located,
                        );
                        let changes = std::mem::take(&mut report.changes);
                        history.record(label, changes, seconds_f64());
                        // The strategy's "after each import", run by the
                        // node.
                        if let Some(backups) = &backed {
                            backups.start(
                                helios_storage::strategy::Trigger::OnImport,
                                Some(report.clone()),
                            );
                        }
                        json!({ "ended": report })
                    }
                    Err(e) => {
                        warn(&format!("import: {e}"));
                        json!({ "failed": e })
                    }
                };
                let _ = lines.send((asked.clone(), end));
                done.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            });
            Ok(json!({ "queued": true }))
        },
    )?;
    // "Back up now": the node runs the copies.
    if let Some(backups) = backups {
        helios_gui::node_jobs::register_back_up_now(shell, backups)?;
    }
    let vault = persist.vault().clone();
    shell.register(
        CommandSpec::new(
            helios_health::checks::ACKNOWLEDGE_CHANGED,
            "Acknowledge the photos changed outside Helios: clears their mark (`images`: ids, every photo when absent); their new content stays recorded",
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
            let cleared = vault.acknowledge_changed(&images)?;
            Ok(json!({ "cleared": cleared }))
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
            let who = helios_gui::persist::who(origin);
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
            let who = helios_gui::persist::who(origin);
            stacks::decline(vault.as_ref(), Some(bus.as_ref()), &id, &who, seconds())
                .map(|d| stacks::to_json(&d))
        },
    )?;
    // The image map: forget a folder or a volume (its sightings not in
    // the Library go; listings skip it until listed again on purpose).
    let vault = persist.vault().clone();
    shell.register(
        CommandSpec::new(
            helios_gui::shell::FORGET_FOLDER,
            "Forget a folder in the image map (`path`): its files not in the Library",
            "Library",
        ),
        move |args, _| {
            let path = text(args, "path").ok_or("`path` is missing")?;
            let mounted = helios_storage::system().volumes();
            let volumes =
                helios_storage::registry::merged(vault.as_ref(), &mounted).unwrap_or(mounted);
            helios_library::map::forget_folder(&vault, &volumes, Path::new(&path), seconds())
                .map(|n| serde_json::json!({ "forgotten": n }))
        },
    )?;
    let vault = persist.vault().clone();
    shell.register(
        CommandSpec::new(
            helios_gui::shell::FORGET_VOLUME,
            "Forget a volume in the image map (`volume`, its id): its files not in the Library",
            "Library",
        ),
        move |args, _| {
            let id = text(args, "volume").ok_or("`volume` is missing")?;
            let id = helios_vault::storage::VolumeId(id);
            vault
                .forget(Some(&id), None, seconds())
                .map(|n| serde_json::json!({ "forgotten": n }))
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

/// A file through the workspace's network (its template's, as edited;
/// an exposure through a mask for a workspace not made from one): the
/// network and the node the viewport shows. A raw's sensor data when the
/// network reads a raw, else a picture or a raw's embedded preview.
fn image_graph(
    path: &Path,
    bundle: &helios_template::Bundle,
) -> Result<(helios_core::Graph, helios_core::NodeId), String> {
    helios_gui::templates::open_file(path, bundle)
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

/// What a thread making an image ready sends.
enum Ready {
    /// A raw's embedded preview, alone in its graph: the first paint
    /// while the raw is developed.
    First(helios_core::Graph, helios_core::NodeId),
    /// The image through the workspace's network, or why it is not.
    Done(Result<Prepared, String>),
}

/// An image being made ready off the window's thread: the window opens it
/// in the first frame after it is (decoding a raw's preview takes long
/// enough to hold the window up).
struct Preparing {
    path: PathBuf,
    /// The library's image, by id, when it was opened from there.
    library: Option<String>,
    ready: std::sync::mpsc::Receiver<Ready>,
}

/// A raw's embedded preview alone in a graph (its source's `image`
/// port), turned by the raw's EXIF orientation as the development will
/// be, to draw while the raw is developed; none for a picture or a raw
/// without one.
fn first_paint(path: &Path) -> Option<(helios_core::Graph, helios_core::NodeId)> {
    if helios_load::Kind::of(path) != helios_load::Kind::Raw {
        return None;
    }
    let (preview, tag) = helios_load::preview_oriented(path).ok()?;
    let preview = helios_load::upright(preview?, tag);
    let mut graph = helios_core::Graph::new();
    let node = graph.add_source("source", preview);
    Some((graph, node))
}

/// Starts making an image of the library ready, off the window's thread:
/// a raw's embedded preview first, then the image through `bundle`'s
/// network (the darkroom applies its head version's edit).
fn prepare_library(
    id: &str,
    image: helios_vault::ImageId,
    path: PathBuf,
    bundle: helios_template::Bundle,
) -> Preparing {
    let (tx, ready) = std::sync::mpsc::channel();
    let file = path.clone();
    let sender = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("helios-image-open".into())
        .spawn(move || {
            if let Some((graph, node)) = first_paint(&file) {
                let _ = sender.send(Ready::First(graph, node));
            }
            let prepared = image_graph(&file, &bundle).map(|(graph, node)| Prepared {
                path: file,
                image: Some(image),
                graph,
                node,
            });
            let _ = sender.send(Ready::Done(prepared));
        });
    if let Err(e) = spawned {
        let _ = tx.send(Ready::Done(Err(format!(
            "no thread to open the image: {e}"
        ))));
    }
    Preparing {
        path,
        library: Some(id.to_string()),
        ready,
    }
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
            let _ = sender.send(Ready::Done(prepared));
        });
    if let Err(e) = spawned {
        let _ = tx.send(Ready::Done(Err(format!(
            "no thread to open the image: {e}"
        ))));
    }
    Preparing {
        path,
        library: None,
        ready,
    }
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
    /// Starts the exports the shell asks for; the node runs them.
    exporter: Option<Arc<helios_gui::export::Exporter<helios_vault_sqlite::SqliteStore>>>,
    /// The GUI's connection to the local node, dropped as the GUI ends.
    node: Option<helios_gui::node::Client>,
}

impl Drop for App {
    fn drop(&mut self) {
        quit::mark("application dropping");
    }
}

impl App {
    /// Opens an image the library activated (Enter, a double click) in
    /// the viewport, from its first copy still present: made ready off
    /// the window's thread, a raw's embedded preview drawn until it is
    /// developed ([`App::open_prepared`]); its large thumbnail (the
    /// cache's 1024 px entry) drawn before either.
    fn open_from_library(&mut self, id: &str) -> Result<(), String> {
        let bundle = self.bundle();
        let persist = self.persist.as_mut().ok_or("no vault open")?;
        let image = helios_vault::ImageId::from_hex(id)?;
        let path = persist
            .vault()
            .locations(&image)?
            .into_iter()
            .find(helios_vault::Location::present)
            .map(|l| l.path)
            .ok_or_else(|| {
                // On a disk not plugged in: say which one.
                self.shell.library().unavailable(id).map_or_else(
                    || format!("no copy of image {id} is present"),
                    |m| self.shell.text(m),
                )
            })?;
        // A newer opening replaces one still being made ready.
        self.preparing.retain(|p| p.library.is_none());
        self.preparing
            .push(prepare_library(id, image, path, bundle));
        // Its large thumbnail drawn until its render is complete.
        self.shell.open_from_library(Some(id));
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

    /// Renders the head versions the library asked for (the Compare
    /// panel's raws), each on a thread of its own through the viewport's
    /// engine, sent back to the library.
    fn render_heads(&mut self) {
        let asked = self.shell.library_mut().take_head_requests();
        if asked.is_empty() {
            return;
        }
        let sink = self.shell.library().head_sink();
        let Some(persist) = &self.persist else {
            return;
        };
        let (vault, engine, bundle) = (
            persist.vault().clone(),
            self.shell.render_engine(),
            self.bundle(),
        );
        for (id, size) in asked {
            let path = self.shell.library().file(&id).map(Path::to_path_buf);
            let Some(path) = path else {
                sink.send(helios_gui::head_render::HeadRender {
                    id: id.clone(),
                    version: None,
                    size,
                    image: Err(format!("no copy of image {id} is present")),
                });
                continue;
            };
            let to = sink.clone();
            let spawned = helios_gui::head_render::spawn(
                vault.clone(),
                engine.clone(),
                bundle.clone(),
                path,
                id.clone(),
                size,
                move |r| to.send(r),
            );
            if let Err(e) = spawned {
                sink.send(helios_gui::head_render::HeadRender {
                    id,
                    version: None,
                    size,
                    image: Err(e),
                });
            }
        }
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
                Ok(Ready::First(graph, node)) => {
                    // The image open before is closed first: what is
                    // drawn now is not saved into its edit.
                    if let Some(persist) = self.persist.as_mut() {
                        persist.close_image(&mut self.shell, seconds_f64());
                    }
                    if let Err(e) = self.shell.set_view_source(graph, node, "image") {
                        warn(&format!("no first paint: {e}"));
                    }
                    i += 1;
                    continue;
                }
                Ok(Ready::Done(Ok(prepared))) => {
                    let path = prepared.path.clone();
                    show_prepared(&mut self.shell, self.persist.as_mut(), prepared)
                        .map_err(|e| format!("{}: {e}", path.display()))
                }
                Ok(Ready::Done(Err(e))) => Err(e),
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(TryRecvError::Disconnected) => Err(format!(
                    "{}: its opening stopped (a panic)",
                    self.preparing[i].path.display()
                )),
            };
            let Preparing { path, library, .. } = self.preparing.remove(i);
            if let Some(id) = library {
                self.library_opened(&id, done);
                continue;
            }
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

    /// An image of the library made ready: shown in the darkroom, or the
    /// error said (a darkroom restored on it is left).
    fn library_opened(&mut self, id: &str, done: Result<(), String>) {
        if done.is_err() {
            // Its large thumbnail is no longer drawn.
            self.shell.open_from_library(None);
        }
        let handle = self.shell.app().handle();
        let opened = Opened::Image(id.to_string());
        match done {
            Ok(()) => {
                self.shown = Some(Shown::Library(id.to_string()));
                contexts::open(handle, &opened, &egui_pop::model::commands::Origin::User);
            }
            Err(e) => {
                if contexts::darkroom(handle) == Some(opened) {
                    contexts::leave_darkroom(handle, &egui_pop::model::commands::Origin::User);
                    self.shell
                        .notify(helios_gui::dialogs::Level::Warning, not_reopened(&e));
                } else {
                    self.shell.notify(helios_gui::dialogs::Level::Error, e);
                }
            }
        }
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

impl App {
    /// The main window closes after this pass and Helios with it: nothing
    /// to draw (work waiting for a frame, thumbnails or tiles, would hold
    /// the window up), only what the user changed to send to the node's
    /// vault. The node saves and closes it once the GUI is gone.
    fn closing(&mut self) {
        quit::requested();
        if let Some(persist) = &mut self.persist {
            persist.frame(&mut self.shell, seconds_f64());
            quit::mark("vault written (closing frame)");
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            // The application's life is the shell's policy, not the main
            // window's: closed while another window is open, that window
            // moves into it and Helios goes on (egui-pop's rule, in the
            // shell's pass below); Quit, the last window closed and the
            // system's end of session end it.
            let closing = if SESSION_ENDING.load(std::sync::atomic::Ordering::SeqCst) {
                self.shell.session_ending();
                Closing::Quit
            } else {
                self.shell.main_window_closing(ui.ctx())
            };
            match closing {
                Closing::Moved => {}
                // A question first: nothing drawn in this pass.
                Closing::Kept => return,
                Closing::Quit => {
                    self.closing();
                    return;
                }
            }
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
        // The workspace's network changed (an edit, a reset, an import,
        // an undo): the image goes through it in place, its values and
        // history kept; opened again when the new network cannot take it.
        if self.shell.take_network_changed() {
            // The Compare panel's renders went through the old network.
            self.shell.library_mut().network_changed();
            match self.shell.keep_image_open() {
                Ok(true) => {
                    if let Some(persist) = &mut self.persist {
                        persist.follow_graph(&self.shell);
                    }
                }
                Ok(false) => {}
                Err(_) => {
                    if let Err(e) = self.reopen() {
                        self.shell.notify(helios_gui::dialogs::Level::Error, e);
                    }
                }
            }
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
            let _ = self.shell.run(
                helios_gui::shell::IMPORT_COMMAND,
                args,
                helios_shell::commands::Origin::User,
            );
        }
        // Exports asked for: planned over the library and run as jobs.
        for ask in self.shell.take_exports() {
            let ran = match &self.exporter {
                Some(exporter) => exporter.run(&self.bundle(), &ask),
                None => Err("no library open: exports take their images from one".into()),
            };
            if let Err(e) = ran {
                self.shell.notify(
                    helios_gui::dialogs::Level::Error,
                    Message::new("gui-export-failed", "The export did not start: { $error }")
                        .with("error", e),
                );
            }
        }
        self.render_heads();
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
            // A scan changed where copies are (a disk plugged in or
            // out): the photos' badges and thumbnails follow.
            if persist.take_locations_changed() {
                self.shell.refresh_volumes();
                if let Err(e) = fill_library(&mut self.shell, persist) {
                    warn(&format!("cannot fill the library: {e}"));
                }
            }
            // While a gesture goes on, come back to save it in time.
            if self.shell.values().lock().is_ok_and(|v| v.uncommitted()) {
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
        // The GUI client disconnects: the node, asked to quit, closes
        // the vault once its last client is gone.
        if self.node.take().is_some() {
            quit::mark("GUI client disconnected from the node");
        }
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

/// Windows: a window program has no console of its own. Started from a
/// terminal with its output not redirected, it attaches to the terminal's
/// console, so `--version`, `--snapshot` and errors print there; with its
/// output redirected (a pipe, a file), it writes there as is.
#[cfg(windows)]
#[allow(unsafe_code)] // two Win32 calls that take and return plain values
fn attach_parent_console() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
    };
    // SAFETY: neither call takes a pointer; GetStdHandle reads the
    // process's standard handle, AttachConsole fails harmlessly when the
    // parent has no console (started from Explorer).
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out.is_null() || out == INVALID_HANDLE_VALUE {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

fn main() -> Result<(), String> {
    #[cfg(windows)]
    attach_parent_console();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag] = args.as_slice()
        && (flag == "--version" || flag == "-V")
    {
        println!("helios-gui {}", env!("HELIOS_VERSION"));
        return Ok(());
    }
    start::begin();
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
    let ending = listen_for_ending();
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
    // One invocation's settings (`--set KEY=VALUE`, the run layer).
    let run_settings = helios_gui::workshops::run_layer(&args)?;
    // `--vault FILE`, else the default library in Helios's data
    // directory, made on the first launch.
    let vault_file = helios_gui::platform::library_file(&args)?;
    // The import window's choices are kept per library: a file named
    // after the vault's path.
    let import_choices = {
        let library = vault_file
            .as_ref()
            .map(|f| std::path::absolute(f).unwrap_or_else(|_| f.clone()))
            .map_or_else(
                || "default".to_string(),
                |f| {
                    f.to_string_lossy()
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                        .collect::<String>()
                        .trim_matches('-')
                        .to_string()
                },
            );
        helios_gui::shell::config_dir()
            .join("import-choices")
            .join(format!("{library}.json"))
    };
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
    let health_file = vault_file.clone();
    let catalogue_file = vault_file.clone();
    let thumbs_file = vault_file.clone();
    let map_file = vault_file.clone();
    // A library that cannot be opened leaves Helios without one: the
    // reason is said, and imports refuse with it. The local node owns the
    // vault (docs/design/node-client-split.md, choice A): it saves it on
    // its own schedule and closes it when it stops; the window is its
    // client, linked to it by `Persist`.
    let (node, no_library) = match vault_file
        .as_deref()
        .map(helios_gui::platform::open_library)
    {
        Some(Ok(store)) => (
            Some(LocalNode::open(
                store,
                bus.clone(),
                helios_gui::node::FLUSH_EVERY,
            )),
            None,
        ),
        Some(Err(e)) => {
            warn(&format!("the library: {e}"));
            (None, Some(e))
        }
        None => (None, None),
    };
    // The node watches the machine's cards and disks, with or without a
    // window; without a library, a watch of the desktop's own still lists
    // the cards.
    let volume_watch = node.as_ref().map_or_else(
        || {
            helios_gui::node_volumes::VolumeWatch::start(
                helios_storage::system(),
                bus.clone(),
                helios_gui::node_volumes::POLL,
            )
        },
        |n| n.watch_volumes(helios_storage::system()),
    );
    let persist = node.as_ref().map(|n| Persist::on(n.vault().clone(), bus));
    start::mark("vault opened (and migrated)");
    // Workshop sessions a former run left unkept are dropped (C6).
    if let Some(Err(e)) = persist.as_ref().map(Persist::drop_unkept_sessions) {
        warn(&format!("the workshop sessions: {e}"));
    }
    let vault = node.as_ref().map(|n| n.vault().clone());
    // The library is read while the window opens.
    let library_reading = vault.clone().map(read_library);
    // Helios's settings, resolved once from their layers: the machine's
    // file, the library's and the person's layers in the vault (the ones
    // `helios-node --vault` keeps there), `--set`. Every part reads them
    // from here; what does not read is left out and logged.
    let config = helios_gui::shell::config_dir();
    let settings = {
        let (sources, problems) = helios_gui::config::read(
            &config,
            &|name| vault.as_ref().map_or(Ok(None), |v| v.config_layer(name)),
            vault
                .as_ref()
                .and(vault_file.as_deref())
                .and_then(helios_gui::config::project_layer),
            &run_settings,
        );
        let settings = helios_gui::config::Settings::resolve(&sources);
        for p in problems.iter().chain(settings.problems()) {
            log::warn!(target: "helios", "the settings: {p}");
        }
        settings
    };
    let queue_when_empty = helios_gui::queue::when_empty(settings.config());
    // The experimental features, read once: on, their panels are
    // declared (and their commands registered) for the whole session.
    let experimental = helios_gui::Experimental {
        workshops: helios_gui::workshops::enabled(settings.config()),
    };
    // The workspace as it stands: its template's network, as edited.
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
    // The plugins installed (built-in, the ones Helios ships beside its
    // executable, the user's), read before anything of them runs: their
    // importers are File > Import from's entries.
    let plugins = {
        let mut broker = helios_plugin::Broker::new(helios_plugin::Policy::default());
        let mut dirs = helios_plugin::default_dirs(None);
        if let (Some(root), Some(programs)) = (
            helios_plugin::bundled::default_root(),
            helios_plugin::bundled::programs_dir(),
        ) {
            match helios_plugin::bundled::add_to(&mut dirs, &root, &programs) {
                Ok(added) => {
                    for name in added.without_program {
                        log::info!(
                            "the {name} plugin is not offered: its program is not beside Helios in {}",
                            programs.display()
                        );
                    }
                }
                Err(e) => warn(&format!(
                    "the plugins Helios ships could not be written to {}: {e}",
                    root.display()
                )),
            }
        }
        broker.discover(&dirs);
        for r in broker.rejected() {
            warn(&format!(
                "plugin {} refused: {}",
                r.dir.display(),
                r.problems.join("; ")
            ));
        }
        Arc::new(broker)
    };
    let known_vault = vault.clone();
    let health_vault = vault.clone();
    let mut builder = Shell::builder(helios_panels_with(experimental), Some(ui_settings))
        .workspace(&workspace)
        .on_exit(move |app| save_workspace(&dir, app))
        .desktop(instance)
        // The desktop speaks the system's language by default.
        .system_language()
        .system_focus(focus_mode(&focus_reading))
        .plugins(&plugins);
    // A darkroom left on an image the vault no longer has is not
    // restored.
    if let Some(vault) = vault.clone() {
        builder = builder.images(Arc::new(move |id: &str| {
            helios_vault::ImageId::from_hex(id)
                .ok()
                .and_then(|image| vault.locations(&image).ok())
                .is_some_and(|l| l.iter().any(helios_vault::Location::present))
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
    // The library's thumbnail cache, beside its catalogue.
    let _ = THUMB_CACHE.set(
        thumbs_file
            .as_ref()
            .filter(|_| vault.is_some())
            .and_then(|file| {
                let limit = helios_gui::thumbnails::cache_limit(&settings);
                helios_gui::thumbnails::open_cache(file, limit)
                    .map_err(|e| warn(&e))
                    .ok()
            }),
    );
    // The grids' thumbnail textures, within their budget.
    let texture_budget = helios_gui::thumb_textures::budget(&settings);
    // The Settings app writes them back: the person's and the library's
    // layers to the vault, the machine's to its file.
    let settings_vault = vault.clone();
    let settings_dir = config.clone();
    // The GUI connects to the node as it starts, and disconnects as it
    // ends (`on_exit`); the shell asks the node to quit.
    let node_link = node.as_ref().map(LocalNode::link);
    let node_client = node.as_ref().map(LocalNode::connect);
    let ran = builder
        .run_native("helios-gui", options, move |mut shell, cc| {
            start::mark("window open, application built (plugins, fonts)");
            if let Some(link) = node_link.clone() {
                shell.attach_node(link);
            }
            // The node runs the backups (its jobs): they go on when the
            // window that started them closes.
            let backups = persist.as_ref().zip(node_link.as_ref()).map(|(p, link)| {
                Backups::new(
                    p.vault().clone(),
                    shell.jobs(),
                    helios_storage::system(),
                    |e| warn(&e),
                )
                .adopted_by(link)
            });
            let _ = WINDOW.set(cc.egui_ctx.clone());
            shell.library_mut().set_texture_budget(texture_budget);
            // The render cache's budget (`system.cache.budget` / `share`).
            helios_gui::caches::apply_settings(&shell, &settings);
            helios_gui::config::install(
                shell.app().handle(),
                settings.clone(),
                settings_vault.clone().map(|v| {
                    Box::new(move |name: &str, text: &str| v.set_config_layer(name, Some(text)))
                        as helios_gui::config::VaultWriter
                }),
                &settings_dir,
            );
            if let Some(rs) = cc.wgpu_render_state.as_ref() {
                shell.attach_gpu(rs);
            }
            start::mark("GPU attached to the viewport");
            shell.set_title(&name);
            // The first run asks "Coming from darktable?" (once).
            shell.offer_pointer_choice();
            // Tips come by themselves in the window, never in tests.
            shell.tips_by_themselves(true);
            shell.set_queue_when_empty(queue_when_empty);
            // The import window's cards, from the node's volume watch.
            shell.follow_volumes(volume_watch.clone());
            // The volumes and where each photo is: present, offline or
            // missing, scanned at start, when a volume comes or goes and
            // every five minutes.
            if let Some(vault) = known_vault.clone() {
                let library = vault.clone();
                let scanner = helios_storage::scan::Scanner::start(
                    vault.clone(),
                    helios_storage::system(),
                    helios_storage::node(),
                    move || {
                        helios_library::import::library_root(&library)
                            .ok()
                            .flatten()
                    },
                    std::time::Duration::from_mins(5),
                    |outcome| match outcome {
                        Ok(report) => log::debug!("volumes scanned: {report:?}"),
                        Err(e) => log::warn!("volumes: the scan failed: {e}"),
                    },
                );
                volume_watch.on_change(move |change| {
                    if !change.at_start() {
                        scanner.poke();
                    }
                });
                // The strategy's "when a backup disk is plugged in": what
                // is short goes to it.
                if let Some(backups) = &backups {
                    backups.follow(&volume_watch);
                }
                // The Storage page, and the volumes offline photos name.
                let marker = helios_library::import::library_root(&vault)
                    .ok()
                    .flatten()
                    .map_or_else(|| "helios".to_string(), |p| p.display().to_string());
                shell.set_storage(Arc::new(helios_gui::shell::VaultStorage::new(
                    vault,
                    helios_storage::system(),
                    marker,
                )));
            }
            // The import accounts, kept in the vault (public facts only).
            if let Some(vault) = known_vault.clone() {
                // Secrets and tokens in the system keyring (Windows has no
                // keyring support yet: kept for the session only); `oauth`
                // steps through the browser sign-in.
                let secrets: Arc<dyn helios_connect::account::Secrets> = if cfg!(windows) {
                    Arc::new(helios_connect::account::MemorySecrets::default())
                } else {
                    Arc::new(helios_connect::keyring::SystemSecrets)
                };
                shell.set_import_sign_in(Arc::new(helios_connect::oauth::OAuth::system()));
                shell.set_import_accounts(
                    Arc::new(helios_connect::account::VaultAccounts(vault)),
                    secrets,
                );
            }
            // What the vault holds, for its "in your Library" prefilter.
            if let Some(vault) = known_vault.clone() {
                shell.set_import_known(helios_gui::import_scan::known_of(vault.clone()));
                // An offline folder's photos, as the image map recorded
                // them, with the thumbnails the library's cache has.
                shell.set_import_recorded(helios_gui::import_scan::recorded_of(vault));
            }
            if let Some(cache) = THUMB_CACHE.get().cloned().flatten() {
                shell.set_import_thumbnail_cache(cache);
            }
            // The cards' glances on disk, in the user cache folder: a card
            // plugged in again shows its previews without a read. Its
            // folder is listed off the interface thread.
            if let Some(root) = helios_cache::user_cache_dir() {
                let spawned = std::thread::Builder::new()
                    .name("helios-glance-disk".into())
                    .spawn(move || {
                        let glances = helios_gui::import_scan::GlanceCache::global();
                        if let Err(e) = glances.open_disk(&root) {
                            log::warn!("import previews on disk: {}: {e}", root.display());
                        }
                    });
                if let Err(e) = spawned {
                    log::warn!("import previews on disk: no thread: {e}");
                }
            }
            // The image map: what the import window's sources list is
            // recorded, Helios's own folders never.
            if let Some(vault) = known_vault.clone() {
                let settings = helios_gui::image_map::settings(settings.config());
                let mut helios = vec![helios_gui::shell::config_dir()];
                helios.extend(helios_gui::platform::data_dir_with(|k| {
                    std::env::var(k).ok()
                }));
                helios.extend(map_file.as_ref().and_then(|f| {
                    std::path::absolute(f)
                        .ok()?
                        .parent()
                        .map(|d| d.join(".helios"))
                }));
                shell.set_import_map(helios_gui::image_map::recorder(
                    vault.clone(),
                    settings.clone(),
                    helios.clone(),
                ));
                // The map's job: the import window's cells in view first,
                // then each volume plugged in.
                let feed = helios_library::map_job::Feed::default();
                shell.set_map_feed(feed.clone());
                // A scan asked for: a volume by its id, or every volume
                // `system.map.record` allows.
                let scan = feed.clone();
                if let Err(e) = shell.register(
                    helios_shell::commands::CommandSpec::new(
                        "library.map.scanNow",
                        "Scan a volume for the image map now (`volume`, its id; none: every volume the settings allow)",
                        "Library",
                    ),
                    move |args, _| {
                        let volume = args
                            .get("volume")
                            .and_then(serde_json::Value::as_str)
                            .map(|v| helios_vault::storage::VolumeId(v.into()));
                        scan.scan_now(volume);
                        Ok(serde_json::json!({ "asked": true }))
                    },
                ) {
                    log::warn!("library.map.scanNow: {e}");
                }
                let plugged = feed.clone();
                volume_watch.on_change(move |change| {
                    if let helios_gui::node_volumes::Change::Mounted(m) = change
                        && !m.at_start
                    {
                        plugged.plugged(m.id.clone());
                    }
                });
                if let Some(cache) = THUMB_CACHE.get().cloned().flatten() {
                    helios_gui::image_map::start_thumb_check(
                        vault.clone(),
                        &shell.jobs(),
                        cache,
                        &settings,
                        helios.clone(),
                        feed.clone(),
                    );
                }
                backfill_captures(vault.clone(), shell.jobs());
                helios_gui::image_map::start_job(vault, shell.jobs(), settings, helios, feed);
            }
            // The health check (Help > Health check): Helios's checks
            // over this machine's volumes and the vault, a while after
            // start, daily and when a volume comes or goes.
            // The node keeps its schedule (its job), with or without a
            // window; the shell shows the same runner's findings.
            let (cards, settings) = (shell.waiting_cards(), shell.app().handle().store());
            let runner = health_runner(
                health_vault.clone(),
                health_file.clone(),
                cards,
                settings,
                plugins.clone(),
            );
            runner.report_to(shell.jobs());
            if let Some(link) = &node_link {
                helios_gui::node_jobs::Health::adopted_by(
                    runner.clone(),
                    link,
                    std::time::Duration::from_secs(10),
                );
            }
            shell.set_health(runner);
            shell.remember_import_sources_in(
                helios_gui::shell::config_dir().join("import-sources.json"),
            );
            shell.remember_import_choices_in(import_choices);
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
                && let Err(e) = library_commands(
                    &mut shell,
                    p,
                    backups.as_ref(),
                    &plugins,
                    &refill,
                    &cc.egui_ctx,
                )
            {
                shell.notes.push(format!("library commands: {e}"));
            }
            if let (Some(p), Some(file)) = (&persist, catalogue_file.clone())
                && let Err(e) = catalogue_commands(&mut shell, p, file, &cc.egui_ctx)
            {
                shell.notes.push(format!("catalogue commands: {e}"));
            }
            if let Err(e) = helios_gui::caches::register(&mut shell) {
                shell.notes.push(format!("cache commands: {e}"));
            }
            if let Err(e) = helios_gui::calibrate::register(&mut shell) {
                shell.notes.push(format!("calibration commands: {e}"));
            }
            // Without a library (no `--vault`), nothing registers the
            // import the shell hands over: its imports say so instead.
            if let Some(e) = &no_library {
                shell.notify(
                    helios_gui::dialogs::Level::Error,
                    Message::new(
                        "gui-library-cannot-open",
                        "The library cannot be opened: { $error }",
                    )
                    .with("error", e.clone()),
                );
            }
            if !shell.has_command(helios_gui::shell::IMPORT_COMMAND) {
                shell.set_no_library(no_library.clone());
            }
            // The node runs the exports (its jobs): they go on when the
            // window that started them closes.
            let exporter = persist.as_ref().zip(node_link.as_ref()).map(|(p, link)| {
                let ctx = cc.egui_ctx.clone();
                // On the viewport's engine: one render cache and one
                // budget for the desktop, so exporting the image open does
                // not compute again what the darkroom holds.
                helios_gui::export::Exporter::new(
                    p.vault().clone(),
                    shell.jobs(),
                    shell.export_sink(),
                    move || ctx.request_repaint(),
                    shell.viewport().engine().clone(),
                )
                .adopted_by(link)
            });
            let mut app = App {
                node: node_client,
                exporter,
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
    // The window is gone: the node stops (its last flush, the vault
    // closed), against its deadline, whether or not the shell asked it
    // to quit (an error ending eframe).
    drop(vault);
    let mut finisher = quit::Finisher::default();
    if let Some(node) = node {
        finisher.spawn("node stopped", quit::VAULT_DEADLINE, move || node.stop());
    }
    for left in finisher.finish() {
        warn(&format!("quit: {left} was left unfinished"));
    }
    ending.finish();
    // A catalogue restore was asked: the next Helios applies it as it
    // opens the library.
    if RESTART.load(Ordering::Relaxed) {
        let started = std::env::current_exe()
            .and_then(|exe| std::process::Command::new(exe).args(&args).spawn());
        if let Err(e) = started {
            warn(&format!("restart after the catalogue restore: {e}"));
        }
    }
    ran
}
