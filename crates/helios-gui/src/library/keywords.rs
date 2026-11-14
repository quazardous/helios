//! The keyword tree in the Library (`docs/design/library.md`, L12): the
//! photos' tags written as `|` paths (`Animals|Dog`) drawn as a tree in
//! the Keywords section, each branch with how many photos carry a
//! keyword of it.
//!
//! - **Counts** come from the photos the Library holds (their tags are
//!   read with them): no read of the vault, built once after the photos
//!   or their tags change, not every frame.
//! - **A click** scopes the grid to a branch (its `keyword:` term, with
//!   the other filters); a click on it again drops the scope.
//! - **Tagging**: a branch dragged onto a photo of the grid tags it (the
//!   selection when the photo is in it), photos dragged onto a branch
//!   take it; the branch's menu adds it to or removes it from the
//!   selection. Each is a mark of the Library (`library.tag`).
//! - **Rename and merge**: a branch renamed, or merged into another (its
//!   menu, or dropped on the other), on every photo at once, through the
//!   node (`library.keywords.rename`, `.merge`): one step of the
//!   library's history.

use std::collections::BTreeMap;

use egui::Ui;
use helios_core::Message;
use helios_vault::keywords::{self, SEPARATOR};

use super::{Batch, DraggedPhotos, DropOn, Library, drop_target, quoted, tokens};

/// Scopes the library to a keyword branch (`path`; null: none).
pub const KEYWORD_COMMAND: &str = "library.keyword";
/// Tags photos with a keyword (`keyword`; `images`, else the selection).
pub const KEYWORD_ADD_COMMAND: &str = "library.keyword.add";
/// Takes a keyword branch off photos (`keyword`; `images`, else the
/// selection): the path and those below it.
pub const KEYWORD_REMOVE_COMMAND: &str = "library.keyword.remove";
/// Renames a branch on every photo (`from`, `to`); without `to`, asks it.
pub const KEYWORD_RENAME_COMMAND: &str = "library.keyword.rename";
/// Merges a branch into another (`from`, `into`); without `into`, asks
/// it.
pub const KEYWORD_MERGE_COMMAND: &str = "library.keyword.merge";
/// Lists the keyword branches with how many photos carry each.
pub const KEYWORD_LIST_COMMAND: &str = "library.keyword.list";

/// A keyword branch dragged out of the Keywords section (its path): the
/// grid's photos and the other branches take it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraggedKeyword(pub String);

/// What the commands asked of the keyword branches, for the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeywordAsk {
    /// Rename the branch `from` to `to`.
    Rename { from: String, to: String },
    /// Merge the branch `from` into `into`.
    Merge { from: String, into: String },
}

impl KeywordAsk {
    /// The branch it moves, and where to.
    pub fn paths(&self) -> (&str, &str) {
        match self {
            Self::Rename { from, to } => (from, to),
            Self::Merge { from, into } => (from, into),
        }
    }
}

/// The branches below `parent` (none: the top), from the counts.
pub fn children<'a>(
    counts: &'a BTreeMap<String, u64>,
    parent: Option<&str>,
) -> Vec<(&'a str, u64)> {
    let in_level = |path: &str, rest: &str| !rest.contains(SEPARATOR) && !path.is_empty();
    match parent {
        None => counts
            .iter()
            .filter(|(p, _)| in_level(p, p))
            .map(|(p, n)| (p.as_str(), *n))
            .collect(),
        Some(parent) => {
            let start = format!("{parent}{SEPARATOR}");
            // `}` follows `|`: the range holds the paths below `parent`.
            let end = format!("{parent}}}");
            counts
                .range(start.clone()..end)
                .filter(|(p, _)| in_level(p, &p[start.len()..]))
                .map(|(p, n)| (p.as_str(), *n))
                .collect()
        }
    }
}

impl Library {
    /// How many photos of the Library carry a keyword of each branch,
    /// built from the photos' tags once after they change.
    pub fn keyword_counts(&mut self) -> &BTreeMap<String, u64> {
        if self.keyword_counts.is_none() {
            let counts = keywords::counts(self.entries.iter().map(|e| e.tags.as_slice()));
            self.keyword_counts = Some(counts);
        }
        self.keyword_counts.as_ref().expect("just built")
    }

