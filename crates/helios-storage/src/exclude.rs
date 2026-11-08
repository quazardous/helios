//! The image map's blacklist (`docs/design/image-map.md`, section 5): the
//! places Helios never records and never thumbnails into its cache, even
//! when the user opens them on purpose. A folder excluded takes its
//! subfolders with it.
//!
//! - the **defaults**, per OS ([`Os`]): Helios's own data, version control
//!   and build trees, hidden folders, NAS thumbnails, trash, other photo
//!   applications' caches and libraries, and the system's folders;
//! - folders tagged as caches by the Cache Directory Tagging spec (a
//!   `CACHEDIR.TAG` file), checked on disk by [`Checker`];
//! - the setting `system.map.exclude`: a JSON list of [`Rule`]s, each a
//!   glob over the path inside a volume (`volume` given) or the absolute
//!   path, `!` first to re-include what a default excludes. The last rule
//!   that matches wins; `system.map.defaults` off drops the defaults.
//!
//! [`Blacklist::excluded`] is pure: it reads no disk, so each OS's rules
//! are tested on every OS.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::VolumeId;

/// The setting that lists the user's rules.
pub const EXCLUDE: &str = "system.map.exclude";
/// The setting that turns the default rules on or off.
pub const DEFAULTS: &str = "system.map.defaults";

/// The first line of a `CACHEDIR.TAG` file, per the spec.
const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// The system whose default rules apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
    MacOs,
}

impl Os {
    /// The system Helios runs on (Linux rules elsewhere).
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// Whether paths are matched ignoring case.
    pub fn case_insensitive(self) -> bool {
        self != Self::Linux
    }
}

/// Why a default rule excludes a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Default {
    /// Helios's own data: the vault's folder, partial copies, its data and
    /// cache directories.
    Helios,
    /// A folder tagged `CACHEDIR.TAG`.
    CacheTag,
    /// Version control and build trees (`.git`, `node_modules`...).
    BuildTree,
    /// A hidden folder or file (its name starts with `.`).
    Hidden,
    /// NAS thumbnails (`@eaDir`, `.@__thumb`).
    NasThumbnails,
    /// A trash folder.
    Trash,
    /// Another photo application's cache or library.
    PhotoApp,
    /// The system's own folders (`/usr`, `C:\Windows`, `~/Library`...).
    System,
}

/// Why a place is excluded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Default(Default),
    /// A rule of `system.map.exclude`, by its glob.
    Rule(String),
}

/// One rule of `system.map.exclude`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// The glob; `!` first re-includes. `**` crosses folders, `*` and `?`
    /// stay in one.
    pub glob: String,
    /// The volume whose paths it matches; absent, it matches absolute
    /// paths on every volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
}

impl Rule {
    fn pattern(&self) -> (&str, bool) {
        match self.glob.strip_prefix('!') {
            Some(p) => (p, true),
            None => (&self.glob, false),
        }
    }
}

/// Reads the rules of `system.map.exclude` (a JSON list; empty text is no
/// rule).
pub fn parse_rules(text: &str) -> Result<Vec<Rule>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text).map_err(|e| format!("{EXCLUDE}: {e}"))
}

/// Where the user's folders are, for the default rules that name them.
#[derive(Clone, Debug, Default)]
pub struct Places {
    /// The home folder (`~`).
    pub home: Option<PathBuf>,
    /// Windows: `%APPDATA%` and `%LOCALAPPDATA%`.
    pub app_data: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    /// Windows: the system drive (`C:`).
    pub system_drive: Option<String>,
    /// Helios's data and cache directories, and the vault's folder.
    pub helios: Vec<PathBuf>,
}

impl Places {
    /// This machine's, read from the environment, with Helios's `dirs`.
    pub fn here(helios: Vec<PathBuf>) -> Self {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        Self {
            home: var("HOME")
                .or_else(|| var("USERPROFILE"))
                .map(PathBuf::from),
            app_data: var("APPDATA").map(PathBuf::from),
            local_app_data: var("LOCALAPPDATA").map(PathBuf::from),
            system_drive: var("SystemDrive").map(|v| v.to_string_lossy().into_owned()),
            helios,
        }
    }
}

/// The rules in force: the defaults of an OS (unless turned off) and the
/// user's.
#[derive(Clone, Debug)]
pub struct Blacklist {
    os: Os,
    defaults: bool,
    rules: Vec<Rule>,
    /// Absolute prefixes the defaults exclude, normalised.
    prefixes: Vec<(String, Default)>,
}

