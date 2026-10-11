//! User profiles (see `docs/gui.md`, "Themes and profiles"): a named set
//! of parts, each optional, that exports to one readable file
//! (`*.helios-profile`, TOML) and imports with a diff per part.
//!
//! Parts: the user's settings (the `user` layer, as `helios-config` writes
//! it), keymap bindings, the theme and its overrides, the panel layout,
//! plugins, export targets and AI preferences. A part a profile leaves out
//! comes from the Default profile ([`Profile::over`]).
//!
//! What never goes in a file:
//! - presets and styles: a library of looks shared by every profile;
//! - the vault and its images;
//! - machine settings (they are not in the user layer);
//! - secrets: a plugin setting its manifest marks `secret` or `machine` is
//!   left out ([`Exclude`]), and [`scan`] refuses an export that still
//!   holds something shaped like a key or a password.
//!
//! Plugins travel as references (id, pinned version, registry, enabled),
//! never as code: an import lists what it would install.
//!
//! Keys are written with `Mod` for the system's command modifier (Ctrl on
//! Linux and Windows, Cmd on macOS), so a profile moves between systems.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// The parts of a profile, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Settings,
    Keymap,
    Theme,
    Layout,
    Plugins,
    ExportTargets,
    Ai,
}

impl Part {
    pub const ALL: [Part; 7] = [
        Part::Settings,
        Part::Keymap,
        Part::Theme,
        Part::Layout,
        Part::Plugins,
        Part::ExportTargets,
        Part::Ai,
    ];
}

/// A user binding, keys written portably (`Mod+Shift+P`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub keys: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub when: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ThemeChoice {
    pub name: String,
    /// Token path to its DTCG value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, Json>,
}

/// A plugin, by reference: what to install, never its code.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginRef {
    pub id: String,
    /// Pinned: an import installs what was exported.
    pub version: String,
    pub registry: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, Json>,
}

/// A profile. Every part is optional; a missing one is the Default's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// The user layer of the settings, as TOML text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keymap: Option<Vec<Binding>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeChoice>,
    /// The panel layout new workspaces start from (helios-shell's JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_targets: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai: Option<Json>,
}

/// Plugin settings to leave out of an export: `(plugin id, setting)` is
/// `secret` or `machine` in the plugin's manifest.
pub type Exclude<'a> = &'a dyn Fn(&str, &str) -> bool;

/// The file format's version, written in every file.
const FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
struct File {
    helios_profile: u32,
    #[serde(flatten)]
    profile: Profile,
}

impl Profile {
    pub fn has(&self, part: Part) -> bool {
        match part {
            Part::Settings => self.settings.is_some(),
            Part::Keymap => self.keymap.is_some(),
            Part::Theme => self.theme.is_some(),
            Part::Layout => self.layout.is_some(),
            Part::Plugins => self.plugins.is_some(),
            Part::ExportTargets => self.export_targets.is_some(),
            Part::Ai => self.ai.is_some(),
        }
    }

    /// Only the given parts.
    pub fn only(&self, parts: &[Part]) -> Profile {
        let keep = |p: Part| parts.contains(&p);
        Profile {
            name: self.name.clone(),
            settings: self.settings.clone().filter(|_| keep(Part::Settings)),
            keymap: self.keymap.clone().filter(|_| keep(Part::Keymap)),
            theme: self.theme.clone().filter(|_| keep(Part::Theme)),
            layout: self.layout.clone().filter(|_| keep(Part::Layout)),
            plugins: self.plugins.clone().filter(|_| keep(Part::Plugins)),
            export_targets: self
                .export_targets
                .clone()
                .filter(|_| keep(Part::ExportTargets)),
            ai: self.ai.clone().filter(|_| keep(Part::Ai)),
        }
    }

