//! Import profiles (`docs/design/import-window.md`): a named set of
//! import choices (copy or add, where copies go and how they are named,
//! duplicates, darktable edits, camera looks, the filter and what is
//! applied during import), kept per library in one file
//! (`import-profiles.json`, beside the last used choices).
//!
//! - **"Everyday"** is built in and cannot be deleted nor renamed: what
//!   can stay where it is is added there, what must be copied (a source
//!   that cannot be added in place, or one the person will wipe, such as a
//!   card) is copied into the Library by date with its original names;
//!   duplicates skipped (the Contents' "New only" leaves out what is
//!   already in the Library). The window does not know source kinds: what
//!   a profile's choices mean for a source comes from what its connector
//!   declares.
//! - A saved source points at a profile by id ([`super::import_sources`]):
//!   changing a profile changes the next import of every source using it.
//! - Each connector may have a default profile, taken by a source of it
//!   that points at none.
//! - **Filter presets**: the named filters of the Include section, kept in
//!   the same file. A named filter only sets which files are taken; folded
//!   into whole profiles it would invent copy and naming choices nobody
//!   made, so it stays a preset any profile's filter can be set from.
//! - **Migration, once**: when the file does not exist yet, the old
//!   presets (`import-presets.json`) become profiles, the named filters
//!   (`import-filters.json`) filter presets, and the last used choices,
//!   when they are neither Everyday's nor a preset's, a "Last used"
//!   profile in use. The old files are left as they are.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use helios_core::Message;
use helios_library::import::{Duplicates, Layout};
use helios_library::import_profile::Filter;
use serde::{Deserialize, Serialize};

use super::import_choices::Choices;

/// The profiles' file, beside the last used choices.
pub const FILE: &str = "import-profiles.json";
/// The old presets' file, read once.
const PRESETS: &str = "import-presets.json";
/// The old named filters' file, read once.
const FILTERS: &str = "import-filters.json";
/// The built-in profile's id.
pub const EVERYDAY: &str = "everyday";

/// A named set of import choices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportProfile {
    /// Stable: what sources point at.
    pub id: String,
    /// As shown.
    pub name: String,
    pub choices: Choices,
}

impl ImportProfile {
    /// The built-in catch-all profile.
    pub fn everyday() -> Self {
        ImportProfile {
            id: EVERYDAY.into(),
            name: "Everyday".into(),
            choices: Choices::default(),
        }
    }

    /// Whether it is the built-in one: not deleted nor renamed.
    pub fn built_in(&self) -> bool {
        self.id == EVERYDAY
    }
}

/// The file, as kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Document {
    profiles: Vec<ImportProfile>,
    /// The profile the window uses when no source has one.
    current: Option<String>,
    /// The default profile of each connector, by the connector's id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    defaults: BTreeMap<String, String>,
    /// The filter presets, by name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    filters: BTreeMap<String, Filter>,
}

/// The library's import profiles, Everyday first, and where they are kept.
#[derive(Debug)]
pub struct Profiles {
    list: Vec<ImportProfile>,
    /// The profile in use when no source has one.
    pub current: String,
    /// The default profile of each connector, by the connector's id.
    pub defaults: BTreeMap<String, String>,
    /// The filter presets, by name.
    pub filters: BTreeMap<String, Filter>,
    file: Option<PathBuf>,
}

impl Default for Profiles {
    fn default() -> Self {
        Profiles {
            list: vec![ImportProfile::everyday()],
            current: EVERYDAY.into(),
            defaults: BTreeMap::new(),
            filters: BTreeMap::new(),
            file: None,
        }
    }
}

impl Profiles {
    /// The profiles kept in `dir` (the last used choices' folder), the
    /// old presets and named filters migrated when none are kept yet;
    /// `last` is the last used choices, for that migration.
    pub fn load(dir: &Path, last: &Choices) -> Self {
        let file = dir.join(FILE);
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
        let kept = read(FILE).and_then(|t| serde_json::from_str::<Document>(&t).ok());
        let mut profiles = if let Some(doc) = kept {
            Profiles::of(doc)
        } else {
            let presets: BTreeMap<String, Choices> = read(PRESETS)
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
            let filters: BTreeMap<String, Filter> = read(FILTERS)
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
            let mut p = Profiles::migrated(presets, filters, last);
            p.file = Some(file.clone());
            p.save();
            p
        };
        profiles.file = Some(file);
        profiles
    }

