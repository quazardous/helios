//! Import profiles (`docs/design/import-window.md`): named sets of import
//! choices, kept by the node as one of the person's documents,
//! `user/import-profiles` (JSON), so the desktop's import window, the
//! node's automatic imports, an import by rule and agents read the same
//! profiles.
//!
//! - `import.profiles.list`: the document (profiles, Everyday first, the
//!   one in use, each connector's default profile, the filter presets).
//! - `import.profiles.get {id}`, `import.profiles.set {profile}` (added,
//!   or the one of its id replaced), `import.profiles.remove {id}`.
//! - `library.import {profile: <id>, …}` imports with the profile's
//!   choices; the options given beside it win ([`expand`]).
//!
//! A profile's `choices` are the desktop's (`copy_folders`, `into`,
//! `layout`, `rename`, `duplicates`, `darktable`, `camera_look`,
//! `filter`, `apply`); a choice missing is the local sources' default.
//! **Everyday** (`everyday`) is always there, first: it cannot be removed
//! and keeps its name.

use std::collections::BTreeMap;
use std::sync::Arc;

use helios_core::Bus;
use helios_vault::{Store, Vault};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The person's document the profiles are kept in.
pub const DOCUMENT: &str = "user/import-profiles";
/// The built-in profile's id.
pub const EVERYDAY: &str = "everyday";

pub const LIST: &str = "import.profiles.list";
pub const GET: &str = "import.profiles.get";
pub const SET: &str = "import.profiles.set";
pub const REMOVE: &str = "import.profiles.remove";

/// A named set of import choices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// Stable: what sources and imports name it by.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub choices: Map<String, Value>,
}

impl Profile {
    fn everyday() -> Self {
        Self {
            id: EVERYDAY.into(),
            name: "Everyday".into(),
            choices: Map::new(),
        }
    }
}

/// The document, as kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Document {
    pub profiles: Vec<Profile>,
    /// The profile used when no source has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// Each connector's default profile, by the connector's id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub defaults: BTreeMap<String, String>,
    /// The filter presets, by name.
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub filters: Map<String, Value>,
}

impl Document {
    /// The document of `text`, Everyday first; an empty one when `text`
    /// does not read.
    pub fn of(text: Option<&str>) -> Self {
        let mut doc: Self = text
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or_default();
        match doc.profiles.iter().position(|p| p.id == EVERYDAY) {
            Some(0) => {}
            Some(i) => {
                let e = doc.profiles.remove(i);
                doc.profiles.insert(0, e);
            }
            None => doc.profiles.insert(0, Profile::everyday()),
        }
        doc.profiles[0].name = Profile::everyday().name;
        doc
    }

    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Adds `profile`, or replaces the one of its id (Everyday keeps its
    /// name); an empty id is made of its name. Its id.
    pub fn put(&mut self, mut profile: Profile) -> String {
        if profile.id.trim().is_empty() {
            profile.id = self.fresh_id(&profile.name);
        }
        if let Some(p) = self.profiles.iter_mut().find(|p| p.id == profile.id) {
            p.choices = profile.choices;
            if p.id != EVERYDAY && !profile.name.trim().is_empty() {
                p.name = profile.name;
            }
        } else {
            self.profiles.push(profile.clone());
        }
        profile.id
    }

    /// Removes the profile `id` (never Everyday); whether it was one.
    pub fn remove(&mut self, id: &str) -> bool {
        if id == EVERYDAY {
            return false;
        }
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        if self.current.as_deref() == Some(id) {
            self.current = None;
        }
        self.defaults.retain(|_, d| d != id);
        before != self.profiles.len()
    }

    fn fresh_id(&self, name: &str) -> String {
        let base: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_lowercase().next().unwrap_or(c)
                } else {
                    '-'
                }
            })
            .collect();
        let base = match base.trim_matches('-') {
            "" => "profile".to_string(),
            b => b.to_string(),
        };
        let mut id = base.clone();
        let mut n = 2;
        while self.get(&id).is_some() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }
}

