//! The import window's Sources column, the left zone
//! (`docs/design/import-window.md`), after Lightroom's import source
//! panel: sections that fold, each holding the same item for every
//! source, and one source selected at a time. egui-pop's places panel
//! (`files::places::PlacesPanel`) draws it; this module only says what is
//! in it, from the connectors' sources and the window's memory.
//!
//! - **Favorites**: the sources the person pinned (the pin on an item).
//! - **History**: the folders imported from lately, newest first, from
//!   the import history; then the imports themselves, each with "Import
//!   again" (the panel's room under the section).
//! - **Folders**: the sources given (those of a connector that does not
//!   detect its sources and needs no account).
//! - **Cards and cameras**: the sources a connector detects (`caps.detect`).
//! - **Services**: the accounts (connectors with `caps.accounts`), each
//!   with its albums under it when its connector lists them
//!   (`caps.collections`, the panel's room under the item).
//!
//! One "Add…" button under the title opens the wizard creating a source
//! of any connector installed ([`crate::shell::import_wizard`]); the
//! sections only list what exists, an empty one saying so.
//!
//! Every item is one size and one shape: an icon, the name, one line
//! under it (the path, the account's address and state, the last
//! import), read out as the item's description; an account to reconnect
//! leads its line with a warning mark. Clicking an item selects that
//! source alone: the middle zone shows its photos, the right zone its
//! detail. A source may show in several sections; it is highlighted in
//! each while selected.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui_pop::api::files::places::{
    PlaceItem, PlaceProvider, PlaceSection, PlaceToggle, PlacesPanel,
};
use egui_pop::core::icon::{Icon, Size};
use egui_pop::core::kit;
use egui_pop::core::tooltip::TooltipExt as _;
use egui_pop::core::ui::{TextRole, UiPop as _};
use egui_pop::core::widgets;
use helios_connect::account::AccountState;
use helios_core::Message;
use helios_ui::{Action, Scroll};

use super::{Event, ImportWindow, MenuAsked, Words, account_event, menu_items, name_of};
use crate::backup_end::CardState;
use crate::shell::import_auto::Trigger;
use crate::shell::import_sources::SourceKey;

/// The sections' ids, which the panel's fold state and the window's own
/// content under a section go by.
const FAVOURITES: &str = "favourites";
const HISTORY: &str = "history";
const GIVEN: &str = "given";
const DETECTED: &str = "detected";
const ACCOUNTS: &str = "accounts";

/// The pin's icon, at an item's end.
const PIN: &str = "phosphor:push-pin";

/// An item's id in the panel: its source's, the same in every section, so
/// that a source selected is highlighted wherever it shows.
fn item_id(key: &SourceKey) -> String {
    match key {
        SourceKey::Folder(p) => format!("path:{}", p.display()),
        SourceKey::Account(id) => format!("account:{id}"),
    }
}

/// The Sources column's sections as egui-pop's places: a snapshot built
/// each frame from what the window holds (never a disk or a network),
/// with the source each item stands for.
#[derive(Debug, Default)]
struct SourcePlaces {
    sections: Vec<PlaceSection>,
    /// Each item's id and its source.
    keys: Vec<(String, SourceKey)>,
}

impl PlaceProvider for SourcePlaces {
    fn id(&self) -> &'static str {
        "helios.import.sources"
    }

    fn sections(&self) -> Vec<PlaceSection> {
        self.sections.clone()
    }
}

impl SourcePlaces {
    /// The source an item of the panel stands for.
    fn key(&self, id: &str) -> Option<SourceKey> {
        self.keys
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, k)| k.clone())
    }

    /// A section that folds, open at first, saying `empty` when it has
    /// no item.
    fn section(&mut self, id: &str, title: &str, empty: Option<&str>, items: Vec<Placed>) {
        let mut section = PlaceSection::new(id, title).foldable(true);
        section = match empty {
            Some(line) => section.empty(line),
            None => section.keep(),
        };
        for placed in items {
            if !self.keys.iter().any(|(i, _)| *i == placed.item.id) {
                self.keys.push((placed.item.id.clone(), placed.key));
            }
            section = section.item(placed.item);
        }
        self.sections.push(section);
    }
}

/// One source as a panel's item.
struct Placed {
    key: SourceKey,
    item: PlaceItem,
}

