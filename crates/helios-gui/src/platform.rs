//! Helios on the desktop's platform services (egui-pop's `platform`).
//!
//! - **One instance**: a second `helios-gui` launch hands its arguments to
//!   the running one and exits (`instance::single`, called by `main`). The
//!   running one comes to the front and opens what it was given: an image
//!   in the viewport, a workspace in a window of its own (one process per
//!   workspace, started with [`NEW_INSTANCE`]). `helios://` links take the
//!   same path ([`link`]).
//! - **Notifications**: a long export or batch that ends while no Helios
//!   window has the focus says so on the desktop ([`job_finished`]), unless
//!   the `notifications.jobs` setting is off.
//! - **Logs and crash reports** go to the user's state directory
//!   ([`log_dir`], [`crash_dir`]): egui-pop's log file, rotated, at the
//!   `developer.log_level` setting, and its crash reports, written by a
//!   panic hook and offered on the next start, never sent. The About
//!   dialog says where ([`about`]); so does the debug bridge's `logs`
//!   collector.
//!
//! The [`Platform`] plugin is part of every shell: its hooks on the
//! instance's steps do nothing where no instance runs (tests, pictures,
//! tools). The desktop adds egui-pop's services with [`desktop`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::{AppBuilder, AppHandle, Ctx, Plugin, PluginError};
use egui_pop::model::ids::{PluginId, SettingId};
use egui_pop::model::values::Descriptor;
use egui_pop::platform::crash::CrashPlugin;
use egui_pop::platform::dirs;
use egui_pop::platform::instance::{Instance, InstancePlugin, STEP_DEEPLINK, STEP_SECOND_LAUNCH};
use egui_pop::platform::logs::{Logs, LogsPlugin};
use egui_pop::platform::notify::{self, Desktop, Notification, NotifyPlugin};
use helios_core::Message;
use serde_json::{Value as Json, json};

/// Helios's id on the desktop: its desktop file's name
/// (`packaging/linux`), the single instance's socket.
pub const APP_ID: &str = "io.github.quazardous.Helios";

/// The application's id for egui-pop (`AppBuilder::app_id`): it names the
/// agent socket, `egui-pop-agent-helios.sock` in the runtime directory.
pub const AGENT_APP_ID: &str = "helios";

/// The scheme of Helios's links: `helios://open?path=...`.
pub const SCHEME: &str = "helios";

/// Starts a window of its own instead of handing over to the running
/// Helios (a workspace opened from the running one).
pub const NEW_INSTANCE: &str = "--new-instance";

pub const PLUGIN: PluginId = PluginId::from_static("helios.platform");

/// Helios's directory in the user's state directory
/// (`~/.local/state/helios` on Linux), and its log file's name
/// (`helios.log`).
pub const STATE_NAME: &str = "helios";

/// Where the log files go, from the environment `env` gives: `logs` in
/// Helios's state directory (`$XDG_STATE_HOME/helios/logs`, else
/// `~/.local/state/helios/logs`; `%LOCALAPPDATA%\helios\logs` on
/// Windows), `~/Library/Logs/helios` on macOS.
pub fn log_dir_with(env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = env("HOME").map(PathBuf::from)?;
        return Some(home.join("Library/Logs").join(STATE_NAME));
    }
    dirs::state_dir_with(STATE_NAME, env).map(|d| d.join("logs"))
}

/// Where crash reports go: `crashes` in Helios's state directory.
pub fn crash_dir_with(env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    dirs::state_dir_with(STATE_NAME, env).map(|d| d.join("crashes"))
}

/// Helios's data directory, from the environment `env` gives:
/// `$XDG_DATA_HOME/helios`, else `~/.local/share/helios`;
/// `%LOCALAPPDATA%\helios` on Windows, `~/Library/Application
/// Support/helios` on macOS. None when the variables it needs are
/// missing (or not absolute).
pub fn data_dir_with(env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let absolute = |v: Option<String>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let base = if cfg!(windows) {
        absolute(env("LOCALAPPDATA"))?
    } else if cfg!(target_os = "macos") {
        absolute(env("HOME"))?.join("Library/Application Support")
    } else {
        match absolute(env("XDG_DATA_HOME")) {
            Some(p) => p,
            None => absolute(env("HOME"))?.join(".local/share"),
        }
    };
    Some(base.join(STATE_NAME))
}

