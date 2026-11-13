//! The accounts as commands (`docs/design/import-connectors.md`, section
//! 2.4.1), for Settings > Accounts (`crate::config::manifest`) and for
//! agents: the process is data, so an agent may start a connection and
//! fill a form the person allows, never a secret on its own.
//!
//! - `import.accounts` (read-only): every account, as rows (`/accounts`):
//!   `id`, `label` (with the account's address once known: "Google
//!   Photos — name@example.com"), `email`, `connector`, `connectorText`,
//!   `state`, `stateText`, `why` (also "Reconnect to see the account" for
//!   one signed in before its connector asked who it is), `tone`.
//! - `import.addSource`: opens the import window and connects a new
//!   account of `connector`.
//! - `import.reconnect`: opens the import window and connects `account`
//!   again, from the step that failed.
//! - `import.signOut`: forgets `account`'s secrets; it stays listed, not
//!   connected.
//! - `import.connection` (read-only): the connection running, `null`
//!   when none: `account`, `state` (`working`, `form`, `browser`,
//!   `connected`, `failed`, `cancelled`), `step`, and `fields`, `error`,
//!   `url` or `why`.
//! - `import.connectionAnswer`: the form's `values`; `cancel: true` gives
//!   the connection up.
//!
//! - `import.oauthClients` (read-only): the connectors signing in through
//!   OAuth, as rows (`/clients`): `connector`, `connectorText`, `source`
//!   (`own`, `environment`, `builtIn`, `manifest`, `null`: none),
//!   `sourceText`, `own`, `clientId` (the person's own), `clientSecret`
//!   (always empty: a secret is never read back).
//! - `import.setOAuthClient`: the person's own client of `connector`
//!   ("Use my own OAuth client"): `clientId`, `clientSecret` (typed by
//!   the person only), each kept in the keyring, an empty one forgotten;
//!   `own: false` forgets both, back to Helios's client.
//!
//! `import.addSource` and `import.reconnect` on a connector whose plugin
//! lacks capabilities answer `consent` (the plugin's id) instead: the
//! window asks the person first.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError};

use helios_connect::ProviderRef;
use helios_connect::account::{Account, AccountBook, AccountState, Secrets};
use helios_connect::connect::{FieldKind, Session, Status, WebSteps};
use helios_connect::oauth::{self, ClientSource};
use helios_core::Message;
use helios_shell::commands::{CommandSpec, Origin};
use serde_json::{Value, json};

use super::Shell;
use super::import_accounts::{Asked, Current, Labels, OauthConnectors, state_text};

/// Lists the accounts (read-only).
pub const ACCOUNTS: &str = "import.accounts";
/// Connects a new account of a connector.
pub const ADD_SOURCE: &str = "import.addSource";
/// Connects an account again.
pub const RECONNECT: &str = "import.reconnect";
/// Signs an account out.
pub const SIGN_OUT: &str = "import.signOut";
/// The connection running (read-only).
pub const CONNECTION: &str = "import.connection";
/// Answers the connection's form, or cancels it.
pub const CONNECTION_ANSWER: &str = "import.connectionAnswer";
/// The OAuth clients sign-ins use (read-only).
pub const OAUTH_CLIENTS: &str = "import.oauthClients";
/// Sets or forgets the person's own OAuth client of a connector.
pub const SET_OAUTH_CLIENT: &str = "import.setOAuthClient";

fn arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("`{key}` is needed"))
}

impl Shell {
    /// Keeps the import accounts in `book` (the library's vault), their
    /// secrets in `secrets` (the system keyring), and declares the
    /// commands that read and change them.
    pub fn set_import_accounts(&mut self, book: Arc<dyn AccountBook>, secrets: Arc<dyn Secrets>) {
        let (current, labels, web, identity) = {
            let mut w = self.import.get_mut();
            w.accounts.book = Some(book.clone());
            w.accounts.secrets = secrets.clone();
            let labels = w.accounts.labels.clone();
            for (id, label) in w.connectors.addable() {
                labels
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(id, label);
            }
            if let Some(web) = w.accounts.web.clone() {
                let (book, secrets) = (book.clone(), secrets.clone());
                w.connectors.set_token(Arc::new(move |id: &str| {
                    web.access_token(book.as_ref(), secrets.as_ref(), id)
                }));
            }
            (
                w.accounts.current.clone(),
                labels,
                w.accounts.web.clone(),
                w.accounts.identity.clone(),
            )
        };
        self.register_account_commands(&book, &secrets, web.clone(), &current, &labels, &identity);
        let oauth = self.import.get_mut().accounts.oauth.clone();
        self.register_client_commands(&secrets, web, &oauth, &labels);
    }