/// The profiles the node keeps in `vault`.
///
/// # Errors
/// The vault cannot read.
pub fn read<S: Store>(vault: &Vault<S>) -> Result<Document, String> {
    Ok(Document::of(vault.config_layer(DOCUMENT)?.as_deref()))
}

/// Keeps `doc` in `vault`; a change is announced on `bus` as the
/// settings' layers are ([`crate::settings::LayerChanged`]).
///
/// # Errors
/// The vault refuses the write.
pub fn write<S: Store>(vault: &Vault<S>, bus: Option<&Bus>, doc: &Document) -> Result<(), String> {
    let text = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    if vault.config_layer(DOCUMENT)?.as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    vault.set_config_layer(DOCUMENT, Some(&text))?;
    if let Some(bus) = bus {
        bus.publish(
            crate::settings::SOURCE,
            None,
            &crate::settings::LayerChanged {
                layer: DOCUMENT.into(),
            },
        );
    }
    Ok(())
}

/// Whether `id` is one of the profile commands.
pub fn handles(id: &str) -> bool {
    [LIST, GET, SET, REMOVE].contains(&id)
}

/// Runs the profile command `id`.
///
/// # Errors
/// An unknown profile, a profile refused, or the vault failing.
pub fn run<S: Store>(
    vault: &Vault<S>,
    bus: &Arc<Bus>,
    id: &str,
    params: &Value,
) -> Result<Value, String> {
    let mut doc = read(vault)?;
    let named = || {
        params["id"]
            .as_str()
            .ok_or_else(|| "`id` is missing: a profile of `import.profiles.list`".to_string())
    };
    match id {
        LIST => serde_json::to_value(&doc).map_err(|e| e.to_string()),
        GET => {
            let id = named()?;
            let profile = doc
                .get(id)
                .ok_or_else(|| format!("no import profile `{id}`"))?;
            Ok(json!({ "profile": profile }))
        }
        SET => {
            let profile: Profile = serde_json::from_value(params["profile"].clone())
                .map_err(|e| format!("`profile`: {e}"))?;
            // Its choices are checked as an import would read them, its
            // copy's folder and name too.
            crate::library::import_options(
                &Value::Object(params_of(&profile.choices, Some(true))),
                None,
            )?;
            let id = doc.put(profile);
            write(vault, Some(bus), &doc)?;
            Ok(json!({ "id": id }))
        }
        REMOVE => {
            let id = named()?;
            if id == EVERYDAY {
                return Err("Everyday is built in: it cannot be removed".into());
            }
            let removed = doc.remove(id);
            if removed {
                write(vault, Some(bus), &doc)?;
            }
            Ok(json!({ "removed": removed }))
        }
        other => Err(format!("no import profile command `{other}`")),
    }
}

/// `library.import`'s options of a profile's `choices`: the local
/// sources' defaults where a choice is missing; `copy` given wins over
/// the profile's. A copy's folder and name only for a copy.
pub fn params_of(choices: &Map<String, Value>, copy: Option<bool>) -> Map<String, Value> {
    let mut params = helios_connect::local::source_contract(false).defaults();
    for key in [
        "into",
        "layout",
        "rename",
        "duplicates",
        "darktable",
        "camera_look",
        "filter",
        "apply",
    ] {
        if let Some(v) = choices.get(key).filter(|v| !v.is_null()) {
            params.insert(key.into(), v.clone());
        }
    }
    let copy = copy
        .or_else(|| choices.get("copy_folders").and_then(Value::as_bool))
        .unwrap_or(false);
    params.insert("copy".into(), json!(copy));
    if !copy {
        for key in ["into", "layout", "rename"] {
            params.remove(key);
        }
    }
    params
}

