//! The Library's sources (`docs/design/library.md`, L5, L6, L12): the
//! Folders, Albums, Keywords and Imports sections, drawn by egui-pop's
//! places panel (`files::places::PlacesPanel`). Helios says what each
//! section holds and acts on what the user did; the panel draws the rows,
//! the trees (volume, folder, subfolders; `Animals`, `Dog`), the folds and
//! the keyboard (Up and Down through the rows shown, Right and Left to open
//! and fold a branch).
//!
//! - **One selection per scope**: a folder, a collection, a keyword and an
//!   import can be chosen together, each highlighted in its section.
//! - **Rows** get their context menu, drop target (photos dropped on an
//!   album, a quick collection or a keyword) and drag source (a keyword)
//!   through the panel's row hook, from egui-pop's drag session.
//! - **Offline volumes** and their folders are dimmed, still choosable, their
//!   menu items disabled.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};

use egui::Ui;
use egui_pop::api::bus::drag::{self, Seen};
use egui_pop::api::files::places::{PlaceItem, PlaceSection, PlacesPanel};
use egui_pop::core::ui::UiPop as _;
use helios_core::Message;
use helios_ui::Action;
use helios_vault::collections::{Album, QUICK, QuickCollection};
use helios_vault::keywords;

use super::keywords::{
    DraggedKeyword, KEYWORD_COMMAND, KEYWORD_MERGE_COMMAND, children, keyword_menu,
};
use super::{
    AlbumTree, COLLECTION_COMMAND, COLLECTION_CREATE_COMMAND, DropOn, FOLDER_COMMAND, FolderCounts,
    FolderNode, IMPORT_SESSION_COMMAND, Library, QUICK_CREATE_COMMAND, VolumeNode, album_menu,
    day_text, drop_refusal, drop_target, folder_menu, quick_label, quick_menu,
};

/// The sections' ids.
const FOLDERS: &str = "folders";
const ALBUMS: &str = "albums";
const KEYWORDS: &str = "keywords";
const IMPORTS: &str = "imports";

/// What a row stands for: what its hook and a click act on.
enum Row {
    /// A volume of the folders tree: its menu acts at its mount point, when
    /// it is a known one.
    Volume {
        mount: Option<String>,
        online: bool,
        known: bool,
    },
    Folder {
        path: String,
        online: bool,
    },
    Quick {
        quick: QuickCollection,
        target: bool,
        filled: bool,
    },
    Album {
        album: Album,
        movable: bool,
    },
    Keyword(String),
    Import(String),
}

impl Row {
    /// The scope a click on it sets: its command, and the value its
    /// argument takes (null when it is the scope already).
    fn scope(&self) -> Option<(&'static str, &'static str, String)> {
        match self {
            Row::Volume { .. } => None,
            Row::Folder { path, .. } => Some((FOLDER_COMMAND, "path", path.clone())),
            Row::Quick { quick, .. } => Some((COLLECTION_COMMAND, "id", quick.id.clone())),
            Row::Album { album, .. } => Some((COLLECTION_COMMAND, "id", album.id.clone())),
            Row::Keyword(path) => Some((KEYWORD_COMMAND, "path", path.clone())),
            Row::Import(id) => Some((IMPORT_SESSION_COMMAND, "id", id.clone())),
        }
    }
}

/// The sections, and what each item's id stands for.
#[derive(Default)]
struct Sources {
    sections: Vec<PlaceSection>,
    rows: HashMap<String, Row>,
}

/// A count as an item's badge.
fn count(n: impl ToString) -> String {
    n.to_string()
}

