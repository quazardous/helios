//! What a dialog carries for a bug report: the details its Copy button
//! appends to the copied text (egui-pop's `Info::details`,
//! `Confirm::details`), hidden on a notice or a question, shown collapsed
//! on an error.
//!
//! One helper, [`details`], fills them: Helios's version and commit, the
//! system and its display server, then what the dialog is about (the
//! command or action that failed, the error's causes, the plugin, the
//! job). Never a secret: no environment variable is copied, and any
//! value of one that looks like a secret (a token, a client id, a key) is
//! blanked wherever it appears; the home folder is written `~`.

use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use crate::library::{Library, Restriction};

/// What a dialog is about, for its details.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context {
    /// The command or action id that ran (or failed).
    pub command: Option<String>,
    /// The error, then what caused it, outermost first.
    pub causes: Vec<String>,
    /// The plugin involved: its id and version.
    pub plugin: Option<(String, String)>,
    /// The job involved.
    pub job: Option<String>,
}

impl Context {
    /// About a command or an action.
    pub fn command(id: impl Into<String>) -> Self {
        Self {
            command: Some(id.into()),
            ..Self::default()
        }
    }

    /// One more cause, after those already given.
    #[must_use]
    pub fn cause(mut self, cause: impl Into<String>) -> Self {
        let cause = cause.into();
        if !cause.is_empty() {
            self.causes.push(cause);
        }
        self
    }

    /// An error and its sources, outermost first.
    #[must_use]
    pub fn error(mut self, e: &dyn std::error::Error) -> Self {
        let mut next = Some(e);
        while let Some(e) = next {
            self = self.cause(e.to_string());
            next = e.source();
        }
        self
    }

    #[must_use]
    pub fn plugin(mut self, id: impl Into<String>, version: impl Into<String>) -> Self {
        self.plugin = Some((id.into(), version.into()));
        self
    }

    #[must_use]
    pub fn job(mut self, id: impl Into<String>) -> Self {
        self.job = Some(id.into());
        self
    }
}

/// The details for a dialog about `context`, read from this process's
/// environment.
pub fn details(context: &Context) -> Vec<(String, String)> {
    details_with(context, &System::current())
}

/// What [`details`] reads of the system: given for tests.
#[derive(Clone, Debug, Default)]
pub struct System {
    /// `linux`, `windows`, `macos`…
    pub os: String,
    pub arch: String,
    /// Wayland, X11, Windows, macOS, or empty when unknown.
    pub display: String,
    /// Written `~`.
    pub home: Option<PathBuf>,
    /// Values never written, wherever they appear.
    pub secrets: Vec<String>,
}

impl System {
    /// This process's.
    pub fn current() -> Self {
        let vars: Vec<(String, String)> = std::env::vars().collect();
        Self::from_env(
            std::env::consts::OS,
            std::env::consts::ARCH,
            &vars,
            std::env::home_dir(),
        )
    }

    /// From an environment's variables.
    pub fn from_env(
        os: &str,
        arch: &str,
        vars: &[(String, String)],
        home: Option<PathBuf>,
    ) -> Self {
        let var = |name: &str| {
            vars.iter()
                .find(|(n, v)| n == name && !v.is_empty())
                .map(|(_, v)| v.as_str())
        };
        let display = match os {
            "windows" => "Windows".to_string(),
            "macos" => "macOS".to_string(),
            _ => match var("XDG_SESSION_TYPE") {
                Some("wayland") => "Wayland".to_string(),
                Some("x11") => "X11".to_string(),
                _ if var("WAYLAND_DISPLAY").is_some() => "Wayland".to_string(),
                _ if var("DISPLAY").is_some() => "X11".to_string(),
                _ => String::new(),
            },
        };
        let secrets = vars
            .iter()
            .filter(|(n, v)| secret_name(n) && v.trim().len() >= 8)
            .map(|(_, v)| v.trim().to_string())
            .collect();
        Self {
            os: os.to_string(),
            arch: arch.to_string(),
            display,
            home: home.filter(|h| h.is_absolute() && h.parent().is_some()),
            secrets,
        }
    }
}

/// A variable whose value is a secret, or close enough to one never to be
/// copied: tokens, client ids and secrets, keys, passwords.
fn secret_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    [
        "TOKEN",
        "SECRET",
        "CLIENT_ID",
        "PASSWORD",
        "PASSWD",
        "KEY",
        "CREDENTIAL",
        "AUTH",
    ]
    .iter()
    .any(|s| n.contains(s))
}

