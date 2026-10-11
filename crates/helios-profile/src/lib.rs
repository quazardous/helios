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
//!
//! # Variants
//!
//! A part can differ from one machine to another: a bigger font on the
//! laptop, another key on macOS. A profile's `[on.<selector>]` sections
//! hold variants of its parts, each overriding the common values:
//!
//! ```toml
//! [theme]
//! name = "helios-dark"
//!
//! [[keymap]]
//! keys = "Mod+e"
//! command = "export.run"
//!
//! [on.machine.laptop.theme]
//! overrides = { "font.size.body" = "15px" }
//!
//! [[on.macos.keymap]]
//! keys = "meta+alt+e"
//! command = "export.run"
//! ```
//!
//! A selector ([`Selector`]) is a system (`macos`, `linux`, `windows`), a
//! screen class (`hidpi`) or a named machine (`machine.<name>`). A
//! machine's name is set once in a local file ([`machine_name`]), never in
//! a profile and never synced. A selector this Helios does not know, or a
//! machine other than this one, stays in the file and is not applied.
//!
//! [`Profile::resolve`] applies the variants that select this machine
//! ([`Machine`]) over the common values: the system's, then the screen
//! class's, then the machine's, so the most specific wins. How a variant
//! overrides depends on the part:
//! - settings, layout, export targets, AI: tables merge key by key, any
//!   other value replaces;
//! - keymap: a binding replaces the common bindings of its command (and
//!   `when`); one with empty `keys` only removes them;
//! - theme: a `name` replaces the theme, `overrides` add to the common ones;
//! - plugins: a plugin replaces the common one of the same id, or is added.
//!
//! Settings variants are user settings like the common ones: machine
//! settings (GPU, threads) stay out of profiles. Exporting the resolved
//! profile writes only this machine's view, the variants flattened.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize, Serializer};
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
    /// Empty in a variant that only adds overrides.
    #[serde(default, skip_serializing_if = "String::is_empty")]
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
    /// Variants of the parts, by selector (see "Variants" above).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty", with = "on_file")]
    pub on: BTreeMap<Selector, Variant>,
}

/// Parts overriding a profile's common ones where a selector applies.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keymap: Option<Vec<Binding>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_targets: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai: Option<Json>,
}

impl Variant {
    /// A profile of these parts as its common ones.
    fn into_profile(self, name: String, on: BTreeMap<Selector, Variant>) -> Profile {
        Profile {
            name,
            settings: self.settings,
            keymap: self.keymap,
            theme: self.theme,
            layout: self.layout,
            plugins: self.plugins,
            export_targets: self.export_targets,
            ai: self.ai,
            on,
        }
    }

    /// A profile's common parts.
    fn common(p: &Profile) -> Variant {
        Variant {
            settings: p.settings.clone(),
            keymap: p.keymap.clone(),
            theme: p.theme.clone(),
            layout: p.layout.clone(),
            plugins: p.plugins.clone(),
            export_targets: p.export_targets.clone(),
            ai: p.ai.clone(),
        }
    }

    /// Its parts as a profile, to reuse what profiles do.
    fn view(&self) -> Profile {
        self.clone().into_profile(String::new(), BTreeMap::new())
    }

    pub fn has(&self, part: Part) -> bool {
        self.view().has_common(part)
    }

    fn only(&self, parts: &[Part]) -> Variant {
        Variant::common(&self.view().only(parts))
    }

    /// Its parts, those it leaves out taken from `other`.
    fn or(&self, other: &Variant) -> Variant {
        Variant::common(&self.view().over(&other.view()))
    }

    fn is_empty(&self) -> bool {
        !Part::ALL.iter().any(|p| self.has(*p))
    }
}

/// Where a variant applies. Ordered as variants apply: systems, screen
/// classes, machines; the last one applied wins.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Selector {
    /// `macos`, `linux`, `windows`.
    System(System),
    /// `hidpi`: a screen of 1.5 pixels per point or more.
    Hidpi,
    /// `machine.<name>`: the machine of that name ([`machine_name`]).
    Machine(String),
    /// A selector this Helios does not know: kept, never applied.
    Other(String),
}

