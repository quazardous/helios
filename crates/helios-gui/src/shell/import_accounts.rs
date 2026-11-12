//! The import window's accounts (`docs/design/import-connectors.md`,
//! sections 2.4.1 and 8.2): the Sources column lists the accounts of the
//! connectors that declare a connection process, each with its state
//! (connected, connecting, needs attention, not connected), and the "Add
//! a source…" wizard offers those connectors. Signing in there, or
//! "Reconnect" on an account, runs its declared steps through the one generic flow
//! ([`helios_connect::connect::Session`]), shown in a dialog of the
//! window: a form's fields, a spinner while a step works, the page the
//! browser was opened on, the reason a step failed.
//!
//! Settings > Accounts lists the same accounts through the commands of
//! [`super::accounts_admin`].
//!
//! A connector served by a plugin that still lacks some of the
//! capabilities it declares asks for them first, in a consent dialog
//! ([`consent_dialog`]): each capability with the reason the plugin's
//! manifest gives, Allow (granted, kept, the action run again) or Don't
//! allow (nothing runs).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use helios_connect::Text;
use helios_connect::account::{Account, AccountBook, AccountState, MemorySecrets, Secrets};
use helios_connect::connect::{Context, FieldKind, Session, Status, WebSteps};
use helios_core::Message;

use super::import_window::Texts;
use crate::import_scan::Connectors;

/// What the window keeps of the accounts.
pub(crate) struct Accounts {
    /// Where accounts are kept: the library's vault; none without a
    /// library (the accounts are not shown).
    pub(crate) book: Option<Arc<dyn AccountBook>>,
    /// Where their secrets go: the system keyring on the desktop.
    pub(crate) secrets: Arc<dyn Secrets>,
    /// How the steps that reach a service run (`oauth`).
    pub(crate) web: Option<Arc<dyn WebSteps>>,
    /// The accounts as last read.
    listed: Vec<Account>,
    /// The connection running, shown in the window's dialog.
    pub(crate) connecting: Option<Connecting>,
    /// The same session, for the commands that read and answer it off
    /// the window (`import.connection`).
    pub(crate) current: Current,
    /// The connectors' names, by id, for Settings > Accounts.
    pub(crate) labels: Labels,
    /// The connectors signing in through OAuth, with their manifest's
    /// client id, for Settings > Accounts' own clients.
    pub(crate) oauth: OauthConnectors,
    /// The picking running ("Pick photos…"), shown in the window's dialog.
    pub(crate) picking: Option<Picking>,
    /// How picked items are imported: `helios_library::fetch::import_remote`
    /// over the library, set by the desktop; none: "Pick photos…" is not
    /// offered.
    pub(crate) import: Option<ImportRemote>,
    /// The plugins installed: a connector of theirs asks for the
    /// capabilities its plugin lacks before it runs.
    pub(crate) plugins: Option<Arc<helios_plugin::Broker>>,
    /// The consent asked, shown in the window's dialog.
    pub(crate) consent: Option<Consent>,
}

/// A plugin's capabilities asked for before one of its actions runs.
pub(crate) struct Consent {
    /// The plugin's id.
    pub(crate) plugin: String,
    /// Its name, as shown.
    title: String,
    /// What it lacks, each with its manifest's reason.
    capabilities: Vec<helios_plugin::manifest::CapabilityDecl>,
    /// The action to run again once allowed.
    then: Asked,
    /// The person said no: the calm word is shown until closed.
    declined: bool,
}

/// Imports the items a connector listed under a source: the items added.
pub type ImportRemote = Arc<
    dyn Fn(
            &dyn helios_connect::Connector,
            helios_connect::SourceRef,
            Vec<helios_connect::Item>,
        ) -> Result<u64, String>
        + Send
        + Sync,
>;

/// How long a picking waits for the person at most.
const PICK_WAIT: std::time::Duration = std::time::Duration::from_mins(30);

/// Where a picking stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickStatus {
    /// Starting the picking at the service.
    Starting,
    /// Waiting for the person, in the browser opened on `url`.
    Browser {
        url: String,
    },
    /// Downloading and importing `items` photos.
    Importing {
        items: usize,
    },
    /// Done: `added` photos added.
    Done {
        added: u64,
    },
    /// Nothing picked before the deadline.
    Nothing,
    /// The service refused the sign-in: the account needs a reconnection.
    SignIn,
    Failed(String),
    Cancelled,
}

