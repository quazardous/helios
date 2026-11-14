//! `helios-plugin.toml`: what a plugin is and what it asks for, read and
//! checked before anything of it runs.
//!
//! ```toml
//! id = "org.example.exif"
//! name = "ExifTool metadata"
//! title = "ExifTool metadata"
//! description = "Reads the metadata of images with exiftool"
//! version = "0.1.0"
//! license = "GPL-3.0-or-later"
//! command = ["python3", "exif_exiftool.py"]
//! services = ["metadata.read"]
//! activation = ["onService:metadata.read"]
//!
//! [compat]
//! helios = ">=0.0.1, <0.1.0"
//! interfaces = { protocol = 1, "metadata.read" = 1 }
//!
//! [settings.exiftool]
//! type = "string"
//! default = "exiftool"
//! label = "Program"
//! help = "The exiftool program to run"
//!
//! [[capabilities]]
//! name = "images.read"
//! reason = "Reads the metadata of the images Helios asks about"
//! ```
//!
//! `title` and `description` are what an interface shows of the plugin, in
//! English; the plugin translates them, and its commands' titles, in its
//! own catalogues beside the manifest (`i18n/<locale>/*.ftl`, keys
//! `plugin-<id>.title`, `plugin-<id>.description`,
//! `command-<command id>.title`), which an interface loads with
//! `helios_i18n::plugins`.
//!
//! `[settings.<key>]` is the plugin's configuration contract
//! ([`helios_core::config`]); a plugin that makes accounts or reads
//! sources declares theirs as `[account.settings.<key>]` and
//! `[source.settings.<key>]`. Their labels and help are translated as
//! `setting-<id>-<key>.label` / `.help` (`account-setting-…`,
//! `source-setting-…`). A declaration with a problem, a default that does
//! not fit its key among them, refuses the plugin.
//!
//! `[[health]]` declares the plugin's problem checks, through the health
//! contract ([`helios_health::Declaration`]): each its id, its title,
//! what it watches, the levels it may raise, the fix commands it may
//! offer and when it runs (`every_minutes`, `on`). Helios runs them through the plugin's process (`health.check`,
//! the `health` interface, started on `onHealth`: [`crate::health`]); an
//! unsound declaration refuses the plugin, like a bad setting.
//!
//! ```toml
//! [[health]]
//! id = "accounts"
//! title = "Google Photos accounts"
//! watches = "Each Google Photos account still signs in"
//! levels = ["ok", "warning"]
//! fixes = ["import.reconnect"]
//! every_minutes = 360
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use helios_core::config::{ConfigKey, Contract, Contracts, KeyType};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// The file a plugin directory holds.
pub const MANIFEST: &str = "helios-plugin.toml";

/// Interfaces this Helios speaks, with the versions it accepts.
pub const INTERFACES: &[(&str, &[u32])] = &[
    ("protocol", &[1]),
    ("metadata.read", &[1]),
    ("device", &[1]),
    ("importer", &[1, 2]),
    ("import.source", &[1]),
    ("health", &[1]),
];

/// The services a plugin may provide; each is also an interface.
pub const SERVICES: &[&str] = &["metadata.read", "device", "importer", "import.source"];

/// How the broker holds a capability to its word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Enforcement {
    /// The broker controls the access: checked on every call.
    Brokered,
    /// The plugin's process could use it on its own: the broker refuses to
    /// start the plugin until the user grants it. Holding the process to
    /// it needs isolation (namespaces, portals), which comes later.
    Ambient,
}

/// Every capability a manifest may ask for; any other is refused.
pub const CAPABILITIES: &[(&str, Enforcement, &str)] = &[
    (
        "images.read",
        Enforcement::Brokered,
        "read the images Helios asks about, and only those",
    ),
    (
        "source.read",
        Enforcement::Brokered,
        "read the file or folder the user picks for an import, and only that",
    ),
    (
        "folder.write",
        Enforcement::Brokered,
        "write into a folder Helios hands over for one call",
    ),
    ("process.spawn", Enforcement::Ambient, "run other programs"),
    (
        "device.usb",
        Enforcement::Ambient,
        "talk to devices (cameras, card readers) over USB",
    ),
    ("network", Enforcement::Ambient, "reach the network"),
    (
        "secrets",
        Enforcement::Ambient,
        "read stored keys and tokens",
    ),
    ("gpu", Enforcement::Ambient, "use the GPU"),
    ("ai.models", Enforcement::Ambient, "run AI models"),
];

