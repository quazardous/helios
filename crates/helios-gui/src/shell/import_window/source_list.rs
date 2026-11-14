//! The import window's Sources column, the left zone
//! (`docs/design/import-window.md`), after Lightroom's import source
//! panel: collapsible panels, each holding the same item for every
//! source, and one source selected at a time.
//!
//! - **Favorites**: the sources the person pinned (the pin on an item).
//! - **History**: the folders imported from lately, newest first, from
//!   the import history; then the imports themselves, each with "Import
//!   again".
//! - **Folders**: the folders saved or given, and "Choose a folder…".
//! - **Cards and cameras**: the cards mounted.
//! - **Services**: the accounts, and "Add a source…".
//!
//! Every item is one size and one shape: an icon, the name, one detail
//! line (the path, the account's address and state, the last import).
//! Clicking an item selects that source alone: the middle zone shows its
//! photos, the right zone its detail. A source may show in several
//! panels; it is highlighted in each while selected.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui_pop::core::icon::{Icon, Size, Tone};
use helios_connect::account::AccountState;
use helios_core::Message;

use super::{Event, ImportWindow, MenuAsked, Words, account_event, menu_items, name_of};
use crate::shell::import_auto::Trigger;
use crate::shell::import_sources::SourceKey;

/// An icon by its Phosphor name, else egui-pop's own `fallback`.
fn icon(name: &str, fallback: Icon) -> Icon {
    egui_pop::core::icon::registry::lookup(&format!("phosphor:{name}")).unwrap_or(fallback)
}

/// What an item's detail line says, beside its text: an account's state
/// mark, coloured.
struct Mark {
    text: &'static str,
    colour: egui::Color32,
}

/// One source in a panel: what every item shows, whatever its kind.
struct Item {
    key: SourceKey,
    icon: Icon,
    name: String,
    /// The detail line's parts, each a label of its own.
    detail: Vec<String>,
    mark: Option<Mark>,
    /// The full path or address, on hover.
    hover: Option<String>,
}

/// What an item asked this frame.
struct ItemAsked {
    response: egui::Response,
    pin: bool,
}

/// An item: one fixed height, the selected one highlighted, its pin at
/// the end of the line (filled when pinned). Named for screen readers and
/// tests by the source's name.
fn item_ui(
    ui: &mut egui::Ui,
    item: &Item,
    selected: bool,
    pinned: bool,
    words: &Words,
) -> ItemAsked {
    let body = ui.text_style_height(&egui::TextStyle::Body);
    let small = ui.text_style_height(&egui::TextStyle::Small);
    let height = body + small + 16.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let name = item.name.clone();
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &name));
    let visuals = ui.visuals().clone();
    let fill = if selected {
        Some(visuals.selection.bg_fill)
    } else if response.hovered() {
        Some(visuals.widgets.hovered.weak_bg_fill)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    let text_colour = if selected {
        visuals.selection.stroke.color
    } else {
        visuals.text_color()
    };
    let pin_size = body + 4.0;
    let inner = rect.shrink2(egui::vec2(6.0, 5.0));
    let mut pin = false;
    // A child in the item's rect, never allocating past it.
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    {
        let ui = &mut child;
        {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            ui.add(item.icon.size(Size::Toolbar));
            let text_width = (ui.available_width() - pin_size - 4.0).max(0.0);
            ui.allocate_ui_with_layout(
                egui::vec2(text_width, inner.height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(text_width);
                    // The name is painted: the item itself carries it.
                    let galley = ui.painter().layout(
                        item.name.clone(),
                        egui::TextStyle::Body.resolve(ui.style()),
                        text_colour,
                        text_width,
                    );
                    let (at, _) =
                        ui.allocate_exact_size(egui::vec2(text_width, body), egui::Sense::hover());
                    ui.painter()
                        .with_clip_rect(at)
                        .galley(at.min, galley, text_colour);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if let Some(mark) = &item.mark {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(mark.text).small().color(mark.colour),
                                )
                                .selectable(false),
                            );
                        }
                        for part in &item.detail {
                            ui.add(
                                egui::Label::new(egui::RichText::new(part).small().weak())
                                    .selectable(false)
                                    .truncate(),
                            );
                        }
                    });
                },
            );
            let text = if pinned {
                &words.remove_favourite
            } else {
                &words.add_favourite
            };
            let tone = if pinned { Tone::Accent } else { Tone::Muted };
            let button = ui.add(
                icon("push-pin", Icon::Dot)
                    .size(Size::Inline)
                    .tone(tone)
                    .label(text)
                    .selected(pinned)
                    .sense_click(),
            );
            if button.on_hover_text(text).clicked() {
                pin = true;
            }
        }
    }
    let response = match &item.hover {
        Some(h) => response.on_hover_text(h),
        None => response,
    };
    ItemAsked { response, pin }
}