/// Folder names excluded wherever they are, and why.
const NAMES: &[(&str, Default)] = &[
    (".helios", Default::Helios),
    (".helios-part", Default::Helios),
    (".git", Default::BuildTree),
    (".hg", Default::BuildTree),
    (".svn", Default::BuildTree),
    ("node_modules", Default::BuildTree),
    ("__pycache__", Default::BuildTree),
    (".venv", Default::BuildTree),
    ("@eadir", Default::NasThumbnails),
    (".@__thumb", Default::NasThumbnails),
    (".trashes", Default::Trash),
    ("$recycle.bin", Default::Trash),
    ("system volume information", Default::System),
    (".spotlight-v100", Default::System),
    (".fseventsd", Default::System),
    (".documentrevisions-v100", Default::System),
];

impl Blacklist {
    /// The rules of `os`, with the user's `rules`; `defaults` false keeps
    /// only the user's.
    pub fn new(os: Os, places: &Places, defaults: bool, rules: Vec<Rule>) -> Self {
        let mut prefixes = Vec::new();
        let mut add = |p: &str, why| prefixes.push((normalise(p, os), why));
        for p in &places.helios {
            add(&p.to_string_lossy(), Default::Helios);
        }
        let home = places
            .home
            .as_ref()
            .map(|h| h.to_string_lossy().into_owned());
        match os {
            Os::Linux => {
                for p in [
                    "/proc", "/sys", "/dev", "/run", "/tmp", "/var", "/usr", "/etc", "/boot",
                    "/snap", "/opt",
                ] {
                    add(p, Default::System);
                }
                if let Some(h) = &home {
                    for p in [".cache", ".local/share", ".var"] {
                        add(&format!("{h}/{p}"), Default::System);
                    }
                }
            }
            Os::Windows => {
                let drive = places.system_drive.as_deref().unwrap_or("C:");
                for p in [
                    "Windows",
                    "Program Files",
                    "Program Files (x86)",
                    "ProgramData",
                ] {
                    add(&format!("{drive}\\{p}"), Default::System);
                }
                for p in [&places.app_data, &places.local_app_data]
                    .into_iter()
                    .flatten()
                {
                    add(&p.to_string_lossy(), Default::System);
                }
            }
            Os::MacOs => {
                for p in [
                    "/System",
                    "/Library",
                    "/Applications",
                    "/private",
                    "/usr",
                    "/opt",
                ] {
                    add(p, Default::System);
                }
                if let Some(h) = &home {
                    add(&format!("{h}/Library"), Default::System);
                }
            }
        }
        Self {
            os,
            defaults,
            rules,
            prefixes,
        }
    }

    /// This machine's rules: its OS, its places, and the settings' values
    /// (`exclude`, the JSON of `system.map.exclude`; `defaults`,
    /// `system.map.defaults`).
    pub fn here(helios: Vec<PathBuf>, defaults: bool, exclude: &str) -> Result<Self, String> {
        Ok(Self::new(
            Os::current(),
            &Places::here(helios),
            defaults,
            parse_rules(exclude)?,
        ))
    }

    /// Why `path` (absolute; inside `volume` at its path there, when known)
    /// is excluded, or `None`. Reads no disk: `CACHEDIR.TAG` is the
    /// [`Checker`]'s.
    pub fn excluded(&self, path: &Path, volume: Option<(&VolumeId, &Path)>) -> Option<Reason> {
        self.decide(path, volume, None)
    }

    /// The defaults' reason for `path`, if any.
    fn default_reason(&self, abs: &str) -> Option<Default> {
        if let Some((_, why)) = self.prefixes.iter().find(|(p, _)| under(abs, p)) {
            return Some(*why);
        }
        let lower = abs.to_lowercase();
        for name in lower.split('/').filter(|c| !c.is_empty()) {
            if let Some((_, why)) = NAMES.iter().find(|(n, _)| *n == name) {
                return Some(*why);
            }
            if name.starts_with(".trash-") || name == ".trash" {
                return Some(Default::Trash);
            }
            if name.ends_with(".lrdata")
                || name.ends_with(".photoslibrary")
                || name.starts_with("mipmaps-")
            {
                return Some(Default::PhotoApp);
            }
            if name.starts_with('.') && name != "." && name != ".." {
                return Some(Default::Hidden);
            }
        }
        // Capture One's cache, in a catalogue or a session.
        let parts: Vec<&str> = lower.split('/').collect();
        if parts
            .windows(2)
            .any(|w| w[1] == "cache" && (w[0].ends_with(".cocatalog") || w[0] == "captureone"))
        {
            return Some(Default::PhotoApp);
        }
        None
    }