/// A picking running.
pub(crate) struct Picking {
    label: String,
    status: Arc<Mutex<PickStatus>>,
    cancel: helios_connect::Cancel,
    /// The browser was opened on this page.
    opened: Option<String>,
}

impl Picking {
    pub(crate) fn status(&self) -> PickStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The connection running, shared with the commands.
pub(crate) type Current = Arc<Mutex<Option<Arc<Session>>>>;
/// The connectors' names, by id.
pub(crate) type Labels = Arc<Mutex<BTreeMap<String, Text>>>;
/// The connectors with an `oauth` step, by id, with their manifest's
/// client id.
pub(crate) type OauthConnectors = Arc<Mutex<BTreeMap<String, Option<String>>>>;

impl Default for Accounts {
    fn default() -> Self {
        Self {
            book: None,
            secrets: Arc::new(MemorySecrets::default()),
            web: None,
            listed: Vec::new(),
            connecting: None,
            current: Current::default(),
            labels: Labels::default(),
            oauth: OauthConnectors::default(),
            picking: None,
            import: None,
            plugins: None,
            consent: None,
        }
    }
}

/// A connection running.
pub(crate) struct Connecting {
    pub(crate) session: Arc<Session>,
    /// The connector's name, as shown.
    label: String,
    /// The form's values as typed, by field.
    values: BTreeMap<String, String>,
    /// The form step the values are for.
    form: Option<String>,
    /// The browser was opened on this page.
    opened: Option<String>,
}

impl Accounts {
    /// The accounts, read again (a pass of the window: a connection or a
    /// command may have changed them).
    /// An account's name as last read.
    pub(crate) fn label_of(&self, id: &str) -> Option<String> {
        self.listed
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.label.clone())
    }

    pub(crate) fn listed(&mut self) -> &[Account] {
        if let Some(book) = &self.book {
            match book.accounts() {
                Ok(list) => self.listed = list,
                Err(e) => log::warn!("import accounts: {e}"),
            }
        }
        &self.listed
    }

    /// Starts connecting: a new account of `connector`, or the account
    /// `account` again (from the step that failed when it needs
    /// attention).
    pub(crate) fn connect(
        &mut self,
        connectors: &Connectors,
        connector: &str,
        account: Option<&str>,
        texts: Option<&Texts>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), String> {
        let book = self.book.clone().ok_or("accounts need a library open")?;
        let provider = connectors
            .provider(connector)
            .ok_or_else(|| format!("no source of kind {connector}"))?;
        let manifest = provider.manifest();
        if manifest.connect.is_empty() {
            return Err(format!("{connector} needs no connection"));
        }
        let label = translated(texts, &manifest.label);
        let (account, new) = match account {
            Some(id) => (
                book.account(id)?
                    .ok_or_else(|| format!("no account {id}"))?,
                false,
            ),
            None => (Account::new(connector, &label, &book.accounts()?), true),
        };
        if let Some(c) = self.connecting.take() {
            c.session.cancel();
        }
        let context = Context {
            provider,
            services: connectors.services(),
            book,
            secrets: self.secrets.clone(),
            web: self.web.clone(),
        };
        let session = Arc::new(Session::start(context, account, new, wake));
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(session.clone());
        self.connecting = Some(Connecting {
            session,
            label,
            values: BTreeMap::new(),
            form: None,
            opened: None,
        });
        Ok(())
    }

    /// Signs an account out: its secrets forgotten, "not connected".
    pub(crate) fn sign_out(&mut self, id: &str) -> Result<(), String> {
        let book = self.book.clone().ok_or("accounts need a library open")?;
        match &self.web {
            Some(web) => web.sign_out(book.as_ref(), self.secrets.as_ref(), id)?,
            None => helios_connect::account::sign_out(book.as_ref(), self.secrets.as_ref(), id)?,
        };
        Ok(())
    }

