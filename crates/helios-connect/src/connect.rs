//! Connecting an account (`docs/design/import-connectors.md`, section
//! 2.4.1): the steps a connector declares (`[[connect]]` in its
//! manifest), driven by Helios itself through one generic flow, so every
//! connector, built in or a plugin, gets the same screens, states and
//! tests.
//!
//! A [`Session`] runs a process on a thread of its own: it saves the
//! account at each step ("connecting", then "connected" or "needs
//! attention"), asks the interface what only the person can give (a
//! form's values, through [`Session::status`] and [`Session::answer`]),
//! and calls the connector for the rest (the test step, through
//! [`crate::Connector::probe`]). A form's fields marked secret go to the
//! keyring ([`Secrets`]), the others to the account's public facts.
//! Reconnecting an account that needs attention replays from the step
//! that failed.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::account::{Account, AccountBook, AccountState, Secrets};
use crate::{
    Cancel, ConnectStep, ConnectorError, ConnectorServices, ProviderRef, Settings, StepKind, Text,
};

/// How a form field is typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FieldKind {
    #[default]
    Text,
    /// Typed hidden, kept in the keyring, never in the library.
    Secret,
    Number,
}

/// A label as a manifest writes it: a text key with its English, or
/// plain English.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum Label {
    Text(Text),
    Plain(String),
}

/// One field of a `form` step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub id: String,
    pub label: Text,
    pub kind: FieldKind,
    pub required: bool,
    /// Its value when the form opens.
    pub default: String,
    /// A line under it.
    pub help: Option<Text>,
}

#[derive(Deserialize)]
struct FieldDecl {
    id: String,
    #[serde(default)]
    label: Option<Label>,
    #[serde(default)]
    kind: FieldKind,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    default: Option<Value>,
    #[serde(default)]
    help: Option<Label>,
}

/// The fields a `form` step's schema declares
/// (`{"fields": [{"id", "label", "kind", "required", "default", "help"}]}`;
/// `kind` is `text`, `secret` or `number`). A field the schema gets wrong
/// is left out.
pub fn fields(connector: &str, schema: &Value) -> Vec<Field> {
    let text = |id: &str, part: &str, label: Option<Label>, en: &str| match label {
        Some(Label::Text(t)) => t,
        Some(Label::Plain(s)) => Text::new(&format!("connect-{connector}-{id}{part}"), &s),
        None => Text::new(&format!("connect-{connector}-{id}{part}"), en),
    };
    schema
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| serde_json::from_value::<FieldDecl>(f.clone()).ok())
        .map(|d| Field {
            label: text(&d.id, "", d.label, &d.id),
            help: d.help.map(|h| text(&d.id, "-help", Some(h), "")),
            default: match d.default {
                Some(Value::String(s)) => s,
                Some(Value::Null) | None => String::new(),
                Some(v) => v.to_string(),
            },
            id: d.id,
            kind: d.kind,
            required: d.required,
        })
        .collect()
}

/// Where a session stands, for the interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// A step Helios or the connector is doing.
    Working { step: String },
    /// A form for the person to fill; `error` says why the last answer
    /// was refused.
    Form {
        step: String,
        fields: Vec<Field>,
        error: Option<String>,
    },
    /// The browser was opened on a page the person answers there.
    Browser { step: String, url: String },
    /// Every step passed: the account is connected.
    Connected(Account),
    /// A step failed; the account needs attention.
    Failed { step: String, why: String },
    /// The person gave up: a new account is forgotten, a known one keeps
    /// its state.
    Cancelled,
}

impl Status {
    /// Whether the session ended.
    pub fn ended(&self) -> bool {
        matches!(
            self,
            Self::Connected(_) | Self::Failed { .. } | Self::Cancelled
        )
    }
}

/// What the person answered.
enum Answer {
    Form(BTreeMap<String, String>),
}