    /// The keyword branch the filter is scoped to (its `keyword:` term).
    pub fn keyword(&self) -> Option<String> {
        super::helios_terms(&self.filter.text).keywords.pop()
    }

    /// Scopes the library to a keyword branch (its `keyword:` term in the
    /// filter, in place of any other), or to none; the rest of the filter
    /// is kept.
    pub fn set_keyword(&mut self, path: Option<&str>) {
        let mut words: Vec<String> = tokens(&self.filter.text)
            .into_iter()
            .filter(|t| !t.to_lowercase().starts_with("keyword:"))
            .collect();
        if let Some(path) = path {
            words.push(format!("keyword:{}", quoted(path)));
        }
        let filter = egui_pop::views::library::filter::Filter {
            text: words.join(" "),
            ..self.filter.clone()
        };
        self.set_filter(filter);
    }

    /// Tags `images` (else the selection) with `keyword`; how many photos.
    ///
    /// # Errors
    /// Not a keyword path, no photo, or a plugin refused the mark.
    pub fn add_keyword(
        &mut self,
        keyword: &str,
        images: Option<Vec<String>>,
    ) -> Result<usize, String> {
        let keyword = keywords::path(keyword)?;
        let keys = self.named_or_targets(images)?;
        let count = keys.len();
        self.mark(egui_pop::views::library::Change::Tag {
            keys,
            add: vec![keyword],
            remove: Vec::new(),
        })?;
        Ok(count)
    }

    /// Takes the branch `keyword` (the path and those below it) off
    /// `images` (else the selection); how many photos had it.
    ///
    /// # Errors
    /// Not a keyword path, no photo, none carrying it, or a plugin
    /// refused the mark.
    pub fn remove_keyword(
        &mut self,
        keyword: &str,
        images: Option<Vec<String>>,
    ) -> Result<usize, String> {
        let keyword = keywords::path(keyword)?;
        let keys = self.named_or_targets(images)?;
        let mut remove: Vec<String> = Vec::new();
        let mut count = 0;
        for k in &keys {
            let Some(e) = self.entry(k) else { continue };
            let mut had = false;
            for t in e.tags.iter().filter(|t| keywords::in_branch(t, &keyword)) {
                had = true;
                if !remove.contains(t) {
                    remove.push(t.clone());
                }
            }
            count += usize::from(had);
        }
        if count == 0 {
            return Err(format!("none of the photos has the keyword “{keyword}”"));
        }
        self.mark(egui_pop::views::library::Change::Tag {
            keys,
            add: Vec::new(),
            remove,
        })?;
        Ok(count)
    }

    /// Asks the branch `from` renamed `to` (a keyword no photo carries
    /// yet), on every photo; how many photos carry it.
    ///
    /// # Errors
    /// Not keyword paths, no photo with `from`, `to` carried already (a
    /// merge), or `to` inside `from`.
    pub fn ask_rename_keyword(&mut self, from: &str, to: &str) -> Result<u64, String> {
        let (from, to) = (keywords::path(from)?, keywords::path(to)?);
        let count = self.check_move(&from, &to)?;
        if self.keyword_counts().contains_key(&to) {
            return Err(format!(
                "“{to}” is a keyword already: merge into it instead"
            ));
        }
        self.ask_keyword(KeywordAsk::Rename { from, to });
        Ok(count)
    }

    /// Asks the branch `from` merged into `into` (a keyword photos
    /// carry), on every photo; how many photos carry `from`.
    ///
    /// # Errors
    /// Not keyword paths, no photo with `from` or with `into`, or `into`
    /// inside `from`.
    pub fn ask_merge_keyword(&mut self, from: &str, into: &str) -> Result<u64, String> {
        let (from, into) = (keywords::path(from)?, keywords::path(into)?);
        let count = self.check_move(&from, &into)?;
        if !self.keyword_counts().contains_key(&into) {
            return Err(format!("no photo has the keyword “{into}”: rename instead"));
        }
        self.ask_keyword(KeywordAsk::Merge { from, into });
        Ok(count)
    }