    /// This profile with the parts it leaves out taken from `default`.
    pub fn over(&self, default: &Profile) -> Profile {
        Profile {
            name: self.name.clone(),
            settings: self.settings.clone().or_else(|| default.settings.clone()),
            keymap: self.keymap.clone().or_else(|| default.keymap.clone()),
            theme: self.theme.clone().or_else(|| default.theme.clone()),
            layout: self.layout.clone().or_else(|| default.layout.clone()),
            plugins: self.plugins.clone().or_else(|| default.plugins.clone()),
            export_targets: self
                .export_targets
                .clone()
                .or_else(|| default.export_targets.clone()),
            ai: self.ai.clone().or_else(|| default.ai.clone()),
        }
    }

    /// The file for the given parts: plugin settings `exclude` names left
    /// out, then checked by [`scan`]; refused if anything looks secret.
    pub fn export(&self, parts: &[Part], exclude: Exclude) -> Result<String, String> {
        let mut out = self.only(parts);
        if let Some(plugins) = &mut out.plugins {
            for p in plugins {
                p.settings.retain(|k, _| !exclude(&p.id, k));
            }
        }
        let text = toml::to_string(&File {
            helios_profile: FORMAT,
            profile: out,
        })
        .map_err(|e| format!("profile: {e}"))?;
        let found = scan(&text);
        if !found.is_empty() {
            return Err(format!(
                "not exported, it would carry what looks like secrets: {}",
                found.join("; ")
            ));
        }
        Ok(text)
    }

    /// Reads a profile file.
    pub fn import(text: &str) -> Result<Profile, String> {
        let file: File = toml::from_str(text).map_err(|e| format!("profile: {e}"))?;
        if file.helios_profile > FORMAT {
            return Err(format!(
                "profile format {} is newer than this Helios reads ({FORMAT})",
                file.helios_profile
            ));
        }
        Ok(file.profile)
    }
}

/// What importing a part would change.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PartDiff {
    pub part: Part,
    /// One line per change, in words: `+ added`, `- removed`, `~ changed`.
    pub changes: Vec<String>,
}

fn json_lines(prefix: &str, value: &Json, out: &mut BTreeMap<String, String>) {
    match value {
        Json::Object(o) => {
            for (k, v) in o {
                let p = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                json_lines(&p, v, out);
            }
        }
        other => {
            out.insert(prefix.to_string(), other.to_string());
        }
    }
}

fn flat(part: Part, p: &Profile) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut from_json = |v: Json| json_lines("", &v, &mut out);
    match part {
        Part::Settings => {
            if let Some(t) = &p.settings
                && let Ok(table) = t.parse::<toml::Table>()
            {
                from_json(serde_json::to_value(table).unwrap_or_default());
            }
        }
        Part::Keymap => {
            for b in p.keymap.iter().flatten() {
                let when = if b.when.is_empty() {
                    String::new()
                } else {
                    format!(" when {}", b.when)
                };
                out.insert(b.command.clone(), format!("{}{when}", b.keys));
            }
        }
        Part::Theme => {
            if let Some(t) = &p.theme {
                out.insert("theme".into(), t.name.clone());
                for (k, v) in &t.overrides {
                    out.insert(format!("override {k}"), v.to_string());
                }
            }
        }
        Part::Plugins => {
            for pl in p.plugins.iter().flatten() {
                out.insert(
                    pl.id.clone(),
                    format!(
                        "{} from {}{}",
                        pl.version,
                        pl.registry,
                        if pl.enabled { "" } else { " (disabled)" }
                    ),
                );
            }
        }
        Part::Layout => from_json(p.layout.clone().unwrap_or_default()),
        Part::ExportTargets => from_json(p.export_targets.clone().unwrap_or_default()),
        Part::Ai => from_json(p.ai.clone().unwrap_or_default()),
    }
    out.retain(|k, _| !k.is_empty());
    out
}