/// What a connection needs of its host.
#[derive(Clone)]
pub struct Context {
    pub provider: ProviderRef,
    pub services: ConnectorServices,
    pub book: Arc<dyn AccountBook>,
    pub secrets: Arc<dyn Secrets>,
    /// How the steps that reach a service run (`oauth`); none: such a
    /// step fails.
    pub web: Option<Arc<dyn WebSteps>>,
}

/// The steps that reach a service on the person's behalf, given by the
/// host (the OAuth flow).
pub trait WebSteps: Send + Sync {
    /// Runs an `oauth` step for `account`: the browser opened on the
    /// consent page (`browser` is told its address), the answer awaited
    /// until `cancel`; the tokens kept in [`Secrets`], their names added
    /// to the account.
    fn oauth(
        &self,
        account: &mut Account,
        step: &StepKind,
        secrets: &dyn Secrets,
        browser: &dyn Fn(&str),
        cancel: &Cancel,
    ) -> Result<(), String>;

    /// Signs the account `id` out: its tokens revoked where the service
    /// can, then forgotten ([`crate::account::sign_out`]).
    fn sign_out(
        &self,
        book: &dyn AccountBook,
        secrets: &dyn Secrets,
        id: &str,
    ) -> Result<Account, String> {
        crate::account::sign_out(book, secrets, id)
    }
    /// A valid access token for the account `id`, renewed when it
    /// expired; a renewal refused leaves the account needing attention.
    fn access_token(
        &self,
        _book: &dyn AccountBook,
        _secrets: &dyn Secrets,
        id: &str,
    ) -> Result<String, String> {
        Err(format!("no sign-in for the account {id}"))
    }

    /// Where the OAuth client a sign-in to `connector` would use comes
    /// from (`declared`: its manifest's client id); `None` without one.
    fn client_source(
        &self,
        _connector: &str,
        _declared: Option<&str>,
    ) -> Option<crate::oauth::ClientSource> {
        None
    }
}

struct Shared {
    status: Mutex<Status>,
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    /// Sets the session's status; how it ended goes to the log too.
    fn set(&self, status: Status) {
        match &status {
            Status::Connected(account) => {
                log::info!("connecting a {} account: connected", account.connector);
            }
            Status::Failed { step, why } if step.is_empty() => {
                log::warn!("connecting an account: {why}");
            }
            Status::Failed { step, why } => {
                log::warn!("connecting an account: the step `{step}` failed: {why}");
            }
            Status::Cancelled => log::info!("connecting an account: cancelled"),
            Status::Working { .. } | Status::Form { .. } | Status::Browser { .. } => {}
        }
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status;
        (self.wake)();
    }
}

/// One account's connection process, running.
pub struct Session {
    /// The account connected.
    pub account: String,
    /// Whether the account is new: cancelled, it is forgotten.
    pub new: bool,
    shared: Arc<Shared>,
    answers: Sender<Answer>,
    cancel: Cancel,
}

impl Session {
    /// Starts connecting `account` through its connector's declared
    /// steps, from the first (a new account, or one signed out) or from
    /// the step that failed (one needing attention). `wake` is called at
    /// each change of [`Session::status`].
    pub fn start(
        context: Context,
        account: Account,
        new: bool,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let steps = context.provider.manifest().connect;
        let from = match &account.state {
            AccountState::NeedsAttention {
                step: Some(step), ..
            } => steps.iter().position(|s| &s.id == step).unwrap_or(0),
            _ => 0,
        };
        let shared = Arc::new(Shared {
            status: Mutex::new(Status::Working {
                step: steps.get(from).map(|s| s.id.clone()).unwrap_or_default(),
            }),
            wake: Box::new(wake),
        });
        let (answers, receive) = mpsc::channel();
        let cancel = Cancel::new();
        let id = account.id.clone();
        let (s, c) = (shared.clone(), cancel.clone());
        std::thread::Builder::new()
            .name(format!("connect {id}"))
            .spawn(move || run(&context, account, &steps[from..], new, &s, &receive, &c))
            .expect("a thread for the connection");
        Self {
            account: id,
            new,
            shared,
            answers,
            cancel,
        }
    }

