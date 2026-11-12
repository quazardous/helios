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
//!    without those the connector cannot honour ([`Honours::of`]).
//! 4. **Name**: the folder's, or the account's, to start with.
//!
//! The source is then saved ([`super::import_sources::Sources::add`]) and
//! selected: a folder ticked, an account clicked. Cancel leaves nothing
//! behind; an account signed in to on the way stays an account.

use std::path::{Path, PathBuf};

use egui_pop::api::files::{self, FILES, FileRequest};
use egui_pop::api::host::Pending;
use egui_pop::core::overlay::DialogLayout;
use helios_connect::ConnectorManifest;
use helios_connect::account::AccountState;
use helios_core::Message;

use super::import_accounts::{Asked, state_text, translated};
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
        }
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
        body.heading(t(Message::new("gui-wizard-title", "Add a source")));
        let step = self.wizard.as_ref().map_or(Step::Kind, |wz| wz.step);
        egui::ScrollArea::vertical()
            .id_salt("helios.import.wizard.scroll")
            .auto_shrink([false, false])
            .show(&mut body, |ui| match step {
                Step::Kind => self.kind_ui(ui, &t),
                Step::Setup => self.setup_ui(ui, &t, app),
                Step::Choices => self.wizard_choices_ui(ui, &t, app),
                Step::Name => self.name_ui(ui, &t),
            });
        let ready = self.wizard_ready();
        let mut go = None;
        layout.footer(ui, |ui| {
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let next = if step == Step::Name {
                    t(Message::new("gui-wizard-add", "Add the source"))
                } else {
                    t(Message::new("gui-wizard-next", "Next"))
                };
                if ui.add_enabled(ready, egui::Button::new(next)).clicked() {
                    go = Some(Go::Next);
                }
                if step != Step::Kind
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
            Step::Setup => match wz.connector.as_ref().map(|(_, s)| *s) {
                Some(Setup::Folder) => wz.folder().is_some(),
                Some(Setup::Account) => wz.account.is_some(),
                Some(Setup::Card) | None => false,
            },
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
                if let Some(wz) = self.wizard.as_mut() {
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
            .and_then(|(id, _)| self.connectors.provider(id))
            .map_or_else(Honours::default, |p| Honours::of(&p.manifest().caps))
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
        self.sources.add(SavedSource {
            connector,
            name: Some(wz.name.trim().to_string()),
            path,
            account,
            choices: Some(wz.choices),
        });
        // The column shows it afresh, even when it was shown already.
        if self.sources.showing.is_some() {
            self.choices = self.sources.window.clone();
            self.sources.showing = None;
        }
        match key {
            SourceKey::Folder(p) => {
                self.sources.account = None;
                if !self.ticked.contains(&p) {
                    self.ticked.push(p);
                }
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
                        if ui.selectable_label(chosen, &account.label).clicked() {
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
