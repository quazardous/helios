//! The import window's "Add…" wizard (`docs/design/import-connectors.md`,
//! section 3.7): the window's body becomes its steps, Back, Next and
//! Cancel under them. The window names no connector: every step comes
//! from what the chosen connector declares
//! ([`helios_connect::ConnectorManifest::setup`]).
//!
//! 1. **Kind**: the connectors installed that declare a setup, each with
//!    its icon, its name and one line saying what it does, as it
//!    declares them.
//! 2. **Setup**, what the connector asks ([`SetupAsks`]): a folder typed
//!    or chosen; a volume among those mounted; an account picked among
//!    the connector's, or signed in to through the connection dialog (a
//!    plugin's consent asked first when it lacks capabilities). Then the
//!    connector's own parameters (its source contract's keys that are
//!    not choices).
//! 3. **How to import**: an import profile ([`super::import_profile_store`]),
//!    its connector's default (else Everyday) preselected, each said in
//!    one line; "Customise…" opens the profile editor and comes back
//!    with the profile it left selected. A connector that cannot add in
//!    place says its photos are copied. "Import automatically", when the
//!    connector offers it ([`helios_connect::AutoImport`]).
//! 4. **Name**: the folder's, the volume's or the account's, to start
//!    with.
//!
//! The source is then saved ([`super::import_sources::Sources::add`]),
//! pointing at its profile, and selected: the middle zone lists it.
//! Cancel or Esc leaves nothing behind; an account signed in to on the
//! way stays an account. The command `import.sources.add`
//! ([`super::Shell::add_import_source`]) saves the same source without
//! the wizard.
//!
//! **"Edit source…"** opens the same steps on a source of the Sources
//! column, from its setup. Save puts it in place of the source edited,
//! one step of the column's Undo; a remembered source (a recent folder,
//! a card) is saved by it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui_pop::api::files::{self, FILES, FileRequest};
use egui_pop::api::host::Pending;
use egui_pop::core::icon::{Icon, Size};
use egui_pop::core::overlay::DialogLayout;
use egui_pop::core::param::{self, Mode};
use egui_pop::model::values::Value as PopValue;
use helios_connect::account::AccountState;
use helios_connect::{ConnectorManifest, Setup, SetupAsks};
use helios_core::Message;
use helios_core::config::{Contract, KeyType, Level};
use serde_json::{Map, Value};

use super::import_accounts::{Asked, state_text, translated};
use super::import_auto::{self, Trigger};
use super::import_choices::Choices;
use super::import_profile_store::EVERYDAY;
use super::import_sources::{Honours, SavedSource, SourceKey};
use super::import_window::ImportWindow;
use crate::config::plugins::SECRET_SET;

/// The wizard's steps, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Step {
    Kind,
    Setup,
    How,
    Name,
}

/// The wizard, while it is open.
pub(crate) struct Wizard {
    pub(crate) step: Step,
    /// The connector chosen, and what its setup asks.
    connector: Option<(String, Setup)>,
    /// The folder, as typed or chosen; a volume's mount point.
    folder: String,
    /// The account picked.
    account: Option<String>,
    /// The accounts there were when the wizard opened: one signed in to
    /// since is picked.
    before: Vec<String>,
    /// The import profile picked (its id).
    profile: String,
    /// What the connector honours, read when its setup is done.
    honours: Honours,
    /// Whether the connector copies its photos unless told otherwise
    /// (its source contract's `copy` default).
    copies: bool,
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
    /// The account's settings (its connector's account contract) and
    /// their values, for the account they were read for; a secret set
    /// shows as a mask.
    account_contract: Contract,
    account_values: Map<String, Value>,
    account_read: Option<String>,
    /// When the source imports on its own.
    auto: Option<Trigger>,
}

impl Wizard {
    fn new(before: Vec<String>) -> Self {
        Self {
            step: Step::Kind,
            connector: None,
            folder: String::new(),
            account: None,
            before,
            profile: EVERYDAY.into(),
            honours: Honours::default(),
            copies: false,
            name: String::new(),
            choosing: None,
            focus: false,
            editing: None,
            contract: Contract::new(),
            params: Map::new(),
            account_contract: Contract::new(),
            account_values: Map::new(),
            account_read: None,
            auto: None,
        }
    }