/// The details for `context` on `system`.
pub fn details_with(context: &Context, system: &System) -> Vec<(String, String)> {
    let version = env!("HELIOS_VERSION");
    let commit = env!("HELIOS_COMMIT");
    let mut out = vec![("helios".to_string(), version.to_string())];
    if !commit.is_empty() {
        out.push(("commit".into(), commit.into()));
    }
    out.push(("os".into(), format!("{} {}", system.os, system.arch)));
    if !system.display.is_empty() {
        out.push(("display".into(), system.display.clone()));
    }
    if let Some(c) = &context.command {
        out.push(("command".into(), c.clone()));
    }
    if let Some((id, v)) = &context.plugin {
        out.push(("plugin".into(), format!("{id} {v}")));
    }
    if let Some(j) = &context.job {
        out.push(("job".into(), j.clone()));
    }
    for (i, cause) in context.causes.iter().enumerate() {
        let key = if i == 0 { "error" } else { "caused by" };
        out.push((key.into(), cause.clone()));
    }
    out.into_iter()
        .map(|(k, v)| (k, redact(&v, system)))
        .collect()
}

/// `text` with the home folder written `~` and no secret of `system`'s.
pub fn redact(text: &str, system: &System) -> String {
    let mut out = text.replace(['\n', '\r'], " ");
    for s in &system.secrets {
        out = out.replace(s.as_str(), "[redacted]");
    }
    if let Some(home) = &system.home {
        out = redact_home(&out, home);
    }
    out
}