    /// Starts picking under the account `id` ("Pick photos…"): the
    /// connector's picking page opened in the system browser by the
    /// dialog, the person waited for (cancellable), what was picked
    /// imported. A sign-in the service refuses leaves the account needing
    /// attention, its Reconnect offered.
    pub(crate) fn pick(
        &mut self,
        connectors: &Connectors,
        id: &str,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), String> {
        let book = self.book.clone().ok_or("accounts need a library open")?;
        let import = self.import.clone().ok_or("no library to import into")?;
        let account = book
            .account(id)?
            .ok_or_else(|| format!("no account {id}"))?;
        let provider = connectors
            .provider(&account.connector)
            .ok_or_else(|| format!("no source of kind {}", account.connector))?;
        let connector = provider
            .create(&serde_json::Value::Null, &connectors.services())
            .map_err(|e| helios_connect::connect::said(&e))?;
        if let Some(p) = self.picking.take() {
            p.cancel.cancel();
        }
        let status = Arc::new(Mutex::new(PickStatus::Starting));
        let cancel = helios_connect::Cancel::new();
        let (st, stop, account_ref) = (status.clone(), cancel.clone(), account.account_ref());
        let set = move |s: PickStatus| {
            *st.lock().unwrap_or_else(PoisonError::into_inner) = s;
            wake();
        };
        std::thread::spawn(move || {
            use helios_connect::ConnectorError;
            let deadline = std::time::Instant::now() + PICK_WAIT;
            let opened = |url: &str| {
                set(PickStatus::Browser {
                    url: url.to_owned(),
                });
            };
            let listed = helios_plugin::source::pick_and_list(
                connector.as_ref(),
                &account_ref,
                deadline,
                &stop,
                &opened,
            );
            let end = match listed {
                Ok(Some((source, items))) => {
                    set(PickStatus::Importing { items: items.len() });
                    let done = import(connector.as_ref(), source, items);
                    let _ = connector.end_pick();
                    match done {
                        Ok(added) => PickStatus::Done { added },
                        Err(e) => PickStatus::Failed(e),
                    }
                }
                Ok(None) => PickStatus::Nothing,
                Err(ConnectorError::Cancelled) => PickStatus::Cancelled,
                Err(ConnectorError::AuthNeeded) => {
                    let why = "the source refused the sign-in: reconnect";
                    if let Err(e) = helios_connect::account::needs_attention(
                        book.as_ref(),
                        &account_ref.id,
                        why,
                    ) {
                        log::warn!("import accounts: {e}");
                    }
                    PickStatus::SignIn
                }
                Err(e) => PickStatus::Failed(helios_connect::connect::said(&e)),
            };
            set(end);
        });
        self.picking = Some(Picking {
            label: account.label,
            status,
            cancel,
            opened: None,
        });
        Ok(())
    }