    fn of(doc: Document) -> Self {
        let mut p = Profiles {
            list: doc.profiles,
            current: doc.current.unwrap_or_else(|| EVERYDAY.into()),
            defaults: doc.defaults,
            filters: doc.filters,
            file: None,
        };
        // Everyday is always there, first.
        match p.list.iter().position(ImportProfile::built_in) {
            Some(0) => {}
            Some(i) => {
                let e = p.list.remove(i);
                p.list.insert(0, e);
            }
            None => p.list.insert(0, ImportProfile::everyday()),
        }
        p.list[0].name = ImportProfile::everyday().name;
        if p.get(&p.current).is_none() {
            p.current = EVERYDAY.into();
        }
        p.defaults
            .retain(|_, id| p.list.iter().any(|x| x.id == *id));
        p
    }

    /// The profiles of the old presets and named filters.
    fn migrated(
        presets: BTreeMap<String, Choices>,
        filters: BTreeMap<String, Filter>,
        last: &Choices,
    ) -> Self {
        let mut p = Profiles {
            filters,
            ..Profiles::default()
        };
        for (name, choices) in presets {
            p.add(&name, choices);
        }
        if let Some(id) = p.find(last) {
            p.current = id;
        } else {
            p.current = p.add("Last used", last.clone());
        }
        p
    }

    /// The profiles, Everyday first.
    pub fn list(&self) -> &[ImportProfile] {
        &self.list
    }

    pub fn get(&self, id: &str) -> Option<&ImportProfile> {
        self.list.iter().find(|p| p.id == id)
    }

    /// The choices of the profile `id`, Everyday's when there is none.
    pub fn choices(&self, id: &str) -> Choices {
        self.get(id)
            .map_or_else(Choices::default, |p| p.choices.clone())
    }

    /// The first profile whose choices are `choices`.
    pub fn find(&self, choices: &Choices) -> Option<String> {
        self.list
            .iter()
            .find(|p| p.choices == *choices)
            .map(|p| p.id.clone())
    }

    /// A profile `name` with `choices`, added; its id.
    pub fn add(&mut self, name: &str, choices: Choices) -> String {
        let id = self.fresh_id(name);
        self.list.push(ImportProfile {
            id: id.clone(),
            name: self.fresh_name(name),
            choices,
        });
        id
    }

    /// An id made of `name` no profile has.
    fn fresh_id(&self, name: &str) -> String {
        let mut base: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_lowercase().next().unwrap_or(c)
                } else {
                    '-'
                }
            })
            .collect();
        base = base.trim_matches('-').to_string();
        if base.is_empty() {
            base = "profile".into();
        }
        let mut id = base.clone();
        let mut n = 2;
        while self.get(&id).is_some() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// `name`, or `name 2`, `name 3`… when a profile has it.
    pub fn fresh_name(&self, name: &str) -> String {
        let taken = |n: &str| self.list.iter().any(|p| p.name == n);
        let mut out = name.to_string();
        let mut n = 2;
        while taken(&out) {
            out = format!("{name} {n}");
            n += 1;
        }
        out
    }

    /// Sets the choices of the profile `id`; whether it changed.
    pub fn set_choices(&mut self, id: &str, choices: Choices) -> bool {
        match self.list.iter_mut().find(|p| p.id == id) {
            Some(p) if p.choices != choices => {
                p.choices = choices;
                true
            }
            _ => false,
        }
    }

    /// Renames the profile `id` (not Everyday); whether it changed.
    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || self.list.iter().any(|p| p.name == name && p.id != id) {
            return false;
        }
        match self.list.iter_mut().find(|p| p.id == id) {
            Some(p) if !p.built_in() && p.name != name => {
                p.name = name.to_string();
                true
            }
            _ => false,
        }
    }

    /// Adds `profile`, or replaces the one of its id (Everyday keeps its
    /// name); its id.
    pub fn put(&mut self, mut profile: ImportProfile) -> String {
        if profile.id.trim().is_empty() {
            profile.id = self.fresh_id(&profile.name);
        }
        if let Some(p) = self.list.iter_mut().find(|p| p.id == profile.id) {
            p.choices = profile.choices.clone();
            if !p.built_in() && !profile.name.trim().is_empty() {
                p.name.clone_from(&profile.name);
            }
        } else {
            profile.name = self.fresh_name(&profile.name);
            self.list.push(profile.clone());
        }
        profile.id
    }

    /// Removes the profile `id` (never Everyday); whether it was one.
    /// What used it takes Everyday.
    pub fn remove(&mut self, id: &str) -> bool {
        if id == EVERYDAY {
            return false;
        }
        let before = self.list.len();
        self.list.retain(|p| p.id != id);
        if self.current == id {
            self.current = EVERYDAY.into();
        }
        self.defaults.retain(|_, d| d != id);
        before != self.list.len()
    }

    /// Keeps the profiles in their file.
    pub fn save(&self) {
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let doc = Document {
            profiles: self.list.clone(),
            current: Some(self.current.clone()),
            defaults: self.defaults.clone(),
            filters: self.filters.clone(),
        };
        let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
        if let Err(e) = std::fs::write(file, text) {
            log::warn!("import profiles: {}: {e}", file.display());
        }
    }
}

