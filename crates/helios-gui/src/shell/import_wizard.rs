//! The import window's "Add a source…" wizard
//! (`docs/design/import-connectors.md`, section 3.7): the window's body
//! becomes its steps, Back, Next and Cancel under them.
//!
//! 1. **Kind**: the connectors installed, each with one line saying what
//!    it does: a folder, cards and cameras, then those signing in to an
//!    account (a plugin's, such as Google Photos).
//! 2. **Setup**, the connector's own: a folder typed or chosen; an
//!    account picked among the connector's, or signed in to through the
//!    connection dialog (a plugin's consent asked first when it lacks
//!    capabilities). A card needs nothing: cards are listed on their own.
//! 3. **Choices**: the Choices column's, started from the window's and
//!    without those the connector cannot honour ([`Honours::of`]); for a
//!    folder or a card, "Import automatically" ([`super::import_auto`]).
//! 4. **Name**: the folder's, or the account's, to start with.
//!
//! The source is then saved ([`super::import_sources::Sources::add`]) and
//! selected: a folder ticked, an account clicked. Cancel leaves nothing
//! behind; an account signed in to on the way stays an account.
//!
//! **"Edit source…"** opens the same steps on a source of the Sources
//! column, from its setup: the folder or account, then its connector's
//! own parameters (the keys of its source contract that are not
//! choices, [`helios_connect::ConnectorProvider::config`]); its choices,
//! copy or add offered only when the connector honours both; its name.
//! Save puts it in place of the source edited, one step of the column's
//! Undo; a remembered source (a recent folder, a card) is saved by it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui_pop::api::files::{self, FILES, FileRequest};
use egui_pop::api::host::Pending;
use egui_pop::core::overlay::DialogLayout;
use helios_connect::ConnectorManifest;
use helios_connect::account::AccountState;
use helios_core::Message;
use helios_core::config::{Contract, KeyType};
use serde_json::{Map, Value};

use super::import_accounts::{Asked, state_text, translated};
use super::import_auto::{self, Trigger};
use super::import_choices::{self, Choices};
use super::import_sources::{Honours, SavedSource, SourceKey};
use super::import_window::ImportWindow;

/// The wizard's steps, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Step {
    Kind,
    Setup,
    Choices,
    Name,
}

/// What a connector's setup asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setup {
    /// A folder on this machine.
    Folder,
    /// Nothing: cards are listed on their own when inserted.
    Card,
    /// An account of the connector.
    Account,
}

impl Setup {
    fn of(manifest: &ConnectorManifest) -> Option<Self> {
        if manifest.id == helios_connect::folder::ID {
            Some(Self::Folder)
        } else if manifest.id == helios_connect::card::ID {
            Some(Self::Card)
        } else if !manifest.connect.is_empty() {
            Some(Self::Account)
        } else {
            None
        }
    }
}

/// The wizard, while it is open.
pub(crate) struct Wizard {
    pub(crate) step: Step,
    /// The connector chosen, and what its setup asks.
    connector: Option<(String, Setup)>,
    /// The folder, as typed or chosen.
    folder: String,
    /// The account picked.
    account: Option<String>,
    /// The accounts there were when the wizard opened: one signed in to
    /// since is picked.
    before: Vec<String>,
    choices: Choices,
    honours: Honours,
    custom: bool,
    name: String,
    /// The folder chooser, while it is open.
    choosing: Option<Pending>,
    /// The step's text field takes the focus at its next frame.
    focus: bool,
    /// The source edited ("Edit source…"); `None`: one added.
    editing: Option<SourceKey>,
    /// The connector's own parameters: its source contract without the
    /// choices' keys, and their values.
    contract: Contract,
    params: Map<String, Value>,
    /// When the source imports on its own.
    auto: Option<Trigger>,
}

impl Wizard {
    fn new(choices: Choices, before: Vec<String>) -> Self {
        Self {
            step: Step::Kind,
            connector: None,
            folder: String::new(),
            account: None,
            before,
            choices,
            honours: Honours::default(),
            custom: false,
            name: String::new(),
            choosing: None,
            focus: false,
            editing: None,
            contract: Contract::new(),
            params: Map::new(),
            auto: None,
        }
    }