    /// Runs the steps that reach a service (`oauth`: the browser sign-in,
    /// its tokens renewed and revoked) through `web`. Called before
    /// [`Shell::set_import_accounts`], so signing out revokes too.
    pub fn set_import_sign_in(&mut self, web: Arc<dyn WebSteps>) {
        self.import.get_mut().accounts.web = Some(web);
    }

    /// How the import window opens a page in the system browser (a
    /// sign-in, a picking): the platform's opener unless set (a test's).
    pub fn set_import_browser(&mut self, opener: Arc<dyn Fn(&str) + Send + Sync>) {
        self.import.get_mut().accounts.opener = opener;
    }

    /// Imports what a connector's picking listed ("Pick photos…" on an
    /// account of a connector that picks), through `import`.
    pub fn set_import_remote(&mut self, import: super::import_accounts::ImportRemote) {
        self.import.get_mut().accounts.import = Some(import);
    }

    /// The library folder picked photos are copied into when their source
    /// has no folder of its own: read before a picking, asked for and
    /// remembered through `library` when there is none.
    pub fn set_import_library_folder(
        &mut self,
        library: Arc<dyn super::import_accounts::LibraryFolder>,
    ) {
        self.import.get_mut().accounts.library = Some(library);
    }

    /// Adds an import connector (a plugin's, a test's): its sources join
    /// the import window, and "Add a source…" offers it when it declares
    /// a connection process.
    pub fn add_import_connector(&mut self, provider: ProviderRef) {
        let manifest = provider.manifest();
        let mut w = self.import.get_mut();
        if helios_connect::oauth::asks_email(&manifest.connect) {
            w.accounts
                .identity
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(manifest.id.clone());
        }
        for step in &manifest.connect {
            if let helios_connect::StepKind::Oauth { client_id, .. } = &step.kind {
                w.accounts
                    .oauth
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(manifest.id.clone(), client_id.clone());
            }
        }
        if !manifest.connect.is_empty() {
            w.accounts
                .labels
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(manifest.id.clone(), manifest.label.clone());
        }
        w.connectors.add_provider(provider);
    }

    /// The plugins installed: a connector of theirs asks for the
    /// capabilities its plugin lacks before it runs (the consent dialog),
    /// and the plugin commands (`plugin.list`, `plugin.capabilities`,
    /// `plugin.grant`…) are declared, for Settings > Plugins and agents.
    pub fn set_plugins(&mut self, broker: Arc<helios_plugin::Broker>) {
        self.import.get_mut().accounts.plugins = Some(broker.clone());
        if self.has_command(helios_plugin::commands::CAPABILITIES) {
            return;
        }
        for (spec, handler) in helios_plugin::commands::admin(&broker) {
            if let Err(e) = self.register(spec, handler) {
                log::warn!("plugin commands: {e}");
            }
        }
    }