/// A panel: its header, open at first, its items or what it lacks.
fn panel(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(egui::RichText::new(title).strong())
        .id_salt(("helios.import.sources.panel", title))
        .default_open(true)
        .show(ui, add);
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
    pub(super) fn select(&mut self, key: SourceKey) {
        self.touched = true;
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
            SourceKey::Folder(p) => self.memory.favourites.contains(p),
            SourceKey::Account(id) => self.memory.pinned.contains(id),
        }
    }

    /// A folder's item: its saved name, else its own.
    fn folder_item(&self, path: &Path, label: Option<String>, words: &Words) -> Item {
        let key = SourceKey::Folder(path.to_path_buf());
        let name = self
            .sources
            .name_of(&key)
            .map(str::to_owned)
            .or(label)
            .unwrap_or_else(|| name_of(path));
        let parent = path
            .parent()
            .map_or_else(|| path.display().to_string(), |p| p.display().to_string());
        let mut detail = vec![parent];
        if let Some(when) = self.last_import_of(path) {
            detail = vec![when];
        }
        Item {
            key,
            icon: if self.is_card(path) {
                icon("sd-card", Icon::Disc)
            } else {
                icon("folder", Icon::Folder)
            },
            name,
            detail,
            // Its own choices, marked.
            mark: self.sources.has_own(path).then_some(Mark {
                text: "●",
                colour: egui::Color32::GRAY,
            }),
            hover: Some(if self.sources.has_own(path) {
                format!("{}\n{}", path.display(), words.choices.own_choices)
            } else {
                path.display().to_string()
            }),
        }
    }

    /// When the folder was last imported from, as the history says.
    fn last_import_of(&self, path: &Path) -> Option<String> {
        let when = self.history.last_import_of(path)?;
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
    /// address when the name does not say it, and its state in a word.
    fn account_item(
        &mut self,
        account: &helios_connect::account::Account,
        saved: &BTreeMap<String, String>,
        ui: &egui::Ui,
    ) -> Item {
        let t = |m: Message| match self.texts.as_ref() {
            Some(t) => t(m),
            None => m.english,
        };
        let attention = matches!(account.state, AccountState::NeedsAttention { .. })
            || self.accounts.identity_missing(account);
        let (mark, colour) = match account.state {
            _ if attention => ("▲", ui.visuals().warn_fg_color),
            AccountState::Connected => ("●", ui.visuals().text_color()),
            _ => ("○", ui.visuals().weak_text_color()),
        };
        let name = saved
            .get(&account.id)
            .cloned()
            .unwrap_or_else(|| account.label.clone());
        let mut detail = Vec::new();
        if let Some(email) = account.email().filter(|e| !name.contains(e)) {
            detail.push(email.to_owned());
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
            detail.push(t(word));
        }
        if detail.is_empty() {
            let connector = self.connectors.provider(&account.connector).map_or_else(
                || account.connector.clone(),
                |p| {
                    crate::shell::import_accounts::translated(
                        self.texts.as_ref(),
                        &p.manifest().label,
                    )
                },
            );
            detail.push(connector);
        }
        Item {
            key: SourceKey::Account(account.id.clone()),
            icon: icon("cloud", Icon::Circle),
            name,
            detail,
            mark: Some(Mark { text: mark, colour }),
            hover: account.email().map(str::to_owned),
        }
    }
}

