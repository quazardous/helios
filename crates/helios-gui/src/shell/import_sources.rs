//! The import window's saved sources (`docs/design/import-connectors.md`,
//! section 3.6): a source is a connector instance (its type and its own
//! parameters: the folder, the account) with its own import choices,
//! the Choices column's.
//!
//! - The sources are kept as one of the person's documents the node
//!   keeps (`user/import-sources`, TOML), read and written through the
//!   desktop's ([`Shell::keep_import_sources_with`]); without it, the
//!   window keeps one set of choices, the last used, as before.
//! - **The Choices column follows the selected source**: the folder last
//!   ticked, or the account last clicked. Changing a choice there saves
//!   it on that source, one step of the column's Undo; "Reset to
//!   defaults" gives the source the window's choices again. A card stays
//!   an automatic source, with the window's choices.
//! - **Choices a connector cannot honour are hidden**: a remote
//!   connector's photos are always copied ("Add where they are" hidden)
//!   and have no darktable sidecars ("Read darktable edits" hidden).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use helios_connect::Caps;
use serde::{Deserialize, Serialize};

use super::Shell;
use super::import_choices::Choices;
use super::import_window::ImportWindow;

/// The name of the person's document the sources are kept in.
pub const DOCUMENT: &str = "user/import-sources";

/// Reads the sources' document: its TOML text, `None` when none is kept.
pub type ReadSources = Box<dyn Fn() -> Result<Option<String>, String>>;
/// Keeps the sources' document's TOML text.
pub type WriteSources = Box<dyn Fn(&str) -> Result<(), String>>;

/// Which source: a folder by its path, or a connector's account by id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceKey {
    Folder(PathBuf),
    Account(String),
}

/// A saved source: a connector instance and its own choices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSource {
    /// The connector's id (`folder`, `google-photos`…).
    pub connector: String,
    /// Its name as shown, when the person gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A folder connector's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// An account connector's account id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Its own choices; `None`: the window's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Choices>,
}

impl SavedSource {
    fn key(&self) -> Option<SourceKey> {
        match (&self.path, &self.account) {
            (Some(p), _) => Some(SourceKey::Folder(p.clone())),
            (None, Some(a)) => Some(SourceKey::Account(a.clone())),
            (None, None) => None,
        }
    }
}

/// The document, as kept.
#[derive(Default, Serialize, Deserialize)]
struct Document {
    #[serde(default, rename = "source")]
    sources: Vec<SavedSource>,
}

/// Which of the choices a source's connector honours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Honours {
    /// "Add where they are": the photos are files on this machine.
    pub add_in_place: bool,
    /// "Read darktable edits": XMP sidecars can sit beside the photos.
    pub darktable: bool,
}

impl Default for Honours {
    fn default() -> Self {
        Self {
            add_in_place: true,
            darktable: true,
        }
    }
}

impl Honours {
    /// What a connector of these capabilities honours: one reached
    /// through an account is remote, its photos always copied and
    /// without sidecars; a local one (folder, card) honours every choice.
    pub fn of(caps: &Caps) -> Self {
        let local = !caps.accounts;
        Self {
            add_in_place: local,
            darktable: local,
        }
    }
}

/// A change of a source's choices, for Undo.
struct Step {
    key: SourceKey,
    before: Option<Choices>,
    at: Instant,
}

/// Changes closer together than this on one source are one Undo step
/// (a name typed letter by letter).
const ONE_STEP: Duration = Duration::from_millis(1500);

/// What the Choices column asked of the sources this frame.
pub(super) enum SourceAsked {
    Undo,
    Reset,
}

/// The window's saved sources and where they are kept.
#[derive(Default)]
pub(crate) struct Sources {
    pub(crate) list: Vec<SavedSource>,
    read: Option<ReadSources>,
    write: Option<WriteSources>,
    undo: Vec<Step>,
    /// The account clicked last, selected until a folder is ticked.
    pub(crate) account: Option<String>,
    /// The source whose choices the column shows (`None`: the window's),
    /// and what it honours.
    pub(crate) showing: Option<SourceKey>,
    pub(crate) honours: Honours,
    /// The window's choices while a source's are shown.
    pub(crate) window: Choices,
    pub(super) asked: Option<SourceAsked>,
}

impl Sources {
    /// Whether the sources are kept: the choices then follow them.
    pub(crate) fn kept(&self) -> bool {
        self.write.is_some()
    }

    fn get(&self, key: &SourceKey) -> Option<&SavedSource> {
        self.list.iter().find(|s| s.key().as_ref() == Some(key))
    }