    /// The connector an action is for: its own, or its account's.
    fn connector_of(&mut self, asked: &Asked) -> Option<String> {
        match asked {
            Asked::Add(connector) => Some(connector.clone()),
            Asked::Reconnect(id) | Asked::Pick(id) => self
                .listed()
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.connector.clone()),
            Asked::SignOut(_) | Asked::Select(_) => None,
        }
    }

    /// Whether `asked` waits for the person's consent: its connector's
    /// plugin lacks capabilities it declares, so the consent dialog asks
    /// for them and the action is held until Allow. The plugin's id when
    /// it waits.
    pub(crate) fn ask_consent(&mut self, asked: &Asked) -> Option<String> {
        let broker = self.plugins.clone()?;
        let connector = self.connector_of(asked)?;
        let plugin = helios_plugin::source::plugin_of(&broker, &connector)?;
        let capabilities = broker.missing(&plugin).ok()?;
        if capabilities.is_empty() {
            return None;
        }
        let title = broker
            .manifest(&plugin)
            .map_or_else(|| plugin.clone(), |m| m.title().to_owned());
        self.consent = Some(Consent {
            plugin: plugin.clone(),
            title,
            capabilities,
            then: asked.clone(),
            declined: false,
        });
        Some(plugin)
    }

    /// The person allowed the consent asked: its capabilities granted
    /// (and kept, by the broker's keeper), the action it held given back
    /// to run again.
    fn allow(&mut self) -> Option<Asked> {
        let consent = self.consent.take()?;
        let broker = self.plugins.clone()?;
        for c in &consent.capabilities {
            if let Err(e) = broker.grant(&consent.plugin, &c.name) {
                log::warn!("plugin consent: {e}");
                return None;
            }
        }
        Some(consent.then)
    }

    /// The session is no longer shown.
    fn close(&mut self) {
        self.connecting = None;
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// A connector's text in the interface's language.
pub(crate) fn translated(texts: Option<&Texts>, text: &Text) -> String {
    match texts {
        Some(t) => t(Message::new(&text.key, &text.en)),
        None => text.en.clone(),
    }
}

/// An account's state, as the Sources column and Settings > Accounts
/// say it.
pub(crate) fn state_text(state: &AccountState) -> Message {
    match state {
        AccountState::NotConnected => Message::new("gui-account-not-connected", "not connected"),
        AccountState::Connecting { .. } => Message::new("gui-account-connecting", "connecting…"),
        AccountState::Connected => Message::new("gui-account-connected", "connected"),
        AccountState::NeedsAttention { .. } => {
            Message::new("gui-account-needs-attention", "needs attention")
        }
    }
}

/// What the accounts' part of the Sources column asked.
#[derive(Clone, Debug)]
pub(crate) enum Asked {
    /// The account clicked: its source selected, the Choices column
    /// showing its choices.
    Select(String),
    Add(String),
    Pick(String),
    Reconnect(String),
    SignOut(String),
}

/// The accounts under their heading, each with its state and, in its
/// menu, "Reconnect" and "Sign out". An account is added through the
/// "Add a source…" wizard ([`super::import_wizard`]). Nothing without a
/// library, or when no account exists.
pub(crate) fn accounts_ui(
    ui: &mut egui::Ui,
    accounts: &mut Accounts,
    connectors: &Connectors,
    selected: Option<&str>,
    t: &dyn Fn(Message) -> String,
) -> Option<Asked> {
    accounts.book.as_ref()?;
    let listed = accounts.listed().to_vec();
    if listed.is_empty() {
        return None;
    }
    let mut asked = None;
    ui.separator();
    ui.strong(t(Message::new("gui-import-window-accounts", "Accounts")));
    for account in &listed {
        let state = t(state_text(&account.state));
        let colour = match account.state {
            AccountState::Connected => ui.visuals().text_color(),
            AccountState::NeedsAttention { .. } => ui.visuals().warn_fg_color,
            _ => ui.visuals().weak_text_color(),
        };
        let why = match &account.state {
            AccountState::NeedsAttention { why, .. } => Some(why.clone()),
            _ => None,
        };
        let row = ui
            .horizontal(|ui| {
                if ui
                    .selectable_label(selected == Some(account.id.as_str()), &account.label)
                    .clicked()
                {
                    asked = Some(Asked::Select(account.id.clone()));
                }
                ui.label(egui::RichText::new(&state).color(colour).small());
            })
            .response;
        let row = row.interact(egui::Sense::click());
        let row = match why {
            Some(why) => row.on_hover_text(why),
            None => row,
        };
        row.context_menu(|ui| {
            if ui
                .button(t(Message::new("gui-account-reconnect", "Reconnect")))
                .clicked()
            {
                asked = Some(Asked::Reconnect(account.id.clone()));
                ui.close();
            }
            if ui
                .button(t(Message::new("gui-account-sign-out", "Sign out")))
                .clicked()
            {
                asked = Some(Asked::SignOut(account.id.clone()));
                ui.close();
            }
        });
        if matches!(account.state, AccountState::NeedsAttention { .. })
            && ui
                .small_button(t(Message::new("gui-account-reconnect", "Reconnect")))
                .clicked()
        {
            asked = Some(Asked::Reconnect(account.id.clone()));
        }
        if matches!(account.state, AccountState::Connected)
            && accounts.import.is_some()
            && connectors
                .provider(&account.connector)
                .is_some_and(|p| p.manifest().caps.picks)
            && ui
                .small_button(t(Message::new("gui-account-pick", "Pick photos…")))
                .clicked()
        {
            asked = Some(Asked::Pick(account.id.clone()));
        }
    }
    asked
}

/// The consent dialog, while a plugin's capabilities are asked for: the
/// plugin's name and id, each capability it lacks with the reason its
/// manifest gives, Allow and Don't allow. Allow grants them all and gives
/// back the action held, to run again; Don't allow says calmly that
/// nothing ran and where to allow them later; Escape closes it, nothing
/// granted.
pub(crate) fn consent_dialog(
    ctx: &egui::Context,
    accounts: &mut Accounts,
    t: &dyn Fn(Message) -> String,
) -> Option<Asked> {
    let consent = accounts.consent.as_mut()?;
    let (mut allow, mut close) = (false, false);
    let shown = egui::Modal::new(egui::Id::unique("helios.plugin.consent")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.heading(t(Message::new("gui-consent-title", "Allow { $plugin }?")
            .with("plugin", consent.title.clone())));
        ui.weak(t(Message::new("gui-consent-from", "Plugin { $id }")
            .with("id", consent.plugin.clone())));
        ui.add_space(6.0);
        if consent.declined {
            ui.label(t(Message::new(
                "gui-consent-declined",
                "Nothing was changed: { $plugin } does not run without these permissions. You can allow them later in Settings > Plugins.",
            )
            .with("plugin", consent.title.clone())));
            if ui
                .button(t(Message::new("gui-connect-close", "Close")))
                .clicked()
            {
                close = true;
            }
            return;
        }
        ui.label(t(Message::new(
            "gui-consent-asks",
            "{ $plugin } asks for your permission to:",
        )
        .with("plugin", consent.title.clone())));
        for c in &consent.capabilities {
            ui.horizontal_wrapped(|ui| {
                ui.label("•");
                ui.label(&c.reason);
            });
        }
        ui.add_space(6.0);
        ui.weak(t(Message::new(
            "gui-consent-later",
            "Helios remembers your answer; Settings > Plugins changes it.",
        )));
        ui.horizontal(|ui| {
            if ui
                .button(t(Message::new("gui-consent-allow", "Allow")))
                .clicked()
            {
                allow = true;
            }
            if ui
                .button(t(Message::new("gui-consent-deny", "Don't allow")))
                .clicked()
            {
                consent.declined = true;
                ui.ctx().request_repaint();
            }
        });
    });
    // Escape or a click beside it closes it as Don't allow would, without
    // its word: nothing granted, nothing run.
    if !allow && shown.should_close() {
        close = true;
    }
    // What the answer changed is drawn at once: the window repaints only
    // when asked.
    if close || allow {
        ctx.request_repaint();
    }
    if close {
        accounts.consent = None;
    }
    if allow {
        return accounts.allow();
    }
    None
}