    pub fn status(&self) -> Status {
        self.shared
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The person's values for the form asked.
    pub fn answer(&self, values: BTreeMap<String, String>) {
        let _ = self.answers.send(Answer::Form(values));
    }

    /// Gives up the connection.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A session dropped before it ended (the window closed) gives up.
        self.cancel.cancel();
    }
}

/// Why a step stopped.
enum Stop {
    Cancelled,
    Failed(String),
}

/// How often a step waiting on the person looks at its cancel flag.
const POLL: Duration = Duration::from_millis(100);

fn run(
    context: &Context,
    mut account: Account,
    steps: &[ConnectStep],
    new: bool,
    shared: &Shared,
    answers: &Receiver<Answer>,
    cancel: &Cancel,
) {
    let before = account.state.clone();
    for step in steps {
        account.state = AccountState::Connecting {
            step: step.id.clone(),
        };
        if let Err(why) = context.book.save(&account) {
            shared.set(Status::Failed {
                step: step.id.clone(),
                why,
            });
            return;
        }
        shared.set(Status::Working {
            step: step.id.clone(),
        });
        let done = match &step.kind {
            StepKind::Form { schema } => {
                form(context, &mut account, step, schema, shared, answers, cancel)
            }
            StepKind::Test => test(context, &mut account, cancel),
            kind @ StepKind::Oauth { .. } => match &context.web {
                Some(web) => {
                    let browser = |url: &str| {
                        shared.set(Status::Browser {
                            step: step.id.clone(),
                            url: url.to_owned(),
                        });
                    };
                    web.oauth(
                        &mut account,
                        kind,
                        context.secrets.as_ref(),
                        &browser,
                        cancel,
                    )
                    .map_err(|e| {
                        if cancel.is_cancelled() {
                            Stop::Cancelled
                        } else {
                            Stop::Failed(e)
                        }
                    })
                }
                None => Err(Stop::Failed("signing in is not available here".into())),
            },
            StepKind::Device { .. } | StepKind::Trust | StepKind::Code => Err(Stop::Failed(
                format!("the step `{}` is not supported yet", step.id),
            )),
        };
        match done {
            Ok(()) => {}
            Err(Stop::Cancelled) => {
                if new {
                    let _ = crate::account::remove(
                        context.book.as_ref(),
                        context.secrets.as_ref(),
                        &account.id,
                    );
                } else {
                    account.state = before;
                    let _ = context.book.save(&account);
                }
                shared.set(Status::Cancelled);
                return;
            }
            Err(Stop::Failed(why)) => {
                account.state = AccountState::NeedsAttention {
                    step: Some(step.id.clone()),
                    why: why.clone(),
                    refused: false,
                };
                let _ = context.book.save(&account);
                shared.set(Status::Failed {
                    step: step.id.clone(),
                    why,
                });
                return;
            }
        }
    }
    account.state = AccountState::Connected;
    account.last_used = Some(crate::account::now());
    match context.book.save(&account) {
        Ok(()) => shared.set(Status::Connected(account)),
        Err(why) => shared.set(Status::Failed {
            step: String::new(),
            why,
        }),
    }
}

/// A `form` step: asks the fields, checks the answer, keeps the secret
/// ones in the keyring and the others in the account.
fn form(
    context: &Context,
    account: &mut Account,
    step: &ConnectStep,
    schema: &Value,
    shared: &Shared,
    answers: &Receiver<Answer>,
    cancel: &Cancel,
) -> Result<(), Stop> {
    let fields = fields(&account.connector, schema);
    let mut error = None;
    loop {
        shared.set(Status::Form {
            step: step.id.clone(),
            fields: fields.clone(),
            error: error.take(),
        });
        let values = loop {
            if cancel.is_cancelled() {
                return Err(Stop::Cancelled);
            }
            match answers.recv_timeout(POLL) {
                Ok(Answer::Form(values)) => break values,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(Stop::Cancelled),
            }
        };
        match checked(&fields, &values) {
            Ok(()) => {}
            Err(e) => {
                error = Some(e);
                continue;
            }
        }
        for field in &fields {
            let value = values.get(&field.id).cloned().unwrap_or_default();
            match field.kind {
                FieldKind::Secret => {
                    context
                        .secrets
                        .set(&account.id, &field.id, &value)
                        .map_err(Stop::Failed)?;
                    if !account.secrets.contains(&field.id) {
                        account.secrets.push(field.id.clone());
                    }
                }
                FieldKind::Number => {
                    let n: f64 = value.trim().parse().unwrap_or_default();
                    account.facts.insert(field.id.clone(), n.into());
                }
                FieldKind::Text => {
                    account.facts.insert(field.id.clone(), value.into());
                }
            }
        }
        return Ok(());
    }
}

