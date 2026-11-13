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

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(vars: &[(&str, &str)]) -> System {
        let vars: Vec<_> = vars
            .iter()
            .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
            .collect();
        System::from_env("linux", "x86_64", &vars, Some(PathBuf::from("/home/ann")))
    }

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
            &Context::command("x").cause("/home/ann/Photos/a.raw: unreadable, /home/anna kept"),
            &system,
        );
        assert_eq!(
            value(&lines, "error"),
            Some("~/Photos/a.raw: unreadable, /home/anna kept")
        );
        assert!(!format!("{lines:?}").contains("/home/ann/"));
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
}