/// `library.import`'s `params` with the choices of the profile it names
/// (`profile`) under the options given beside it; as they are when it
/// names none. `profile` stays: expanding again changes nothing, and an
/// import kept for later (a promise, the history) says what it used.
///
/// # Errors
/// No profile of that id, or the vault cannot read.
pub fn expand<S: Store>(vault: &Vault<S>, params: &Value) -> Result<Value, String> {
    let Some(id) = params["profile"].as_str() else {
        return Ok(params.clone());
    };
    let doc = read(vault)?;
    let profile = doc
        .get(id)
        .ok_or_else(|| format!("no import profile `{id}`"))?;
    let mut out = params.clone();
    let Some(fields) = out.as_object_mut() else {
        return Ok(out);
    };
    let copy = fields.get("copy").and_then(Value::as_bool);
    for (key, value) in params_of(&profile.choices, copy) {
        fields.entry(key).or_insert(value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> Vault<helios_vault::MemoryStore> {
        Vault::new(helios_vault::MemoryStore::default())
    }

    #[test]
    fn the_commands_round_trip_a_profile_and_keep_everyday() {
        let (vault, bus) = (vault(), Arc::new(Bus::new()));
        let listed = run(&vault, &bus, LIST, &json!({})).unwrap();
        assert_eq!(listed["profiles"][0]["id"], EVERYDAY);
        let monthly = json!({ "id": "", "name": "Monthly",
            "choices": { "copy_folders": true, "layout": "month", "rename": "{date}-{seq}" } });
        let set = run(&vault, &bus, SET, &json!({ "profile": monthly })).unwrap();
        assert_eq!(set["id"], "monthly");
        let got = run(&vault, &bus, GET, &json!({ "id": "monthly" })).unwrap();
        assert_eq!(got["profile"]["choices"]["layout"], "month");
        assert_eq!(got["profile"]["name"], "Monthly");
        assert_eq!(
            run(&vault, &bus, LIST, &json!({})).unwrap()["profiles"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // A choice an import would refuse is refused.
        let bad =
            json!({ "profile": { "id": "x", "name": "X", "choices": { "layout": "weekly" } } });
        assert!(run(&vault, &bus, SET, &bad).is_err());
        assert!(run(&vault, &bus, REMOVE, &json!({ "id": EVERYDAY })).is_err());
        let removed = run(&vault, &bus, REMOVE, &json!({ "id": "monthly" })).unwrap();
        assert_eq!(removed["removed"], true);
        assert!(run(&vault, &bus, GET, &json!({ "id": "monthly" })).is_err());
    }

    #[test]
    fn an_import_naming_a_profile_takes_its_choices_under_those_given() {
        let (vault, bus) = (vault(), Arc::new(Bus::new()));
        let p = json!({ "id": "cards", "name": "Cards", "choices": {
            "copy_folders": true, "layout": "month", "rename": "{date}_{stem}",
            "duplicates": "keep", "darktable": false } });
        run(&vault, &bus, SET, &json!({ "profile": p })).unwrap();
        let params = json!({ "source": "/media/card", "rule": "new",
                             "profile": "cards", "duplicates": "skip" });
        let out = expand(&vault, &params).unwrap();
        assert_eq!(out["copy"], true);
        assert_eq!(out["layout"], "month");
        assert_eq!(out["rename"], "{date}_{stem}");
        assert_eq!(out["darktable"], false);
        assert_eq!(out["duplicates"], "skip", "the option given wins");
        assert_eq!(expand(&vault, &out).unwrap(), out, "expanded once");
        // Added in place: no copy's folder nor name.
        let out = expand(
            &vault,
            &json!({ "paths": ["/p"], "profile": "cards", "copy": false }),
        )
        .unwrap();
        assert!(out.get("rename").is_none() && out.get("layout").is_none());
        assert!(expand(&vault, &json!({ "profile": "gone" })).is_err());
        assert_eq!(
            expand(&vault, &json!({ "paths": [] })).unwrap(),
            json!({ "paths": [] })
        );
    }
}
