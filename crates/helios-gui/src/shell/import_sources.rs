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
//! - A source added through the "Add a source…" wizard
//!   ([`super::import_wizard`]) has a name: it is listed under it, among
//!   the saved sources.
//! - **Choices a connector cannot honour are hidden**: a remote
//!   connector's photos are always copied ("Add where they are" hidden)
//!   and have no darktable sidecars ("Read darktable edits" hidden).
//! - **"Edit source…"** in a source's menu opens the wizard's steps on it
//!   ([`super::import_wizard`]): its parameters (the folder or account,
//!   then the keys of its connector's source contract that are not
//!   choices), its choices, its name. A remembered source (a recent
//!   folder, a card) is saved by it; **"Forget this source"** takes a
//!   source off the column. Both are steps of the column's Undo.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use helios_connect::Caps;
use serde::{Deserialize, Serialize};

use super::Shell;
pub use super::import_auto::Trigger;
use super::import_choices::Choices;
use super::import_window::ImportWindow;

/// The name of the person's document the sources are kept in.
pub const DOCUMENT: &str = "user/import-sources";

/// Reads the sources' document: its TOML text, `None` when none is kept.
pub type ReadSources = Box<dyn Fn() -> Result<Option<String>, String>>;
/// Keeps the sources' document's TOML text.
pub type WriteSources = Box<dyn Fn(&str) -> Result<(), String>>;

/// `write` on a thread of its own, as the window's [`WriteSources`]: the
/// window's pass hands the text over and goes on, never waiting on the
/// node (its vault may be held by another writer, a scan's image map or
/// a job). Texts given while one is written are coalesced, the last one
/// kept. The thread ends when the writer is dropped; it is never joined.
pub fn write_in_background(
    write: impl Fn(&str) -> Result<(), String> + Send + 'static,
) -> WriteSources {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let spawned = std::thread::Builder::new()
        .name("helios-import-sources".into())
        .spawn(move || {
            while let Ok(mut text) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    text = newer;
                }
                if let Err(e) = write(&text) {
                    log::warn!("import sources: {DOCUMENT}: {e}");
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("import sources: no thread to write them on: {e}");
    }
    Box::new(move |text| {
        tx.send(text.to_owned())
            .map_err(|_| "the sources' writer has stopped".to_owned())
    })
}

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
    /// Its connector's own parameters: the keys of its source contract
    /// that are not choices, as set.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
    /// The browser it was last shown in, among its connector's
    /// ([`super::import_browsers`]); `None`: the connector's first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<helios_connect::Browser>,
    /// When it imports on its own ([`super::import_auto`]); `None`: only
    /// when asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto: Option<Trigger>,
}

impl SavedSource {
    /// A source of `connector` at `key`, with nothing of its own yet.
    pub(crate) fn new(connector: String, key: &SourceKey) -> Self {
        let (path, account) = match key {
            SourceKey::Folder(p) => (Some(p.clone()), None),
            SourceKey::Account(a) => (None, Some(a.clone())),
        };
        Self {
            connector,
            name: None,
            path,
            account,
            choices: None,
            params: BTreeMap::new(),
            browser: None,
            auto: None,
        }
    }