impl ImportWindow {
    /// The source selected: the account, else the folder or card.
    fn selected_key(&self) -> Option<SourceKey> {
        self.sources
            .account
            .clone()
            .map(SourceKey::Account)
            .or_else(|| self.ticked.first().cloned().map(SourceKey::Folder))
    }

    /// Selects `key` alone: the middle and right zones follow it.
    pub(in crate::shell) fn select(&mut self, key: SourceKey) {
        self.touched = true;
        // An album selected is left for the source clicked.
        self.albums.close();
        match key {
            SourceKey::Folder(path) => {
                self.sources.account = None;
                self.ticked = vec![path];
            }
            SourceKey::Account(id) => {
                self.ticked.clear();
                self.select_account(id);
            }
        }
    }

    fn pinned(&self, key: &SourceKey) -> bool {
        match key {
            SourceKey::Folder(p) => self.memory.favourite(p, &self.sources.mounted),
            SourceKey::Account(id) => self.memory.pinned.contains(id),
        }
    }

    /// An item of `key`: its icon, its name, its line, its tip and its
    /// pin, in its state.
    fn placed(
        &self,
        key: SourceKey,
        icon: &str,
        name: String,
        mark: Option<&str>,
        line: &[String],
        tip: Option<String>,
        words: &Words,
    ) -> Placed {
        let pinned = self.pinned(&key);
        let pin = if pinned {
            &words.remove_favourite
        } else {
            &words.add_favourite
        };
        let mut item = PlaceItem::new(item_id(&key), name)
            .icon(icon)
            .toggle(PlaceToggle::new(PIN, pinned, pin.clone()));
        // A state mark leads the line, as a word's sign, not one of its
        // parts.
        let line = match mark {
            Some(mark) => format!("{mark} {}", line.join(" · ")),
            None => line.join(" · "),
        };
        if !line.is_empty() {
            item = item.line(line);
        }
        if let Some(tip) = tip {
            item = item.detail(tip);
        }
        Placed { key, item }
    }

    /// A source of a path's item: its saved name, else its connector's
    /// label, else its own; its last import, else where it is.
    fn path_item(
        &self,
        path: &Path,
        label: Option<String>,
        detected: bool,
        line: Option<Vec<String>>,
        words: &Words,
    ) -> Placed {
        let key = SourceKey::Folder(path.to_path_buf());
        let name = self
            .sources
            .name_of(&key)
            .map(str::to_owned)
            .or(label)
            .unwrap_or_else(|| name_of(path));
        let line = line.unwrap_or_else(|| vec![self.where_line(path)]);
        // Its own choices, marked.
        let own = self.sources.has_own(path);
        let tip = if own {
            format!("{}\n{}", path.display(), words.choices.own_choices)
        } else {
            path.display().to_string()
        };
        // A source its connector detects (a card, a camera) shows as one.
        let icon = if detected || self.is_card(path) {
            "phosphor:sd-card"
        } else {
            "phosphor:folder"
        };
        let mark = own.then_some("●");
        self.placed(key, icon, name, mark, &line, Some(tip), words)
    }

    /// A path source's line by default: its last import, else where it is.
    fn where_line(&self, path: &Path) -> String {
        self.last_import_of(path).unwrap_or_else(|| {
            path.parent()
                .map_or_else(|| path.display().to_string(), |p| p.display().to_string())
        })
    }

    /// When the folder was last imported from, as the history says.
    fn last_import_of(&self, path: &Path) -> Option<String> {
        let when = self.history.last_import_of(path, &self.sources.mounted)?;
        let texts = self.texts.as_ref()?;
        let when = chrono::DateTime::from_timestamp(i64::try_from(when).ok()?, 0)?
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        Some(texts(
            Message::new("gui-import-window-last-import", "Last import { $when }")
                .with("when", when),
        ))
    }

