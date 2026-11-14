//! The import window's browsers (`docs/design/import-connectors.md`,
//! section 3.9): each connector declares the ways its sources can be
//! browsed ([`helios_connect::Browser`], its manifest's `browsers`), and
//! the Contents column shows the selected source in one of them, with a
//! switch between them when it offers more than one.
//!
//! - **All photos** (`folder`): every photo of the source in one flat
//!   grid, the browser's own modes (details, thumbnails…) kept.
//! - **Folder tree** (`tree`): the folders holding photos beside the grid;
//!   a folder clicked shows its photos and its subfolders' alone, and an
//!   Import takes only those.
//! - **By date** (`timeline`): the same grid in the browser's agenda, by
//!   capture day; the mode before comes back when another is chosen.
//! - **Choose in the service's page** (`picker`): no grid yet, the
//!   service's own page ("Choose photos in Google Photos…",
//!   [`super::import_accounts::choose_ui`]).
//! - **Albums** (`collections`): declared by connectors that will list a
//!   service's albums; nothing lists an account's photos yet, so it says
//!   so.
//!
//! The browser chosen is remembered per source: on the saved source
//! (`browser` in `user/import-sources`) when the sources are kept, a
//! folder or an account being saved by it; for a card not saved, or
//! sources not kept, until the window's process ends.

use std::path::{Path, PathBuf};

use egui_pop::api::files::browser::{FileBrowser, MODE_AGENDA};
use helios_connect::Browser;
use helios_core::Message;

use super::import_sources::SourceKey;
use super::import_window::ImportWindow;

/// What the Contents column browses, and how.
#[derive(Default)]
pub(crate) struct Browsing {
    /// The source browsed: the account selected, else the folder or
    /// card ticked last.
    pub(crate) key: Option<SourceKey>,
    /// Its connector's id.
    connector: String,
    /// The browsers its connector offers, in its order.
    pub(crate) offered: Vec<Browser>,
    /// The one shown.
    pub(crate) current: Option<Browser>,
    /// The browsers chosen for sources not kept as saved sources.
    chosen: Vec<(SourceKey, Browser)>,
    /// The grid's mode before the timeline put it on its agenda.
    mode_before: Option<String>,
    /// The source browsed was looked at since the contents or the
    /// sources were read afresh.
    pub(crate) fresh: bool,
}

/// The switch's and the browsers' words.
pub(super) struct BrowseWords {
    pub(super) browse: String,
    tree: String,
    folder: String,
    collections: String,
    timeline: String,
    picker: String,
    pub(super) not_listed: String,
    pub(super) every_folder: String,
}

impl BrowseWords {
    pub(super) fn new(t: &dyn Fn(Message) -> String) -> Self {
        Self {
            browse: t(Message::new("gui-import-browse", "Browse")),
            tree: t(Message::new("gui-import-browse-tree", "Folder tree")),
            folder: t(Message::new("gui-import-browse-folder", "All photos")),
            collections: t(Message::new("gui-import-browse-collections", "Albums")),
            timeline: t(Message::new("gui-import-browse-timeline", "By date")),
            picker: t(Message::new(
                "gui-import-browse-picker",
                "Choose in the service's page",
            )),
            not_listed: t(Message::new(
                "gui-import-browse-not-listed",
                "Helios does not list this source's photos here yet.",
            )),
            every_folder: t(Message::new(
                "gui-import-browse-every-folder",
                "Every folder",
            )),
        }
    }

    pub(super) fn name(&self, b: Browser) -> &str {
        match b {
            Browser::Tree => &self.tree,
            Browser::Folder => &self.folder,
            Browser::Collections => &self.collections,
            Browser::Timeline => &self.timeline,
            Browser::Picker => &self.picker,
        }
    }
}

impl ImportWindow {
    /// The source the Contents column browses: the account selected,
    /// else the folder or card ticked last.
    fn browsed(&self) -> Option<SourceKey> {
        if let Some(id) = &self.sources.account {
            return Some(SourceKey::Account(id.clone()));
        }
        self.ticked.last().map(|p| SourceKey::Folder(p.clone()))
    }

    /// Before the window is drawn: the browsers of the source browsed,
    /// and the one it was last shown in.
    pub(super) fn follow_browser(&mut self) {
        let key = self.browsed();
        if key != self.browsing.key || !self.browsing.fresh {
            self.browsing.fresh = true;
            let (connector, offered) = match &key {
                Some(k) => {
                    let connector = self.connector_of(k);
                    let offered = self
                        .connectors
                        .provider(&connector)
                        .map_or_else(|| vec![Browser::Folder], |p| p.manifest().browsers());
                    (connector, offered)
                }
                None => (String::new(), Vec::new()),
            };
            let remembered = key.as_ref().and_then(|k| {
                self.sources.get(k).and_then(|s| s.browser).or_else(|| {
                    self.browsing
                        .chosen
                        .iter()
                        .find(|(c, _)| c == k)
                        .map(|(_, b)| *b)
                })
            });
            let current = remembered
                .filter(|b| offered.contains(b))
                .or_else(|| offered.first().copied());
            self.browsing.key = key;
            self.browsing.connector = connector;
            self.browsing.offered = offered;
            self.show_in(current);
        }
    }