    /// A source's own choices.
    pub(crate) fn own(&self, key: &SourceKey) -> Option<&Choices> {
        self.get(key).and_then(|s| s.choices.as_ref())
    }

    /// Whether the folder at `path` has its own choices.
    pub(crate) fn has_own(&self, path: &std::path::Path) -> bool {
        self.own(&SourceKey::Folder(path.to_path_buf())).is_some()
    }

    /// The saved folders, in the order saved.
    pub(crate) fn folders(&self) -> Vec<PathBuf> {
        self.list.iter().filter_map(|s| s.path.clone()).collect()
    }

    /// Sets a source's own choices, saving the source when new.
    fn set(&mut self, key: &SourceKey, connector: &str, choices: Option<Choices>) {
        if let Some(s) = self.list.iter_mut().find(|s| s.key().as_ref() == Some(key)) {
            s.choices = choices;
        } else {
            let (path, account) = match key {
                SourceKey::Folder(p) => (Some(p.clone()), None),
                SourceKey::Account(a) => (None, Some(a.clone())),
            };
            self.list.push(SavedSource {
                connector: connector.to_string(),
                name: None,
                path,
                account,
                choices,
            });
        }
        self.store();
    }

    fn read_kept(&mut self) {
        let Some(read) = &self.read else { return };
        let doc: Document = match read() {
            Ok(Some(text)) => toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("import sources: {DOCUMENT}: {e}");
                Document::default()
            }),
            Ok(None) => Document::default(),
            Err(e) => {
                log::warn!("import sources: {DOCUMENT}: {e}");
                Document::default()
            }
        };
        self.list = doc.sources;
    }

    fn store(&self) {
        let Some(write) = &self.write else { return };
        let doc = Document {
            sources: self.list.clone(),
        };
        let text = match toml::to_string(&doc) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("import sources: {e}");
                return;
            }
        };
        if let Err(e) = write(&text) {
            log::warn!("import sources: {DOCUMENT}: {e}");
        }
    }
}

impl ImportWindow {
    /// The source selected: the account clicked last, else the folder
    /// ticked last; none for a card (an automatic source) or when the
    /// sources are not kept.
    fn selected_source(&self) -> Option<SourceKey> {
        if !self.sources.kept() {
            return None;
        }
        if let Some(id) = &self.sources.account {
            return Some(SourceKey::Account(id.clone()));
        }
        let last = self.ticked.last()?;
        (!self.is_card(last)).then(|| SourceKey::Folder(last.clone()))
    }

    /// The window's choices: the column's while it shows them.
    pub(super) fn window_choices(&self) -> &Choices {
        if self.sources.showing.is_none() {
            &self.choices
        } else {
            &self.sources.window
        }
    }

    /// The choices an import from `path` runs with: the folder's own, else
    /// the window's.
    pub(super) fn choices_for(&self, path: &std::path::Path) -> Choices {
        if self.is_card(path) {
            return self.window_choices().clone();
        }
        self.sources
            .own(&SourceKey::Folder(path.to_path_buf()))
            .unwrap_or_else(|| self.window_choices())
            .clone()
    }

    /// An account's name, its id when not read yet.
    pub(super) fn account_label(&self, id: &str) -> String {
        self.accounts.label_of(id).unwrap_or_else(|| id.to_string())
    }

    /// Selects an account's source: the column shows its choices.
    pub(super) fn select_account(&mut self, id: String) {
        self.sources.account = Some(id);
    }

    /// Before the window is drawn: the column shows the selected source's
    /// choices (its own, else the window's), and what its connector
    /// honours.
    pub(super) fn follow_selection(&mut self) {
        let key = self.selected_source();
        if key == self.sources.showing {
            return;
        }
        if self.sources.showing.is_none() {
            self.sources.window = self.choices.clone();
        }
        self.choices = match &key {
            Some(k) => self.sources.own(k).unwrap_or(&self.sources.window).clone(),
            None => self.sources.window.clone(),
        };
        self.sources.honours = match &key {
            Some(SourceKey::Account(id)) => self.account_honours(id),
            _ => Honours::default(),
        };
        self.custom = false;
        self.sources.showing = key;
    }