    /// Whether the parameters set fit the connector's contract.
    fn params_fit(&self) -> bool {
        self.contract
            .check(&Value::Object(self.params.clone()))
            .is_ok()
    }

    /// The folder typed, when it is one.
    fn folder(&self) -> Option<PathBuf> {
        let typed = self.folder.trim();
        let path = Path::new(typed);
        (!typed.is_empty() && path.is_dir()).then(|| path.to_path_buf())
    }

    /// The choices given to an Into chosen while they are shown.
    pub(crate) fn take_into(&mut self, into: PathBuf) -> bool {
        if self.step == Step::Choices {
            self.choices.into = Some(into);
            return true;
        }
        false
    }
}

/// The connectors the wizard offers: a folder, cards, then those signing
/// in to an account, in the order installed.
fn kinds(w: &ImportWindow) -> Vec<(ConnectorManifest, Setup)> {
    let mut kinds: Vec<_> = w
        .connectors
        .manifests()
        .into_iter()
        .filter_map(|m| Setup::of(&m).map(|s| (m, s)))
        .collect();
    kinds.sort_by_key(|(_, s)| match s {
        Setup::Folder => 0,
        Setup::Card => 1,
        Setup::Account => 2,
    });
    kinds
}

/// The keys of a connector's source contract that are its own
/// parameters, not choices.
fn own_params(contract: &Contract) -> Contract {
    let mut own = contract.clone();
    own.keys
        .retain(|name, key| !Choices::KEYS.contains(&name.as_str()) && key.ty != KeyType::Secret);
    own
}

/// What the wizard's buttons asked this frame.
enum Go {
    Back,
    Next,
    Cancel,
}

impl ImportWindow {
    /// Opens the wizard at its first step, the window's choices to start
    /// from.
    pub(super) fn open_wizard(&mut self) {
        let before = self
            .accounts
            .listed()
            .iter()
            .map(|a| a.id.clone())
            .collect();
        self.wizard = Some(Wizard::new(self.window_choices().clone(), before));
    }

    /// The connector's own parameters for the wizard: its source
    /// contract without the choices' keys, and `saved` over their
    /// defaults.
    fn wizard_params(
        &self,
        connector: &str,
        saved: &BTreeMap<String, Value>,
    ) -> (Contract, Map<String, Value>) {
        let contract = self
            .connectors
            .provider(connector)
            .map(|p| own_params(&p.config().source))
            .unwrap_or_default();
        let mut params = contract.defaults();
        for (k, v) in saved {
            if contract.get(k).is_some() {
                params.insert(k.clone(), v.clone());
            }
        }
        (contract, params)
    }

    /// "Edit source…": the wizard on the source of `key`, from its setup;
    /// a remembered source (a recent folder, a card) is saved by it.
    pub(super) fn open_editor(&mut self, key: SourceKey) {
        let connector = self.connector_of(&key);
        let saved = self.sources.get(&key).cloned();
        let manifest = self.connectors.provider(&connector).map(|p| p.manifest());
        let Some(setup) = manifest.as_ref().and_then(Setup::of) else {
            return;
        };
        let mut wz = Wizard::new(
            saved
                .as_ref()
                .and_then(|s| s.choices.clone())
                .unwrap_or_else(|| self.window_choices().clone()),
            Vec::new(),
        );
        let (contract, params) = self.wizard_params(
            &connector,
            &saved.as_ref().map(|s| s.params.clone()).unwrap_or_default(),
        );
        wz.contract = contract;
        wz.params = params;
        wz.step = Step::Setup;
        wz.connector = Some((connector, setup));
        match &key {
            SourceKey::Folder(p) => wz.folder = p.display().to_string(),
            SourceKey::Account(id) => wz.account = Some(id.clone()),
        }
        wz.auto = saved.as_ref().and_then(|s| s.auto);
        wz.name = saved.and_then(|s| s.name).unwrap_or_default();
        wz.focus = true;
        wz.editing = Some(key);
        self.wizard = Some(wz);
    }

