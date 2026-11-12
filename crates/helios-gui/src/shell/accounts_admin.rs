//! The accounts as commands (`docs/design/import-connectors.md`, section
//! 2.4.1), for Settings > Accounts (`crate::config::manifest`) and for
//! agents: the process is data, so an agent may start a connection and
//! fill a form the person allows, never a secret on its own.
//!
//! - `import.accounts` (read-only): every account, as rows (`/accounts`):
//!   `id`, `label`, `connector`, `connectorText`, `state`, `stateText`,
//!   `why`, `tone`.
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

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError};

use helios_connect::ProviderRef;
use helios_connect::account::{Account, AccountBook, AccountState, Secrets};
use helios_connect::connect::{FieldKind, Session, Status};
use helios_core::Message;
use helios_shell::commands::{CommandSpec, Origin};
use serde_json::{Value, json};

use super::Shell;
use super::import_accounts::{Current, Labels, state_text};

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
        let (current, labels) = {
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
            (w.accounts.current.clone(), labels)
        };
        self.register_account_commands(&book, &secrets, &current, &labels);
    }

    /// Adds an import connector (a plugin's, a test's): its sources join
    /// the import window, and "Add a source…" offers it when it declares
    /// a connection process.
    pub fn add_import_connector(&mut self, provider: ProviderRef) {
        let manifest = provider.manifest();
        let mut w = self.import.get_mut();
        if !manifest.connect.is_empty() {
            w.accounts
                .labels
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(manifest.id.clone(), manifest.label.clone());
        }
        w.connectors.add_provider(provider);
    }

    /// The import accounts' commands that read or change them off the
    /// window, declared once.
    fn register_account_commands(
        &mut self,
        book: &Arc<dyn AccountBook>,
        secrets: &Arc<dyn Secrets>,
        current: &Current,
        labels: &Labels,
    ) {
        if self.has_command(ACCOUNTS) {
            return;
        }
        let (b, l, i18n) = (book.clone(), labels.clone(), self.core.i18n.clone());
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
                Ok(account_rows(&t, &b.accounts()?, &label))
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
                let account = helios_connect::account::sign_out(b.as_ref(), s.as_ref(), id)?;
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
) -> Value {
    let rows: Vec<Value> = accounts
        .iter()
        .map(|a| {
            let why = match &a.state {
                AccountState::NeedsAttention { why, .. } => why.clone(),
                _ => String::new(),
            };
            let tone = match a.state {
                AccountState::Connected => json!("success"),
                AccountState::NeedsAttention { .. } => json!("warning"),
                _ => Value::Null,
            };
            json!({
                "id": a.id,
                "label": a.label,
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