    fn account_honours(&mut self, id: &str) -> Honours {
        let connector = self
            .accounts
            .listed()
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.connector.clone());
        connector
            .and_then(|c| self.connectors.provider(&c))
            .map_or_else(Honours::default, |p| Honours::of(&p.manifest().caps))
    }

    fn connector_of(&mut self, key: &SourceKey) -> String {
        match key {
            SourceKey::Folder(_) => helios_connect::folder::ID.to_string(),
            SourceKey::Account(id) => self
                .accounts
                .listed()
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.connector.clone())
                .unwrap_or_default(),
        }
    }

    /// After the window is drawn: a choice changed on the source shown is
    /// saved on it (one Undo step); then Undo or Reset, when asked.
    pub(super) fn keep_source_choices(&mut self, shown: Option<SourceKey>, before: &Choices) {
        if let Some(key) = shown.filter(|k| Some(k) == self.sources.showing.as_ref())
            && self.choices != *before
        {
            let earlier = self.sources.own(&key).cloned();
            let now = Instant::now();
            let joined = self
                .sources
                .undo
                .last()
                .is_some_and(|s| s.key == key && now.duration_since(s.at) < ONE_STEP);
            match self.sources.undo.last_mut() {
                Some(step) if joined => step.at = now,
                _ => self.sources.undo.push(Step {
                    key: key.clone(),
                    before: earlier,
                    at: now,
                }),
            }
            let connector = self.connector_of(&key);
            self.sources
                .set(&key, &connector, Some(self.choices.clone()));
        }
        match self.sources.asked.take() {
            Some(SourceAsked::Undo) => self.undo_source_choice(),
            Some(SourceAsked::Reset) => self.reset_source_choices(),
            None => {}
        }
    }

    /// Takes back the last change of a source's choices.
    pub(super) fn undo_source_choice(&mut self) {
        let Some(step) = self.sources.undo.pop() else {
            return;
        };
        let connector = self.connector_of(&step.key);
        self.sources.set(&step.key, &connector, step.before);
        if self.sources.showing.as_ref() == Some(&step.key) {
            self.choices = self
                .sources
                .own(&step.key)
                .unwrap_or(&self.sources.window)
                .clone();
            self.custom = false;
        }
    }

    /// The source shown takes the window's choices again.
    pub(super) fn reset_source_choices(&mut self) {
        let Some(key) = self.sources.showing.clone() else {
            return;
        };
        let Some(before) = self.sources.own(&key).cloned() else {
            return;
        };
        // Its own step: a change right after it is not joined to it.
        let now = Instant::now();
        self.sources.undo.push(Step {
            key: key.clone(),
            before: Some(before),
            at: now.checked_sub(ONE_STEP).unwrap_or(now),
        });
        let connector = self.connector_of(&key);
        self.sources.set(&key, &connector, None);
        self.choices = self.sources.window.clone();
        self.custom = false;
    }

    /// Whether the column's Undo has a step.
    pub(super) fn can_undo_source(&self) -> bool {
        !self.sources.undo.is_empty()
    }
}

impl Shell {
    /// Keeps the import window's sources, with their own choices, in the
    /// person's document [`DOCUMENT`] the node keeps, read now through
    /// `read` and written through `write` on each change. From then on
    /// the Choices column follows the selected source.
    pub fn keep_import_sources_with(&mut self, read: ReadSources, write: WriteSources) {
        let mut w = self.import.get_mut();
        w.sources.read = Some(read);
        w.sources.write = Some(write);
        w.sources.read_kept();
        w.sources.showing = None;
    }

    /// The import window's saved sources.
    pub fn import_sources(&self) -> Vec<SavedSource> {
        self.import.get().sources.list.clone()
    }

    /// Selects an account's source in the import window, as a click on it
    /// does.
    pub fn select_import_account(&mut self, id: &str) {
        self.import.get_mut().select_account(id.to_string());
    }

    /// Takes back the last change of an import source's choices.
    pub fn undo_import_source_choice(&mut self) {
        self.import.get_mut().undo_source_choice();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_keeps_a_folders_and_an_accounts_choices() {
        let doc = Document {
            sources: vec![
                SavedSource {
                    connector: "folder".into(),
                    name: None,
                    path: Some("/photos/trips".into()),
                    account: None,
                    choices: Some(Choices {
                        copy_folders: true,
                        rename: "{date}-{seq}".into(),
                        ..Choices::default()
                    }),
                },
                SavedSource {
                    connector: "google-photos".into(),
                    name: Some("Family".into()),
                    path: None,
                    account: Some("google-photos-1".into()),
                    choices: None,
                },
            ],
        };
        let text = toml::to_string(&doc).unwrap();
        let back: Document = toml::from_str(&text).unwrap();
        assert_eq!(back.sources, doc.sources, "{text}");
    }

    #[test]
    fn a_remote_connector_hides_add_in_place_and_darktable() {
        let remote = Caps {
            accounts: true,
            ..Caps::default()
        };
        assert_eq!(
            Honours::of(&remote),
            Honours {
                add_in_place: false,
                darktable: false
            }
        );
        assert_eq!(Honours::of(&Caps::default()), Honours::default());
    }
}