    /// What the chosen connector's setup asks.
    fn asks(&self) -> Option<SetupAsks> {
        self.connector.as_ref().map(|(_, s)| s.asks)
    }

    /// Whether the parameters set fit the connector's contract.
    fn params_fit(&self) -> bool {
        self.params_check().is_ok()
    }

    /// Why the parameters set do not fit the connector's contract.
    fn params_check(&self) -> Result<(), String> {
        self.contract.check(&Value::Object(self.params.clone()))?;
        self.account_contract
            .check(&Value::Object(self.account_values.clone()))
    }

    /// The folder typed, when it is one.
    fn folder(&self) -> Option<PathBuf> {
        let typed = self.folder.trim();
        let path = Path::new(typed);
        (!typed.is_empty() && path.is_dir()).then(|| path.to_path_buf())
    }

    /// The profile the editor left selected, picked as the wizard comes
    /// back.
    pub(super) fn take_profile(&mut self, id: &str) {
        id.clone_into(&mut self.profile);
    }
}

/// The connectors the wizard offers: those declaring a setup, in the
/// order installed.
fn kinds(w: &ImportWindow) -> Vec<(ConnectorManifest, Setup)> {
    w.connectors
        .manifests()
        .into_iter()
        .filter_map(|m| m.setup().map(|s| (m, s)))
        .collect()
}

/// An icon by its Phosphor name, else egui-pop's own `fallback`.
fn icon(name: &str, fallback: Icon) -> Icon {
    egui_pop::core::icon::registry::lookup(&format!("phosphor:{name}")).unwrap_or(fallback)
}