impl Selector {
    pub fn parse(s: &str) -> Selector {
        match s {
            "linux" => Selector::System(System::Linux),
            "macos" => Selector::System(System::MacOs),
            "windows" => Selector::System(System::Windows),
            "hidpi" => Selector::Hidpi,
            _ => match s.strip_prefix("machine.") {
                Some(name) if !name.is_empty() => Selector::Machine(name.into()),
                _ => Selector::Other(s.into()),
            },
        }
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Selector::System(s) => f.write_str(s.name()),
            Selector::Hidpi => f.write_str("hidpi"),
            Selector::Machine(name) => write!(f, "machine.{name}"),
            Selector::Other(s) => f.write_str(s),
        }
    }
}

impl Serialize for Selector {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// `on` as written: `on.<system or class>` and `on.machine.<name>`.
mod on_file {
    use super::{BTreeMap, Deserialize, Selector, Serialize, Serializer, Variant};
    use serde::Deserializer;

    /// Written in the order variants apply: systems and classes, then
    /// machines.
    #[derive(Default, Serialize, Deserialize)]
    struct On {
        #[serde(flatten)]
        rest: BTreeMap<String, Variant>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        machine: BTreeMap<String, Variant>,
    }

    pub fn serialize<S: Serializer>(
        on: &BTreeMap<Selector, Variant>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let mut file = On::default();
        for (selector, v) in on {
            match selector {
                Selector::Machine(name) => file.machine.insert(name.clone(), v.clone()),
                other => file.rest.insert(other.to_string(), v.clone()),
            };
        }
        file.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<Selector, Variant>, D::Error> {
        let file = On::deserialize(d)?;
        let machines = file
            .machine
            .into_iter()
            .map(|(name, v)| (Selector::Machine(name), v));
        let rest = file.rest.into_iter().map(|(k, v)| (Selector::parse(&k), v));
        Ok(machines.chain(rest).collect())
    }
}

/// This machine, as variants select it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Machine {
    pub system: System,
    /// From the machine's local file ([`machine_name`]), if named.
    pub name: Option<String>,
    pub hidpi: bool,
}

impl Machine {
    /// This system, named by the local file at `name_file`, its screen
    /// not known to be hidpi.
    pub fn here(name_file: &Path) -> Machine {
        Machine {
            system: System::current(),
            name: machine_name(name_file),
            hidpi: false,
        }
    }

    pub fn selects(&self, selector: &Selector) -> bool {
        match selector {
            Selector::System(s) => *s == self.system,
            Selector::Hidpi => self.hidpi,
            Selector::Machine(name) => self.name.as_ref() == Some(name),
            Selector::Other(_) => false,
        }
    }
}

/// This machine's name, from its local file (`machine.toml` in the user's
/// config directory, `name = "laptop"`): set once per machine, never in a
/// profile, never synced.
pub fn machine_name(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let table = text.parse::<toml::Table>().ok()?;
    let name = table.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// A profile resolved for one machine.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    /// Its parts with the variants applied, no variants left.
    pub profile: Profile,
    /// The values a variant set, by part and key as [`diff`] names them.
    pub from: Vec<FromVariant>,
}

/// A value a variant set, or removed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FromVariant {
    pub part: Part,
    pub key: String,
    pub selector: Selector,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
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
    /// Whether the profile has the part, common or in a variant.
    pub fn has(&self, part: Part) -> bool {
        self.has_common(part) || self.on.values().any(|v| v.has(part))
    }