/// The connection's dialog, while one runs or until its end is seen:
/// the step's form, a spinner, the browser's page, why it failed, or
/// that the account is connected. Closed by Cancel or Close.
pub(crate) fn connect_dialog(
    ctx: &egui::Context,
    accounts: &mut Accounts,
    t: &dyn Fn(Message) -> String,
) {
    let Some(connecting) = accounts.connecting.as_mut() else {
        return;
    };
    let status = connecting.session.status();
    // A browser step opens the page once, in the system browser (never
    // a web view: services refuse them).
    if let Status::Browser { url, .. } = &status
        && connecting.opened.as_deref() != Some(url.as_str())
    {
        connecting.opened = Some(url.clone());
        ctx.open_url(egui::OpenUrl::new_tab(url));
    }
    let mut close = false;
    egui::Modal::new(egui::Id::unique("helios.import.connect")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.heading(t(Message::new("gui-connect-title", "Connect { $source }")
            .with("source", connecting.label.clone())));
        ui.add_space(4.0);
        match &status {
            Status::Working { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t(Message::new("gui-connect-working", "Connecting…")));
                });
                if ui
                    .button(t(Message::new("gui-connect-cancel", "Cancel")))
                    .clicked()
                {
                    connecting.session.cancel();
                }
            }
            Status::Form {
                step,
                fields,
                error,
            } => {
                if connecting.form.as_deref() != Some(step.as_str()) {
                    connecting.form = Some(step.clone());
                    connecting.values = fields
                        .iter()
                        .map(|f| (f.id.clone(), f.default.clone()))
                        .collect();
                }
                egui::Grid::new("helios.import.connect.form")
                    .num_columns(2)
                    .show(ui, |ui| {
                        for field in fields {
                            let label =
                                ui.label(t(Message::new(&field.label.key, &field.label.en)));
                            let value = connecting.values.entry(field.id.clone()).or_default();
                            ui.add(
                                egui::TextEdit::singleline(value)
                                    .password(field.kind == FieldKind::Secret),
                            )
                            .labelled_by(label.id);
                            ui.end_row();
                            if let Some(help) = &field.help {
                                ui.label("");
                                ui.weak(t(Message::new(&help.key, &help.en)));
                                ui.end_row();
                            }
                        }
                    });
                if let Some(e) = error {
                    ui.colored_label(ui.visuals().warn_fg_color, e);
                }
                ui.horizontal(|ui| {
                    if ui
                        .button(t(Message::new("gui-connect-next", "Connect")))
                        .clicked()
                    {
                        connecting.session.answer(connecting.values.clone());
                    }
                    if ui
                        .button(t(Message::new("gui-connect-cancel", "Cancel")))
                        .clicked()
                    {
                        connecting.session.cancel();
                    }
                });
            }
            Status::Browser { url, .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t(Message::new(
                        "gui-connect-browser",
                        "Sign in in your browser, then come back here.",
                    )));
                });
                ui.hyperlink_to(
                    t(Message::new(
                        "gui-connect-open-again",
                        "Open the page again",
                    )),
                    url,
                );
                if ui
                    .button(t(Message::new("gui-connect-cancel", "Cancel")))
                    .clicked()
                {
                    connecting.session.cancel();
                }
            }
            Status::Connected(account) => {
                ui.label(t(Message::new(
                    "gui-connect-done",
                    "{ $account } is connected.",
                )
                .with("account", account.label.clone())));
                if ui
                    .button(t(Message::new("gui-connect-close", "Close")))
                    .clicked()
                {
                    close = true;
                }
            }
            Status::Failed { why, .. } => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    t(
                        Message::new("gui-connect-failed", "Not connected: { $why }")
                            .with("why", why.clone()),
                    ),
                );
                if ui
                    .button(t(Message::new("gui-connect-close", "Close")))
                    .clicked()
                {
                    close = true;
                }
            }
            Status::Cancelled => close = true,
        }
    });
    if close {
        accounts.close();
    }
}