    /// An account's item: its saved source's name, else its own; its
    /// address when the name does not say it, and its state in a word,
    /// marked in the state's tone.
    fn account_item(
        &self,
        account: &helios_connect::account::Account,
        saved: &BTreeMap<String, String>,
        words: &Words,
    ) -> Placed {
        let t = |m: Message| match self.texts.as_ref() {
            Some(t) => t(m),
            None => m.english,
        };
        let attention = matches!(account.state, AccountState::NeedsAttention { .. })
            || self.accounts.identity_missing(account);
        let name = saved
            .get(&account.id)
            .cloned()
            .unwrap_or_else(|| account.label.clone());
        // A connected account's mark is plain, another's muted; one to
        // reconnect is the warning badge leading the line.
        let mark = match account.state {
            _ if attention => None,
            AccountState::Connected => Some("●"),
            _ => Some("○"),
        };
        let mut line = Vec::new();
        if let Some(email) = account.email().filter(|e| !name.contains(e)) {
            line.push(email.to_owned());
        }
        // Why in a word, on the item itself: never in a tooltip only.
        let word = match &account.state {
            _ if attention => Some(Message::new(
                "gui-account-needs-reconnecting",
                "needs reconnecting",
            )),
            AccountState::Connected => None,
            state => Some(crate::shell::import_accounts::state_text(state)),
        };
        if let Some(word) = word {
            line.push(t(word));
        }
        if line.is_empty() {
            let connector = self.connectors.provider(&account.connector).map_or_else(
                || account.connector.clone(),
                |p| {
                    crate::shell::import_accounts::translated(
                        self.texts.as_ref(),
                        &p.manifest().label,
                    )
                },
            );
            line.push(connector);
        }
        let mut placed = self.placed(
            SourceKey::Account(account.id.clone()),
            "phosphor:cloud",
            name,
            mark,
            &line,
            account.email().map(str::to_owned),
            words,
        );
        if attention {
            placed.item = placed.item.badge("▲").badge_tone(kit::Tone::Warning);
        }
        placed
    }

    /// The column's sections, from the connectors' sources, the accounts,
    /// the history and the window's memory.
    fn source_places(&mut self, words: &Words) -> SourcePlaces {
        // The registry's sources: those detected, those given (recent and
        // favourite).
        let mut places = self.memory.recent.clone();
        places.extend(self.memory.favourites.iter().cloned());
        self.connectors.set_places(places);
        let sources = self.connectors.sources();
        let (detected, given): (Vec<_>, Vec<_>) = sources.into_iter().partition(|(_, d)| *d);
        let label_of = |path: &Path| {
            given
                .iter()
                .find(|(s, _)| s.root.as_deref() == Some(path))
                .map(|(s, _)| s.label.clone())
        };
        let named = self.sources.named();
        let saved_accounts: BTreeMap<String, String> = named
            .iter()
            .filter_map(|(k, name)| match k {
                SourceKey::Account(id) => Some((id.clone(), name.clone())),
                SourceKey::Folder(_) => None,
            })
            .collect();
        let accounts = if self.accounts.has_book() {
            self.accounts.listed().to_vec()
        } else {
            Vec::new()
        };
        let account_items: Vec<Placed> = accounts
            .iter()
            .map(|a| self.account_item(a, &saved_accounts, words))
            .collect();
        let detected_items: Vec<Placed> = detected
            .iter()
            .filter_map(|(source, _)| {
                let root = source.root.as_ref()?;
                let mut line = Vec::new();
                if let Some(b) = source.capacity {
                    line.push(helios_storage::capacity_text(b));
                }
                // Its volume, briefly: the same card wherever it is
                // mounted.
                if let Some(id) = self
                    .sources
                    .card_at(root)
                    .and_then(|m| crate::shell::import_sources::brief_id(&m.id))
                {
                    line.push(id);
                }
                let mut line = if line.is_empty() {
                    vec![self.where_line(root)]
                } else {
                    vec![line.join(" · ")]
                };
                // Whether it may be formatted, from the one card list.
                if let Some(state) = self.card_list.state_of(root) {
                    line.push(match state {
                        CardState::Safe => words.card_safe.clone(),
                        CardState::Waiting => words.card_waiting.clone(),
                    });
                }
                Some(self.path_item(root, Some(source.label.clone()), true, Some(line), words))
            })
            .collect();
        let mut out = SourcePlaces::default();
        // Favorites: the sources pinned.
        let mut favourites = Vec::new();
        for path in &self.memory.favourites {
            // Another card mounted where a pinned card was is not the
            // favourite.
            if self.memory.favourite(path, &self.sources.mounted) {
                favourites.push(self.path_item(path, label_of(path), false, None, words));
            }
        }
        for a in &accounts {
            if self.memory.pinned.contains(&a.id) {
                favourites.push(self.account_item(a, &saved_accounts, words));
            }
        }
        out.section(
            FAVOURITES,
            &words.favourites,
            Some(&words.no_favourites),
            favourites,
        );
        // History: the folders imported from lately, then the imports
        // (drawn under the section).
        let recent = self
            .history
            .recent_folders(&self.sources.mounted)
            .iter()
            .map(|p| self.path_item(p, label_of(p), false, None, words))
            .collect();
        out.section(HISTORY, &words.history, None, recent);
        // The sources given: those saved, then those given lately.
        let mut shown: Vec<PathBuf> = Vec::new();
        let mut paths: Vec<PathBuf> = named
            .iter()
            .filter_map(|(k, _)| match k {
                SourceKey::Folder(p) => Some(p.clone()),
                SourceKey::Account(_) => None,
            })
            .collect();
        paths.extend(self.sources.folders());
        paths.extend(self.memory.recent.iter().cloned());
        let mut given_items = Vec::new();
        for path in paths {
            if shown.contains(&path) || self.is_card(&path) {
                continue;
            }
            given_items.push(self.path_item(&path, label_of(&path), false, None, words));
            shown.push(path);
        }
        out.section(GIVEN, &words.folders, Some(&words.no_folders), given_items);
        out.section(
            DETECTED,
            &words.cards,
            Some(&words.no_cards),
            detected_items,
        );
        out.section(
            ACCOUNTS,
            &words.services,
            Some(&words.no_services),
            account_items,
        );
        out
    }
}