/// What importing `incoming` over `current` would change, part by part:
/// only the parts `incoming` has, and only those that change.
pub fn diff(current: &Profile, incoming: &Profile) -> Vec<PartDiff> {
    Part::ALL
        .iter()
        .filter(|p| incoming.has(**p))
        .filter_map(|&part| {
            let (a, b) = (flat(part, current), flat(part, incoming));
            let mut changes = Vec::new();
            for (k, v) in &b {
                match a.get(k) {
                    None => changes.push(format!("+ {k}: {v}")),
                    Some(old) if old != v => changes.push(format!("~ {k}: {old} → {v}")),
                    _ => {}
                }
            }
            for k in a.keys().filter(|k| !b.contains_key(*k)) {
                changes.push(format!("- {k}"));
            }
            (!changes.is_empty()).then_some(PartDiff { part, changes })
        })
        .collect()
}

/// The plugins an import would install or change: those not installed
/// at the same version.
pub fn to_install<'a>(installed: &[PluginRef], incoming: &'a Profile) -> Vec<&'a PluginRef> {
    incoming
        .plugins
        .iter()
        .flatten()
        .filter(|p| {
            !installed
                .iter()
                .any(|i| i.id == p.id && i.version == p.version)
        })
        .collect()
}

/// Words in a setting's name that say it holds a secret.
const SECRET_NAMES: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "private_key",
    "credential",
];

/// Prefixes of well-known key formats.
const KEY_PREFIXES: &[&str] = &[
    "sk-",
    "sk_live_",
    "ghp_",
    "gho_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "AKIA",
    "AIza",
    "glpat-",
    "-----BEGIN",
];

/// Whether a string looks like a key: a known prefix, or a long run of
/// key-like characters mixing letters and digits.
fn looks_like_a_key(s: &str) -> bool {
    if KEY_PREFIXES.iter().any(|p| s.starts_with(p)) {
        return true;
    }
    let keyish = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_+/=".contains(c));
    let (letters, digits) = s.chars().fold((0, 0), |(l, d), c| {
        (
            l + usize::from(c.is_ascii_alphabetic()),
            d + usize::from(c.is_ascii_digit()),
        )
    });
    keyish && s.len() >= 32 && letters >= 8 && digits >= 4
}

/// What in a profile file looks secret, described without the value.
pub fn scan(text: &str) -> Vec<String> {
    let Ok(table) = text.parse::<toml::Table>() else {
        return vec!["the file is not TOML".into()];
    };
    let mut found = Vec::new();
    fn walk(prefix: &str, v: &toml::Value, found: &mut Vec<String>) {
        match v {
            toml::Value::Table(t) => {
                for (k, v) in t {
                    let path = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    let lower = k.to_ascii_lowercase();
                    if SECRET_NAMES.iter().any(|n| lower.contains(n))
                        && !matches!(v, toml::Value::Table(_))
                    {
                        found.push(format!("`{path}` is named like a secret"));
                        continue;
                    }
                    walk(&path, v, found);
                }
            }
            toml::Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    walk(&format!("{prefix}[{i}]"), v, found);
                }
            }
            toml::Value::String(s) if s.split_whitespace().any(looks_like_a_key) => {
                found.push(format!("`{prefix}` holds what looks like a key"));
            }
            _ => {}
        }
    }
    walk("", &toml::Value::Table(table), &mut found);
    found
}

/// A system, for its command modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    Linux,
    Windows,
    MacOs,
}

impl System {
    pub fn current() -> System {
        if cfg!(target_os = "macos") {
            System::MacOs
        } else if cfg!(target_os = "windows") {
            System::Windows
        } else {
            System::Linux
        }
    }

    fn command_modifier(self) -> &'static str {
        match self {
            System::MacOs => "meta",
            _ => "ctrl",
        }
    }
}

fn map_modifier(keys: &str, from: &str, to: &str) -> String {
    keys.split_whitespace()
        .map(|chord| {
            chord
                .split('+')
                .map(|part| {
                    if part.eq_ignore_ascii_case(from) {
                        to.to_string()
                    } else {
                        part.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Keys as written in a profile: the system's command modifier as `Mod`.
pub fn portable_keys(keys: &str, system: System) -> String {
    map_modifier(keys, system.command_modifier(), "Mod")
}

/// Keys from a profile, `Mod` as this system's command modifier.
pub fn local_keys(keys: &str, system: System) -> String {
    map_modifier(keys, "Mod", system.command_modifier())
}