    /// The import accounts' commands that read or change them off the
    /// window, declared once.
    fn register_account_commands(
        &mut self,
        book: &Arc<dyn AccountBook>,
        secrets: &Arc<dyn Secrets>,
        web: Option<Arc<dyn WebSteps>>,
        current: &Current,
        labels: &Labels,
        identity: &super::import_accounts::IdentityConnectors,
    ) {
        if self.has_command(ACCOUNTS) {
            return;
        }
        let (b, l, i18n) = (book.clone(), labels.clone(), self.core.i18n.clone());
        let asking = identity.clone();
        let listed = self.register(
            CommandSpec::new(
                ACCOUNTS,
                "List the import accounts: their connector and state (connected, connecting, needs attention, not connected)",
                "Library",
            )
            .read_only(),
            move |_, _| {
                let t = |m: Message| helios_i18n::message(&i18n, &m);
                let labels = l.lock().unwrap_or_else(PoisonError::into_inner).clone();
                let label = |id: &str| {
                    labels
                        .get(id)
                        .map_or_else(|| id.to_owned(), |l| t(Message::new(&l.key, &l.en)))
                };
                let asks = |id: &str| {
                    asking
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .contains(id)
                };
                Ok(account_rows(&t, &b.accounts()?, &label, &asks))
            },
        );
        let (b, s) = (book.clone(), secrets.clone());
        let sign_out = self.register(
            CommandSpec::new(
                SIGN_OUT,
                "Sign an import account out: its keys and tokens are forgotten, it stays listed (`account`)",
                "Library",
            ),
            move |args, _| {
                let id = arg(args, "account")?;
                let account = match &web {
                    Some(web) => web.sign_out(b.as_ref(), s.as_ref(), id)?,
                    None => helios_connect::account::sign_out(b.as_ref(), s.as_ref(), id)?,
                };
                Ok(json!({ "account": account.id, "state": account.state.name() }))
            },
        );
        let c = current.clone();
        let connection = self.register(
            CommandSpec::new(
                CONNECTION,
                "The connection running: its step, and the form it asks, the page opened in the browser or why it failed",
                "Library",
            )
            .read_only(),
            move |_, _| {
                let session = c.lock().unwrap_or_else(PoisonError::into_inner).clone();
                Ok(session.map_or(Value::Null, |s| connection(&s)))
            },
        );
        let (c, i18n) = (current.clone(), self.core.i18n.clone());
        let answer = self.register(
            CommandSpec::new(
                CONNECTION_ANSWER,
                "Answer the connection's form (`values`), never a secret unless typed by the person; `cancel: true` gives it up",
                "Library",
            ),
            move |args, origin| {
                let session = c
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
                    .ok_or("no connection is running")?;
                connection_answer(&session, args, origin).map_err(|m| match m {
                    Refused::Secret => helios_i18n::message(
                        &i18n,
                        &Message::new(
                            "gui-connect-secret-refused",
                            "A secret is typed by the person, never given by an agent",
                        ),
                    ),
                    Refused::Other(e) => e,
                })
            },
        );
        for r in [listed, sign_out, connection, answer] {
            if let Err(e) = r {
                log::warn!("import accounts: {e}");
            }
        }
    }

    /// `import.oauthClients` and `import.setOAuthClient`, declared once:
    /// Settings > Accounts' "Use my own OAuth client".
    fn register_client_commands(
        &mut self,
        secrets: &Arc<dyn Secrets>,
        web: Option<Arc<dyn WebSteps>>,
        oauth: &OauthConnectors,
        labels: &Labels,
    ) {
        if self.has_command(OAUTH_CLIENTS) {
            return;
        }
        let (s, o, l, i18n) = (
            secrets.clone(),
            oauth.clone(),
            labels.clone(),
            self.core.i18n.clone(),
        );
        let listed = self.register(
            CommandSpec::new(
                OAUTH_CLIENTS,
                "List the OAuth clients sign-ins use, by source: the person's own, the environment's or Helios's",
                "Library",
            )
            .read_only(),
            move |_, _| {
                let t = |m: Message| helios_i18n::message(&i18n, &m);
                let connectors = o.lock().unwrap_or_else(PoisonError::into_inner).clone();
                let labels = l.lock().unwrap_or_else(PoisonError::into_inner).clone();
                let source = |connector: &str, declared: Option<&str>| {
                    web.as_ref()
                        .and_then(|w| w.client_source(connector, declared))
                };
                Ok(client_rows(&t, s.as_ref(), &connectors, &labels, &source))
            },
        );
        let (s, o, i18n) = (secrets.clone(), oauth.clone(), self.core.i18n.clone());
        let set = self.register(
            CommandSpec::new(
                SET_OAUTH_CLIENT,
                "Use the person's own OAuth client for a connector (`connector`, `clientId`, `clientSecret` typed by the person), or Helios's again (`own: false`)",
                "Library",
            ),
            move |args, origin| {
                let connector = arg(args, "connector")?;
                if !o
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains_key(connector)
                {
                    return Err(format!("{connector} does not sign in through OAuth"));
                }
                set_oauth_client(s.as_ref(), connector, args, origin).map_err(|m| match m {
                    Refused::Secret => helios_i18n::message(
                        &i18n,
                        &Message::new(
                            "gui-connect-secret-refused",
                            "A secret is typed by the person, never given by an agent",
                        ),
                    ),
                    Refused::Other(e) => e,
                })
            },
        );
        for r in [listed, set] {
            if let Err(e) = r {
                log::warn!("import accounts: {e}");
            }
        }
    }