/// An item drawn and answered: selected on a click, pinned or unpinned,
/// its menu on a secondary click.
fn show_item(
    ui: &mut egui::Ui,
    w: &mut ImportWindow,
    words: &Words,
    item: &Item,
    event: &mut Option<Event>,
) {
    let selected = w.selected_key().as_ref() == Some(&item.key);
    let pinned = w.pinned(&item.key);
    let asked = item_ui(ui, item, selected, pinned, words);
    if asked.response.clicked() && !selected {
        w.select(item.key.clone());
    }
    if asked.pin {
        match &item.key {
            SourceKey::Folder(p) => *event = Some(Event::Favourite(p.clone(), !pinned)),
            SourceKey::Account(id) => *event = Some(Event::Pin(id.clone(), !pinned)),
        }
    }
    match &item.key {
        SourceKey::Folder(path) => folder_menu(&asked.response, w, words, path, event),
        SourceKey::Account(id) => account_menu(&asked.response, w, words, id, event),
    }
}

/// A folder's or card's menu: "Edit source…" and "Import automatically"
/// for a saved one, "Forget this folder".
fn folder_menu(
    response: &egui::Response,
    w: &mut ImportWindow,
    words: &Words,
    path: &Path,
    event: &mut Option<Event>,
) {
    let mut asked: Option<MenuAsked> = None;
    let kept = w.sources.kept();
    let auto = (
        Trigger::offered(w.is_card(path)),
        w.sources
            .get(&SourceKey::Folder(path.to_path_buf()))
            .and_then(|s| s.auto),
    );
    response.context_menu(|ui| {
        if kept {
            asked = menu_items(ui, words, Some(auto));
            ui.separator();
        }
        if ui
            .button(&words.forget)
            .on_hover_text(&words.forget_hint)
            .clicked()
        {
            *event = Some(Event::Forget(path.to_path_buf()));
            ui.close();
        }
    });
    if let Some(asked) = asked {
        w.source_menu_asked(asked, SourceKey::Folder(path.to_path_buf()));
    }
}

/// An account's menu: "Choose photos…" when its service's page chooses
/// them, Reconnect, "Manage accounts…", and its saved source's items.
fn account_menu(
    response: &egui::Response,
    w: &mut ImportWindow,
    words: &Words,
    id: &str,
    event: &mut Option<Event>,
) {
    let Some(texts) = w.texts.clone() else { return };
    let saved = w.sources.get(&SourceKey::Account(id.to_owned())).is_some();
    let kept = w.sources.kept();
    let mut item = None;
    let asked = crate::shell::import_accounts::account_menu(
        response,
        &mut w.accounts,
        &w.connectors,
        id,
        &mut |ui: &mut egui::Ui| {
            if kept && saved {
                ui.separator();
                item = menu_items(ui, words, None);
            }
        },
        &|m| texts(m),
    );
    if let Some(a) = item {
        w.source_menu_asked(a, SourceKey::Account(id.to_owned()));
    }
    if let Some(asked) = asked {
        *event = Some(account_event(asked));
    }
}