impl Library {
    /// The sources: Folders, Albums, Keywords and Imports, one panel.
    pub(super) fn sources_panel(&mut self, ui: &mut Ui) {
        self.drop_hover.set(None);
        if self.entries.is_empty() {
            return;
        }
        let ctx = ui.ctx().clone();
        let word = |m: Message| crate::i18n::text(&ctx, m);
        let mut sources = Sources::default();
        self.folder_section(&ctx, &mut sources);
        self.album_section(&ctx, &mut sources);
        self.keyword_section(&ctx, &mut sources);
        self.import_section(&ctx, &mut sources);

        // One selection per scope, each highlighted in its section.
        let mut chosen_ids = Vec::new();
        if let Some(f) = self.folder() {
            chosen_ids.push(format!("folder:{f}"));
        }
        if let Some(c) = self.collection() {
            chosen_ids.push(format!("quick:{c}"));
            chosen_ids.push(format!("album:{c}"));
        }
        if let Some(k) = self.keyword() {
            chosen_ids.push(format!("keyword:{k}"));
        }
        if let Some(i) = self.import_session() {
            chosen_ids.push(format!("import:{i}"));
        }
        let selected: Vec<&str> = chosen_ids.iter().map(String::as_str).collect();

        let selection = self.targets().is_ok();
        let wanted = Cell::new(false);
        let creates: RefCell<Vec<(&'static str, serde_json::Value)>> = RefCell::default();
        let runs: RefCell<Vec<(&'static str, serde_json::Value)>> = RefCell::default();
        let counted = matches!(self.folders, FolderCounts::Read(_));
        let no_album = self.albums.as_ref().is_some_and(Vec::is_empty);
        let rows = &sources.rows;
        let hover = &self.drop_hover;
        let name = format!("helios.library.sources.{}", self.view_id);
        let out = PlacesPanel::new(name, &sources.sections)
            .selected_set(&selected)
            .on_row(|_ui, _section, item, r| {
                let Some(row) = rows.get(&item.id) else {
                    return;
                };
                row_hook(row, r, selection, hover, &runs);
            })
            .under_section(|ui, section| match section.id.as_str() {
                // Counted once by the host, when the section is open: one
                // frame more for the host's turn (no spinner: it would
                // repaint on and on).
                FOLDERS if !counted => wanted.set(true),
                ALBUMS => {
                    if no_album {
                        ui.muted(word(Message::new(
                            "gui-library-albums-none",
                            "No album yet",
                        )));
                    }
                    album_buttons(ui, &word, &mut creates.borrow_mut());
                }
                _ => {}
            })
            .show(ui);

        if wanted.get() && matches!(self.folders, FolderCounts::Unread) {
            self.folders = FolderCounts::Wanted;
            ui.ctx().request_repaint();
        }
        let mut runs = runs.into_inner();
        runs.extend(creates.into_inner());
        if let Some((_, item)) = out.chosen
            && let Some((command, key, value)) = sources.rows.get(&item.id).and_then(Row::scope)
        {
            // A click on the scope in force drops it. Through the shell: the
            // scope is kept with the workspace.
            let same = selected.contains(&item.id.as_str());
            runs.push((
                command,
                serde_json::json!({ key: (!same).then_some(value) }),
            ));
        }
        for (command, args) in runs {
            crate::dialogs::request(
                ui.ctx(),
                crate::dialogs::Request::Run(
                    command.into(),
                    args,
                    helios_shell::commands::Origin::User,
                ),
            );
        }
    }

    /// The Folders section: the volumes, each with its folders, counted by
    /// the vault once the section opens; a click scopes the grid to a
    /// folder (its `folder:` term).
    fn folder_section(&self, ctx: &egui::Context, out: &mut Sources) {
        let title = crate::i18n::text(ctx, Message::new("gui-library-folders", "Folders"));
        let mut section = PlaceSection::new(FOLDERS, title).foldable(false).keep();
        for volume in self.folder_tree().unwrap_or_default() {
            section = section.item(volume_item(ctx, &volume, &mut out.rows));
        }
        out.sections.push(section);
    }

    /// The Albums section: the quick collections pinned first (⚡, the
    /// target marked "+" as in Lightroom), then the albums by name, a set
    /// holding its members; "New album…" and the others below.
    fn album_section(&self, ctx: &egui::Context, out: &mut Sources) {
        let word = |m: Message| crate::i18n::text(ctx, m);
        let mut section =
            PlaceSection::new(ALBUMS, word(Message::new("gui-library-albums", "Albums")))
                .foldable(false)
                .keep();
        // The quick collections, the first always there.
        let first = [QuickCollection {
            id: QUICK.to_string(),
            name: None,
            images: Vec::new(),
        }];
        let quicks = if self.quicks.is_empty() {
            &first[..]
        } else {
            &self.quicks[..]
        };
        for quick in quicks {
            let n = self.quick_count_of(&quick.id);
            let target = quick.id == self.quick_target;
            let tip = if target {
                Message::new(
                    "gui-library-quick-tip",
                    "B adds the selected photos or takes them out",
                )
            } else {
                Message::new(
                    "gui-library-quick-tip-other",
                    "Drop photos here; its menu makes it the target of B",
                )
            };
            let badge = if target { format!("+ {n}") } else { count(n) };
            let id = format!("quick:{}", quick.id);
            section = section.item(
                PlaceItem::new(id.clone(), format!("⚡ {}", quick_label(ctx, quick)))
                    .badge(badge)
                    .detail(word(tip)),
            );
            out.rows.insert(
                id,
                Row::Quick {
                    quick: quick.clone(),
                    target,
                    filled: n > 0,
                },
            );
        }
        if let Some(albums) = self.albums.as_deref() {
            let tree = AlbumTree::of(albums);
            for item in self.album_items(ctx, &tree, None, &mut out.rows) {
                section = section.item(item);
            }
        }
        out.sections.push(section);
    }

    /// The items of the collections in the set `parent` (none: the top), a
    /// set holding its members, open at first.
    fn album_items(
        &self,
        ctx: &egui::Context,
        tree: &AlbumTree<'_>,
        parent: Option<&str>,
        rows: &mut HashMap<String, Row>,
    ) -> Vec<PlaceItem> {
        let word = |m: Message| crate::i18n::text(ctx, m);
        let mut items = Vec::new();
        for album in tree.members(parent) {
            let id = format!("album:{}", album.id);
            let mark = if album.is_smart() { "⚙ " } else { "" };
            let mut item = PlaceItem::new(id.clone(), format!("{mark}{}", album.name))
                .badge(count(self.album_count(album)));
            if album.is_smart() {
                item = item.detail(word(Message::new(
                    "gui-library-album-smart-tip",
                    "Smart album: its rules choose its photos",
                )));
            }
            if album.is_set() {
                item = item
                    .detail(word(Message::new(
                        "gui-library-set-tip",
                        "Album set: its albums' photos together",
                    )))
                    .children(self.album_items(ctx, tree, Some(&album.id), rows))
                    .open_at_first(true);
            }
            rows.insert(
                id,
                Row::Album {
                    album: album.clone(),
                    movable: tree.has_sets_for(album),
                },
            );
            items.push(item);
        }
        items
    }

    /// The Keywords section: the keyword tree with each branch's count.
    fn keyword_section(&mut self, ctx: &egui::Context, out: &mut Sources) {
        let word = |m: Message| crate::i18n::text(ctx, m);
        let counts = self.keyword_counts().clone();
        let section = PlaceSection::new(
            KEYWORDS,
            word(Message::new("gui-library-keywords", "Keywords")),
        )
        .foldable(false)
        .empty(word(Message::new(
            "gui-library-keywords-none",
            "No keyword yet: write levels with | (Animals|Dog)",
        )))
        .items(keyword_items(&counts, None, &mut out.rows));
        out.sections.push(section);
    }

    /// The Imports section: every import session of the import history,
    /// newest first (when, source, how many photos).
    fn import_section(&self, ctx: &egui::Context, out: &mut Sources) {
        let word = |m: Message| crate::i18n::text(ctx, m);
        let mut section = PlaceSection::new(
            IMPORTS,
            word(Message::new("gui-library-imports", "Imports")),
        )
        .foldable(false)
        .empty(word(Message::new(
            "gui-library-imports-none",
            "No import yet",
        )));
        for session in &self.imports {
            let id = format!("import:{}", session.id);
            section = section.item(PlaceItem::new(id.clone(), session.line()));
            out.rows.insert(id, Row::Import(session.id.clone()));
        }
        out.sections.push(section);
    }
}

/// A volume's item, open at first, its folders below: an offline one dimmed,
/// with the day it was last seen.
fn volume_item(
    ctx: &egui::Context,
    volume: &VolumeNode,
    rows: &mut HashMap<String, Row>,
) -> PlaceItem {
    let name = if volume.id.is_some() {
        volume.name.clone()
    } else {
        crate::i18n::text(
            ctx,
            Message::new("gui-library-folders-elsewhere", "Other folders"),
        )
    };
    let id = format!("volume:{}", volume.id.as_deref().unwrap_or_default());
    let mut item = PlaceItem::new(id.clone(), name)
        .badge(count(volume.count))
        .open_at_first(true)
        .dimmed(!volume.online)
        .children(
            volume
                .folders
                .iter()
                .map(|top| folder_item(top, true, volume.online, rows)),
        );
    if !volume.online {
        let day = volume
            .last_seen
            .and_then(|at| i64::try_from(at).ok())
            .and_then(day_text)
            .unwrap_or_else(|| "?".into());
        item = item.line(crate::i18n::text(
            ctx,
            Message::new(
                "gui-library-volume-offline-line",
                "Offline, last seen { $date }",
            )
            .with("date", day),
        ));
    }
    rows.insert(
        id,
        Row::Volume {
            mount: volume.mount.as_ref().map(|m| m.display().to_string()),
            online: volume.online,
            known: volume.id.is_some(),
        },
    );
    item
}

/// A folder's item and the folders below it (a top folder open at first);
/// dimmed on an offline volume.
fn folder_item(
    node: &FolderNode,
    top: bool,
    online: bool,
    rows: &mut HashMap<String, Row>,
) -> PlaceItem {
    let id = format!("folder:{}", node.path);
    let item = PlaceItem::new(id.clone(), node.name.clone())
        .badge(count(node.count))
        .open_at_first(top)
        .dimmed(!online)
        .children(
            node.children
                .iter()
                .map(|child| folder_item(child, false, online, rows)),
        );
    rows.insert(
        id,
        Row::Folder {
            path: node.path.clone(),
            online,
        },
    );
    item
}

/// The keyword branches below `parent` (none: the top), each with its
/// count and the branches below it.
fn keyword_items(
    counts: &BTreeMap<String, u64>,
    parent: Option<&str>,
    rows: &mut HashMap<String, Row>,
) -> Vec<PlaceItem> {
    children(counts, parent)
        .into_iter()
        .map(|(path, n)| {
            let id = format!("keyword:{path}");
            rows.insert(id.clone(), Row::Keyword(path.to_string()));
            PlaceItem::new(id, keywords::leaf(path))
                .badge(count(n))
                .detail(path)
                .children(keyword_items(counts, Some(path), rows))
        })
        .collect()
}

/// "New album…", "New smart album…", "New set…", "New quick collection…".
fn album_buttons(
    ui: &mut Ui,
    word: &dyn Fn(Message) -> String,
    runs: &mut Vec<(&'static str, serde_json::Value)>,
) {
    ui.horizontal_wrapped(|ui| {
        let buttons = [
            (
                Message::new("gui-library-album-new", "New album…"),
                COLLECTION_CREATE_COMMAND,
                serde_json::json!({}),
            ),
            (
                Message::new("gui-library-album-new-smart", "New smart album…"),
                COLLECTION_CREATE_COMMAND,
                serde_json::json!({ "kind": helios_vault::collections::SMART }),
            ),
            (
                Message::new("gui-library-set-new", "New set…"),
                COLLECTION_CREATE_COMMAND,
                serde_json::json!({ "kind": helios_vault::collections::SET }),
            ),
            (
                Message::new("gui-library-quick-new", "New quick collection…"),
                QUICK_CREATE_COMMAND,
                serde_json::json!({}),
            ),
        ];
        for (label, command, args) in buttons {
            if helios_ui::action(ui, Action::Secondary, &word(label)).clicked() {
                runs.push((command, args));
            }
        }
    });
}

/// What a row adds to the panel's: its context menu, and the drag session's
/// drop target or drag source it is.
fn row_hook(
    row: &Row,
    r: &egui::Response,
    selection: bool,
    hover: &Cell<Option<(DropOn, bool)>>,
    runs: &RefCell<Vec<(&'static str, serde_json::Value)>>,
) {
    match row {
        Row::Volume {
            mount,
            online,
            known,
        } => {
            // Known volumes only: "Other folders" is no place on disk.
            if *known {
                folder_menu(r, mount.as_deref(), *online && mount.is_some());
            }
        }
        Row::Folder { path, online } => folder_menu(r, Some(path), *online),
        Row::Quick {
            quick,
            target,
            filled,
        } => {
            quick_menu(r, quick, *target, selection, *filled);
            drop_target(r, DropOn::Quick(quick.id.clone()), None, hover);
        }
        Row::Album { album, movable } => {
            album_menu(r, album, selection, *movable);
            drop_target(
                r,
                DropOn::Album(album.id.clone()),
                drop_refusal(album),
                hover,
            );
        }
        Row::Keyword(path) => keyword_row(r, path, selection, hover, runs),
        Row::Import(_) => {}
    }
}

/// A keyword's row: dragged onto photos or other rows, it tags them or
/// merges into them; photos dropped on it take it.
fn keyword_row(
    r: &egui::Response,
    path: &str,
    selection: bool,
    hover: &Cell<Option<(DropOn, bool)>>,
    runs: &RefCell<Vec<(&'static str, serde_json::Value)>>,
) {
    let ctx = &r.ctx;
    if r.interact(egui::Sense::drag()).drag_started() {
        drag::start(ctx, "keyword", DraggedKeyword(path.to_string()));
    }
    keyword_menu(r, path, selection);
    drop_target(r, DropOn::Keyword(path.to_string()), None, hover);
    // Another branch let go on it merges into it; the branch carried is no
    // target of its own (a drop on itself is neither taken nor refused).
    let own = drag::payload::<DraggedKeyword>(ctx).is_some_and(|d| d.0 == path);
    if !own
        && let Seen::Dropped(dragged) = drag::target::<DraggedKeyword>(
            ctx,
            &format!("library.keyword-merge:{path}"),
            r.rect,
            |_| None,
        )
        && dragged.0 != path
    {
        runs.borrow_mut().push((
            KEYWORD_MERGE_COMMAND,
            serde_json::json!({ "from": dragged.0, "into": path }),
        ));
    }
}