/// The default library's file name, in Helios's data directory.
pub const LIBRARY_FILE: &str = "library.db";

/// The library (vault file) a launch opens: `--vault FILE` when given,
/// else the default library, `library.db` in Helios's data directory
/// ([`data_dir_with`]); none when there is no data directory either.
pub fn library_file_with(
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<PathBuf>, String> {
    if let Some(i) = args.iter().position(|a| a == "--vault") {
        return args
            .get(i + 1)
            .map(|f| Some(PathBuf::from(f)))
            .ok_or_else(|| "--vault takes a FILE".into());
    }
    Ok(data_dir_with(env).map(|d| d.join(LIBRARY_FILE)))
}

/// Opens the library (vault file) at `path`, its folder made first (the
/// default library's, on the first launch). A catalogue snapshot whose
/// restore was asked for (`library.catalogue.restore`) is put in place
/// first, the catalogue it replaces kept aside; a restore that fails
/// leaves the catalogue as it was, and says why in the log.
pub fn open_library(path: &Path) -> Result<helios_vault_sqlite::SqliteStore, String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    match helios_vault_sqlite::snapshot::apply_pending(path, now) {
        Ok(Some(r)) => log::info!(
            "catalogue restored from {}; the one it replaced is kept as {}",
            r.from.display(),
            r.aside.display()
        ),
        Ok(None) => {}
        Err(e) => log::error!("catalogue restore refused: {e}"),
    }
    helios_vault_sqlite::SqliteStore::open(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// [`library_file_with`] from this process's environment.
pub fn library_file(args: &[String]) -> Result<Option<PathBuf>, String> {
    library_file_with(args, |name| std::env::var(name).ok())
}

/// [`log_dir_with`] from this process's environment.
pub fn log_dir() -> Option<PathBuf> {
    log_dir_with(|name| std::env::var(name).ok())
}

/// [`crash_dir_with`] from this process's environment.
pub fn crash_dir() -> Option<PathBuf> {
    crash_dir_with(|name| std::env::var(name).ok())
}

/// Whether a long job's end is said on the desktop, at
/// `settings/notifications.jobs` (on by default).
pub const SETTING_JOBS: SettingId = SettingId::from_static("notifications.jobs");

/// Days a backup disk may stay away before the health check warns, at
/// `settings/health.destinationDays` (14 by default).
pub const SETTING_DESTINATION_DAYS: SettingId = SettingId::from_static("health.destinationDays");

/// The `health.destinationDays` setting from the settings' `store`.
pub fn destination_days(store: &egui_pop::core::binding::SharedStore) -> u64 {
    store
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&format!("settings/{SETTING_DESTINATION_DAYS}"))
        .and_then(|e| e.value.as_i64())
        .and_then(|d| u64::try_from(d).ok())
        .unwrap_or(helios_health::backup::DESTINATION_DAYS)
}

/// A job is long, and its end worth a notification, from this many
/// seconds.
pub const LONG_JOB_SECONDS: f64 = 10.0;

/// A job whose end may be notified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Export,
    Batch,
}

/// How a job ended: what it did (`Ok`, a summary in the interface's
/// language) or why it failed, and how long it took.
#[derive(Clone, Debug, PartialEq)]
pub struct JobEnd {
    pub job: Job,
    pub outcome: Result<String, String>,
    pub seconds: f64,
}

/// Whether a job's end shows on the desktop: the setting on, no Helios
/// window with the focus, and a long job (a short one ends under the
/// user's eyes).
pub fn should_notify(enabled: bool, focused: bool, seconds: f64) -> bool {
    enabled && !focused && seconds >= LONG_JOB_SECONDS
}

/// Whether the `notifications.jobs` setting is on.
pub fn jobs_enabled(app: &AppHandle) -> bool {
    app.store()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&format!("settings/{SETTING_JOBS}"))
        .and_then(|e| e.value.as_bool())
        .unwrap_or(true)
}