/// The Sources column: its panels, then nothing else.
pub(super) fn sources_ui(
    ui: &mut egui::Ui,
    w: &mut ImportWindow,
    words: &Words,
    event: &mut Option<Event>,
) {
    ui.heading(&words.sources);
    egui::ScrollArea::vertical()
        .id_salt("helios.import.sources")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            // The registry's sources: the cards detected, the folders
            // given (recent and favourite).
            let mut places = w.memory.recent.clone();
            places.extend(w.memory.favourites.iter().cloned());
            w.connectors.set_places(places);
            let sources = w.connectors.sources();
            let (detected, given): (Vec<_>, Vec<_>) = sources.into_iter().partition(|(_, d)| *d);
            let label_of = |path: &Path| {
                given
                    .iter()
                    .find(|(s, _)| s.root.as_deref() == Some(path))
                    .map(|(s, _)| s.label.clone())
            };
            let named = w.sources.named();
            let saved_accounts: BTreeMap<String, String> = named
                .iter()
                .filter_map(|(k, name)| match k {
                    SourceKey::Account(id) => Some((id.clone(), name.clone())),
                    SourceKey::Folder(_) => None,
                })
                .collect();
            let accounts = if w.accounts.has_book() {
                w.accounts.listed().to_vec()
            } else {
                Vec::new()
            };
            let mut account_items = Vec::new();
            for a in &accounts {
                account_items.push(w.account_item(a, &saved_accounts, ui));
            }
            let card_items: Vec<Item> = detected
                .iter()
                .filter_map(|(source, _)| {
                    let root = source.root.as_ref()?;
                    let mut item = w.folder_item(root, Some(source.label.clone()), words);
                    let mut detail = Vec::new();
                    if let Some(b) = source.capacity {
                        detail.push(helios_storage::capacity_text(b));
                    }
                    // The card's volume, briefly: the same card wherever
                    // it is mounted.
                    if let Some(id) = w
                        .sources
                        .card_at(root)
                        .and_then(|m| crate::shell::import_sources::brief_id(&m.id))
                    {
                        detail.push(id);
                    }
                    if !detail.is_empty() {
                        item.detail = vec![detail.join(" · ")];
                    }
                    item.icon = icon("sd-card", Icon::Disc);
                    Some(item)
                })
                .collect();
            // Favorites: the folders and accounts pinned.
            panel(ui, &words.favourites, |ui| {
                let mut any = false;
                for path in w.memory.favourites.clone() {
                    let item = w.folder_item(&path, label_of(&path), words);
                    show_item(ui, w, words, &item, event);
                    any = true;
                }
                let pinned: Vec<&Item> =
                    account_items.iter().filter(|i| w.pinned(&i.key)).collect();
                for item in pinned {
                    show_item(ui, w, words, item, event);
                    any = true;
                }
                if !any {
                    ui.weak(&words.no_favourites);
                }
            });
            // History: the folders imported from lately, then the imports.
            let history_title = words.history.clone();
            panel(ui, &history_title, |ui| {
                for path in w.history.recent_folders() {
                    let item = w.folder_item(&path, label_of(&path), words);
                    show_item(ui, w, words, &item, event);
                }
                if let Some(texts) = w.texts.clone() {
                    let hw = crate::shell::import_history::Words::new(&|m| texts(m));
                    if let Some(id) =
                        crate::shell::import_history::history_ui(ui, &w.history, &hw, &|m| texts(m))
                    {
                        *event = Some(Event::Again(id));
                    }
                }
            });
            // Folders: those saved, then those given lately.
            panel(ui, &words.folders, |ui| {
                let mut shown: Vec<PathBuf> = Vec::new();
                let mut folders: Vec<PathBuf> = named
                    .iter()
                    .filter_map(|(k, _)| match k {
                        SourceKey::Folder(p) => Some(p.clone()),
                        SourceKey::Account(_) => None,
                    })
                    .collect();
                folders.extend(w.sources.folders());
                folders.extend(w.memory.recent.iter().cloned());
                for path in folders {
                    if shown.contains(&path) || w.is_card(&path) {
                        continue;
                    }
                    let item = w.folder_item(&path, label_of(&path), words);
                    show_item(ui, w, words, &item, event);
                    shown.push(path);
                }
                if ui.button(&words.choose).clicked() {
                    *event = Some(Event::Choose);
                }
            });
            panel(ui, &words.cards, |ui| {
                if card_items.is_empty() {
                    ui.weak(&words.no_cards);
                }
                for item in &card_items {
                    show_item(ui, w, words, item, event);
                }
            });
            panel(ui, &words.services, |ui| {
                for item in &account_items {
                    show_item(ui, w, words, item, event);
                }
                if ui.button(&words.add_source).clicked() {
                    w.open_wizard();
                }
            });
        });
}