    /// The defaults (with `tag`, a reason found on disk), then the user's
    /// rules, the last that matches winning.
    fn decide(
        &self,
        path: &Path,
        volume: Option<(&VolumeId, &Path)>,
        tag: Option<Default>,
    ) -> Option<Reason> {
        let abs = normalise(&path.to_string_lossy(), self.os);
        let rel = volume.map(|(id, rel)| (id, normalise(&rel.to_string_lossy(), self.os)));
        let mut reason = if self.defaults {
            self.default_reason(&abs).or(tag).map(Reason::Default)
        } else {
            None
        };
        for rule in &self.rules {
            let (pattern, include) = rule.pattern();
            let target = match (&rule.volume, &rel) {
                (None, _) => &abs,
                (Some(v), Some((id, rel))) if v == id.as_str() => rel,
                _ => continue,
            };
            let pattern = if self.os.case_insensitive() {
                pattern.to_lowercase()
            } else {
                pattern.to_string()
            };
            let pattern = pattern.replace('\\', "/");
            if matches_or_ancestor(&pattern, target) {
                reason = (!include).then(|| Reason::Rule(rule.glob.clone()));
            }
        }
        reason
    }
}

/// The blacklist with what it finds on disk: folders tagged
/// `CACHEDIR.TAG`, each folder read once.
#[derive(Debug)]
pub struct Checker {
    pub blacklist: Blacklist,
    tagged: HashMap<PathBuf, bool>,
}

impl Checker {
    pub fn new(blacklist: Blacklist) -> Self {
        Self {
            blacklist,
            tagged: HashMap::new(),
        }
    }

    /// Why `path` is excluded, `CACHEDIR.TAG` in it or a folder above
    /// included.
    pub fn excluded(&mut self, path: &Path, volume: Option<(&VolumeId, &Path)>) -> Option<Reason> {
        let tag = path
            .ancestors()
            .any(|dir| {
                *self
                    .tagged
                    .entry(dir.to_path_buf())
                    .or_insert_with(|| cache_tagged(dir))
            })
            .then_some(Default::CacheTag);
        self.blacklist.decide(path, volume, tag)
    }
}

/// Whether `dir` holds a `CACHEDIR.TAG` with the spec's signature.
pub fn cache_tagged(dir: &Path) -> bool {
    let Ok(file) = std::fs::File::open(dir.join("CACHEDIR.TAG")) else {
        return false;
    };
    let mut head = Vec::with_capacity(CACHEDIR_SIGNATURE.len());
    file.take(CACHEDIR_SIGNATURE.len() as u64)
        .read_to_end(&mut head)
        .is_ok()
        && head == CACHEDIR_SIGNATURE
}

/// A path as rules see it: `/` between folders, no trailing `/`, lower
/// case where the OS ignores case.
fn normalise(path: &str, os: Os) -> String {
    let mut p = if os == Os::Windows {
        path.replace('\\', "/")
    } else {
        path.to_string()
    };
    while p.len() > 1 && p.ends_with('/') {
        p.pop();
    }
    if os.case_insensitive() {
        p = p.to_lowercase();
    }
    p
}