/// The Library folder, as a path's first part in examples.
pub fn library_name() -> Message {
    Message::new("gui-import-profile-library", "Library")
}

/// What a profile does, in one line: "Copied into Library/2026/2026-11-14,
/// original names, duplicates skipped". `copies`: whether the photos
/// shown are copied (the profile's choice, or what their connector
/// requires); `date`: the first photo ticked's (`year, month, day`), the
/// folder then shown as its layout's pattern without one.
pub fn summary(
    choices: &Choices,
    copies: bool,
    date: Option<(i64, u32, u32)>,
    t: &dyn Fn(Message) -> String,
) -> String {
    let mut parts = Vec::new();
    if copies {
        let folder = match (date, choices.layout) {
            (Some(date), layout) => layout
                .folder(Some(date))
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
            (None, Layout::Day) => "YYYY/YYYY-MM-DD".into(),
            (None, Layout::Month) => "YYYY/YYYY-MM".into(),
            (None, Layout::Year) => "YYYY".into(),
            (None, Layout::Flat) => String::new(),
        };
        let into = choices
            .into
            .as_deref()
            .and_then(|p| p.file_name())
            .map_or_else(|| t(library_name()), |n| n.to_string_lossy().into_owned());
        let place = if folder.is_empty() {
            into
        } else {
            format!("{into}/{folder}")
        };
        parts.push(t(Message::new(
            "gui-import-profile-summary-copied",
            "Copied into { $place }",
        )
        .with("place", place)));
        parts.push(if choices.rename == "{stem}" {
            t(Message::new(
                "gui-import-profile-summary-original",
                "original names",
            ))
        } else {
            t(
                Message::new("gui-import-profile-summary-renamed", "renamed { $pattern }")
                    .with("pattern", choices.rename.clone()),
            )
        });
    } else {
        parts.push(t(Message::new(
            "gui-import-profile-summary-added",
            "Added where they are",
        )));
    }
    parts.push(match choices.duplicates {
        Duplicates::Skip => t(Message::new(
            "gui-import-profile-summary-skip",
            "duplicates skipped",
        )),
        Duplicates::Keep => t(Message::new(
            "gui-import-profile-summary-keep",
            "duplicates kept",
        )),
        Duplicates::Ask => t(Message::new(
            "gui-import-profile-summary-ask",
            "asked about duplicates",
        )),
    });
    if !choices.filter.is_all() {
        parts.push(t(Message::new(
            "gui-import-profile-summary-filtered",
            "filtered",
        )));
    }
    if !choices.apply.is_empty() {
        parts.push(t(Message::new(
            "gui-import-profile-summary-applied",
            "keywords or marks applied",
        )));
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everyday_is_first_and_cannot_be_removed_or_renamed() {
        let mut p = Profiles::of(Document::default());
        assert_eq!(p.list()[0].id, EVERYDAY);
        assert!(!p.remove(EVERYDAY));
        assert!(!p.rename(EVERYDAY, "Other"));
        let id = p.add("Monthly", Choices::default());
        assert!(p.rename(&id, "By month"));
        assert!(p.remove(&id));
    }

    #[test]
    fn presets_and_filters_migrate_into_profiles_and_filter_presets() {
        let monthly = Choices {
            layout: Layout::Month,
            ..Choices::default()
        };
        let presets = BTreeMap::from([("Monthly".to_string(), monthly.clone())]);
        let filters = BTreeMap::from([(
            "Raws".to_string(),
            Filter {
                new_only: true,
                ..Filter::default()
            },
        )]);
        let p = Profiles::migrated(presets, filters, &monthly);
        assert_eq!(p.list().len(), 2);
        assert_eq!(p.list()[1].name, "Monthly");
        assert_eq!(p.current, p.list()[1].id, "the last used was the preset");
        assert!(p.filters.contains_key("Raws"));
        let other = Choices {
            rename: "{date}-{seq}".into(),
            ..Choices::default()
        };
        let p = Profiles::migrated(BTreeMap::new(), BTreeMap::new(), &other);
        assert_eq!(p.get(&p.current).unwrap().name, "Last used");
    }
}