/// The picking's dialog, while one runs or until its end is seen: the
/// browser opened once on the picking page (never a web view), a
/// spinner while the person picks or the photos come in, then what was
/// imported, or why not. Cancel stops the wait.
pub(crate) fn pick_dialog(
    ctx: &egui::Context,
    accounts: &mut Accounts,
    t: &dyn Fn(Message) -> String,
) {
    let Some(picking) = accounts.picking.as_mut() else {
        return;
    };
    let status = picking.status();
    if let PickStatus::Browser { url } = &status
        && picking.opened.as_deref() != Some(url.as_str())
    {
        picking.opened = Some(url.clone());
        ctx.open_url(egui::OpenUrl::new_tab(url));
    }
    let mut close = false;
    egui::Modal::new(egui::Id::unique("helios.import.pick")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.heading(t(Message::new(
            "gui-pick-title",
            "Pick photos in { $source }",
        )
        .with("source", picking.label.clone())));
        ui.add_space(4.0);
        let working = |ui: &mut egui::Ui, text: String| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(text);
            });
        };
        let ended = |ui: &mut egui::Ui, text: String, warn: bool| {
            if warn {
                ui.colored_label(ui.visuals().warn_fg_color, text);
            } else {
                ui.label(text);
            }
        };
        match &status {
            PickStatus::Starting => working(
                ui,
                t(Message::new("gui-pick-starting", "Opening the picking…")),
            ),
            PickStatus::Browser { url } => {
                working(
                    ui,
                    t(Message::new(
                        "gui-pick-browser",
                        "Pick your photos in your browser, press Done, then come back here.",
                    )),
                );
                ui.hyperlink_to(
                    t(Message::new(
                        "gui-connect-open-again",
                        "Open the page again",
                    )),
                    url,
                );
            }
            PickStatus::Importing { items } => working(
                ui,
                t(
                    Message::new("gui-pick-importing", "Importing { $count } photos…")
                        .with("count", *items as i64),
                ),
            ),
            PickStatus::Done { added } => ended(
                ui,
                t(Message::new("gui-pick-done", "{ $count } photos imported.")
                    .with("count", *added as i64)),
                false,
            ),
            PickStatus::Nothing => ended(
                ui,
                t(Message::new(
                    "gui-pick-nothing",
                    "Nothing was picked in time.",
                )),
                false,
            ),
            PickStatus::SignIn => ended(
                ui,
                t(Message::new(
                    "gui-pick-sign-in",
                    "The source refused the sign-in: reconnect the account.",
                )),
                true,
            ),
            PickStatus::Failed(why) => ended(
                ui,
                t(Message::new("gui-pick-failed", "Not imported: { $why }")
                    .with("why", why.clone())),
                true,
            ),
            PickStatus::Cancelled => close = true,
        }
        let running = matches!(
            status,
            PickStatus::Starting | PickStatus::Browser { .. } | PickStatus::Importing { .. }
        );
        if running {
            if ui
                .button(t(Message::new("gui-connect-cancel", "Cancel")))
                .clicked()
            {
                picking.cancel.cancel();
            }
        } else if ui
            .button(t(Message::new("gui-connect-close", "Close")))
            .clicked()
        {
            close = true;
        }
    });
    if close {
        accounts.picking = None;
    }
}