/// A path source's menu: "Edit source…" and "Import automatically" for a
/// saved one, "Forget this folder".
fn path_menu(
    ui: &mut egui::Ui,
    w: &mut ImportWindow,
    words: &Words,
    path: &Path,
    event: &mut Option<Event>,
) {
    let kept = w.sources.kept();
    // The automatic import its connector offers, if any.
    let set = w
        .sources
        .get(&SourceKey::Folder(path.to_path_buf()))
        .and_then(|s| s.auto);
    let auto = w
        .path_setup(path)
        .auto
        .map(|offered| (Trigger::from(offered), set));
    let mut asked: Option<MenuAsked> = None;
    if kept {
        asked = menu_items(ui, words, auto);
        ui.divider();
    }
    // The action's button, which egui's menu style draws as a menu's item.
    if helios_ui::action(ui, Action::Secondary, &words.forget)
        .tip(&words.forget_hint)
        .clicked()
    {
        *event = Some(Event::Forget(path.to_path_buf()));
        ui.close();
    }
    if let Some(asked) = asked {
        w.source_menu_asked(asked, SourceKey::Folder(path.to_path_buf()));
    }
}

/// An account's menu: "Choose photos…" when its service's page chooses
/// them, Reconnect, "Manage accounts…", and its saved source's items.
fn account_menu(
    ui: &mut egui::Ui,
    w: &mut ImportWindow,
    words: &Words,
    id: &str,
    event: &mut Option<Event>,
) {
    let Some(texts) = w.texts.clone() else { return };
    let key = SourceKey::Account(id.to_owned());
    let saved = w.sources.get(&key).map(|s| s.auto);
    let kept = w.sources.kept();
    // The automatic import its connector offers, if any.
    let connector = w.connector_of(&key);
    let auto = saved.and_then(|set| {
        w.setup_of(&connector)
            .auto
            .map(|offered| (Trigger::from(offered), set))
    });
    let saved = saved.is_some();
    let picks = w.accounts.picks(id, &w.connectors);
    let mut item = None;
    let asked = crate::shell::import_accounts::account_menu(
        ui,
        picks,
        id,
        &mut |ui: &mut egui::Ui| {
            if kept && saved {
                ui.divider();
                item = menu_items(ui, words, auto);
            }
        },
        &|m| texts(m),
    );
    if let Some(a) = item {
        w.source_menu_asked(a, key);
    }
    if let Some(asked) = asked {
        *event = Some(account_event(asked));
    }
}