    /// How many photos carry `from`, when it may move to `to`.
    fn check_move(&mut self, from: &str, to: &str) -> Result<u64, String> {
        if keywords::in_branch(to, from) {
            return Err(format!(
                "“{to}” is in “{from}”: a branch cannot go inside itself"
            ));
        }
        self.keyword_counts()
            .get(from)
            .copied()
            .ok_or_else(|| format!("no photo has the keyword “{from}”"))
    }

    /// Queues a move for the host; a scope on the branch follows it.
    fn ask_keyword(&mut self, ask: KeywordAsk) {
        let (from, to) = ask.paths();
        if let Some(moved) = self
            .keyword()
            .and_then(|scope| keywords::moved(&scope, from, to))
        {
            self.set_keyword(Some(&moved));
        }
        self.keyword_asks
            .push((ask, crate::library_history::current_batch()));
    }

    /// What was asked of the keyword branches since, each with its batch.
    pub fn take_keyword_asks(&mut self) -> Vec<(KeywordAsk, Batch)> {
        std::mem::take(&mut self.keyword_asks)
    }

    /// The Keywords section: the keyword tree with each branch's count; a
    /// click scopes the grid to a branch, a click on it again drops the
    /// scope; a branch with branches below it opens and closes; each
    /// row's menu tags or untags the selection, renames or merges it.
    /// Rows are dragged onto photos or other rows, and take photos.
    pub(super) fn keywords_section(&mut self, ui: &mut Ui) {
        if self.entries.is_empty() {
            return;
        }
        let ctx = ui.ctx().clone();
        let word = |m: Message| crate::i18n::text(&ctx, m);
        let selected = self.keyword();
        let selection = self.targets().is_ok();
        let mut chosen = None;
        let mut runs: Vec<(&'static str, serde_json::Value)> = Vec::new();
        egui::CollapsingHeader::new(word(Message::new("gui-library-keywords", "Keywords")))
            .id_salt("helios.library.keywords")
            .show(ui, |ui| {
                let counts = self.keyword_counts().clone();
                if counts.is_empty() {
                    ui.weak(word(Message::new(
                        "gui-library-keywords-none",
                        "No keyword yet: write levels with | (Animals|Dog)",
                    )));
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("helios.library.keywords.scroll")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        self.keyword_rows(
                            ui,
                            &counts,
                            None,
                            selected.as_deref(),
                            selection,
                            &mut chosen,
                            &mut runs,
                        );
                    });
            });
        if let Some(path) = chosen {
            let same = selected.as_deref() == Some(path.as_str());
            runs.push((
                KEYWORD_COMMAND,
                serde_json::json!({ "path": (!same).then_some(path) }),
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

    /// The rows of the branches below `parent` (none: the top), a branch
    /// with branches below it as a group that opens and closes.
    #[allow(clippy::too_many_arguments)]
    fn keyword_rows(
        &self,
        ui: &mut Ui,
        counts: &BTreeMap<String, u64>,
        parent: Option<&str>,
        selected: Option<&str>,
        selection: bool,
        chosen: &mut Option<String>,
        runs: &mut Vec<(&'static str, serde_json::Value)>,
    ) {
        let ctx = ui.ctx().clone();
        for (path, count) in children(counts, parent) {
            let below = !children(counts, Some(path)).is_empty();
            let key = ui.make_persistent_id(("helios.library.keyword", path));
            let mut state =
                egui::collapsing_header::CollapsingState::load_with_default_open(&ctx, key, false);
            let open = state.is_open();
            ui.horizontal(|ui| {
                if below {
                    let toggle =
                        state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
                    let name = crate::i18n::text(
                        &ctx,
                        Message::new(
                            "gui-library-keyword-toggle",
                            "Show or hide the keywords below { $name }",
                        )
                        .with("name", path),
                    );
                    toggle.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::DisclosureTriangle, true, &name)
                    });
                } else {
                    ui.add_space(ui.spacing().icon_width + ui.spacing().item_spacing.x);
                }
                let text = format!("{} {count}", keywords::leaf(path));
                let r = ui
                    .selectable_label(selected == Some(path), text)
                    .interact(egui::Sense::drag())
                    .on_hover_text(path);
                if r.drag_started() {
                    egui::DragAndDrop::set_payload(&ctx, DraggedKeyword(path.to_string()));
                }
                keyword_menu(&r, path, selection);
                drop_target(
                    &r,
                    DropOn::Keyword(path.to_string()),
                    None,
                    &self.drop_hover,
                );
                if let Some(dragged) = r.dnd_release_payload::<DraggedKeyword>()
                    && dragged.0 != path
                {
                    runs.push((
                        KEYWORD_MERGE_COMMAND,
                        serde_json::json!({ "from": dragged.0, "into": path }),
                    ));
                }
                if r.clicked() {
                    *chosen = Some(path.to_string());
                }
            });
            state.store(&ctx);
            if below && open {
                ui.indent(key, |ui| {
                    self.keyword_rows(ui, counts, Some(path), selected, selection, chosen, runs);
                });
            }
        }
    }

    /// A keyword branch released over the grid: the photo under the
    /// pointer takes it (the selection, when the photo is in it), as a
    /// mark of the Library. `grid` is the grid's response; its cells' ids
    /// follow egui-pop's (`grid.id.with(("cell", key))`).
    pub(super) fn drop_keyword_on_grid(&self, grid: &egui::Response) {
        let ctx = &grid.ctx;
        let Some(dragged) = egui::DragAndDrop::payload::<DraggedKeyword>(ctx) else {
            return;
        };
        let Some(pointer) = ctx.pointer_latest_pos() else {
            return;
        };
        let cell = self.in_view.borrow().iter().find_map(|key: &String| {
            ctx.read_response(grid.id.with(("cell", key)))
                .filter(|r| r.rect.contains(pointer))
                .map(|_| key.clone())
        });
        let Some(cell) = cell else {
            return;
        };
        let selection = self.state.selection();
        let images = if selection.contains(&cell) {
            selection.keys().to_vec()
        } else {
            vec![cell]
        };
        if !ctx.input(|i| i.pointer.any_released()) {
            let text = crate::i18n::text(
                ctx,
                Message::new(
                    "gui-library-keyword-drop",
                    "Tag { $count } photos with { $keyword }",
                )
                .with("count", images.len())
                .with("keyword", dragged.0.as_str()),
            );
            // egui hides hover tooltips while something is dragged.
            egui::Tooltip::for_widget(grid).at_pointer().show(|ui| {
                ui.label(text);
            });
            return;
        }
        let Some(keyword) = egui::DragAndDrop::take_payload::<DraggedKeyword>(ctx) else {
            return;
        };
        crate::dialogs::request(
            ctx,
            crate::dialogs::Request::Run(
                KEYWORD_ADD_COMMAND.into(),
                serde_json::json!({ "keyword": keyword.0, "images": images }),
                helios_shell::commands::Origin::User,
            ),
        );
    }
}

/// A keyword row's context menu: tag or untag the selected photos
/// (greyed with none), rename or merge the branch.
fn keyword_menu(row: &egui::Response, path: &str, selection: bool) {
    use crate::dialogs::MenuItem;
    if !row.secondary_clicked() {
        return;
    }
    let ctx = &row.ctx;
    let word = |m: Message| crate::i18n::text(ctx, m);
    let keyword = serde_json::json!({ "keyword": path });
    let branch = serde_json::json!({ "from": path });
    crate::dialogs::context_menu(
        row,
        vec![
            MenuItem::run(KEYWORD_ADD_COMMAND, keyword.clone())
                .labeled(word(Message::new(
                    "gui-library-keyword-add",
                    "Add to the selected photos",
                )))
                .enabled(selection),
            MenuItem::run(KEYWORD_REMOVE_COMMAND, keyword)
                .labeled(word(Message::new(
                    "gui-library-keyword-remove",
                    "Remove from the selected photos",
                )))
                .enabled(selection),
            MenuItem::Separator,
            MenuItem::run(KEYWORD_RENAME_COMMAND, branch.clone())
                .labeled(word(Message::new("gui-library-keyword-rename", "Rename…"))),
            MenuItem::run(KEYWORD_MERGE_COMMAND, branch).labeled(word(Message::new(
                "gui-library-keyword-merge",
                "Merge into…",
            ))),
        ],
    );
}

/// Photos dropped on a keyword row take it.
pub(super) fn drop_args(path: &str, photos: &DraggedPhotos) -> serde_json::Value {
    serde_json::json!({ "keyword": path, "images": photos.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, tags: &[&str]) -> super::super::Entry {
        super::super::Entry {
            id: id.into(),
            name: format!("{id}.jpg"),
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
            ..super::super::Entry::default()
        }
    }

    #[test]
    fn the_tree_counts_each_photo_once_per_branch() {
        let mut lib = Library::default();
        lib.set_entries(vec![
            entry("a", &["Animals|Dog", "Animals|Cat"]),
            entry("b", &["Animals|Dog|Puppy", "sea"]),
            entry("c", &[]),
        ]);
        let counts = lib.keyword_counts().clone();
        assert_eq!(counts["Animals"], 2);
        assert_eq!(counts["Animals|Dog"], 2);
        assert_eq!(counts["Animals|Dog|Puppy"], 1);
        assert_eq!(
            children(&counts, None),
            [("Animals", 2), ("sea", 1)],
            "the top"
        );
        assert_eq!(
            children(&counts, Some("Animals")),
            [("Animals|Cat", 1), ("Animals|Dog", 2)]
        );
        assert_eq!(children(&counts, Some("Animals|Cat")), []);
        // A tag changes: built again.
        lib.show_field("c", "tags", Some(&serde_json::json!(["Animals|Bird"])));
        assert_eq!(lib.keyword_counts()["Animals"], 3);
    }

    #[test]
    fn a_branch_scopes_tags_untags_and_is_asked_moved() {
        let mut lib = Library::default();
        lib.set_entries(vec![
            entry("a", &["Animals|Dog"]),
            entry("b", &["Pets|Dog"]),
        ]);
        lib.set_keyword(Some("Animals|Dog"));
        assert_eq!(lib.keyword().as_deref(), Some("Animals|Dog"));
        assert_eq!(
            super::super::query_of(lib.filter()).keywords,
            ["Animals|Dog"]
        );
        assert_eq!(lib.add_keyword("Pets | Cat", Some(vec!["a".into()])), Ok(1));
        assert_eq!(lib.entry("a").unwrap().tags, ["Animals|Dog", "Pets|Cat"]);
        assert_eq!(
            lib.remove_keyword("Pets", Some(vec!["a".into(), "b".into()])),
            Ok(1)
        );
        assert_eq!(lib.entry("a").unwrap().tags, ["Animals|Dog"]);
        assert!(lib.remove_keyword("Pets", Some(vec!["a".into()])).is_err());

        assert!(
            lib.ask_rename_keyword("Animals", "Pets").is_err(),
            "a merge"
        );
        assert!(lib.ask_rename_keyword("Animals", "Animals|All").is_err());
        assert!(lib.ask_merge_keyword("Animals", "Nowhere").is_err());
        assert_eq!(lib.ask_rename_keyword("Animals", "Fauna"), Ok(1));
        assert_eq!(
            lib.keyword().as_deref(),
            Some("Fauna|Dog"),
            "the scope follows"
        );
        assert_eq!(lib.ask_merge_keyword("Animals|Dog", "Pets|Dog"), Ok(1));
        let asks: Vec<KeywordAsk> = lib
            .take_keyword_asks()
            .into_iter()
            .map(|(a, _)| a)
            .collect();
        assert_eq!(
            asks,
            [
                KeywordAsk::Rename {
                    from: "Animals".into(),
                    to: "Fauna".into()
                },
                KeywordAsk::Merge {
                    from: "Animals|Dog".into(),
                    into: "Pets|Dog".into()
                },
            ]
        );
        lib.set_keyword(None);
        assert_eq!(lib.keyword(), None);
    }
}