/// The keys of a connector's source contract that are its own
/// parameters, not choices.
pub(super) fn own_params(contract: &Contract) -> Contract {
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

/// A new source, as the wizard or `import.sources.add` gives it: its
/// connector, what its setup asked, and how it imports.
pub(crate) struct NewSource {
    pub(crate) connector: String,
    /// A folder or a volume's mount point.
    pub(crate) path: Option<PathBuf>,
    pub(crate) account: Option<String>,
    pub(crate) name: String,
    pub(crate) profile: String,
    pub(crate) params: BTreeMap<String, Value>,
    pub(crate) auto: Option<Trigger>,
}

impl ImportWindow {
    /// Opens the wizard at its first step.
    pub(super) fn open_wizard(&mut self) {
        let before = self
            .accounts
            .listed()
            .iter()
            .map(|a| a.id.clone())
            .collect();
        self.wizard = Some(Wizard::new(before));
    }

    /// The profile a new source of `connector` starts with: the
    /// connector's default, else Everyday.
    pub(super) fn default_profile(&self, connector: &str) -> String {
        self.profiles
            .defaults
            .get(connector)
            .filter(|id| self.profiles.get(id).is_some())
            .cloned()
            .unwrap_or_else(|| EVERYDAY.into())
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
        let Some(setup) = manifest.as_ref().and_then(ConnectorManifest::setup) else {
            return;
        };
        let mut wz = Wizard::new(Vec::new());
        let (contract, params) = self.wizard_params(
            &connector,
            &saved.as_ref().map(|s| s.params.clone()).unwrap_or_default(),
        );
        wz.contract = contract;
        wz.params = params;
        wz.step = Step::Setup;
        wz.profile = saved
            .as_ref()
            .and_then(|s| s.profile.clone())
            .filter(|id| self.profiles.get(id).is_some())
            .unwrap_or_else(|| self.default_profile(&connector));
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
    /// Cancel. Esc cancels it.
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
                    self.account_settings_ui(ui, &t);
                    self.params_ui(ui, &t);
                }
                Step::How => self.how_ui(ui, &t),
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
        // Esc leaves everything as it was, as Cancel does; not while the
        // folder chooser or a connection dialog is over it.
        let dialog = self.wizard.as_ref().is_some_and(|wz| wz.choosing.is_some())
            || self.accounts.connecting.is_some()
            || self.accounts.consent.is_some();
        if go.is_none() && !dialog && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            go = Some(Go::Cancel);
        }
        match go {
            Some(Go::Cancel) => self.wizard = None,
            Some(Go::Back) => {
                if let Some(wz) = self.wizard.as_mut() {
                    wz.step = match wz.step {
                        Step::Kind | Step::Setup => Step::Kind,
                        Step::How => Step::Setup,
                        Step::Name => Step::How,
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
                    && match wz.asks() {
                        Some(SetupAsks::Path | SetupAsks::Volume) => wz.folder().is_some(),
                        Some(SetupAsks::Account) => wz.account.is_some(),
                        None => false,
                    }
            }
            Step::How => self.profiles.get(&wz.profile).is_some(),
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
                let profile = self.default_profile(&connector);
                if let Some(wz) = self.wizard.as_mut() {
                    wz.contract = contract;
                    wz.params = params;
                    wz.profile = profile;
                    wz.step = Step::Setup;
                    wz.focus = true;
                }
            }
            Step::Setup => {
                let connector = self
                    .wizard
                    .as_ref()
                    .and_then(|wz| wz.connector.as_ref())
                    .map(|(id, _)| id.clone())
                    .unwrap_or_default();
                let honours = self.connector_honours(&connector);
                let copies = self.connector_copies(&connector);
                if let Some(wz) = self.wizard.as_mut() {
                    wz.honours = honours;
                    wz.copies = copies;
                    wz.step = Step::How;
                }
            }
            Step::How => {
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

    /// The name to start with: the account's, the volume's label, or
    /// the folder's.
    fn wizard_default_name(&self) -> String {
        let Some(wz) = &self.wizard else {
            return String::new();
        };
        self.default_name(wz.folder().as_deref(), wz.account.as_deref())
    }

    /// A new source's name to start with: the account's, the volume's
    /// label, or the folder's.
    fn default_name(&self, path: Option<&Path>, account: Option<&str>) -> String {
        if let Some(id) = account {
            return self.account_label(id);
        }
        let Some(p) = path else {
            return String::new();
        };
        if let Some(card) = self.sources.card_at(p) {
            return card.label.clone();
        }
        p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }

    /// The source saved, with its profile and its name, and selected.
    fn finish_wizard(&mut self) {
        let Some(wz) = self.wizard.take() else {
            return;
        };
        let Some((connector, setup)) = wz.connector.clone() else {
            return;
        };
        if let Some(id) = wz
            .account_read
            .as_ref()
            .filter(|id| wz.account.as_ref() == Some(id))
        {
            self.keep_account_settings(id, &wz.account_contract, &wz.account_values);
        }
        let (path, account) = match setup.asks {
            SetupAsks::Path | SetupAsks::Volume => (wz.folder(), None),
            SetupAsks::Account => (None, wz.account.clone()),
        };
        let new = NewSource {
            connector,
            path,
            account,
            name: wz.name.trim().to_string(),
            profile: wz.profile.clone(),
            params: wz.params.into_iter().collect(),
            auto: wz.auto,
        };
        let Some(old) = wz.editing else {
            if let Some(key) = self.add_new_source(new) {
                self.select_new(key);
            }
            return;
        };
        let Some(key) = new
            .path
            .clone()
            .map(SourceKey::Folder)
            .or_else(|| new.account.clone().map(SourceKey::Account))
        else {
            return;
        };
        // An edited source keeps the browser it was shown in, and the
        // card it is on while its folder is the same.
        let edited = self.sources.get(&old);
        let browser = edited.and_then(|s| s.browser);
        let (volume, label) = edited
            .filter(|s| s.path == new.path)
            .map(|s| (s.volume.clone(), s.label.clone()))
            .unwrap_or_default();
        let source = SavedSource {
            connector: new.connector,
            name: Some(new.name),
            path: new.path,
            account: new.account,
            choices: Some(self.profiles.choices(&new.profile)),
            profile: Some(new.profile),
            params: new.params,
            browser,
            auto: new.auto,
            volume,
            label,
            confirmed: edited.and_then(|s| s.confirmed.clone()),
        };
        self.replace_source(&old, source);
        // A folder moved is ticked where it is now.
        if let SourceKey::Folder(was) = &old {
            self.ticked.retain(|p| p != was);
        }
        self.select_new(key);
    }

    /// Saves a new source pointing at its profile; its key. `None`: it
    /// has neither a folder nor an account.
    pub(super) fn add_new_source(&mut self, new: NewSource) -> Option<SourceKey> {
        let key = new
            .path
            .clone()
            .map(SourceKey::Folder)
            .or_else(|| new.account.clone().map(SourceKey::Account))?;
        let source = SavedSource {
            name: Some(new.name),
            choices: Some(self.profiles.choices(&new.profile)),
            profile: Some(new.profile),
            params: new.params,
            auto: new.auto,
            ..SavedSource::new(new.connector, &key)
        };
        self.sources.add(source);
        // The column shows it afresh, even when it was shown already.
        if self.sources.showing.is_some() {
            self.choices = self.sources.window.clone();
            self.sources.showing = None;
        }
        Some(key)
    }

    /// Selects a source just saved: the middle zone lists it.
    pub(super) fn select_new(&mut self, key: SourceKey) {
        match key {
            SourceKey::Folder(p) => {
                self.sources.account = None;
                self.ticked = vec![p];
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
            (Some((id, setup)), None)
                if wz.step == Step::Setup && setup.asks == SetupAsks::Account =>
            {
                id.clone()
            }
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
            // Accounts are kept in the library.
            let enabled = setup.asks != SetupAsks::Account || library;
            let label = translated(texts.as_ref(), &manifest.label);
            let mut clicked = false;
            ui.horizontal(|ui| {
                ui.add(icon(&setup.icon, Icon::Circle).size(Size::Toolbar));
                let row = ui.add_enabled(enabled, egui::Button::selectable(chosen, label));
                let row = if enabled {
                    row
                } else {
                    row.on_disabled_hover_text(t(Message::new(
                        "gui-wizard-no-library",
                        "Open a library first: accounts are kept in it",
                    )))
                };
                clicked = row.clicked();
            });
            if clicked && !chosen {
                // Another kind: its setup starts afresh.
                wz.connector = Some((manifest.id.clone(), setup));
                wz.folder.clear();
                wz.account = None;
                wz.name.clear();
                wz.auto = None;
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
        let editing = self.wizard.as_ref().is_some_and(|wz| wz.editing.is_some());
        match setup.asks {
            SetupAsks::Path => {
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
            SetupAsks::Volume if editing => {
                let folder = self.wizard.as_ref().map(|wz| wz.folder.clone());
                ui.label(t(Message::new("gui-editor-card", "Card or camera")));
                ui.weak(folder.unwrap_or_default());
            }
            SetupAsks::Volume => {
                ui.label(t(Message::new("gui-wizard-volume", "Which one?")));
                let mounted = self.sources.mounted.clone();
                let Some(wz) = self.wizard.as_mut() else {
                    return;
                };
                if mounted.is_empty() {
                    ui.add(
                        egui::Label::new(egui::RichText::new(t(Message::new(
                            "gui-wizard-no-volume",
                            "Insert a card or plug in a camera: it is listed here as soon as it is mounted.",
                        )))
                        .weak())
                        .wrap(),
                    );
                }
                for card in mounted {
                    let root = card.root.display().to_string();
                    let chosen = wz.folder == root;
                    if ui
                        .selectable_label(chosen, &card.label)
                        .on_hover_text(&root)
                        .clicked()
                    {
                        wz.folder.clone_from(&root);
                    }
                }
            }
            SetupAsks::Account => {
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

    /// How to import: the import profiles, each said in one line, the
    /// one picked kept on the source; "Customise…" opens the profile
    /// editor; then "Import automatically" when the connector offers it.
    fn how_ui(&mut self, ui: &mut egui::Ui, t: &dyn Fn(Message) -> String) {
        let Some(wz) = self.wizard.as_ref() else {
            return;
        };
        let (honours, copies, picked) = (wz.honours, wz.copies, wz.profile.clone());
        ui.label(t(Message::new(
            "gui-wizard-how",
            "How its photos are imported: an import profile. You can change it later in the Detail column.",
        )));
        ui.add_space(4.0);
        let mut chosen = None;
        for p in self.profiles.list() {
            let copied = !honours.add_in_place || copies || p.choices.copy_folders;
            if ui.radio(p.id == picked, &p.name).clicked() {
                chosen = Some(p.id.clone());
            }
            ui.indent(("helios.import.wizard.profile", &p.id), |ui| {
                let line = super::import_profile_store::summary(&p.choices, copied, None, t);
                ui.add(egui::Label::new(egui::RichText::new(line).weak()).wrap());
            });
        }
        if !honours.add_in_place {
            ui.add(
                egui::Label::new(t(Message::new(
                    "gui-wizard-copied",
                    "This source's photos are always copied into the Library: it cannot add them where they are.",
                )))
                .wrap(),
            );
        }
        let customise = ui
            .button(t(Message::new("gui-wizard-customise", "Customise…")))
            .on_hover_text(t(Message::new(
                "gui-wizard-customise-help",
                "Opens the import profiles: edit this one, or make a new one; the profile left selected is this source's",
            )))
            .clicked();
        if let Some(id) = chosen
            && let Some(wz) = self.wizard.as_mut()
        {
            wz.profile = id;
        }
        if customise {
            self.open_manager_on(&picked);
        }
        self.wizard_auto_ui(ui, t);
    }

    /// "Import automatically", when the connector offers it: off, or the
    /// trigger it declares.
    fn wizard_auto_ui(&mut self, ui: &mut egui::Ui, t: &dyn Fn(Message) -> String) {
        let Some(wz) = self.wizard.as_mut() else {
            return;
        };
        let Some(offered) = wz
            .connector
            .as_ref()
            .and_then(|(_, s)| s.auto)
            .map(Trigger::from)
        else {
            return;
        };
        ui.add_space(8.0);
        ui.strong(t(import_auto::title_message()));
        ui.horizontal(|ui| {
            ui.radio_value(&mut wz.auto, None, t(import_auto::off_message()));
            ui.radio_value(&mut wz.auto, Some(offered), t(offered.message()));
        });
    }

    /// The connector's own parameters, from its source contract: each
    /// key drawn by egui-pop's parameter widget from the descriptor the
    /// Settings window draws a plugin's settings from
    /// ([`crate::config::plugins::descriptor`]), its label and help
    /// translated through the plugin's catalogue
    /// (`source-setting-<plugin>-<key>`); those hidden by another's value
    /// left out. A value the contract refuses is said with its reason, and
    /// Next waits.
    fn params_ui(&mut self, ui: &mut egui::Ui, t: &dyn Fn(Message) -> String) {
        let owner = self.params_owner();
        let Some(wz) = self.wizard.as_mut() else {
            return;
        };
        if wz.contract.is_empty() {
            return;
        }
        ui.add_space(8.0);
        ui.strong(t(Message::new("gui-editor-params", "Parameters")));
        let contract = wz.contract.clone();
        let fit = contract_ui(ui, t, &owner, Level::Source, &contract, &mut wz.params);
        refused_ui(ui, t, fit);
    }

    /// The account chosen's own settings (its connector's
    /// `[account.settings.*]`), read from its document and the keyring
    /// when it is chosen, drawn as the source's parameters are; kept on
    /// the account when the wizard ends ([`ImportWindow::keep_account_settings`]).
    fn account_settings_ui(&mut self, ui: &mut egui::Ui, t: &dyn Fn(Message) -> String) {
        self.read_account_settings();
        let owner = self.params_owner();
        let Some(wz) = self.wizard.as_mut() else {
            return;
        };
        if wz.account_contract.is_empty() || wz.account.is_none() {
            return;
        }
        ui.add_space(8.0);
        ui.strong(t(Message::new(
            "gui-wizard-account-settings",
            "Account settings",
        )));
        let contract = wz.account_contract.clone();
        let fit = contract_ui(
            ui,
            t,
            &owner,
            Level::Account,
            &contract,
            &mut wz.account_values,
        );
        refused_ui(ui, t, fit);
    }

    /// Reads the account chosen's settings, once per account chosen.
    fn read_account_settings(&mut self) {
        let Some(wz) = self.wizard.as_ref() else {
            return;
        };
        let Some(((connector, setup), Some(id))) =
            wz.connector.as_ref().map(|c| (c, wz.account.as_ref()))
        else {
            return;
        };
        if setup.asks != SetupAsks::Account {
            return;
        }
        if wz.account_read.as_ref() == Some(id) {
            return;
        }
        let (connector, id) = (connector.clone(), id.clone());
        let contract = self
            .connectors
            .provider(&connector)
            .map(|p| p.config().account)
            .unwrap_or_default();
        let mut values = contract.defaults();
        if let Some(book) = &self.accounts.book
            && let Ok(Some(account)) = book.account(&id)
        {
            for (k, v) in &account.facts {
                if contract.get(k).is_some_and(|key| key.ty != KeyType::Secret) {
                    values.insert(k.clone(), v.clone());
                }
            }
        }
        for name in contract.secrets() {
            if let Ok(Some(_)) = self.accounts.secrets.get(&id, name) {
                values.insert(name.to_owned(), Value::String(SECRET_SET.into()));
            }
        }
        if let Some(wz) = self.wizard.as_mut() {
            wz.account_contract = contract;
            wz.account_values = values;
            wz.account_read = Some(id);
        }
    }

    /// Keeps the account's settings the wizard shows: the others on its
    /// document, its secrets in the keyring (a mask left as it is keeps
    /// the secret, an empty one forgets it). Their values are never in a
    /// message.
    fn keep_account_settings(&self, id: &str, contract: &Contract, values: &Map<String, Value>) {
        if contract.is_empty() {
            return;
        }
        let Some(book) = self.accounts.book.clone() else {
            return;
        };
        let secrets = self.accounts.secrets.clone();
        let mut account = match book.account(id) {
            Ok(Some(a)) => a,
            Ok(None) => return,
            Err(e) => {
                log::warn!("the account {id}'s settings not kept: {e}");
                return;
            }
        };
        for (name, key) in &contract.keys {
            let value = values.get(name);
            if key.ty != KeyType::Secret {
                match value {
                    Some(v) => account.facts.insert(name.clone(), v.clone()),
                    None => account.facts.remove(name),
                };
                continue;
            }
            let text = value.and_then(Value::as_str).unwrap_or_default();
            if text == SECRET_SET {
                continue;
            }
            let text = text.strip_prefix(SECRET_SET).unwrap_or(text);
            let kept = if text.is_empty() {
                account.secrets.retain(|s| s != name);
                secrets.delete(id, name)
            } else {
                if !account.secrets.contains(name) {
                    account.secrets.push(name.clone());
                }
                secrets.set(id, name, text)
            };
            if let Err(e) = kept {
                log::warn!("the account {id}'s secret `{name}` not kept: {e}");
            }
        }
        if let Err(e) = book.save(&account) {
            log::warn!("the account {id}'s settings not kept: {e}");
        }
    }

    /// Whose catalogue translates the parameters: the plugin behind the
    /// connector, else the connector.
    fn params_owner(&self) -> String {
        let connector = self
            .wizard
            .as_ref()
            .and_then(|wz| wz.connector.as_ref())
            .map(|(id, _)| id.clone())
            .unwrap_or_default();
        self.accounts
            .plugins
            .as_ref()
            .and_then(|b| helios_plugin::source::plugin_of(b, &connector))
            .unwrap_or(connector)
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

/// Draws a contract's keys with egui-pop's parameter widgets, each
/// from the descriptor Settings draws a plugin's settings from
/// ([`crate::config::plugins::descriptor`]), its label and help through
/// the owner's catalogue; those hidden by another's value left out.
/// Whether the values fit the contract, else why not.
pub(super) fn contract_ui(
    ui: &mut egui::Ui,
    t: &dyn Fn(Message) -> String,
    owner: &str,
    level: Level,
    contract: &Contract,
    values: &mut Map<String, Value>,
) -> Result<(), String> {
    for (name, key) in contract.ordered() {
        if !contract.visible(name, values) {
            continue;
        }
        let msg = helios_core::config::message(owner, level, name);
        let label = if key.label.is_empty() {
            name
        } else {
            key.label.as_str()
        };
        let label = t(Message::new(&format!("{msg}.label"), label));
        let help = if key.help.is_empty() {
            String::new()
        } else {
            t(Message::new(&format!("{msg}.help"), &key.help))
        };
        let d = crate::config::plugins::descriptor(key, &label, &help);
        let mut value = values
            .get(name)
            .and_then(|v| PopValue::from_json(d.ty, v).ok())
            .unwrap_or_else(|| d.default.clone());
        let before = value.clone();
        let response = param::param_shown(ui, &d, Mode::Plain(&mut value), &|c| c.to_owned());
        if !help.is_empty() {
            response.on_hover_text(&help);
            ui.weak(&help);
        }
        if value != before {
            values.insert(name.to_owned(), value.to_json());
        }
    }
    contract.check(&Value::Object(values.clone()))
}

/// Says why values do not fit their contract.
pub(super) fn refused_ui(
    ui: &mut egui::Ui,
    t: &dyn Fn(Message) -> String,
    fit: Result<(), String>,
) {
    if let Err(why) = fit {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            t(Message::new(
                "gui-editor-params-refused",
                "A parameter does not fit what the connector takes: { $why }",
            )
            .with("why", why)),
        );
    }
}

/// The command saving a new import source without the wizard.
pub const ADD: &str = "import.sources.add";

impl ImportWindow {
    /// `import.sources.add`: the source the wizard would save, from the
    /// connector's id and what its setup asks; selected when the window
    /// is open.
    fn add_source_command(&mut self, args: &Value) -> Result<Value, String> {
        let text = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let connector = text("connector").ok_or("`connector` is required")?;
        let setup = self
            .connectors
            .provider(&connector)
            .ok_or_else(|| format!("no connector `{connector}`"))?
            .manifest()
            .setup()
            .ok_or_else(|| format!("the connector `{connector}` sets up no source"))?;
        let (path, account) = match setup.asks {
            SetupAsks::Path => {
                let p = PathBuf::from(text("path").ok_or("`path` is required: the folder")?);
                if !p.is_absolute() || !p.is_dir() {
                    return Err(format!("no folder at {}", p.display()));
                }
                (Some(p), None)
            }
            SetupAsks::Volume => {
                let (path, volume) = (text("path"), text("volume"));
                let card = self
                    .sources
                    .mounted
                    .iter()
                    .find(|m| {
                        path.as_deref().is_some_and(|p| Path::new(p) == m.root)
                            || volume
                                .as_deref()
                                .is_some_and(|v| v == m.id || v == m.label)
                    })
                    .ok_or("no such volume mounted (`path`: its mount point, or `volume`: its id or label)")?;
                (Some(card.root.clone()), None)
            }
            SetupAsks::Account => {
                let id = text("account").ok_or(
                    "`account` is required: an account of the connector, as `import.accounts` lists them",
                )?;
                let known = self
                    .accounts
                    .listed()
                    .iter()
                    .any(|a| a.id == id && a.connector == connector);
                if !known {
                    return Err(format!("no account `{id}` of `{connector}`"));
                }
                (None, Some(id))
            }
        };
        let profile = match text("profile") {
            Some(id) if self.profiles.get(&id).is_none() => {
                return Err(format!("no import profile `{id}`"));
            }
            Some(id) => id,
            None => self.default_profile(&connector),
        };
        let auto = match args.get("auto") {
            None | Some(Value::Null | Value::Bool(false)) => None,
            Some(Value::Bool(true)) => Some(Trigger::from(setup.auto.ok_or_else(|| {
                format!("the connector `{connector}` offers no automatic import")
            })?)),
            Some(other) => return Err(format!("`auto` is true or false, not {other}")),
        };
        let (contract, mut params) = self.wizard_params(&connector, &BTreeMap::new());
        if let Some(given) = args.get("params") {
            let given = given.as_object().ok_or("`params` is an object")?;
            for (k, v) in given {
                if contract.get(k).is_none() {
                    return Err(format!("`{connector}` has no parameter `{k}`"));
                }
                params.insert(k.clone(), v.clone());
            }
        }
        contract.check(&Value::Object(params.clone()))?;
        let name =
            text("name").unwrap_or_else(|| self.default_name(path.as_deref(), account.as_deref()));
        let new = NewSource {
            connector: connector.clone(),
            path: path.clone(),
            account: account.clone(),
            name: name.clone(),
            profile: profile.clone(),
            params: params.into_iter().collect(),
            auto,
        };
        let key = self
            .add_new_source(new)
            .ok_or("the source has neither a folder nor an account")?;
        if self.open {
            self.select_new(key);
        }
        Ok(serde_json::json!({
            "connector": connector,
            "name": name,
            "path": path,
            "account": account,
            "profile": profile,
            "auto": auto,
        }))
    }
}

impl super::Shell {
    /// `import.sources.add`: saves a new import source of a connector,
    /// from what its setup declares (`connector`; `path` for a folder;
    /// `path` or `volume` for a volume; `account`; `name`, `profile`,
    /// `auto`, `params`), as the "Add…" wizard does.
    pub fn add_import_source(&mut self, args: &Value) -> Result<Value, String> {
        self.import.get_mut().add_source_command(args)
    }

    /// Whether the import window's wizard is open.
    pub fn import_wizard_open(&self) -> bool {
        self.import.get().wizard_open()
    }
}