/// Whether a Helios window (the main one or a detached panel's) has the
/// focus; when no window says, it is taken as focused.
pub fn window_focused(ctx: &egui::Context) -> bool {
    ctx.input(|i| {
        let known: Vec<bool> = i.raw.viewports.values().filter_map(|v| v.focused).collect();
        known.is_empty() || known.contains(&true)
    })
}

/// The notification for a job's end, its text through `text` (the
/// interface's language).
pub fn notification(end: &JobEnd, text: impl Fn(Message) -> String) -> Notification {
    let title = match (end.job, end.outcome.is_ok()) {
        (Job::Export, true) => Message::new("gui-job-export-done", "Export finished"),
        (Job::Export, false) => Message::new("gui-job-export-failed", "Export failed"),
        (Job::Batch, true) => Message::new("gui-job-batch-done", "Batch finished"),
        (Job::Batch, false) => Message::new("gui-job-batch-failed", "Batch failed"),
    };
    let (body, level) = match &end.outcome {
        Ok(summary) => (summary.as_str(), notify::Level::Success),
        Err(why) => (why.as_str(), notify::Level::Error),
    };
    Notification::new(&text(title), body)
        .level(level)
        .icon(APP_ID)
}

/// Says a job's end on the desktop when [`should_notify`] says so
/// (`focused`: [`window_focused`]); whether it was posted. Without the
/// notifications plugin (tests, pictures) nothing is posted.
pub fn job_finished(
    app: &AppHandle,
    end: &JobEnd,
    focused: bool,
    text: impl Fn(Message) -> String,
) -> bool {
    if !should_notify(jobs_enabled(app), focused, end.seconds) {
        return false;
    }
    let Some(notifier) = app.service(&notify::NOTIFIER) else {
        return false;
    };
    notifier.post(notification(end, text));
    true
}

/// Something a launch or a link asks to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Open {
    /// An image, shown in the viewport.
    Image(PathBuf),
    /// A workspace directory, in a window of its own.
    Workspace(PathBuf),
}

impl Open {
    /// A directory is a workspace, anything else an image.
    pub fn of(path: PathBuf) -> Open {
        if path.is_dir() {
            Open::Workspace(path)
        } else {
            Open::Image(path)
        }
    }
}

/// What the launches and links asked to open, waiting for the window's
/// next frame (shared with the plugin's hooks), and whether a single
/// instance hands launches over at all.
#[derive(Clone, Debug, Default)]
pub struct Opens {
    queue: Arc<Mutex<VecDeque<Open>>>,
    listening: Arc<AtomicBool>,
}

impl Opens {
    /// A single instance hands launches over (egui-pop's `InstancePlugin`
    /// is added): [`Platform`] follows its steps.
    pub fn listen(&self) {
        self.listening.store(true, Ordering::Relaxed);
    }

    pub fn listening(&self) -> bool {
        self.listening.load(Ordering::Relaxed)
    }

    pub fn push(&self, open: Open) {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(open);
    }

    /// What was asked, oldest first, taken.
    pub fn take(&self) -> Vec<Open> {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }
}

/// Flags of `helios-gui` followed by a value that is not something to open.
const VALUED: [&str; 5] = [
    "--vault",
    "--node",
    "--token-file",
    crate::mask_display::FLAG,
    crate::workshops::SET_FLAG,
];

/// What a launch's arguments ask to open: paths (relative ones from
/// `cwd`), `file://` URIs as a file manager passes them, `--image FILE`.
/// Other flags and their values are skipped, and so are `helios:` links,
/// which go through `deeplink.open` ([`link`]).
pub fn opens(args: &[String], cwd: Option<&Path>) -> Vec<Open> {
    let mut out = Vec::new();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        if a == "--image" {
            if let Some(file) = args.next() {
                out.push(Open::Image(resolve(Path::new(file), cwd)));
            }
            continue;
        }
        if a == "--snapshot" {
            args.next();
            args.next();
            continue;
        }
        if VALUED.contains(&a.as_str()) {
            args.next();
            continue;
        }
        if a.starts_with('-') || is_link(a) {
            continue;
        }
        let path = match a.strip_prefix("file://") {
            Some(uri) => match file_uri(uri) {
                Some(p) => p,
                None => continue,
            },
            None => resolve(Path::new(a), cwd),
        };
        out.push(Open::of(path));
    }
    out
}