/// Whether `path` is `prefix` or inside it.
fn under(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Whether `glob` matches `path` or a folder above it (a folder's rule
/// covers what it holds).
fn matches_or_ancestor(glob: &str, path: &str) -> bool {
    let glob: Vec<&str> = glob.split('/').collect();
    let parts: Vec<&str> = path.split('/').collect();
    (1..=parts.len()).any(|n| glob_parts(&glob, &parts[..n]))
}

/// Matches glob components against path components: `**` any number of
/// components, else one component by [`glob_name`].
fn glob_parts(glob: &[&str], parts: &[&str]) -> bool {
    match glob.split_first() {
        None => parts.is_empty(),
        Some((&"**", rest)) => (0..=parts.len()).any(|i| glob_parts(rest, &parts[i..])),
        Some((g, rest)) => match parts.split_first() {
            Some((p, prest)) => glob_name(g.as_bytes(), p.as_bytes()) && glob_parts(rest, prest),
            None => false,
        },
    }
}

/// Matches one component: `*` any run, `?` one character.
fn glob_name(g: &[u8], n: &[u8]) -> bool {
    match g.split_first() {
        None => n.is_empty(),
        Some((b'*', rest)) => (0..=n.len()).any(|i| glob_name(rest, &n[i..])),
        Some((b'?', rest)) => !n.is_empty() && glob_name(rest, &n[1..]),
        Some((c, rest)) => n.first() == Some(c) && glob_name(rest, &n[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places() -> Places {
        Places {
            home: Some("/home/ann".into()),
            app_data: Some(r"C:\Users\ann\AppData\Roaming".into()),
            local_app_data: Some(r"C:\Users\ann\AppData\Local".into()),
            system_drive: Some("C:".into()),
            helios: vec![
                "/home/ann/.local/share/helios".into(),
                "/srv/helios-data".into(),
            ],
        }
    }

    fn list(os: Os, rules: &str) -> Blacklist {
        Blacklist::new(os, &places(), true, parse_rules(rules).unwrap())
    }

    fn why(b: &Blacklist, p: &str) -> Option<Reason> {
        b.excluded(Path::new(p), None)
    }

    const D: fn(Default) -> Option<Reason> = |d| Some(Reason::Default(d));

    #[test]
    fn everywhere() {
        for os in [Os::Linux, Os::MacOs] {
            let b = list(os, "");
            assert_eq!(why(&b, "/photos/2026/a.cr3"), None);
            assert_eq!(why(&b, "/photos/.helios/thumbs/x.jpg"), D(Default::Helios));
            assert_eq!(why(&b, "/srv/helios-data/a.jpg"), D(Default::Helios));
            assert_eq!(why(&b, "/code/app/.git/a.png"), D(Default::BuildTree));
            assert_eq!(
                why(&b, "/code/app/node_modules/x/a.png"),
                D(Default::BuildTree)
            );
            assert_eq!(
                why(&b, "/nas/photos/@eaDir/a.jpg"),
                D(Default::NasThumbnails)
            );
            assert_eq!(why(&b, "/media/card/.Trash-1000/a.jpg"), D(Default::Trash));
            assert_eq!(why(&b, "/photos/.hidden/a.jpg"), D(Default::Hidden));
            assert_eq!(
                why(&b, "/photos/Cat Previews.lrdata/a/b.lrprev"),
                D(Default::PhotoApp)
            );
            assert_eq!(
                why(&b, "/photos/Photos Library.photoslibrary/originals/a.heic"),
                D(Default::PhotoApp)
            );
            assert_eq!(
                why(&b, "/s/Shoot/CaptureOne/Cache/a.jpg"),
                D(Default::PhotoApp)
            );
            assert_eq!(why(&b, "/photos/CaptureOne/a.jpg"), None);
        }
    }

    #[test]
    fn linux() {
        let b = list(Os::Linux, "");
        for p in [
            "/proc/1/x",
            "/usr/share/a.png",
            "/tmp/a.jpg",
            "/home/ann/.cache/x.jpg",
        ] {
            assert_eq!(why(&b, p), D(Default::System), "{p}");
        }
        assert_eq!(
            why(&b, "/home/ann/.local/share/Trash/a.jpg"),
            D(Default::System)
        );
        assert_eq!(why(&b, "/home/ann/Pictures/a.jpg"), None);
        assert_eq!(
            why(&b, "/usrdata/a.jpg"),
            None,
            "a prefix is a whole folder"
        );
        // Case counts on Linux.
        assert_eq!(why(&b, "/USR/a.jpg"), None);
    }

    #[test]
    fn windows() {
        let b = list(Os::Windows, "");
        for p in [
            r"C:\Windows\Web\a.jpg",
            r"c:\program files\App\a.png",
            r"C:\Program Files (x86)\a.png",
            r"C:\ProgramData\a.png",
            r"C:\Users\ann\AppData\Local\darktable\a.jpg",
            r"D:\System Volume Information\a.jpg",
        ] {
            assert_eq!(why(&b, p), D(Default::System), "{p}");
        }
        assert_eq!(why(&b, r"E:\$RECYCLE.BIN\a.jpg"), D(Default::Trash));
        assert_eq!(why(&b, r"C:\Users\ann\Pictures\a.jpg"), None);
    }

    #[test]
    fn macos() {
        let b = list(Os::MacOs, "");
        for p in [
            "/System/Library/a.png",
            "/Library/Desktop Pictures/a.heic",
            "/Applications/X.app/a.png",
            "/home/ann/Library/Caches/a.jpg",
            "/Volumes/CARD/.Spotlight-V100/a",
        ] {
            assert!(why(&b, p).is_some(), "{p}");
        }
        assert_eq!(
            why(&b, "/library/a.png"),
            D(Default::System),
            "case ignored"
        );
        assert_eq!(why(&b, "/home/ann/Pictures/a.jpg"), None);
    }

    #[test]
    fn rules_exclude_and_reinclude() {
        let b = list(
            Os::Linux,
            r#"[{"glob": "**/Private/**"},
                {"glob": "Archive/Clients/**", "volume": "uuid:5A3C"},
                {"glob": "!**/.photos/**"}]"#,
        );
        assert_eq!(
            why(&b, "/home/ann/Private/a.jpg"),
            Some(Reason::Rule("**/Private/**".into()))
        );
        // A glob without `/**` covers what the folder holds too.
        let c = list(Os::Linux, r#"[{"glob": "/data/sk?p"}]"#);
        assert!(why(&c, "/data/skip/a/b.jpg").is_some());
        assert_eq!(why(&c, "/data/skipped/a.jpg"), None);
        // `!` re-includes a default (a hidden folder).
        assert_eq!(why(&b, "/home/ann/.photos/a.jpg"), None);
        // A volume's rule matches the path inside it, on that volume only.
        let vol = VolumeId("uuid:5A3C".into());
        let other = VolumeId("uuid:FFFF".into());
        let p = Path::new("/media/disk/Archive/Clients/x/a.jpg");
        let rel = Path::new("Archive/Clients/x/a.jpg");
        assert!(b.excluded(p, Some((&vol, rel))).is_some());
        assert_eq!(b.excluded(p, Some((&other, rel))), None);
        assert_eq!(b.excluded(p, None), None);
        // Windows matches rules ignoring case.
        let w = list(Os::Windows, r#"[{"glob": "**/private/**"}]"#);
        assert!(why(&w, r"D:\PRIVATE\a.jpg").is_some());
    }

    #[test]
    fn defaults_turn_off() {
        let b = Blacklist::new(Os::Linux, &places(), false, Vec::new());
        assert_eq!(why(&b, "/usr/share/a.png"), None);
        assert_eq!(why(&b, "/p/.git/a.png"), None);
    }

    #[test]
    fn bad_rules_say_so() {
        assert!(parse_rules("{").is_err());
        assert_eq!(parse_rules("  ").unwrap(), Vec::new());
    }

    #[test]
    fn cache_tag_on_disk() {
        let dir = std::env::temp_dir().join(format!("helios-exclude-{}", std::process::id()));
        let tagged = dir.join("target");
        std::fs::create_dir_all(tagged.join("deep")).unwrap();
        std::fs::write(
            tagged.join("CACHEDIR.TAG"),
            b"Signature: 8a477f597d28d172789f06886806bc55\n# a cache\n",
        )
        .unwrap();
        let wrong = dir.join("fake");
        std::fs::create_dir_all(&wrong).unwrap();
        std::fs::write(wrong.join("CACHEDIR.TAG"), b"Signature: nope").unwrap();
        // The temporary folder is a Linux default: test under another OS's.
        let os = if cfg!(windows) {
            Os::Windows
        } else {
            Os::MacOs
        };
        let mut c = Checker::new(Blacklist::new(os, &Places::default(), true, Vec::new()));
        assert_eq!(
            c.excluded(&tagged.join("deep/a.jpg"), None),
            D(Default::CacheTag)
        );
        assert_eq!(c.excluded(&wrong.join("a.jpg"), None), None);
        // A `!` rule wins over the tag.
        let glob = format!("!{}/**", tagged.display());
        let mut c = Checker::new(Blacklist::new(
            os,
            &Places::default(),
            true,
            vec![Rule { glob, volume: None }],
        ));
        assert_eq!(c.excluded(&tagged.join("deep/a.jpg"), None), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