    fn has_common(&self, part: Part) -> bool {
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

    /// Only the given parts, with their variants.
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
            on: self
                .on
                .iter()
                .map(|(s, v)| (s.clone(), v.only(parts)))
                .filter(|(_, v)| !v.is_empty())
                .collect(),
        }
    }

    /// This profile with what it leaves out taken from `default`: a
    /// missing common part is the default's, and the default's variants
    /// come along for the parts this profile has nowhere.
    pub fn over(&self, default: &Profile) -> Profile {
        let missing: Vec<Part> = Part::ALL.into_iter().filter(|p| !self.has(*p)).collect();
        let mut on = self.on.clone();
        for (selector, v) in &default.on {
            let theirs = v.only(&missing);
            if !theirs.is_empty() {
                let mine = on.entry(selector.clone()).or_default();
                *mine = mine.or(&theirs);
            }
        }
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
            on,
        }
    }

    /// The profile as `machine` uses it: the variants that select it
    /// applied in order, the others dropped, and where each value set by a
    /// variant came from.
    pub fn resolve(&self, machine: &Machine) -> Resolved {
        let mut profile = Profile {
            on: BTreeMap::new(),
            ..self.clone()
        };
        let mut from = BTreeMap::new();
        // The map's order is the order variants apply in.
        for (selector, v) in self.on.iter().filter(|(s, _)| machine.selects(s)) {
            for part in Part::ALL.into_iter().filter(|p| v.has(*p)) {
                let before = flat(part, &profile);
                profile.apply(part, v);
                let after = flat(part, &profile);
                let set = after
                    .iter()
                    .filter(|(k, value)| before.get(*k) != Some(value))
                    .map(|(k, _)| (k, false));
                let removed = before
                    .keys()
                    .filter(|k| !after.contains_key(*k))
                    .map(|k| (k, true));
                for (key, removed) in set.chain(removed) {
                    let value = FromVariant {
                        part,
                        key: key.clone(),
                        selector: selector.clone(),
                        removed,
                    };
                    from.insert((part, key.clone()), value);
                }
            }
        }
        Resolved {
            profile,
            from: from.into_values().collect(),
        }
    }

    /// Overrides one part with a variant's (see "Variants" above).
    fn apply(&mut self, part: Part, v: &Variant) {
        match part {
            Part::Settings => {
                if let Some(over) = &v.settings {
                    self.settings = Some(merge_settings(self.settings.as_deref(), over));
                }
            }
            Part::Keymap => {
                if let Some(over) = &v.keymap {
                    let keys = self.keymap.get_or_insert_default();
                    keys.retain(|b| {
                        !over
                            .iter()
                            .any(|o| o.command == b.command && o.when == b.when)
                    });
                    keys.extend(over.iter().filter(|o| !o.keys.is_empty()).cloned());
                }
            }
            Part::Theme => {
                if let Some(over) = &v.theme {
                    let theme = self.theme.get_or_insert_default();
                    if !over.name.is_empty() {
                        theme.name.clone_from(&over.name);
                    }
                    theme.overrides.extend(over.overrides.clone());
                }
            }
            Part::Plugins => {
                if let Some(over) = &v.plugins {
                    let plugins = self.plugins.get_or_insert_default();
                    for o in over {
                        match plugins.iter_mut().find(|p| p.id == o.id) {
                            Some(p) => *p = o.clone(),
                            None => plugins.push(o.clone()),
                        }
                    }
                }
            }
            Part::Layout => merge_part(&mut self.layout, v.layout.as_ref()),
            Part::ExportTargets => merge_part(&mut self.export_targets, v.export_targets.as_ref()),
            Part::Ai => merge_part(&mut self.ai, v.ai.as_ref()),
        }
    }

    /// This profile, changed since `source` was resolved into `resolved`,
    /// back as a profile with the source's variants. A part still as it
    /// was resolved is the source's, common values and variants; a part
    /// changed since has this profile's values as common ones, the
    /// source's variants for it kept over them. Parts this profile does
    /// not hold are left out.
    pub fn rebase(&self, source: &Profile, resolved: &Profile) -> Profile {
        let held: Vec<Part> = Part::ALL
            .into_iter()
            .filter(|p| self.has_common(*p))
            .collect();
        let changed: Vec<Part> = held
            .iter()
            .copied()
            .filter(|p| flat(*p, self) != flat(*p, resolved))
            .collect();
        let theirs = source.only(&held);
        let common = Variant::common(&self.only(&changed)).or(&Variant::common(&theirs));
        common.into_profile(self.name.clone(), theirs.on)
    }

    /// The file for the given parts: plugin settings `exclude` names left
    /// out, then checked by [`scan`]; refused if anything looks secret.
    pub fn export(&self, parts: &[Part], exclude: Exclude) -> Result<String, String> {
        let mut out = self.only(parts);
        let variants = out.on.values_mut().filter_map(|v| v.plugins.as_mut());
        for plugins in out.plugins.iter_mut().chain(variants) {
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

/// Settings text with another's values over it, tables merged.
fn merge_settings(base: Option<&str>, over: &str) -> String {
    fn merge(base: &mut toml::Table, over: &toml::Table) {
        for (k, v) in over {
            match (base.get_mut(k), v) {
                (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
                _ => {
                    base.insert(k.clone(), v.clone());
                }
            }
        }
    }
    let (Some(Ok(mut base)), Ok(over_table)) = (
        base.map(str::parse::<toml::Table>),
        over.parse::<toml::Table>(),
    ) else {
        return over.to_string();
    };
    merge(&mut base, &over_table);
    base.to_string()
}

fn merge_json(base: &mut Json, over: &Json) {
    match (base, over) {
        (Json::Object(b), Json::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(old) => merge_json(old, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (base, over) => *base = over.clone(),
    }
}

fn merge_part(base: &mut Option<Json>, over: Option<&Json>) {
    if let Some(over) = over {
        merge_json(base.get_or_insert(Json::Null), over);
    }
}

/// What importing a part would change.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PartDiff {
    pub part: Part,
    /// One line per change, in words: `+ added`, `- removed`, `~ changed`;
    /// a variant's lines start with `on <selector>:`.
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
                // A variant may only add overrides.
                if !t.name.is_empty() {
                    out.insert("theme".into(), t.name.clone());
                }
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

/// The lines from `a` to `b`, each after `prefix`.
fn lines(
    a: &BTreeMap<String, String>,
    b: &BTreeMap<String, String>,
    prefix: &str,
    out: &mut Vec<String>,
) {
    for (k, v) in b {
        match a.get(k) {
            None => out.push(format!("{prefix}+ {k}: {v}")),
            Some(old) if old != v => out.push(format!("{prefix}~ {k}: {old} → {v}")),
            _ => {}
        }
    }
    for k in a.keys().filter(|k| !b.contains_key(*k)) {
        out.push(format!("{prefix}- {k}"));
    }
}

/// What importing `incoming` over `current` would change, part by part:
/// only the parts `incoming` has, and only those that change; variants
/// are compared selector by selector.
pub fn diff(current: &Profile, incoming: &Profile) -> Vec<PartDiff> {
    Part::ALL
        .iter()
        .filter(|p| incoming.has(**p))
        .filter_map(|&part| {
            let mut changes = Vec::new();
            if incoming.has_common(part) {
                lines(
                    &flat(part, current),
                    &flat(part, incoming),
                    "",
                    &mut changes,
                );
            }
            let variant = |p: &Profile, s: &Selector| {
                p.on.get(s)
                    .map(|v| flat(part, &v.view()))
                    .unwrap_or_default()
            };
            let selectors: std::collections::BTreeSet<&Selector> = current
                .on
                .iter()
                .chain(&incoming.on)
                .filter(|(_, v)| v.has(part))
                .map(|(s, _)| s)
                .collect();
            for s in selectors {
                let (a, b) = (variant(current, s), variant(incoming, s));
                lines(&a, &b, &format!("on {s}: "), &mut changes);
            }
            (!changes.is_empty()).then_some(PartDiff { part, changes })
        })
        .collect()
}

/// The plugins an import would install or change: those not installed
/// at the same version, variants' included.
pub fn to_install<'a>(installed: &[PluginRef], incoming: &'a Profile) -> Vec<&'a PluginRef> {
    let variants = incoming
        .on
        .values()
        .flat_map(|v| v.plugins.iter().flatten());
    incoming
        .plugins
        .iter()
        .flatten()
        .chain(variants)
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
    let Ok(table) = text.parse::<toml::Table>() else {
        return vec!["the file is not TOML".into()];
    };
    let mut found = Vec::new();
    walk("", &toml::Value::Table(table), &mut found);
    found
}

/// A system, for its command modifier and the variants that select it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

    /// As a selector names it.
    pub fn name(self) -> &'static str {
        match self {
            System::Linux => "linux",
            System::Windows => "windows",
            System::MacOs => "macos",
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