fn resolve(path: &Path, cwd: Option<&Path>) -> PathBuf {
    match cwd {
        Some(cwd) if path.is_relative() => cwd.join(path),
        _ => path.to_path_buf(),
    }
}

/// Whether an argument is a link in Helios's scheme.
pub fn is_link(arg: &str) -> bool {
    arg.split_once(':')
        .is_some_and(|(s, _)| s.eq_ignore_ascii_case(SCHEME))
}

/// A `file://` URI's path (what follows `file://`): an empty host or
/// `localhost`, then an absolute path, percent-decoded.
fn file_uri(rest: &str) -> Option<PathBuf> {
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    rest.starts_with('/')
        .then(|| percent_decode(rest))
        .flatten()
        .map(PathBuf::from)
}

/// `%XX` escapes decoded; none for a broken escape or text that is not
/// UTF-8.
pub fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// What a `helios:` link asks to open:
///
/// - `helios://open?path=<absolute path>`: an image, or a workspace when
///   the path is a directory;
/// - `helios://workspace?path=<absolute path>`: a workspace.
///
/// The path is percent-encoded; a relative one is refused (a link has no
/// working directory).
pub fn link(url: &str) -> Result<Open, String> {
    let (scheme, rest) = url
        .split_once(':')
        .ok_or_else(|| format!("{url}: not a link"))?;
    if !scheme.eq_ignore_ascii_case(SCHEME) {
        return Err(format!("{url}: not a {SCHEME}: link"));
    }
    let rest = rest.trim_start_matches('/');
    let (verb, query) = rest.split_once('?').unwrap_or((rest, ""));
    let path = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("path="))
        .ok_or_else(|| format!("{url}: no `path`"))?;
    let path =
        PathBuf::from(percent_decode(path).ok_or_else(|| format!("{url}: a broken `path`"))?);
    if !path.is_absolute() {
        return Err(format!("{url}: the path is not absolute"));
    }
    match verb.trim_end_matches('/') {
        "open" => Ok(Open::of(path)),
        "workspace" => Ok(Open::Workspace(path)),
        other => Err(format!("{url}: nothing opens `{other}`")),
    }
}

/// The strings of a JSON array.
fn strings(v: &Json) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(String::from)
        .collect()
}

/// Helios's part of the platform services: a second launch's files and
/// `helios:` links become [`Open`]s for the window.
pub struct Platform {
    opens: Opens,
}

impl Platform {
    pub fn new(opens: Opens) -> Self {
        Self { opens }
    }
}

impl Plugin for Platform {
    fn id(&self) -> PluginId {
        PLUGIN
    }

    fn title(&self) -> String {
        "Helios platform".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        ctx.setting(
            SETTING_JOBS,
            Descriptor::bool("Notify when a long job ends", true).description(
                "Shows a desktop notification when a long export or batch finishes or fails while no Helios window has the focus.",
            ),
        )?;
        ctx.setting(
            SETTING_DESTINATION_DAYS,
            Descriptor::int(
                "Warn when a backup disk is away for (days)",
                1,
                365,
                i64::try_from(helios_health::backup::DESTINATION_DAYS).unwrap_or(14),
            )
            .description(
                "The health check warns when a disk the backup strategy copies to has not been plugged in for longer.",
            ),
        )?;
        ctx.setting(
            crate::catalogue::SETTING_EVERY,
            Descriptor::int(
                "Catalogue snapshot every (hours)",
                1,
                24 * 30,
                i64::try_from(crate::catalogue::EVERY_HOURS).unwrap_or(24),
            )
            .description(
                "Helios takes a verified snapshot of the catalogue this often, keeps it next to the catalogue and copies it to the backup disks plugged in.",
            ),
        )?;
        ctx.setting(
            crate::catalogue::SETTING_KEPT,
            Descriptor::int(
                "Catalogue snapshots kept",
                1,
                100,
                i64::try_from(crate::catalogue::KEPT).unwrap_or(5),
            )
            .description(
                "How many catalogue snapshots are kept next to the catalogue and on each backup disk; the oldest go first.",
            ),
        )?;
        // Without a single instance (tests, pictures, tools) there are
        // no launches to follow.
        if !self.opens.listening() {
            return Ok(());
        }
        // A second launch: egui-pop brought the window to the front and
        // opens its links; its other arguments are files and workspaces.
        let queue = self.opens.clone();
        ctx.after(STEP_SECOND_LAUNCH, move |_, args, result| {
            if result.is_err() {
                return;
            }
            let links = strings(&args["links"]);
            let plain: Vec<String> = strings(&args["args"])
                .into_iter()
                .filter(|a| !links.contains(a))
                .collect();
            let cwd = args["cwd"].as_str().map(Path::new);
            for open in opens(&plain, cwd) {
                queue.push(open);
            }
        });
        let queue = self.opens.clone();
        ctx.replace(STEP_DEEPLINK, move |_, args| {
            let url = args["url"].as_str().ok_or("`url` is missing")?;
            let open = link(url).inspect_err(|e| log::warn!(target: "helios", "{e}"))?;
            let shown = format!("{open:?}");
            queue.push(open);
            Ok(json!({ "open": shown }))
        });
        Ok(())
    }
}