    /// `import.addSource` and `import.reconnect`, on the interface thread:
    /// the import window opens and the connection starts in its dialog.
    pub(super) fn account_command(&mut self, command: &str, args: &Value) -> Result<Value, String> {
        let (connector, account) = match command {
            ADD_SOURCE => (arg(args, "connector")?.to_owned(), None),
            RECONNECT => {
                let id = arg(args, "account")?.to_owned();
                let connector = self
                    .import
                    .get_mut()
                    .accounts
                    .listed()
                    .iter()
                    .find(|a| a.id == id)
                    .map(|a| a.connector.clone())
                    .ok_or_else(|| format!("no account {id}"))?;
                (connector, Some(id))
            }
            other => return Err(format!("unknown command {other}")),
        };
        self.open_import_window(&Value::Null);
        let mut w = self.import.get_mut();
        // A plugin's capabilities not granted yet are asked for in the
        // window first; the connection starts once the person allows.
        let asked = match &account {
            Some(id) => Asked::Reconnect(id.clone()),
            None => Asked::Add(connector.clone()),
        };
        if let Some(plugin) = w.accounts.ask_consent(&asked) {
            return Ok(json!({ "account": null, "consent": plugin }));
        }
        w.connect_account(&connector, account.as_deref())?;
        let id = w
            .accounts
            .connecting
            .as_ref()
            .map(|c| c.session.account.clone());
        Ok(json!({ "account": id }))
    }
}

/// The accounts as Settings > Accounts' rows.
fn account_rows(
    t: &dyn Fn(Message) -> String,
    accounts: &[Account],
    connector: &dyn Fn(&str) -> String,
    asks_email: &dyn Fn(&str) -> bool,
) -> Value {
    let rows: Vec<Value> = accounts
        .iter()
        .map(|a| {
            let unknown = a.identity_missing(asks_email(&a.connector));
            let why = match &a.state {
                AccountState::NeedsAttention { why, .. } => why.clone(),
                _ if unknown => t(Message::new(
                    "gui-account-identity-missing",
                    "Reconnect to see the account",
                )),
                _ => String::new(),
            };
            let tone = match a.state {
                AccountState::Connected if unknown => json!("warning"),
                AccountState::Connected => json!("success"),
                AccountState::NeedsAttention { .. } => json!("warning"),
                _ => Value::Null,
            };
            json!({
                "id": a.id,
                "label": a.shown(),
                "email": a.email(),
                "connector": a.connector,
                "connectorText": connector(&a.connector),
                "state": a.state.name(),
                "stateText": t(state_text(&a.state)),
                "why": why,
                "tone": tone,
                "connected": a.state == AccountState::Connected,
            })
        })
        .collect();
    json!({ "accounts": rows })
}