    pub(crate) fn key(&self) -> Option<SourceKey> {
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

/// The sources of the document's TOML `text`; none when it does not read.
pub(crate) fn parse(text: &str) -> Vec<SavedSource> {
    toml::from_str::<Document>(text).map_or_else(
        |e| {
            log::warn!("import sources: {DOCUMENT}: {e}");
            Vec::new()
        },
        |d| d.sources,
    )
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

    /// What a connector honours, its source contract having the last
    /// word when it declares one: without a `copy` key its photos are
    /// always copied, without `darktable` no sidecars are read.
    pub fn of_connector(caps: &Caps, contract: &helios_core::config::Contract) -> Self {
        let mut honours = Self::of(caps);
        if !contract.is_empty() {
            honours.add_in_place &= contract.get("copy").is_some();
            honours.darktable &= contract.get("darktable").is_some();
        }
        honours
    }
}

/// A change of a source (its choices, an edit, a forget), for Undo.
struct Step {
    /// The source as it is after the change, when there is one.
    key: Option<SourceKey>,
    /// The source before, and its place in the list.
    before: Option<(usize, SavedSource)>,
    /// The recent folders and favourites before, when the change took a
    /// folder off them.
    memory: Option<(Vec<PathBuf>, Vec<PathBuf>)>,
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

    pub(crate) fn get(&self, key: &SourceKey) -> Option<&SavedSource> {
        self.list.iter().find(|s| s.key().as_ref() == Some(key))
    }

    /// The source of `key` and its place, as a step's `before`.
    fn before(&self, key: &SourceKey) -> Option<(usize, SavedSource)> {
        self.list
            .iter()
            .position(|s| s.key().as_ref() == Some(key))
            .map(|i| (i, self.list[i].clone()))
    }

    /// Puts `source` in place of the one of `key` (at its place), or takes
    /// it away (`None`); one Undo step, its own.
    fn replace(&mut self, key: Option<&SourceKey>, source: Option<SavedSource>) {
        let before = key.and_then(|k| self.before(k));
        let after = source.as_ref().and_then(SavedSource::key);
        let at = before.as_ref().map(|(i, _)| *i);
        self.list
            .retain(|s| s.key().is_none() || (s.key().as_ref() != key && s.key() != after));
        if let Some(source) = source {
            let at = at.unwrap_or(self.list.len()).min(self.list.len());
            self.list.insert(at, source);
        }
        let now = Instant::now();
        self.undo.push(Step {
            key: after,
            before,
            memory: None,
            at: now.checked_sub(ONE_STEP).unwrap_or(now),
        });
        self.store();
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

    /// Saves a source (the "Add a source…" wizard's), in place of one
    /// of the same folder or account.
    pub(crate) fn add(&mut self, source: SavedSource) {
        let key = source.key();
        self.list.retain(|s| key.is_none() || s.key() != key);
        self.list.push(source);
        self.store();
    }

    /// A source's name, when the person gave one.
    pub(crate) fn name_of(&self, key: &SourceKey) -> Option<&str> {
        self.get(key).and_then(|s| s.name.as_deref())
    }

    /// The sources named, in the order saved: their own rows in the
    /// Sources column.
    pub(crate) fn named(&self) -> Vec<(SourceKey, String)> {
        self.list
            .iter()
            .filter_map(|s| Some((s.key()?, s.name.clone()?)))
            .collect()
    }

    /// Sets a source's own choices, saving the source when new.
    fn set(&mut self, key: &SourceKey, connector: &str, choices: Option<Choices>) {
        if self.get(key).is_none() && choices.is_none() {
            return;
        }
        if let Some(s) = self.list.iter_mut().find(|s| s.key().as_ref() == Some(key)) {
            s.choices = choices;
        } else {
            self.list.push(SavedSource {
                choices,
                ..SavedSource::new(connector.to_string(), key)
            });
        }
        self.store();
    }

    /// Sets the browser a source is shown in, saving the source when new;
    /// not a step of the column's Undo (the view, not the source, changes).
    pub(crate) fn set_browser(
        &mut self,
        key: &SourceKey,
        connector: &str,
        browser: helios_connect::Browser,
    ) {
        if let Some(s) = self.list.iter_mut().find(|s| s.key().as_ref() == Some(key)) {
            s.browser = Some(browser);
        } else {
            self.list.push(SavedSource {
                browser: Some(browser),
                ..SavedSource::new(connector.to_string(), key)
            });
        }
        self.store();
    }

    fn read_kept(&mut self) {
        let Some(read) = &self.read else { return };
        self.list = match read() {
            Ok(Some(text)) => parse(&text),
            Ok(None) => Vec::new(),
            Err(e) => {
                log::warn!("import sources: {DOCUMENT}: {e}");
                Vec::new()
            }
        };
    }

    pub(crate) fn store(&self) {
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
        let key = SourceKey::Folder(last.clone());
        // A card is an automatic source, unless saved.
        (!self.is_card(last) || self.sources.get(&key).is_some()).then_some(key)
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
        if self.is_card(path) && !self.sources.has_own(path) {
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
        connector.map_or_else(Honours::default, |c| self.connector_honours(&c))
    }

    /// What the connector `id` honours, by its capabilities and its
    /// source contract.
    pub(super) fn connector_honours(&self, id: &str) -> Honours {
        self.connectors
            .provider(id)
            .map_or_else(Honours::default, |p| {
                Honours::of_connector(&p.manifest().caps, &p.config().source)
            })
    }

    pub(super) fn connector_of(&mut self, key: &SourceKey) -> String {
        if let Some(saved) = self.sources.get(key) {
            return saved.connector.clone();
        }
        match key {
            SourceKey::Folder(p) if self.is_card(p) => helios_connect::card::ID.to_string(),
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
        let _stall = crate::stall::section("import window: a source's choices kept");
        if let Some(key) = shown.filter(|k| Some(k) == self.sources.showing.as_ref())
            && self.choices != *before
        {
            let earlier = self.sources.before(&key);
            let now = Instant::now();
            let joined = self.sources.undo.last().is_some_and(|s| {
                s.key.as_ref() == Some(&key) && now.duration_since(s.at) < ONE_STEP
            });
            match self.sources.undo.last_mut() {
                Some(step) if joined => step.at = now,
                _ => self.sources.undo.push(Step {
                    key: Some(key.clone()),
                    before: earlier,
                    memory: None,
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

    /// Takes back the last change of a source: its choices, an edit or
    /// a forget.
    pub(super) fn undo_source_choice(&mut self) {
        let Some(step) = self.sources.undo.pop() else {
            return;
        };
        let list = &mut self.sources.list;
        if let Some(key) = &step.key {
            list.retain(|s| s.key().as_ref() != Some(key));
        }
        let back = step.before.as_ref().and_then(|(_, s)| s.key());
        if let Some((at, source)) = step.before {
            list.retain(|s| s.key().is_none() || s.key() != back);
            list.insert(at.min(list.len()), source);
        }
        self.sources.store();
        if let Some((recent, favourites)) = step.memory {
            self.memory.recent = recent;
            self.memory.favourites = favourites;
            self.save();
        }
        // The column shows the selected source afresh.
        if self.sources.showing.is_some() {
            self.choices = self.sources.window.clone();
            self.sources.showing = None;
        }
    }

    /// Saves `source` in place of the source of `key` (an edit, or a
    /// remembered source promoted when it had none): one Undo step.
    pub(super) fn replace_source(&mut self, key: &SourceKey, source: SavedSource) {
        self.sources.replace(Some(key), Some(source));
        if self.sources.showing.is_some() {
            self.choices = self.sources.window.clone();
            self.sources.showing = None;
        }
    }

    /// "Forget this source": the source of `key` is no longer saved, nor a
    /// recent folder or a favourite; it is unticked. One Undo step.
    pub(super) fn forget_source(&mut self, key: &SourceKey) {
        let memory = (self.memory.recent.clone(), self.memory.favourites.clone());
        self.sources.replace(Some(key), None);
        let mut took = false;
        match key {
            SourceKey::Folder(path) => {
                took = self.memory.recent.contains(path) || self.memory.favourites.contains(path);
                self.memory.recent.retain(|p| p != path);
                self.memory.favourites.retain(|p| p != path);
                self.ticked.retain(|p| p != path);
                if took {
                    self.save();
                }
            }
            SourceKey::Account(id) => {
                if self.sources.account.as_deref() == Some(id.as_str()) {
                    self.sources.account = None;
                }
            }
        }
        if took && let Some(step) = self.sources.undo.last_mut() {
            step.memory = Some(memory);
        }
        if self.sources.showing.is_some() {
            self.choices = self.sources.window.clone();
            self.sources.showing = None;
        }
    }

    /// The source shown takes the window's choices again.
    pub(super) fn reset_source_choices(&mut self) {
        let Some(key) = self.sources.showing.clone() else {
            return;
        };
        if self.sources.own(&key).is_none() {
            return;
        }
        // Its own step: a change right after it is not joined to it.
        let now = Instant::now();
        self.sources.undo.push(Step {
            key: Some(key.clone()),
            before: self.sources.before(&key),
            memory: None,
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
        w.browsing.fresh = false;
    }

    /// The browsers the import window's Contents column offers for the
    /// source it shows, and the one shown.
    pub fn import_source_browsers(
        &self,
    ) -> (
        Vec<helios_connect::Browser>,
        Option<helios_connect::Browser>,
    ) {
        let w = self.import.get();
        (w.browsing.offered.clone(), w.browsing.current)
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

    /// Takes back the last change of an import source: its choices, an
    /// edit or a forget.
    pub fn undo_import_source_choice(&mut self) {
        self.import.get_mut().undo_source_choice();
    }

    /// "Forget this source" on the import window's folder `path`.
    pub fn forget_import_source(&mut self, path: &std::path::Path) {
        self.import
            .get_mut()
            .forget_source(&SourceKey::Folder(path.to_path_buf()));
    }

    /// The import window's recent folders.
    pub fn import_recent(&self) -> Vec<PathBuf> {
        self.import.get().memory.recent.clone()
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
                    params: BTreeMap::new(),
                    browser: Some(helios_connect::Browser::Timeline),
                    auto: Some(Trigger::NewFiles),
                },
                SavedSource {
                    connector: "google-photos".into(),
                    name: Some("Family".into()),
                    path: None,
                    account: Some("google-photos-1".into()),
                    choices: None,
                    params: [("album".to_owned(), serde_json::json!("Holidays"))].into(),
                    browser: None,
                    auto: None,
                },
            ],
        };
        let text = toml::to_string(&doc).unwrap();
        assert!(text.contains("browser = \"timeline\""), "{text}");
        assert!(text.contains("auto = \"new-files\""), "{text}");
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