pub fn capability(name: &str) -> Option<(Enforcement, &'static str)> {
    CAPABILITIES
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|(_, e, about)| (*e, *about))
}

/// A version, `major.minor.patch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    pub fn parse(s: &str) -> Result<Version, String> {
        let bad = || format!("`{s}` is not a version (major.minor.patch)");
        let parts: Vec<u64> = s
            .trim()
            .split('.')
            .map(|p| p.parse().map_err(|_| bad()))
            .collect::<Result<_, _>>()?;
        match parts[..] {
            [a, b, c] => Ok(Version(a, b, c)),
            _ => Err(bad()),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Comparators joined by commas: `>=0.0.1, <0.1.0`; a bare version means
/// exactly that one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionReq(Vec<(String, Version)>);

impl VersionReq {
    pub fn parse(s: &str) -> Result<VersionReq, String> {
        let mut all = Vec::new();
        for part in s.split(',') {
            let part = part.trim();
            let op = [">=", "<=", ">", "<", "="]
                .into_iter()
                .find(|op| part.starts_with(op))
                .unwrap_or("");
            let v = Version::parse(&part[op.len()..])
                .map_err(|e| format!("version requirement `{s}`: {e}"))?;
            all.push((if op.is_empty() { "=" } else { op }.to_string(), v));
        }
        Ok(VersionReq(all))
    }

    pub fn matches(&self, v: Version) -> bool {
        self.0.iter().all(|(op, r)| match op.as_str() {
            ">=" => v >= *r,
            "<=" => v <= *r,
            ">" => v > *r,
            "<" => v < *r,
            _ => v == *r,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    /// The Helios versions the plugin works with.
    pub helios: String,
    /// Each interface the plugin uses, with its version.
    pub interfaces: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDecl {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub category: String,
}

/// What the user picks for an importer: a file or a folder.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    #[default]
    Folder,
    File,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Folder => "folder",
            SourceKind::File => "file",
        }
    }
}

/// An importer a plugin contributes: an entry under File > Import from,
/// which asks for a source and has the plugin list what to import there
/// (`importer.scan`); Helios imports the files itself.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImporterDecl {
    /// Unique in the plugin: `ratings-folder`.
    pub id: String,
    /// The menu entry, in English; translated in the plugin's catalogues
    /// as `<plugin id>-importer-<importer id>` (both slugged).
    pub label: String,
    /// What the user picks.
    #[serde(default)]
    pub source: SourceKind,
    /// A file source: the names it accepts, `*` matching any run of
    /// characters (`library.db`, `*.lrcat`); none: any file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<String>,
    /// A folder source: names the folder must hold, checked before the
    /// plugin starts (`library.db`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holds: Vec<String>,
    /// The images may lie outside the source (a catalogue naming them
    /// where they are); otherwise any outside it is refused.
    #[serde(default)]
    pub anywhere: bool,
}

/// Whether `name` matches a pattern whose `*` matches any run of
/// characters, case ignored.
pub fn glob(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.to_lowercase(), name.to_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    let [first, middle @ .., last] = parts.as_slice() else {
        return p == n;
    };
    if parts.len() == 1 {
        return p == n;
    }
    let Some(mut rest) = n.strip_prefix(first) else {
        return false;
    };
    for m in middle {
        match rest.find(m) {
            Some(i) => rest = &rest[i + m.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

impl ImporterDecl {
    /// Whether a file of this name is a source this importer reads.
    pub fn accepts(&self, name: &str) -> bool {
        self.patterns.is_empty() || self.patterns.iter().any(|p| glob(p, name))
    }

    /// The extensions its patterns give (`*.lrcat`: `lrcat`), for a
    /// file picker; none when a pattern is not of that form.
    pub fn extensions(&self) -> Vec<String> {
        let ext: Vec<String> = self
            .patterns
            .iter()
            .filter_map(|p| p.strip_prefix("*."))
            .filter(|e| !e.contains('*') && !e.contains('.'))
            .map(str::to_lowercase)
            .collect();
        if ext.len() == self.patterns.len() {
            ext
        } else {
            Vec::new()
        }
    }
}

/// One step of a source's connection process (`[[connect]]`), which
/// Helios drives itself: `oauth`, `form`, `device`, `trust`, `code` or
/// `test`, the step's own keys beside (`helios_connect::ConnectStep`
/// reads them).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConnectDecl {
    pub id: String,
    pub kind: String,
    #[serde(flatten)]
    pub rest: BTreeMap<String, Json>,
}

/// The step kinds Helios drives.
pub const CONNECT_KINDS: &[&str] = &["oauth", "form", "device", "trust", "code", "test"];

/// A problem check the plugin declares (`[[health]]`): its id within the
/// plugin, its title and its health contract.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HealthDecl {
    /// Unique in the plugin: `accounts`. Helios runs it as
    /// `<plugin id>.<id>`.
    pub id: String,
    /// Its name for people, in English.
    pub title: String,
    /// What it watches, in English (one sentence).
    pub watches: String,
    /// The levels its findings may have: `ok`, `warning`, `problem`.
    pub levels: Vec<helios_health::Level>,
    /// The commands its findings' fixes may run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<String>,
    /// How often it runs again, in minutes (at least 5); none: Helios's
    /// own period (daily).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_minutes: Option<u32>,
    /// The events that run it at once besides its clock: `volumes` (a
    /// volume came or went).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<helios_health::Event>,
}

impl HealthDecl {
    /// Its health contract.
    pub fn declaration(&self) -> helios_health::Declaration {
        helios_health::Declaration {
            watches: self.watches.clone(),
            levels: self.levels.clone(),
            fixes: self.fixes.clone(),
            every_minutes: self.every_minutes,
            on: self.on.clone(),
        }
    }
}

/// The event that starts a plugin to run one of its checks.
pub const ON_HEALTH: &str = "onHealth";

/// A setting's type: the configuration contract's ([`helios_core::config`]).
pub type SettingType = KeyType;

/// A setting: a key of the configuration contract.
pub type SettingDecl = ConfigKey;

/// What each account or each source the plugin creates is configured by:
/// `[account.settings.<key>]`, `[source.settings.<key>]`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceDecl {
    #[serde(default)]
    pub settings: Contract,
}

impl InstanceDecl {
    fn is_empty(&self) -> bool {
        self.settings.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDecl {
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Reverse domain: `org.example.exif`.
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// What an interface calls the plugin, in English; `name` when absent.
    #[serde(default)]
    pub title: String,
    /// What it does, in a sentence, in English.
    #[serde(default)]
    pub description: String,
    pub version: String,
    pub license: String,
    /// The program and its arguments, run in the plugin's directory. A
    /// program found in that directory is run from there, any other is
    /// looked up on `PATH` (`python3`).
    pub command: Vec<String>,
    pub compat: Compat,
    #[serde(default)]
    pub services: Vec<String>,
    /// `onStartup`, `onService:<service>`, `onCommand:<command id>`,
    /// `onImporter:<importer id>`.
    #[serde(default)]
    pub activation: Vec<String>,
    #[serde(default)]
    pub commands: Vec<CommandDecl>,
    /// Importers, under File > Import from (the `importer` service).
    #[serde(default)]
    pub importers: Vec<ImporterDecl>,
    /// A source's connection process (the `import.source` service).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connect: Vec<ConnectDecl>,
    /// How the import window browses its sources (`tree`, `folder`,
    /// `collections`, `timeline`, `picker`), in the order offered; empty:
    /// `picker` (the `import.source` service picks).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub browsers: Vec<String>,
    /// Its source gives a preview of an item without fetching it
    /// (`source.thumbnail`): the import window's glances.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub thumbnails: bool,
    /// What its service lets Helios ask (`[limits]`: `per_minute`,
    /// `per_day`): the read-ahead of glances pauses there.
    #[serde(default, skip_serializing_if = "no_limits")]
    pub limits: helios_connect::Limits,
    /// The plugin's own settings, each under its id (`<id>.<key>`): the
    /// configuration contract's first level.
    #[serde(default)]
    pub settings: Contract,
    /// Each account's settings (`[account.settings.<key>]`).
    #[serde(default, skip_serializing_if = "InstanceDecl::is_empty")]
    pub account: InstanceDecl,
    /// Each source's settings (`[source.settings.<key>]`).
    #[serde(default, skip_serializing_if = "InstanceDecl::is_empty")]
    pub source: InstanceDecl,
    #[serde(default)]
    pub capabilities: Vec<CapabilityDecl>,
    /// Its problem checks (`[[health]]`), run through its process.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub health: Vec<HealthDecl>,
}

fn no_limits(limits: &helios_connect::Limits) -> bool {
    *limits == helios_connect::Limits::default()
}

fn reverse_domain(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| {
            p.starts_with(|c: char| c.is_ascii_lowercase())
                && p.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let m: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
        Ok(m)
    }

    pub fn read(dir: &Path) -> Result<Manifest, String> {
        let path = dir.join(MANIFEST);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Manifest::parse(&text)
    }

    /// Everything that keeps this plugin from running with this Helios,
    /// `helios` being its version. Empty: the manifest is accepted.
    pub fn problems(&self, helios: Version) -> Vec<String> {
        let mut p = Vec::new();
        if !reverse_domain(&self.id) {
            p.push(format!(
                "id `{}` is not a reverse domain name (`org.example.name`)",
                self.id
            ));
        }
        if let Err(e) = Version::parse(&self.version) {
            p.push(format!("version: {e}"));
        }
        if self.license.trim().is_empty() {
            p.push("no license".into());
        }
        match self.command.first().map(Path::new) {
            None => p.push("no command to start the plugin".into()),
            Some(c)
                if c.is_absolute()
                    || c.components()
                        .any(|x| matches!(x, std::path::Component::ParentDir)) =>
            {
                p.push(format!(
                    "command `{}`: give a program in the plugin's directory or on PATH",
                    c.display()
                ));
            }
            Some(_) => {}
        }
        match VersionReq::parse(&self.compat.helios) {
            Ok(req) if !req.matches(helios) => p.push(format!(
                "works with Helios {}, this is {helios}",
                self.compat.helios
            )),
            Ok(_) => {}
            Err(e) => p.push(e),
        }
        for (name, v) in &self.compat.interfaces {
            match INTERFACES.iter().find(|(n, _)| n == name) {
                None => p.push(format!("unknown interface `{name}`")),
                Some((_, ok)) if !ok.contains(v) => p.push(format!(
                    "interface `{name}` version {v} is not supported (supported: {ok:?})"
                )),
                Some(_) => {}
            }
        }
        if !self.compat.interfaces.contains_key("protocol") {
            p.push("`compat.interfaces` does not give the `protocol` version".into());
        }
        for s in &self.services {
            if !SERVICES.contains(&s.as_str()) {
                p.push(format!("unknown service `{s}`"));
            } else if !self.compat.interfaces.contains_key(s) {
                p.push(format!(
                    "service `{s}` without its interface version in `compat.interfaces`"
                ));
            }
        }
        for c in &self.commands {
            if c.id.is_empty() || c.title.is_empty() {
                p.push("a command needs an id and a title".into());
            }
        }
        p.extend(self.importer_problems());
        p.extend(self.connect_problems());
        p.extend(self.health_problems());
        for e in &self.activation {
            let ok = match e.split_once(':') {
                None if e == ON_HEALTH => !self.health.is_empty(),
                None => e == "onStartup",
                Some(("onService", s)) => self.services.iter().any(|x| x == s),
                Some(("onCommand", c)) => self.commands.iter().any(|x| x.id == c),
                Some(("onImporter", i)) => self.importers.iter().any(|x| x.id == i),
                Some(_) => false,
            };
            if !ok {
                p.push(format!(
                    "activation event `{e}` names nothing this plugin declares"
                ));
            }
        }
        let contracts = self.contracts();
        p.extend(contracts.problems());
        if !contracts.account.is_empty() && self.connect.is_empty() {
            p.push("`[account.settings]` without `[[connect]]`: it makes no accounts".into());
        }
        if !contracts.source.is_empty()
            && !self
                .services
                .iter()
                .any(|s| s == "import.source" || s == "importer")
        {
            p.push("`[source.settings]` without the service `import.source` or `importer`".into());
        }
        for c in &self.capabilities {
            if capability(&c.name).is_none() {
                let known: Vec<_> = CAPABILITIES.iter().map(|(n, ..)| *n).collect();
                p.push(format!(
                    "unknown capability `{}` (known: {})",
                    c.name,
                    known.join(", ")
                ));
            }
            if c.reason.trim().is_empty() {
                p.push(format!("capability `{}` without a reason", c.name));
            }
        }
        p
    }

    /// What keeps its checks from running: each declaration is sound
    /// (the health contract's own problems), has a title and an id of its
    /// own; the plugin starts on `onHealth` and gives the `health`
    /// interface's version.
    fn health_problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.health.is_empty() {
            return p;
        }
        if !self.activation.iter().any(|e| e == ON_HEALTH) {
            p.push(format!(
                "`[[health]]` without the activation event `{ON_HEALTH}`"
            ));
        }
        if !self.compat.interfaces.contains_key("health") {
            p.push(
                "`[[health]]` without the `health` interface version in `compat.interfaces`".into(),
            );
        }
        let mut seen = std::collections::BTreeSet::new();
        for check in &self.health {
            p.extend(
                check
                    .declaration()
                    .problems(&check.id)
                    .into_iter()
                    .map(|e| format!("health {e}")),
            );
            if check.title.trim().is_empty() {
                p.push(format!("health check `{}`: a title", check.id));
            }
            if !seen.insert(check.id.as_str()) {
                p.push(format!("health check `{}` is declared twice", check.id));
            }
        }
        p
    }

    /// What keeps a connection process from running: it belongs to the
    /// `import.source` service, each step has a unique id and a known
    /// kind, and an `oauth` step gives where to authorise and get tokens.
    fn connect_problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if !self.connect.is_empty() && !self.services.iter().any(|s| s == "import.source") {
            p.push("`[[connect]]` without the service `import.source`".into());
        }
        let source = self.services.iter().any(|s| s == "import.source");
        if !self.browsers.is_empty() && !source {
            p.push("`browsers` without the service `import.source`".into());
        }
        if (self.thumbnails || !no_limits(&self.limits)) && !source {
            p.push("`thumbnails` or `[limits]` without the service `import.source`".into());
        }
        if self.limits.per_minute == Some(0) || self.limits.per_day == Some(0) {
            p.push("a limit of 0 requests: leave it out for no limit".into());
        }
        for name in &self.browsers {
            if helios_connect::Browser::from_name(name).is_none() {
                let known: Vec<_> = helios_connect::Browser::ALL
                    .iter()
                    .map(|b| b.name())
                    .collect();
                p.push(format!(
                    "unknown browser `{name}` (known: {})",
                    known.join(", ")
                ));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for step in &self.connect {
            if step.id.trim().is_empty() || !seen.insert(step.id.as_str()) {
                p.push(format!("connection step `{}`: an id of its own", step.id));
            }
            if !CONNECT_KINDS.contains(&step.kind.as_str()) {
                p.push(format!(
                    "connection step `{}`: unknown kind `{}`",
                    step.id, step.kind
                ));
            }
            if step.kind == "oauth" {
                for key in ["authorize", "token"] {
                    if !step.rest.get(key).is_some_and(Json::is_string) {
                        p.push(format!("connection step `{}`: no `{key}`", step.id));
                    }
                }
            }
        }
        p
    }

    /// What keeps the importers from being offered: each needs the
    /// `importer` service and the `source.read` capability, a unique id
    /// and a label; patterns are for a file source, `holds` for a folder.
    fn importer_problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.importers.is_empty() {
            if self.services.iter().any(|s| s == "importer") {
                p.push("service `importer` without any `[[importers]]`".into());
            }
            return p;
        }
        if !self.services.iter().any(|s| s == "importer") {
            p.push("`[[importers]]` without the service `importer`".into());
        }
        if !self.declares("source.read") {
            p.push("`[[importers]]` without the capability `source.read`".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for i in &self.importers {
            if i.id.trim().is_empty() || i.label.trim().is_empty() {
                p.push("an importer needs an id and a label".into());
            }
            if !seen.insert(i.id.as_str()) {
                p.push(format!("importer `{}` is declared twice", i.id));
            }
            if i.source == SourceKind::Folder && !i.patterns.is_empty() {
                p.push(format!(
                    "importer `{}`: `patterns` are for a file source; a folder has `holds`",
                    i.id
                ));
            }
            if i.source == SourceKind::File && !i.holds.is_empty() {
                p.push(format!(
                    "importer `{}`: `holds` is for a folder source; a file has `patterns`",
                    i.id
                ));
            }
            if i.holds
                .iter()
                .any(|h| h.is_empty() || h.contains(['/', '\\']) || h == "..")
            {
                p.push(format!(
                    "importer `{}`: `holds` names entries of the folder itself",
                    i.id
                ));
            }
        }
        p
    }

    /// An importer it declares.
    pub fn importer(&self, id: &str) -> Option<&ImporterDecl> {
        self.importers.iter().find(|i| i.id == id)
    }

    /// What an interface calls the plugin, in English: its title, else its
    /// name, else its id.
    pub fn title(&self) -> &str {
        [&self.title, &self.name, &self.id]
            .into_iter()
            .find(|t| !t.trim().is_empty())
            .map_or("", String::as_str)
    }

    pub fn declares(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c.name == capability)
    }

    /// Its configuration contracts: its own, each account's and each
    /// source's.
    pub fn contracts(&self) -> Contracts {
        Contracts {
            own: self.settings.clone(),
            account: self.account.settings.clone(),
            source: self.source.settings.clone(),
        }
    }

    /// Its own settings with their defaults, keyed `<id>.<key>`; secrets
    /// and settings without a default left out.
    pub fn default_settings(&self) -> BTreeMap<String, Json> {
        self.settings
            .defaults()
            .into_iter()
            .map(|(k, v)| (format!("{}.{k}", self.id), v))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_requirements() {
        let req = VersionReq::parse(">=0.0.1, <0.1.0").unwrap();
        assert!(req.matches(Version(0, 0, 1)));
        assert!(req.matches(Version(0, 0, 9)));
        assert!(!req.matches(Version(0, 1, 0)));
        assert!(
            VersionReq::parse("1.2.3")
                .unwrap()
                .matches(Version(1, 2, 3))
        );
        assert!(VersionReq::parse(">=x").is_err());
    }

    #[test]
    fn globs() {
        assert!(glob("library.db", "library.db"));
        assert!(glob("*.lrcat", "Photos.LRCAT"));
        assert!(glob("lib*.db", "library.db"));
        assert!(glob("*", "anything"));
        assert!(glob("a*b*c", "a-b-c"));
        assert!(!glob("*.lrcat", "photos.lrcat-wal"));
        assert!(!glob("library.db", "data.db"));
        assert!(!glob("ab*ba", "aba"));
        let i = ImporterDecl {
            id: "x".into(),
            label: "X".into(),
            source: SourceKind::File,
            patterns: vec!["*.lrcat".into(), "*.JSON".into()],
            holds: Vec::new(),
            anywhere: false,
        };
        assert_eq!(i.extensions(), ["lrcat", "json"]);
        assert!(i.accepts("a.json"));
        assert!(!i.accepts("a.db"));
    }

    const SOURCE: &str = r#"
id = "org.example.photos"
version = "1.0.0"
license = "MIT"
command = ["run.sh"]
services = ["import.source"]
activation = ["onService:import.source"]

[compat]
helios = ">=0.0.1"
interfaces = { protocol = 1, "import.source" = 1 }

[settings.api]
type = "string"
default = "https://example.org/v1"
about = "The API's base address"

[account.settings.album]
type = "string"
label = "Album"

[account.settings.token]
type = "secret"
label = "Access token"

[source.settings.size]
type = "int"
default = 2048
min = 256
max = 8192
step = 256

[[connect]]
id = "sign-in"
kind = "form"
"#;

    fn problems(text: &str) -> Vec<String> {
        match Manifest::parse(text) {
            Ok(m) => m.problems(Version(0, 0, 9)),
            Err(e) => vec![e],
        }
    }

    #[test]
    fn the_contract_is_read_at_its_three_levels() {
        let m = Manifest::parse(SOURCE).unwrap();
        assert_eq!(m.problems(Version(0, 0, 9)), Vec::<String>::new());
        let c = m.contracts();
        // An older `[settings.*]`, its `about` read as help.
        assert_eq!(c.own.get("api").unwrap().help, "The API's base address");
        assert_eq!(c.account.secrets().collect::<Vec<_>>(), ["token"]);
        assert_eq!(c.source.get("size").unwrap().max, Some(8192.0));
        assert_eq!(
            m.default_settings().get("org.example.photos.api"),
            Some(&serde_json::json!("https://example.org/v1"))
        );
        assert!(c.source.check(&serde_json::json!({ "size": 512 })).is_ok());
        assert!(c.source.check(&serde_json::json!({ "size": 100 })).is_err());
    }

    #[test]
    fn a_bad_contract_refuses_the_plugin() {
        let refused = |from: &str, to: &str, expect: &str| {
            let p = problems(&SOURCE.replace(from, to));
            assert!(p.iter().any(|e| e.contains(expect)), "{to}: {p:?}");
        };
        refused(
            "default = 2048",
            "default = 100",
            "source.setting `size`: default 100: 100 is below 256",
        );
        refused(
            "default = \"https://example.org/v1\"",
            "default = 3",
            "setting `api`: default 3: expected string, got integer",
        );
        refused(
            "type = \"secret\"\nlabel",
            "type = \"secret\"\ndefault = \"x\"\nlabel",
            "account.setting `token`: a secret has no default",
        );
        refused(
            "type = \"secret\"",
            "type = \"password\"",
            "unknown variant `password`",
        );
        refused(
            "[[connect]]\nid = \"sign-in\"\nkind = \"form\"",
            "",
            "it makes no accounts",
        );
        refused(
            "services = [\"import.source\"]",
            "services = [\"metadata.read\"]",
            "`[source.settings]` without the service",
        );
        refused(
            "services = [\"import.source\"]",
            "services = [\"import.source\"]\nbrowsers = [\"picker\", \"albums\"]",
            "unknown browser `albums` (known: tree, folder, collections, timeline, picker)",
        );
        refused(
            "services = [\"import.source\"]",
            "services = [\"metadata.read\"]\nthumbnails = true",
            "`thumbnails` or `[limits]` without the service `import.source`",
        );
        refused(
            "services = [\"import.source\"]",
            "services = [\"import.source\"]\nlimits = { per_minute = 0 }",
            "a limit of 0 requests",
        );
    }

    #[test]
    fn google_photos_declares_its_accounts_check_soundly() {
        let text = include_str!("../../../plugins/sources/google-photos/helios-plugin.toml");
        let m = Manifest::parse(text).unwrap();
        let helios = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        assert_eq!(m.problems(helios), Vec::<String>::new());
        let ids: Vec<&str> = m.health.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["accounts"]);
        assert_eq!(m.health[0].declaration().fixes, ["import.reconnect"]);
    }

    #[test]
    fn google_photos_offers_its_picker_only() {
        let text = include_str!("../../../plugins/sources/google-photos/helios-plugin.toml");
        let m = Manifest::parse(text).unwrap();
        assert_eq!(m.browsers, ["picker"]);
        // Its glances come from Google's previews, read ahead within its
        // declared limits.
        assert!(m.thumbnails);
        assert!(m.limits.per_minute.is_some() && m.limits.per_day.is_some());
        let p = problems(&SOURCE.replace(
            "services = [\"import.source\"]",
            "services = [\"import.source\"]\nbrowsers = [\"collections\", \"timeline\"]",
        ));
        assert_eq!(p, Vec::<String>::new());
    }
}