/// The OAuth connectors as Settings > Accounts' own-client rows.
fn client_rows(
    t: &dyn Fn(Message) -> String,
    secrets: &dyn Secrets,
    connectors: &BTreeMap<String, Option<String>>,
    labels: &BTreeMap<String, helios_connect::Text>,
    source: &dyn Fn(&str, Option<&str>) -> Option<ClientSource>,
) -> Value {
    let rows: Vec<Value> = connectors
        .iter()
        .map(|(id, declared)| {
            let own = oauth::own_client(secrets, id);
            let source = source(id, declared.as_deref());
            let text = match source {
                Some(ClientSource::Own) => Message::new("gui-oauth-client-own", "Your own client"),
                Some(ClientSource::Environment) => {
                    Message::new("gui-oauth-client-environment", "From the environment")
                }
                Some(ClientSource::BuiltIn | ClientSource::Manifest) => {
                    Message::new("gui-oauth-client-built-in", "Helios's client")
                }
                None => Message::new("gui-oauth-client-none", "None in this build: use your own"),
            };
            json!({
                "connector": id,
                "connectorText": labels
                    .get(id)
                    .map_or_else(|| id.clone(), |l| t(Message::new(&l.key, &l.en))),
                "source": source.map(ClientSource::name),
                "sourceText": t(text),
                "tone": if source.is_none() { json!("warning") } else { Value::Null },
                "own": own.is_some(),
                "clientId": own.map_or_else(String::new, |c| c.id),
                "clientSecret": "",
            })
        })
        .collect();
    json!({ "clients": rows })
}

/// Keeps the parts of the person's own client `args` give; someone
/// other than the person (an agent) never sets its secret.
fn set_oauth_client(
    secrets: &dyn Secrets,
    connector: &str,
    args: &Value,
    origin: &Origin,
) -> Result<Value, Refused> {
    if args.get("own").and_then(Value::as_bool) == Some(false) {
        oauth::set_own_client(secrets, connector, None).map_err(Refused::Other)?;
        return Ok(json!({ "own": false }));
    }
    let text = |key: &str| args.get(key).and_then(Value::as_str);
    if text("clientSecret").is_some() && *origin != Origin::User {
        return Err(Refused::Secret);
    }
    for (key, name) in [
        ("clientId", oauth::CLIENT_ID),
        ("clientSecret", oauth::CLIENT_SECRET),
    ] {
        if let Some(v) = text(key) {
            oauth::set_own_client_part(secrets, connector, name, v).map_err(Refused::Other)?;
        }
    }
    Ok(json!({ "own": oauth::own_client(secrets, connector).is_some() }))
}

/// A connection, as data.
fn connection(session: &Session) -> Value {
    let mut answer = json!({ "account": session.account });
    let (state, more) = match session.status() {
        Status::Working { step } => ("working", json!({ "step": step })),
        Status::Form {
            step,
            fields,
            error,
        } => (
            "form",
            json!({
                "step": step,
                "error": error,
                "fields": fields.iter().map(|f| json!({
                    "id": f.id,
                    "label": f.label.en,
                    "secret": f.kind == FieldKind::Secret,
                    "required": f.required,
                })).collect::<Vec<_>>(),
            }),
        ),
        Status::Browser { step, url } => ("browser", json!({ "step": step, "url": url })),
        Status::Connected(a) => ("connected", json!({ "label": a.label })),
        Status::Failed { step, why } => ("failed", json!({ "step": step, "why": why })),
        Status::Cancelled => ("cancelled", json!({})),
    };
    answer["state"] = json!(state);
    if let (Some(o), Value::Object(m)) = (answer.as_object_mut(), more) {
        o.extend(m);
    }
    answer
}

/// Why an answer is refused.
enum Refused {
    /// Someone other than the person gave a secret.
    Secret,
    Other(String),
}

/// Answers a connection's form, or cancels it. Someone other than the
/// person (an agent) may fill a form but never one of its secrets.
fn connection_answer(session: &Session, args: &Value, origin: &Origin) -> Result<Value, Refused> {
    if args.get("cancel").and_then(Value::as_bool) == Some(true) {
        session.cancel();
        return Ok(json!({ "cancelled": true }));
    }
    let Status::Form { fields, .. } = session.status() else {
        return Err(Refused::Other("the connection asks no form now".into()));
    };
    let given = args
        .get("values")
        .and_then(Value::as_object)
        .ok_or_else(|| Refused::Other("`values` is needed".into()))?;
    let values: BTreeMap<String, String> = given
        .iter()
        .map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), s)
        })
        .collect();
    if *origin != Origin::User
        && fields
            .iter()
            .any(|f| f.kind == FieldKind::Secret && values.contains_key(&f.id))
    {
        return Err(Refused::Secret);
    }
    session.answer(values);
    Ok(json!({ "answered": true }))
}