/// Why a form's answer is refused: a required field empty, a number that
/// is not one.
fn checked(fields: &[Field], values: &BTreeMap<String, String>) -> Result<(), String> {
    for field in fields {
        let value = values.get(&field.id).map_or("", |v| v.trim());
        if field.required && value.is_empty() {
            return Err(format!("{} is needed", field.label.en));
        }
        if field.kind == FieldKind::Number && !value.is_empty() && value.parse::<f64>().is_err() {
            return Err(format!("{} is not a number", field.label.en));
        }
    }
    Ok(())
}

/// The settings a connector of an account is created with: its public
/// facts and its secrets.
pub fn settings_of(account: &Account, secrets: &dyn Secrets) -> Result<Settings, String> {
    let mut all: serde_json::Map<String, Value> = account
        .facts
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for name in &account.secrets {
        if let Some(value) = secrets.get(&account.id, name)? {
            all.insert(name.clone(), value.into());
        }
    }
    Ok(Value::Object(all))
}

/// The `test` step: the connector, created with the account's settings,
/// proves the connection; the name it gives becomes the account's.
fn test(context: &Context, account: &mut Account, cancel: &Cancel) -> Result<(), Stop> {
    let settings = settings_of(account, context.secrets.as_ref()).map_err(Stop::Failed)?;
    let connector = context
        .provider
        .create(&settings, &context.services)
        .map_err(|e| Stop::Failed(said(&e)))?;
    match connector.probe(Some(&account.account_ref()), cancel) {
        Ok(probed) => {
            if let Some(label) = probed.label.filter(|l| !l.trim().is_empty()) {
                account.label = label;
            }
            Ok(())
        }
        Err(ConnectorError::Cancelled) => Err(Stop::Cancelled),
        Err(e) => Err(Stop::Failed(said(&e))),
    }
}

/// A connector's error in plain words.
pub fn said(e: &ConnectorError) -> String {
    match e {
        ConnectorError::AuthNeeded => "the source refused the sign-in".into(),
        ConnectorError::Offline => "the source cannot be reached".into(),
        ConnectorError::Unsupported => "this source cannot test its connection".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_form_schema_gives_its_fields_with_keys_for_plain_labels() {
        let f = fields(
            "nas",
            &json!({ "fields": [
                { "id": "host", "label": "Server", "required": true },
                { "id": "password", "label": { "key": "k-pass", "en": "Password" },
                  "kind": "secret", "help": "Kept in the keyring" },
                { "id": "port", "kind": "number", "default": 445 },
                { "label": "no id: left out" }
            ] }),
        );
        let ids: Vec<_> = f.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["host", "password", "port"]);
        assert_eq!(f[0].label, Text::new("connect-nas-host", "Server"));
        assert!(f[0].required);
        assert_eq!(f[1].kind, FieldKind::Secret);
        assert_eq!(f[1].label.key, "k-pass");
        assert_eq!(f[1].help.as_ref().unwrap().key, "connect-nas-password-help");
        assert_eq!(f[2].default, "445");
        let values = BTreeMap::from([("port".to_owned(), "x".to_owned())]);
        assert_eq!(checked(&f, &values).unwrap_err(), "Server is needed");
    }
}