    /// Whether the wizard is open.
    pub(super) fn wizard_open(&self) -> bool {
        self.wizard.is_some()
    }

    /// The wizard in the window's room: its step, then Back, Next and
    /// Cancel.
    pub(super) fn wizard_ui(&mut self, ui: &mut egui::Ui, app: &egui_pop::api::app::AppHandle) {
        let Some(texts) = self.texts.clone() else {
            return;
        };
        let t = |m: Message| texts(m);
        self.follow_wizard_setup();
        let layout = DialogLayout::new(ui, "helios.import.wizard").fill(true);
        let height = layout.body_height(ui);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        let mut body = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("helios.import.wizard.body")
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        body.set_clip_rect(rect.intersect(body.clip_rect()));
        let editing = self.wizard.as_ref().is_some_and(|wz| wz.editing.is_some());
        body.heading(if editing {
            t(Message::new("gui-editor-title", "Edit source"))
        } else {
            t(Message::new("gui-wizard-title", "Add a source"))
        });
        let step = self.wizard.as_ref().map_or(Step::Kind, |wz| wz.step);
        egui::ScrollArea::vertical()
            .id_salt("helios.import.wizard.scroll")
            .auto_shrink([false, false])
            .show(&mut body, |ui| match step {
                Step::Kind => self.kind_ui(ui, &t),
                Step::Setup => {
                    self.setup_ui(ui, &t, app);
                    self.params_ui(ui, &t);
                }
                Step::Choices => self.wizard_choices_ui(ui, &t, app),
                Step::Name => self.name_ui(ui, &t),
            });
        let ready = self.wizard_ready();
        let mut go = None;
        layout.footer(ui, |ui| {
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let next = if step == Step::Name && editing {
                    t(Message::new("gui-editor-save", "Save the source"))
                } else if step == Step::Name {
                    t(Message::new("gui-wizard-add", "Add the source"))
                } else {
                    t(Message::new("gui-wizard-next", "Next"))
                };
                if ui.add_enabled(ready, egui::Button::new(next)).clicked() {
                    go = Some(Go::Next);
                }
                // The editor starts at the setup: its kind is the source's.
                let first = if editing { Step::Setup } else { Step::Kind };
                if step != first
                    && ui
                        .button(t(Message::new("gui-wizard-back", "Back")))
                        .clicked()
                {
                    go = Some(Go::Back);
                }
                if ui
                    .button(t(Message::new("gui-import-window-cancel", "Cancel")))
                    .clicked()
                {
                    go = Some(Go::Cancel);
                }
            });
        });
        match go {
            Some(Go::Cancel) => self.wizard = None,
            Some(Go::Back) => {
                if let Some(wz) = self.wizard.as_mut() {
                    wz.step = match wz.step {
                        Step::Kind | Step::Setup => Step::Kind,
                        Step::Choices => Step::Setup,
                        Step::Name => Step::Choices,
                    };
                }
            }
            Some(Go::Next) => self.wizard_next(),
            None => {}
        }
    }

    /// Whether the step shown is complete: Next can be pressed.
    fn wizard_ready(&self) -> bool {
        let Some(wz) = &self.wizard else {
            return false;
        };
        match wz.step {
            Step::Kind => wz.connector.is_some(),
            Step::Setup => {
                wz.params_fit()
                    && match wz.connector.as_ref().map(|(_, s)| *s) {
                        Some(Setup::Folder) => wz.folder().is_some(),
                        Some(Setup::Account) => wz.account.is_some(),
                        // A card is edited where it is mounted.
                        Some(Setup::Card) => wz.editing.is_some(),
                        None => false,
                    }
            }
            Step::Choices => helios_library::rename::Pattern::parse(&wz.choices.rename).is_ok(),
            Step::Name => !wz.name.trim().is_empty(),
        }
    }

    /// Next: the step after, its fields prepared; after the name, the
    /// source saved and selected.
    fn wizard_next(&mut self) {
        let Some(step) = self.wizard.as_ref().map(|wz| wz.step) else {
            return;
        };
        match step {
            Step::Kind => {
                let connector = self
                    .wizard
                    .as_ref()
                    .and_then(|wz| wz.connector.as_ref())
                    .map(|(id, _)| id.clone())
                    .unwrap_or_default();
                let (contract, params) = self.wizard_params(&connector, &BTreeMap::new());
                if let Some(wz) = self.wizard.as_mut() {
                    wz.contract = contract;
                    wz.params = params;
                    wz.step = Step::Setup;
                    wz.focus = true;
                }
            }
            Step::Setup => {
                let honours = self.wizard_honours();
                let Some(wz) = self.wizard.as_mut() else {
                    return;
                };
                // What the connector cannot honour is never kept on it.
                wz.honours = honours;
                if !honours.add_in_place {
                    wz.choices.copy_folders = true;
                }
                if !honours.darktable {
                    wz.choices.darktable = false;
                }
                wz.step = Step::Choices;
            }
            Step::Choices => {
                let name = self.wizard_default_name();
                if let Some(wz) = self.wizard.as_mut() {
                    if wz.name.trim().is_empty() {
                        wz.name = name;
                    }
                    wz.step = Step::Name;
                    wz.focus = true;
                }
            }
            Step::Name => self.finish_wizard(),
        }
    }

    /// What the chosen connector honours.
    fn wizard_honours(&self) -> Honours {
        self.wizard
            .as_ref()
            .and_then(|wz| wz.connector.as_ref())
            .map_or_else(Honours::default, |(id, _)| self.connector_honours(id))
    }

    /// The name to start with: the folder's, or the account's.
    fn wizard_default_name(&self) -> String {
        let Some(wz) = &self.wizard else {
            return String::new();
        };
        if let Some(id) = &wz.account {
            return self.account_label(id);
        }
        wz.folder().map_or_else(String::new, |p| {
            p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        })
    }

    /// The source saved, with its choices and its name, and selected.
    fn finish_wizard(&mut self) {
        let Some(wz) = self.wizard.take() else {
            return;
        };
        let Some((connector, _)) = wz.connector.clone() else {
            return;
        };
        let path = wz.folder();
        let account = if path.is_some() { None } else { wz.account };
        let key = match (&path, &account) {
            (Some(p), _) => SourceKey::Folder(p.clone()),
            (None, Some(a)) => SourceKey::Account(a.clone()),
            (None, None) => return,
        };
        let mut choices = wz.choices;
        // What the connector cannot honour is never kept on it, whatever
        // the source held before: a copy-only connector's photos are
        // copied.
        if !wz.honours.add_in_place {
            choices.copy_folders = true;
        }
        if !wz.honours.darktable {
            choices.darktable = false;
        }
        // An edited source keeps the browser it was shown in.
        let browser = wz
            .editing
            .as_ref()
            .and_then(|k| self.sources.get(k))
            .and_then(|s| s.browser);
        let source = SavedSource {
            connector,
            name: Some(wz.name.trim().to_string()),
            path,
            account,
            choices: Some(choices),
            params: wz.params.into_iter().collect(),
            browser,
            auto: wz.auto,
        };
        if let Some(old) = &wz.editing {
            self.replace_source(old, source);
            // A folder moved is ticked where it is now.
            if let SourceKey::Folder(was) = old {
                self.ticked.retain(|p| p != was);
            }
        } else {
            self.sources.add(source);
            // The column shows it afresh, even when it was shown already.
            if self.sources.showing.is_some() {
                self.choices = self.sources.window.clone();
                self.sources.showing = None;
            }
        }
        match key {
            SourceKey::Folder(p) => {
                self.sources.account = None;
                self.ticked.retain(|t| *t != p);
                self.ticked.push(p);
            }
            SourceKey::Account(id) => self.select_account(id),
        }
    }

    /// The setup step, each frame: the folder the chooser gave, and an
    /// account signed in to since the wizard opened picked.
    fn follow_wizard_setup(&mut self) {
        let Some(wz) = self.wizard.as_mut() else {
            return;
        };
        if let Some(result) = wz.choosing.as_ref().and_then(Pending::take) {
            wz.choosing = None;
            if let Ok(files::Chosen::Paths(paths)) = files::chosen(&result)
                && let Some(p) = paths.into_iter().next()
            {
                wz.folder = p.display().to_string();
            }
        }
        let wants = match (&wz.connector, &wz.account) {
            (Some((id, Setup::Account)), None) if wz.step == Step::Setup => id.clone(),
            _ => return,
        };
        let before = wz.before.clone();
        let new = self
            .accounts
            .listed()
            .iter()
            .find(|a| {
                a.connector == wants
                    && a.state == AccountState::Connected
                    && !before.contains(&a.id)
            })
            .map(|a| a.id.clone());
        if let Some(wz) = self.wizard.as_mut() {
            wz.account = new;
        }
    }

    fn kind_ui(&mut self, ui: &mut egui::Ui, t: &dyn Fn(Message) -> String) {
        ui.label(t(Message::new(
            "gui-wizard-kind",
            "What kind of source is it?",
        )));
        ui.add_space(4.0);
        let kinds = kinds(self);
        let library = self.accounts.book.is_some();
        let texts = self.texts.clone();
        let Some(wz) = self.wizard.as_mut() else {
            return;
        };
        for (manifest, setup) in kinds {
            let chosen = wz
                .connector
                .as_ref()
                .is_some_and(|(id, _)| *id == manifest.id);
            let enabled = setup != Setup::Account || library;
            let label = translated(texts.as_ref(), &manifest.label);
            let row = ui.add_enabled(enabled, egui::Button::selectable(chosen, label));
            let row = if enabled {
                row
            } else {
                row.on_disabled_hover_text(t(Message::new(
                    "gui-wizard-no-library",
                    "Open a library first: accounts are kept in it",
                )))
            };
            if row.clicked() && !chosen {
                // Another kind: its setup starts afresh.
                wz.connector = Some((manifest.id.clone(), setup));
                wz.folder.clear();
                wz.account = None;
                wz.name.clear();
            }
            ui.indent(("helios.import.wizard.about", &manifest.id), |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(translated(texts.as_ref(), &manifest.description))
                            .weak(),
                    )
                    .wrap(),
                );
            });
        }
    }

    fn setup_ui(
        &mut self,
        ui: &mut egui::Ui,
        t: &dyn Fn(Message) -> String,
        app: &egui_pop::api::app::AppHandle,
    ) {
        let Some((connector, setup)) = self.wizard.as_ref().and_then(|wz| wz.connector.clone())
        else {
            return;
        };
        match setup {
            Setup::Folder => {
                let Some(wz) = self.wizard.as_mut() else {
                    return;
                };
                let mut choose = false;
                ui.horizontal(|ui| {
                    let label = ui.label(t(Message::new("gui-wizard-folder", "Folder")));
                    let field = ui
                        .add(egui::TextEdit::singleline(&mut wz.folder).desired_width(320.0))
                        .labelled_by(label.id);
                    if std::mem::take(&mut wz.focus) {
                        field.request_focus();
                    }
                    choose = ui
                        .button(t(Message::new("gui-wizard-choose", "Choose…")))
                        .clicked();
                });
                if !wz.folder.trim().is_empty() && wz.folder().is_none() {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        t(Message::new("gui-wizard-no-folder", "No folder there")),
                    );
                }
                if choose && let Some(f) = app.service(&FILES) {
                    let request = FileRequest::folder(&t(Message::new(
                        "gui-import-window-choose",
                        "Choose a folder…",
                    )))
                    .purpose("helios.import");
                    wz.choosing = Some(f.choose(app, request));
                }
            }
            Setup::Card if self.wizard.as_ref().is_some_and(|wz| wz.editing.is_some()) => {
                let folder = self.wizard.as_ref().map(|wz| wz.folder.clone());
                ui.label(t(Message::new("gui-editor-card", "Card or camera")));
                ui.weak(folder.unwrap_or_default());
            }
            Setup::Card => {
                ui.add(
                    egui::Label::new(t(Message::new(
                        "gui-wizard-card",
                        "Cards and cameras are listed on their own when they are inserted, with the default choices: there is nothing to add.",
                    )))
                    .wrap(),
                );
            }
            Setup::Account => {
                ui.label(t(Message::new("gui-wizard-account", "Account")));
                let accounts: Vec<_> = self
                    .accounts
                    .listed()
                    .iter()
                    .filter(|a| a.connector == connector)
                    .cloned()
                    .collect();
                let mut sign_in = false;
                let Some(wz) = self.wizard.as_mut() else {
                    return;
                };
                if accounts.is_empty() {
                    ui.weak(t(Message::new("gui-wizard-no-account", "No account yet")));
                }
                for account in accounts {
                    ui.horizontal(|ui| {
                        let chosen = wz.account.as_deref() == Some(account.id.as_str());
                        if ui.selectable_label(chosen, account.shown()).clicked() {
                            wz.account = Some(account.id.clone());
                        }
                        ui.label(egui::RichText::new(t(state_text(&account.state))).small());
                    });
                }
                if ui
                    .button(t(Message::new(
                        "gui-wizard-sign-in",
                        "Sign in to an account…",
                    )))
                    .clicked()
                {
                    sign_in = true;
                }
                if sign_in {
                    // The connection's dialog runs over the wizard; a
                    // plugin's consent is asked first.
                    self.account_asked(Asked::Add(connector));
                }
            }
        }
    }

    /// The Choices column's, on the wizard's choices: what the connector
    /// cannot honour hidden, copy or add kept among them.
    fn wizard_choices_ui(
        &mut self,
        ui: &mut egui::Ui,
        t: &dyn Fn(Message) -> String,
        app: &egui_pop::api::app::AppHandle,
    ) {
        let Some(wz) = self.wizard.as_mut() else {
            return;
        };
        ui.label(t(Message::new(
            "gui-wizard-choices",
            "How its photos are imported. You can change these later in the Choices column.",
        )));
        // The column's widgets, on the wizard's choices: shown as a saved
        // source's (copy or add is one of its choices), no card ticked.
        let mut choices = std::mem::take(&mut wz.choices);
        let mut custom = wz.custom;
        let honours = wz.honours;
        std::mem::swap(&mut self.choices, &mut choices);
        std::mem::swap(&mut self.custom, &mut custom);
        let saved = (
            std::mem::replace(&mut self.sources.honours, honours),
            self.sources
                .showing
                .replace(SourceKey::Folder(PathBuf::new())),
            self.copy.take(),
            std::mem::take(&mut self.ticked),
        );
        let mut words = self.choice_words(t, None);
        words.shown_for = None;
        let asked = import_choices::choices_ui(ui, self, &words, &mut |_| {});
        std::mem::swap(&mut self.choices, &mut choices);
        std::mem::swap(&mut self.custom, &mut custom);
        (
            self.sources.honours,
            self.sources.showing,
            self.copy,
            self.ticked,
        ) = saved;
        if let Some(wz) = self.wizard.as_mut() {
            wz.choices = choices;
            wz.custom = custom;
        }
        if let Some(import_choices::Asked::Into) = asked
            && let Some(f) = app.service(&FILES)
        {
            let request = FileRequest::folder(&words.change).purpose("helios.import.into");
            self.choose_into(f.choose(app, request));
        }
        self.wizard_auto_ui(ui, t);
    }

    /// "Import automatically", for a folder or a card: off, or the
    /// trigger of its kind.
    fn wizard_auto_ui(&mut self, ui: &mut egui::Ui, t: &dyn Fn(Message) -> String) {
        let Some(wz) = self.wizard.as_mut() else {
            return;
        };
        let offered = match wz.connector.as_ref().map(|(_, s)| *s) {
            Some(Setup::Folder) => Trigger::offered(false),
            Some(Setup::Card) => Trigger::offered(true),
            _ => return,
        };
        ui.add_space(8.0);
        ui.strong(t(import_auto::title_message()));
        ui.horizontal(|ui| {
            ui.radio_value(&mut wz.auto, None, t(import_auto::off_message()));
            ui.radio_value(&mut wz.auto, Some(offered), t(offered.message()));
        });
    }

    /// The connector's own parameters, from its source contract: each
    /// key under its label, by its type; those hidden by another's value
    /// left out.
    fn params_ui(&mut self, ui: &mut egui::Ui, t: &dyn Fn(Message) -> String) {
        let Some(wz) = self.wizard.as_mut() else {
            return;
        };
        if wz.contract.is_empty() {
            return;
        }
        ui.add_space(8.0);
        ui.strong(t(Message::new("gui-editor-params", "Parameters")));
        let contract = wz.contract.clone();
        for (name, key) in contract.ordered() {
            if !contract.visible(name, &wz.params) {
                continue;
            }
            let label = if key.label.is_empty() {
                name
            } else {
                key.label.as_str()
            };
            let value = wz.params.get(name).cloned().unwrap_or(Value::Null);
            let changed = ui
                .horizontal(|ui| {
                    let mut changed = None;
                    match key.ty {
                        KeyType::Bool => {
                            let mut on = value.as_bool().unwrap_or(false);
                            if ui.checkbox(&mut on, label).changed() {
                                changed = Some(Value::Bool(on));
                            }
                        }
                        KeyType::Choice => {
                            ui.label(label);
                            for choice in &key.choices {
                                let chosen = value.as_str() == Some(choice.as_str());
                                if ui.radio(chosen, choice).clicked() && !chosen {
                                    changed = Some(Value::String(choice.clone()));
                                }
                            }
                        }
                        KeyType::Int | KeyType::Float => {
                            let l = ui.label(label);
                            let mut n = value.as_f64().unwrap_or(0.0);
                            let mut drag = egui::DragValue::new(&mut n);
                            if let (Some(min), Some(max)) = (key.min, key.max) {
                                drag = drag.range(min..=max);
                            }
                            if key.ty == KeyType::Int {
                                drag = drag.fixed_decimals(0);
                            }
                            if ui.add(drag).labelled_by(l.id).changed() {
                                changed = Some(if key.ty == KeyType::Int {
                                    serde_json::json!(n.round() as i64)
                                } else {
                                    serde_json::json!(n)
                                });
                            }
                        }
                        KeyType::String | KeyType::Path | KeyType::Secret => {
                            let l = ui.label(label);
                            let mut text = value.as_str().unwrap_or_default().to_owned();
                            if ui
                                .add(egui::TextEdit::singleline(&mut text).desired_width(240.0))
                                .labelled_by(l.id)
                                .changed()
                            {
                                changed = Some(Value::String(text));
                            }
                        }
                    }
                    changed
                })
                .inner;
            if !key.help.is_empty() {
                ui.weak(&key.help);
            }
            if let Some(v) = changed {
                wz.params.insert(name.to_owned(), v);
            }
        }
        if !wz.params_fit() {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                t(Message::new(
                    "gui-editor-params-wrong",
                    "A parameter does not fit what the connector takes",
                )),
            );
        }
    }

    fn name_ui(&mut self, ui: &mut egui::Ui, t: &dyn Fn(Message) -> String) {
        let Some(wz) = self.wizard.as_mut() else {
            return;
        };
        ui.horizontal(|ui| {
            let label = ui.label(t(Message::new("gui-wizard-name", "Source name")));
            let field = ui
                .add(egui::TextEdit::singleline(&mut wz.name).desired_width(240.0))
                .labelled_by(label.id);
            if std::mem::take(&mut wz.focus) {
                field.request_focus();
            }
        });
        ui.weak(t(Message::new(
            "gui-wizard-name-help",
            "How the source is listed in the Sources column",
        )));
    }
}

impl super::Shell {
    /// Whether the import window's wizard is open.
    pub fn import_wizard_open(&self) -> bool {
        self.import.get().wizard_open()
    }
}