/// egui-pop's platform services for the desktop window: the single
/// instance (its launches handed to [`Platform`]), the desktop's
/// notifications, tied to Helios's desktop file, the log file and crash
/// reports in the user's state directory, the cache of compiled kit
/// and theme files, and the desktop's text scaling ("larger text", read
/// once at start), the text size's default until the user picks one.
pub fn desktop(app: AppBuilder, instance: Instance) -> AppBuilder {
    use egui_pop_agent::AgentSocketExt as _;
    let mut logs = LogsPlugin::new(STATE_NAME);
    if let Some(dir) = log_dir() {
        logs = logs.logs(Logs::new(dir, STATE_NAME));
    }
    let mut crashes = CrashPlugin::new("Helios", env!("CARGO_PKG_VERSION"));
    if let Some(dir) = crash_dir() {
        crashes = crashes.dir(dir);
    }
    // User kits and themes read from disk are compiled once into the
    // user's cache directory, under Helios's id.
    app.cache(egui_pop::tokens::cache::Cache::for_app(APP_ID))
        .system_text_scale(egui_pop::platform::text_scale::read())
        .plugin(logs)
        .plugin(crashes)
        .plugin(InstancePlugin::new(instance).scheme(SCHEME))
        .plugin(NotifyPlugin::new("Helios").backend(Desktop::new("Helios").app_id(APP_ID)))
        // The agent socket, off unless the user asks for it: the
        // `developer.agentSocket` setting, live, or `EGUI_POP_AGENT_SOCKET=1`
        // (`$XDG_RUNTIME_DIR/egui-pop-agent-helios.sock`; or a path),
        // read-only (docs/gui.md, "Agents").
        .app_id(AGENT_APP_ID)
        .agent_socket_from_env_or_setting()
        .debug_bridge_if(agent_writes(|n| std::env::var(n).ok()))
}

/// `HELIOS_AGENT_WRITE=1`: the agents may change the image and the
/// library, not only read them. For scenario runs on a throwaway library
/// (docs/gui.md, "Agents"); the bridge stays read-only otherwise.
pub const AGENT_WRITE: &str = "HELIOS_AGENT_WRITE";

/// Whether the environment lets agents write.
fn agent_writes(env: impl Fn(&str) -> Option<String>) -> bool {
    env(AGENT_WRITE).is_some_and(|v| v == "1")
}

/// The bridge made writable when asked, left to its default otherwise.
trait DebugBridgeIf {
    fn debug_bridge_if(self, on: bool) -> Self;
}

