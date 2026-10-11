//! `helios-plugin.toml`: what a plugin is and what it asks for, read and
//! checked before anything of it runs.
//!
//! ```toml
//! id = "org.example.exif"
//! name = "ExifTool metadata"
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
//! about = "The exiftool program to run"
//!
//! [[capabilities]]
//! name = "images.read"
//! reason = "Reads the metadata of the images Helios asks about"
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// The file a plugin directory holds.
pub const MANIFEST: &str = "helios-plugin.toml";

/// Interfaces this Helios speaks, with the versions it accepts.
pub const INTERFACES: &[(&str, &[u32])] = &[
    ("protocol", &[1]),
    ("metadata.read", &[1]),
    ("device", &[1]),
];

/// The services a plugin may provide; each is also an interface.
pub const SERVICES: &[&str] = &["metadata.read", "device"];

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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SettingType {
    Bool,
    Int,
    Float,
    String,
    Choice,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SettingDecl {
    #[serde(rename = "type")]
    pub ty: SettingType,
    pub default: Json,
    #[serde(default)]
    pub about: String,
    /// The values a `choice` may take.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

impl SettingDecl {
    /// Whether a value fits the declared type.
    pub fn check(&self, v: &Json) -> Result<(), String> {
        let ok = match self.ty {
            SettingType::Bool => v.is_boolean(),
            SettingType::Int => v.is_i64() || v.is_u64(),
            SettingType::Float => v.is_number(),
            SettingType::String => v.is_string(),
            SettingType::Choice => v
                .as_str()
                .is_some_and(|s| self.choices.iter().any(|c| c == s)),
        };
        if ok {
            Ok(())
        } else {
            Err(format!("{v} is not a {:?}", self.ty).to_lowercase())
        }
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
    pub version: String,
    pub license: String,
    /// The program and its arguments, run in the plugin's directory. A
    /// program found in that directory is run from there, any other is
    /// looked up on `PATH` (`python3`).
    pub command: Vec<String>,
    pub compat: Compat,
    #[serde(default)]
    pub services: Vec<String>,
    /// `onStartup`, `onService:<service>`, `onCommand:<command id>`.
    #[serde(default)]
    pub activation: Vec<String>,
    #[serde(default)]
    pub commands: Vec<CommandDecl>,
    /// Typed settings, each under the plugin's id (`<id>.<key>`).
    #[serde(default)]
    pub settings: BTreeMap<String, SettingDecl>,
    #[serde(default)]
    pub capabilities: Vec<CapabilityDecl>,
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
                ))
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
        for e in &self.activation {
            let ok = match e.split_once(':') {
                None => e == "onStartup",
                Some(("onService", s)) => self.services.iter().any(|x| x == s),
                Some(("onCommand", c)) => self.commands.iter().any(|x| x.id == c),
                Some(_) => false,
            };
            if !ok {
                p.push(format!(
                    "activation event `{e}` names nothing this plugin declares"
                ));
            }
        }
        for (key, s) in &self.settings {
            if s.ty == SettingType::Choice && s.choices.is_empty() {
                p.push(format!("setting `{key}`: a choice without `choices`"));
            }
            if let Err(e) = s.check(&s.default) {
                p.push(format!("setting `{key}`: default {e}"));
            }
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

    pub fn declares(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c.name == capability)
    }

    /// The settings with their defaults, keyed `<id>.<key>`.
    pub fn default_settings(&self) -> BTreeMap<String, Json> {
        self.settings
            .iter()
            .map(|(k, s)| (format!("{}.{k}", self.id), s.default.clone()))
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
}