fn redact_home(text: &str, home: &Path) -> String {
    let home = home.to_string_lossy();
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return text.to_string();
    }
    // Only the whole folder: `/home/ann` in `/home/ann/x`, not in
    // `/home/anna`.
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(home) {
        let after = &rest[at + home.len()..];
        out.push_str(&rest[..at]);
        if after.is_empty() || after.starts_with(['/', '\\', ' ', '"', '\'', ')', ',', ':']) {
            out.push('~');
        } else {
            out.push_str(home);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Whether a token of a problem report is an image id (a file's content
/// hash, 64 hexadecimal digits): egui-pop's report replaces each by an
/// alias, `photo-1`, `photo-2`… (`AboutPlugin::alias`).
pub fn is_image_id(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The library's shape, for a problem report (`AboutPlugin::context`):
/// how many photos, of which kinds (raw, JPEG, other, raw+JPEG pairs by
/// their names' stems), stacked, edited, offline or missing; how many the
/// views show, picked, rejected and selected; what limits the view (an
/// album, a folder, an import, a set) and the filter's terms. Never a
/// name, a path or a term's value; the open photo by its id, which the
/// report aliases.
pub fn library_shape(lib: &Library, ctx: &egui::Context) -> Json {
    let (mut raw, mut jpeg, mut other) = (0, 0, 0);
    let (mut stacked, mut edited, mut offline, mut missing) = (0, 0, 0, 0);
    let mut stems: std::collections::HashMap<String, (bool, bool)> =
        std::collections::HashMap::new();
    for e in &lib.entries {
        let path = Path::new(&e.name);
        let ext = path
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let stem = path
            .file_stem()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_string();
        let kind = stems.entry(stem).or_default();
        if helios_load::Kind::of(path) == helios_load::Kind::Raw {
            raw += 1;
            kind.0 = true;
        } else if matches!(ext.as_str(), "jpg" | "jpeg") {
            jpeg += 1;
            kind.1 = true;
        } else {
            other += 1;
        }
        stacked += usize::from(e.stack.is_some());
        edited += usize::from(e.edited);
        match e.presence {
            helios_vault::Presence::Offline => offline += 1,
            helios_vault::Presence::Missing => missing += 1,
            helios_vault::Presence::Present => {}
        }
    }
    let pairs = stems.values().filter(|(r, j)| *r && *j).count();
    let (shown, picked, rejected) = lib.counts();
    let limits: Vec<&str> = lib
        .restrictions(ctx)
        .into_iter()
        .map(|(_, r)| match r {
            Restriction::Album => "album",
            Restriction::Folder => "folder",
            Restriction::Import => "import",
            Restriction::Set => "set",
        })
        .collect();
    json!({
        "photos": lib.entries.len(),
        "raw": raw,
        "jpeg": jpeg,
        "other": other,
        "raw_jpeg_pairs": pairs,
        "stacked": stacked,
        "edited": edited,
        "offline": offline,
        "missing": missing,
        "shown": shown,
        "picked": picked,
        "rejected": rejected,
        "selected": lib.selection().len(),
        "limited_to": limits,
        "filter_terms": filter_terms(lib.filter()),
        "open": lib.selected().and_then(|i| lib.entries.get(i)).map(|e| e.id.clone()),
    })
}

/// A filter's criteria without their private values: the stars, how many
/// labels, flags and tags, whether it has dates; the text's terms by their
/// key alone (`folder`), and how many free words it has. Never a tag, a
/// day or a term's value, which may name a place or a person.
fn filter_terms(filter: &egui_pop::views::library::filter::Filter) -> Json {
    let mut keys: Vec<String> = Vec::new();
    let mut words = 0;
    for token in filter.text.split_whitespace() {
        match token.split_once(':') {
            Some((key, _))
                if !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '!') =>
            {
                keys.push(key.to_ascii_lowercase());
            }
            _ => words += 1,
        }
    }
    json!({
        "keys": keys,
        "words": words,
        "min_rating": filter.min_rating,
        "labels": filter.labels.len(),
        "flags": filter.flags.len(),
        "tags": filter.tags.len(),
        "dates": filter.from.is_some() || filter.to.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(vars: &[(&str, &str)]) -> System {
        let vars: Vec<_> = vars
            .iter()
            .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
            .collect();
        System::from_env("linux", "x86_64", &vars, Some(PathBuf::from(HOME)))
    }

    /// An absolute home folder on the platform running the test (a home
    /// that is not absolute is ignored).
    #[cfg(not(windows))]
    const HOME: &str = "/home/ann";
    #[cfg(windows)]
    const HOME: &str = r"C:\Users\ann";

    fn value<'a>(lines: &'a [(String, String)], key: &str) -> Option<&'a str> {
        lines
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn the_details_name_the_version_the_system_and_the_command() {
        let system = linux(&[("XDG_SESSION_TYPE", "wayland")]);
        let lines = details_with(
            &Context::command("library.export")
                .cause("no room left")
                .cause("disk full")
                .plugin("helios.export.web", "1.2.0")
                .job("17"),
            &system,
        );
        assert_eq!(value(&lines, "helios"), Some(env!("HELIOS_VERSION")));
        assert_eq!(value(&lines, "os"), Some("linux x86_64"));
        assert_eq!(value(&lines, "display"), Some("Wayland"));
        assert_eq!(value(&lines, "command"), Some("library.export"));
        assert_eq!(value(&lines, "plugin"), Some("helios.export.web 1.2.0"));
        assert_eq!(value(&lines, "job"), Some("17"));
        assert_eq!(value(&lines, "error"), Some("no room left"));
        assert_eq!(value(&lines, "caused by"), Some("disk full"));
    }

    #[test]
    fn the_display_server_is_read_from_the_session() {
        assert_eq!(linux(&[("DISPLAY", ":0")]).display, "X11");
        assert_eq!(
            linux(&[("WAYLAND_DISPLAY", "wayland-0")]).display,
            "Wayland"
        );
        assert_eq!(linux(&[]).display, "");
        let w = System::from_env("windows", "x86_64", &[], None);
        assert_eq!(w.display, "Windows");
    }

    #[test]
    fn the_home_folder_is_written_as_a_tilde() {
        let system = linux(&[]);
        let lines = details_with(
            &Context::command("x").cause(format!("{HOME}/Photos/a.raw: unreadable, {HOME}a kept")),
            &system,
        );
        assert_eq!(
            value(&lines, "error"),
            Some(format!("~/Photos/a.raw: unreadable, {HOME}a kept").as_str())
        );
        assert!(!format!("{lines:?}").contains(&format!("{HOME}/")));
    }

    #[test]
    fn a_windows_home_folder_is_written_as_a_tilde() {
        let home = Path::new(r"C:\Users\ann");
        assert_eq!(
            redact_home(
                r"C:\Users\ann\Pictures\a.raw: unreadable, C:\Users\anna kept",
                home
            ),
            r"~\Pictures\a.raw: unreadable, C:\Users\anna kept"
        );
        assert_eq!(
            redact_home(r"in C:\Users\ann", Path::new(r"C:\Users\ann\")),
            "in ~"
        );
    }

    #[test]
    fn a_secret_in_the_environment_never_appears() {
        let system = linux(&[
            ("HELIOS_GOOGLE_PHOTOS_CLIENT_ID", "1234-abcd.apps.example"),
            ("SOME_TOKEN", "tok-5678"),
        ]);
        let lines = details_with(
            &Context::command("accounts.connect")
                .cause("refused for client 1234-abcd.apps.example with tok-5678"),
            &system,
        );
        let text = format!("{lines:?}");
        assert!(!text.contains("1234-abcd"), "{text}");
        assert!(!text.contains("tok-5678"), "{text}");
        assert!(text.contains("[redacted]"), "{text}");
    }

    #[test]
    fn image_ids_are_64_hex_digits() {
        assert!(is_image_id(&"a1".repeat(32)));
        assert!(!is_image_id(&"a1".repeat(31)));
        assert!(!is_image_id(&"zz".repeat(32)));
    }

    /// A library of five photos: a raw+JPEG pair named after a person,
    /// a stacked and edited raw, an offline JPEG, a missing PNG.
    fn fixture() -> Library {
        use crate::library::{Entry, StackOf};
        use helios_vault::Presence;
        let id = |c: char| c.to_string().repeat(64);
        let mut lib = Library::default();
        lib.set_entries(vec![
            Entry {
                id: id('a'),
                name: "Jane wedding 001.CR2".into(),
                picked: true,
                ..Entry::default()
            },
            Entry {
                id: id('b'),
                name: "Jane wedding 001.JPG".into(),
                picked: true,
                ..Entry::default()
            },
            Entry {
                id: id('c'),
                name: "Ann at home.NEF".into(),
                rejected: true,
                edited: true,
                stack: Some(StackOf {
                    id: "s1".into(),
                    top: id('c'),
                    count: 1,
                }),
                ..Entry::default()
            },
            Entry {
                id: id('d'),
                name: "beach.jpeg".into(),
                presence: Presence::Offline,
                offline_on: Some("Ann's disk".into()),
                ..Entry::default()
            },
            Entry {
                id: id('e'),
                name: "scan.png".into(),
                presence: Presence::Missing,
                ..Entry::default()
            },
        ]);
        lib.select(0);
        lib
    }

    #[test]
    fn the_library_shape_counts_a_fixture() {
        let lib = fixture();
        let shape = library_shape(&lib, &egui::Context::default());
        let n = |key: &str| {
            shape[key]
                .as_u64()
                .unwrap_or_else(|| panic!("{key}: {shape}"))
        };
        assert_eq!(n("photos"), 5);
        assert_eq!(n("raw"), 2);
        assert_eq!(n("jpeg"), 2);
        assert_eq!(n("other"), 1);
        assert_eq!(n("raw_jpeg_pairs"), 1);
        assert_eq!(n("stacked"), 1);
        assert_eq!(n("edited"), 1);
        assert_eq!(n("offline"), 1);
        assert_eq!(n("missing"), 1);
        assert_eq!(n("shown"), 5);
        assert_eq!(n("picked"), 2);
        assert_eq!(n("rejected"), 1);
        assert_eq!(n("selected"), 1);
        assert_eq!(shape["limited_to"], json!([]));
        // The open photo by its id, which the report aliases.
        assert_eq!(shape["open"], json!("a".repeat(64)));
    }

    #[test]
    fn the_library_shape_holds_no_name_nor_path() {
        let mut lib = fixture();
        lib.set_filter(egui_pop::views::library::filter::Filter::parse(
            "folder:\"/home/ann/Wedding\" tag:Jane rating:3 beach",
        ));
        let shape = library_shape(&lib, &egui::Context::default());
        let text = shape.to_string();
        for leak in [
            "Jane", "Ann", "ann", "wedding", "Wedding", "beach", "scan", "CR2", "NEF", ".jpeg",
            "home", "/", "disk",
        ] {
            assert!(!text.contains(leak), "{leak:?} in {text}");
        }
        let terms = &shape["filter_terms"];
        assert_eq!(terms["keys"], json!(["folder"]));
        assert_eq!(terms["words"], 1);
        assert_eq!(terms["tags"], 1);
        assert_eq!(terms["min_rating"], 3);
    }

    #[test]
    fn a_filter_gives_its_keys_never_its_values() {
        let t = filter_terms(&egui_pop::views::library::filter::Filter::parse(
            "folder:\"/home/ann/Wedding\" kind:raw rating:3 tag:jane date:2026-01-01.. jane",
        ));
        assert_eq!(t["keys"], json!(["folder", "kind"]));
        assert_eq!(t["words"], 1);
        assert_eq!(t["min_rating"], 3);
        assert_eq!(t["tags"], 1);
        assert_eq!(t["dates"], true);
        assert!(!t.to_string().contains("2026"));
        assert!(!t.to_string().contains("ann"));
        assert!(!t.to_string().contains("jane"));
    }
}