/// The albums of the account `id` under its item, when it is selected and
/// its connector lists them ([`crate::shell::import_albums`]): each a
/// source of its own, selected alone on a click; "More albums" reads the
/// next page.
fn albums_ui(ui: &mut egui::Ui, w: &mut ImportWindow, words: &Words, id: &str) {
    if w.sources.account.as_deref() != Some(id) || !w.lists_albums(id) {
        return;
    }
    w.start_albums(id);
    let Some(listed) = w.albums.of(id) else {
        return;
    };
    let b = &words.grid.browse;
    let selected = w.albums.selected(id).map(|s| s.id.clone());
    let mut chosen = None;
    let mut more = false;
    // Indented under their account's item.
    ui.indent(("helios.import.albums", id), |ui| {
        for album in &listed.albums {
            ui.horizontal(|ui| {
                ui.add(
                    egui_pop::core::icon::registry::lookup("phosphor:images")
                        .unwrap_or(Icon::Folder)
                        .size(Size::Inline),
                );
                let on = selected.as_deref() == Some(album.source.id.as_str());
                if widgets::selectable_label(ui, on, &album.source.label).clicked() && !on {
                    chosen = Some(album.source.clone());
                }
                if let Some(n) = album.count {
                    ui.muted(n.to_string());
                }
            });
        }
        if listed.reading {
            ui.muted(&b.albums_reading);
        } else if let Some(why) = &listed.failed {
            ui.muted(format!("{} {why}", b.albums_failed));
        } else if listed.albums.is_empty() {
            ui.muted(&b.albums_none);
        }
        if listed.more() && helios_ui::action(ui, Action::Secondary, &b.albums_more).clicked() {
            more = true;
        }
    });
    if let Some(source) = chosen {
        w.select_album(id, source);
    }
    if more {
        w.more_albums(id);
    }
}

/// The Sources column: its title and "Add…", then its sections in
/// egui-pop's places panel.
pub(super) fn sources_ui(
    ui: &mut egui::Ui,
    w: &mut ImportWindow,
    words: &Words,
    event: &mut Option<Event>,
) {
    ui.horizontal(|ui| {
        ui.text_role(TextRole::Title, &words.sources);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if helios_ui::action(ui, Action::Secondary, &words.add)
                .tip(&words.add_hint)
                .clicked()
            {
                w.open_wizard();
            }
        });
    });
    let places = w.source_places(words);
    // An account is not highlighted while one of its albums is.
    let selected = w
        .selected_key()
        .filter(|k| !matches!(k, SourceKey::Account(id) if w.albums.selected(id).is_some()))
        .map(|k| item_id(&k));
    let sections = places.sections();
    let window = RefCell::new((w, event));
    let out = helios_ui::scroll(ui, "helios.import.sources", Scroll::Region, |ui| {
        PlacesPanel::new("helios.import.sources.places", &sections)
            .selected(selected.as_deref())
            .menu(|ui, _, item| {
                let mut window = window.borrow_mut();
                let (w, event) = &mut *window;
                match places.key(&item.id) {
                    Some(SourceKey::Folder(path)) => path_menu(ui, w, words, &path, event),
                    Some(SourceKey::Account(id)) => account_menu(ui, w, words, &id, event),
                    None => {}
                }
            })
            .under_item(|ui, section, item| {
                if section.id != ACCOUNTS {
                    return;
                }
                if let Some(SourceKey::Account(id)) = places.key(&item.id) {
                    let mut window = window.borrow_mut();
                    albums_ui(ui, window.0, words, &id);
                }
            })
            .under_section(|ui, section| {
                if section.id != HISTORY {
                    return;
                }
                let mut window = window.borrow_mut();
                let (w, event) = &mut *window;
                if let Some(texts) = w.texts.clone() {
                    let hw = crate::shell::import_history::Words::new(&|m| texts(m));
                    if let Some(id) =
                        crate::shell::import_history::history_ui(ui, &w.history, &hw, &|m| texts(m))
                    {
                        **event = Some(Event::Again(id));
                    }
                }
            })
            .show(ui)
    });
    let (w, event) = window.into_inner();
    if let Some((_, item)) = out.chosen
        && let Some(key) = places.key(&item.id)
        && selected.as_deref() != Some(item.id.as_str())
    {
        w.select(key);
    }
    if let Some((_, item)) = out.toggled
        && let Some(key) = places.key(&item.id)
    {
        let pinned = w.pinned(&key);
        *event = Some(match key {
            SourceKey::Folder(p) => Event::Favourite(p, !pinned),
            SourceKey::Account(id) => Event::Pin(id, !pinned),
        });
    }
}
