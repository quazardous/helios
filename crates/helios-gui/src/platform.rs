//! Helios on the desktop's platform services (egui-pop's `platform`).
//!
//! - **One instance**: a second `helios-gui` launch hands its arguments to
//!   the running one and exits (`instance::single`, called by `main`). The
//!   running one comes to the front and opens what it was given: an image
//!   in the viewport, a workspace in a window of its own (one process per
//!   workspace, started with [`NEW_INSTANCE`]). `helios://` links take the
//!   same path ([`link`]).
//!
//! The [`Platform`] plugin is part of every shell: its hooks on the
//! instance's steps do nothing where no instance runs (tests, pictures,
//! tools). The desktop adds egui-pop's services with [`desktop`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::{AppBuilder, Ctx, Plugin, PluginError};
use egui_pop::model::ids::PluginId;
use egui_pop::platform::instance::{Instance, InstancePlugin, STEP_DEEPLINK, STEP_SECOND_LAUNCH};
use serde_json::{Value as Json, json};

/// Helios's id on the desktop: its desktop file's name
/// (`packaging/linux`), the single instance's socket.
pub const APP_ID: &str = "io.github.quazardous.Helios";

/// The scheme of Helios's links: `helios://open?path=...`.
pub const SCHEME: &str = "helios";

/// Starts a window of its own instead of handing over to the running
/// Helios (a workspace opened from the running one).
pub const NEW_INSTANCE: &str = "--new-instance";

pub const PLUGIN: PluginId = PluginId::from_static("helios.platform");

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
const VALUED: [&str; 1] = ["--vault"];

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
/// instance (its launches handed to [`Platform`]).
pub fn desktop(app: AppBuilder, instance: Instance) -> AppBuilder {
    app.plugin(InstancePlugin::new(instance).scheme(SCHEME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
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