impl DebugBridgeIf for AppBuilder {
    fn debug_bridge_if(self, on: bool) -> Self {
        if on {
            self.debug_bridge(egui_pop::core::debug::BridgeMode::On)
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn agents_write_only_when_the_environment_says_1() {
        assert!(agent_writes(|n| (n == AGENT_WRITE).then(|| "1".to_string())));
        assert!(!agent_writes(
            |n| (n == AGENT_WRITE).then(|| "yes".to_string())
        ));
        assert!(!agent_writes(|_| None));
    }

    #[test]
    fn without_vault_the_default_library_is_in_the_data_directory() {
        let env = |name: &str| match name {
            "XDG_DATA_HOME" => Some("/data".to_string()),
            "HOME" => Some("/home/me".to_string()),
            "LOCALAPPDATA" => Some("C:\\Users\\me\\AppData\\Local".to_string()),
            _ => None,
        };
        let given = library_file_with(&args(&["--vault", "v.db", "a.jpg"]), env).unwrap();
        assert_eq!(given, Some(PathBuf::from("v.db")));
        assert!(library_file_with(&args(&["--vault"]), env).is_err());
        let default = library_file_with(&args(&["a.jpg"]), env).unwrap().unwrap();
        assert!(default.ends_with("helios/library.db"), "{default:?}");
        if cfg!(all(unix, not(target_os = "macos"))) {
            assert_eq!(default, PathBuf::from("/data/helios/library.db"));
            // Without XDG_DATA_HOME, under the home's .local/share.
            let home = |name: &str| (name == "HOME").then(|| "/home/me".to_string());
            assert_eq!(
                library_file_with(&[], home).unwrap(),
                Some(PathBuf::from("/home/me/.local/share/helios/library.db"))
            );
            // A relative XDG_DATA_HOME is ignored, as the spec says.
            let relative = |name: &str| match name {
                "XDG_DATA_HOME" => Some("data".to_string()),
                "HOME" => Some("/home/me".to_string()),
                _ => None,
            };
            assert_eq!(
                data_dir_with(relative),
                Some(PathBuf::from("/home/me/.local/share/helios"))
            );
        }
        assert_eq!(library_file_with(&[], |_| None).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn arguments_name_images_and_workspaces() {
        let dir = std::env::temp_dir();
        let cwd = Path::new("/photos");
        let got = opens(
            &args(&[
                "--vault",
                "v.db",
                "--mask-display",
                "grey",
                "a.jpg",
                "--image",
                "b.png",
                "helios://open?path=/x.jpg",
                "file:///photos/c%20d.jpg",
                dir.to_str().unwrap(),
                "--new-instance",
            ]),
            Some(cwd),
        );
        assert_eq!(
            got,
            vec![
                Open::Image("/photos/a.jpg".into()),
                Open::Image("/photos/b.png".into()),
                Open::Image("/photos/c d.jpg".into()),
                Open::Workspace(dir),
            ]
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn logs_and_crash_reports_go_to_the_state_directory() {
        const HOME: &[(&str, &str)] = &[("HOME", "/home/me")];
        const RELATIVE: &[(&str, &str)] = &[("XDG_STATE_HOME", "state"), ("HOME", "/home/me")];
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_string())
            }
        };
        let xdg = env(&[("XDG_STATE_HOME", "/state"), ("HOME", "/home/me")]);
        assert_eq!(log_dir_with(xdg), Some("/state/helios/logs".into()));
        assert_eq!(crash_dir_with(xdg), Some("/state/helios/crashes".into()));
        // Without XDG_STATE_HOME, or a relative one: the home's.
        for vars in [HOME, RELATIVE] {
            assert_eq!(
                log_dir_with(env(vars)),
                Some("/home/me/.local/state/helios/logs".into())
            );
        }
        assert_eq!(log_dir_with(env(&[])), None);
    }

    #[cfg(unix)]
    #[test]
    fn links_open_absolute_paths_only() {
        assert_eq!(
            link("helios://open?path=/a%20b.jpg"),
            Ok(Open::Image("/a b.jpg".into()))
        );
        assert_eq!(
            link("HELIOS://workspace?path=%2Fshoot"),
            Ok(Open::Workspace("/shoot".into()))
        );
        let tmp = std::env::temp_dir();
        assert_eq!(
            link(&format!("helios://open?path={}", tmp.display())),
            Ok(Open::Workspace(tmp))
        );
        assert!(link("helios://open?path=rel.jpg").is_err());
        assert!(link("helios://delete?path=/a.jpg").is_err());
        assert!(link("helios://open").is_err());
        assert!(link("other://open?path=/a.jpg").is_err());
        assert!(link("helios://open?path=/a%zz").is_err());
    }
}