    /// The Contents column shows the source in `browser`.
    fn show_in(&mut self, browser: Option<Browser>) {
        let was = self.browsing.current;
        self.browsing.current = browser;
        if browser != Some(Browser::Tree) {
            self.contents.set_folder(None);
        }
        let Some(grid) = self.browser.as_mut() else {
            return;
        };
        if browser == Some(Browser::Timeline) && was != Some(Browser::Timeline) {
            self.browsing.mode_before = Some(grid.mode().to_owned());
            grid.set_mode(MODE_AGENDA);
        } else if browser != Some(Browser::Timeline)
            && was == Some(Browser::Timeline)
            && let Some(mode) = self.browsing.mode_before.take()
        {
            grid.set_mode(&mode);
        }
    }

    /// The person chose `browser` for the source browsed: shown, and
    /// remembered on it.
    pub(super) fn choose_browser(&mut self, browser: Browser) {
        let Some(key) = self.browsing.key.clone() else {
            return;
        };
        if self.browsing.current == Some(browser) {
            return;
        }
        self.show_in(Some(browser));
        // A card is an automatic source: saved only by the person.
        let card = matches!(&key, SourceKey::Folder(p) if self.is_card(p))
            && self.sources.get(&key).is_none();
        if self.sources.kept() && !card {
            let connector = self.browsing.connector.clone();
            self.sources.set_browser(&key, &connector, browser);
        } else {
            self.browsing.chosen.retain(|(k, _)| k != &key);
            self.browsing.chosen.push((key, browser));
        }
    }

    /// The grid shows its agenda when the timeline is shown, once the
    /// grid exists.
    pub(super) fn browser_made(grid: &mut FileBrowser, browsing: &mut Browsing) {
        if browsing.current == Some(Browser::Timeline) {
            browsing.mode_before = Some(grid.mode().to_owned());
            grid.set_mode(MODE_AGENDA);
        }
    }
}

/// What the Contents column draws for the source browsed.
pub(super) enum Shown {
    /// The grid (all photos, the tree's folder, the timeline).
    Grid,
    /// An account whose photos are chosen in its service's page (from
    /// the Detail column).
    Picker,
    /// Nothing Helios lists.
    NotListed,
}

/// The switch between the browsers of the source browsed, when it offers
/// more than one; what to draw below it.
pub(super) fn switch_ui(ui: &mut egui::Ui, w: &mut ImportWindow, words: &BrowseWords) -> Shown {
    let offered = w.browsing.offered.clone();
    if offered.len() > 1 {
        let mut chosen = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(&words.browse);
            for b in &offered {
                if ui
                    .selectable_label(w.browsing.current == Some(*b), words.name(*b))
                    .clicked()
                {
                    chosen = Some(*b);
                }
            }
        });
        if let Some(b) = chosen {
            w.choose_browser(b);
        }
    }
    let account = match &w.browsing.key {
        Some(SourceKey::Account(id)) => Some(id.clone()),
        _ => None,
    };
    match (w.browsing.current, account) {
        (Some(Browser::Picker), Some(_)) => Shown::Picker,
        (_, Some(_)) | (Some(Browser::Collections | Browser::Picker), None) => Shown::NotListed,
        _ => Shown::Grid,
    }
}

/// The folder tree beside the grid: "Every folder", then the folders
/// holding photos, indented under their parents; the one clicked is
/// shown alone, with its subfolders.
pub(super) fn tree_ui(ui: &mut egui::Ui, w: &mut ImportWindow, words: &BrowseWords) {
    let folders = w.contents.folders();
    let shown = w.contents.folder().map(Path::to_path_buf);
    let mut chosen: Option<Option<PathBuf>> = None;
    egui::ScrollArea::vertical()
        .id_salt("helios.import.tree")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if ui
                .selectable_label(shown.is_none(), &words.every_folder)
                .clicked()
            {
                chosen = Some(None);
            }
            for (folder, source) in &folders {
                let depth = folder
                    .strip_prefix(source)
                    .map_or(0, |rel| rel.components().count());
                let name = folder.file_name().map_or_else(
                    || folder.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                ui.horizontal(|ui| {
                    #[allow(clippy::cast_precision_loss)]
                    ui.add_space(12.0 * (depth + 1) as f32);
                    if ui
                        .selectable_label(shown.as_ref() == Some(folder), name)
                        .on_hover_text(folder.display().to_string())
                        .clicked()
                    {
                        chosen = Some(Some(folder.clone()));
                    }
                });
            }
        });
    if let Some(folder) = chosen {
        w.contents.set_folder(folder);
    }
}
