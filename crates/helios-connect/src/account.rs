//! Accounts (`docs/design/import-connectors.md`, sections 2.4 and 2.4.1):
//! one identity on a connector that declares a connection process, its
//! state, and where it is kept.
//!
//! An [`Account`] holds public facts only: its connector, its name as
//! shown, the remote volume its items are sighted on, its connection
//! state, the form values that are not secret, and the names of the
//! secrets kept for it ([`Secrets`]: the system keyring), never their
//! values. The desktop keeps accounts in the vault ([`VaultAccounts`],
//! the namespace `import/accounts`); tests in memory ([`MemoryAccounts`]).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use helios_vault::{Store, Vault};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AccountRef;

/// The account fact holding the address of the identity it signed in
/// as (an OAuth account asking the `email` scope reads it from the id
/// token): public, shown beside the connector's name.
pub const EMAIL: &str = "email";

/// Where an account stands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum AccountState {
    /// Signed out, or never connected on this machine.
    #[default]
    NotConnected,
    /// Its connection process runs, at this step.
    Connecting {
        step: String,
    },
    Connected,
    /// A step failed (a token expired, a password changed): reconnecting
    /// replays from that step.
    NeedsAttention {
        #[serde(default)]
        step: Option<String>,
        why: String,
        /// The service refused the account's token (revoked, aged out)
        /// while it was used: shown as "access refused".
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        refused: bool,
    },
}

impl AccountState {
    /// Its name as stored and shown to agents: `not-connected`,
    /// `connecting`, `connected`, `needs-attention`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotConnected => "not-connected",
            Self::Connecting { .. } => "connecting",
            Self::Connected => "connected",
            Self::NeedsAttention { .. } => "needs-attention",
        }
    }
}

/// One identity on a connector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Unique in the library: `<connector>-<n>`.
    pub id: String,
    pub connector: String,
    /// Its name as shown: the connector's until its test step names it
    /// (an e-mail address, a server).
    pub label: String,
    /// The id of the remote volume its items are sighted on.
    pub volume: String,
    #[serde(flatten)]
    pub state: AccountState,
    /// The values of its forms' fields that are not secret.
    #[serde(default)]
    pub facts: BTreeMap<String, Value>,
    /// The names of the secrets kept for it in the keyring.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// When it was last used (connected, or a token handed to its
    /// connector), in seconds since the Unix epoch; to the hour
    /// ([`touch`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
}

impl Account {
    /// A new account of `connector`, its id the first `<connector>-<n>`
    /// none of `taken` has.
    pub fn new(connector: &str, label: &str, taken: &[Account]) -> Self {
        // One more number than accounts: a free one is among them.
        let id = (1..=taken.len() + 1)
            .map(|n| format!("{connector}-{n}"))
            .find(|id| taken.iter().all(|a| &a.id != id))
            .unwrap_or_default();
        let volume = helios_storage::Volume::remote(connector, &id, label).id.0;
        Self {
            id,
            connector: connector.to_owned(),
            label: label.to_owned(),
            volume,
            state: AccountState::NotConnected,
            facts: BTreeMap::new(),
            secrets: Vec::new(),
            last_used: None,
        }
    }

    /// The address of the identity it signed in as, once known.
    pub fn email(&self) -> Option<&str> {
        self.facts
            .get(EMAIL)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|e| !e.is_empty())
    }

    /// Its name as shown: "Google Photos — name@example.com" once the
    /// identity is known, so several accounts of one connector stay
    /// apart; its label alone otherwise (or when the label already says
    /// the address).
    pub fn shown(&self) -> String {
        match self.email() {
            Some(email) if !self.label.contains(email) => format!("{} — {email}", self.label),
            _ => self.label.clone(),
        }
    }

    /// Whether it signed in before its connector asked who it is
    /// (`asks_email`: the connector's `oauth` step asks the `email`
    /// scope, [`crate::oauth::asks_email`]): no address known, one
    /// reconnection (one consent) shows it.
    pub fn identity_missing(&self, asks_email: bool) -> bool {
        asks_email && self.email().is_none() && self.facts.contains_key(crate::oauth::CLIENT_ID)
    }

    pub fn account_ref(&self) -> AccountRef {
        AccountRef {
            connector: self.connector.clone(),
            id: self.id.clone(),
        }
    }

    /// Its document, as the vault keeps it.
    pub fn to_document(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_document(doc: &Value) -> Result<Self, String> {
        serde_json::from_value(doc.clone()).map_err(|e| format!("an account document: {e}"))
    }
}

/// Where accounts are kept.
pub trait AccountBook: Send + Sync {
    /// Every account, by id.
    fn accounts(&self) -> Result<Vec<Account>, String>;
    /// Keeps an account (a new one or a change).
    fn save(&self, account: &Account) -> Result<(), String>;
    /// Forgets an account.
    fn remove(&self, id: &str) -> Result<(), String>;

    fn account(&self, id: &str) -> Result<Option<Account>, String> {
        Ok(self.accounts()?.into_iter().find(|a| a.id == id))
    }
}

/// Accounts in memory, for tests and a desktop without a library.
#[derive(Default)]
pub struct MemoryAccounts(Mutex<BTreeMap<String, Account>>);

impl AccountBook for MemoryAccounts {
    fn accounts(&self) -> Result<Vec<Account>, String> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect())
    }

    fn save(&self, account: &Account) -> Result<(), String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(account.id.clone(), account.clone());
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        Ok(())
    }
}

/// Accounts as vault documents (`import/accounts`).
pub struct VaultAccounts<S: Store>(pub Arc<Vault<S>>);

impl<S: Store + Send + 'static> AccountBook for VaultAccounts<S> {
    fn accounts(&self) -> Result<Vec<Account>, String> {
        self.0
            .account_documents()?
            .iter()
            .map(|(_, d)| Account::from_document(d))
            .collect()
    }

    fn save(&self, account: &Account) -> Result<(), String> {
        self.0
            .set_account_document(&account.id, Some(&account.to_document()))
    }

    fn remove(&self, id: &str) -> Result<(), String> {
        self.0.set_account_document(id, None)
    }
}

/// Where an account's secrets go: form fields marked secret, refresh
/// tokens. The desktop gives the system keyring; tests
/// [`MemorySecrets`].
pub trait Secrets: Send + Sync {
    fn get(&self, account: &str, name: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, name: &str, value: &str) -> Result<(), String>;
    /// Forgets a secret; one not kept is no error.
    fn delete(&self, account: &str, name: &str) -> Result<(), String>;
}

/// Secrets in memory, gone when the process ends.
#[derive(Default)]
pub struct MemorySecrets(Mutex<BTreeMap<(String, String), String>>);

impl Secrets for MemorySecrets {
    fn get(&self, account: &str, name: &str) -> Result<Option<String>, String> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(account.to_owned(), name.to_owned()))
            .cloned())
    }

    fn set(&self, account: &str, name: &str, value: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((account.to_owned(), name.to_owned()), value.to_owned());
        Ok(())
    }

    fn delete(&self, account: &str, name: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(account.to_owned(), name.to_owned()));
        Ok(())
    }
}

/// Signs an account out: its secrets forgotten, its state "not
/// connected". Its sightings and its document stay, so it can be
/// reconnected.
pub fn sign_out(
    book: &dyn AccountBook,
    secrets: &dyn Secrets,
    id: &str,
) -> Result<Account, String> {
    let mut account = book
        .account(id)?
        .ok_or_else(|| format!("no account {id}"))?;
    for name in std::mem::take(&mut account.secrets) {
        secrets.delete(&account.id, &name)?;
    }
    account.state = AccountState::NotConnected;
    book.save(&account)?;
    Ok(account)
}

/// Marks an account "needs attention" because the source refused its
/// sign-in during a listing or a fetch ("access refused"): reconnecting
/// it replays its steps. An account already marked keeps its step and
/// reason.
pub fn needs_attention(book: &dyn AccountBook, id: &str, why: &str) -> Result<Account, String> {
    let mut account = book
        .account(id)?
        .ok_or_else(|| format!("no account {id}"))?;
    if !matches!(account.state, AccountState::NeedsAttention { .. }) {
        account.state = AccountState::NeedsAttention {
            step: None,
            why: why.to_owned(),
            refused: true,
        };
        book.save(&account)?;
    }
    Ok(account)
}

/// Now, in seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// How stale an account's "last used" may get before [`touch`] writes it
/// again: one write an hour at most, not one per token handed out.
pub const TOUCH_EVERY: u64 = 3600;

/// Records that the account `id` is used now, when its "last used" is
/// older than [`TOUCH_EVERY`]; an account not kept is no error.
pub fn touch(book: &dyn AccountBook, id: &str) -> Result<(), String> {
    let Some(mut account) = book.account(id)? else {
        return Ok(());
    };
    let now = now();
    if account
        .last_used
        .is_some_and(|t| now.saturating_sub(t) < TOUCH_EVERY)
    {
        return Ok(());
    }
    account.last_used = Some(now);
    book.save(&account)
}

/// Renames the account `id` (its name as shown, the address read at its
/// sign-in kept beside it); its name before, for an undo.
pub fn rename(book: &dyn AccountBook, id: &str, label: &str) -> Result<String, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("an account's name cannot be empty".into());
    }
    let mut account = book
        .account(id)?
        .ok_or_else(|| format!("no account {id}"))?;
    let before = std::mem::replace(&mut account.label, label.to_owned());
    book.save(&account)?;
    Ok(before)
}

/// Forgets an account whole: its secrets and its document.
pub fn remove(book: &dyn AccountBook, secrets: &dyn Secrets, id: &str) -> Result<(), String> {
    if let Some(account) = book.account(id)? {
        for name in &account.secrets {
            secrets.delete(&account.id, name)?;
        }
    }
    book.remove(id)
}

#[cfg(test)]
mod tests {
    use helios_vault::MemoryStore;

    use super::*;

    #[test]
    fn an_account_is_numbered_by_connector_and_keeps_public_facts_only_in_the_vault() {
        let book = VaultAccounts(Arc::new(Vault::new(MemoryStore::default())));
        let mut first = Account::new("nas", "NAS", &[]);
        first.facts.insert("host".into(), "nas.local".into());
        first.secrets.push("password".into());
        first.state = AccountState::Connected;
        book.save(&first).unwrap();
        let second = Account::new("nas", "NAS", &book.accounts().unwrap());
        assert_eq!((first.id.as_str(), second.id.as_str()), ("nas-1", "nas-2"));
        assert_eq!(first.volume, "remote:nas:nas-1");
        let doc = first.to_document();
        assert_eq!(doc["state"], "connected");
        assert_eq!(doc["secrets"], serde_json::json!(["password"]));
        assert_eq!(book.accounts().unwrap(), [first.clone()]);

        let secrets = MemorySecrets::default();
        secrets.set("nas-1", "password", "hunter2").unwrap();
        let out = sign_out(&book, &secrets, "nas-1").unwrap();
        assert_eq!(out.state, AccountState::NotConnected);
        assert_eq!(secrets.get("nas-1", "password").unwrap(), None);
        assert_eq!(book.accounts().unwrap().len(), 1);
        remove(&book, &secrets, "nas-1").unwrap();
        assert!(book.accounts().unwrap().is_empty());
    }

    #[test]
    fn an_account_is_shown_with_its_address_and_an_old_one_asks_a_reconnection() {
        let mut a = Account::new("google-photos", "Google Photos", &[]);
        assert_eq!(a.shown(), "Google Photos");
        assert!(!a.identity_missing(true), "never signed in");
        a.facts
            .insert(crate::oauth::CLIENT_ID.into(), "client".into());
        assert!(a.identity_missing(true));
        assert!(!a.identity_missing(false));
        a.facts.insert(EMAIL.into(), "ann@example.com".into());
        assert_eq!(a.shown(), "Google Photos — ann@example.com");
        assert!(!a.identity_missing(true));
        a.label = "ann@example.com".into();
        assert_eq!(a.shown(), "ann@example.com");
    }

    #[test]
    fn a_state_needing_attention_keeps_its_step_and_reason() {
        let mut a = Account::new("nas", "NAS", &[]);
        a.state = AccountState::NeedsAttention {
            step: Some("probe".into()),
            why: "refused".into(),
            refused: false,
        };
        let again = Account::from_document(&a.to_document()).unwrap();
        assert_eq!(again, a);
        assert_eq!(again.state.name(), "needs-attention");
        assert!(a.to_document().get("refused").is_none());
    }

    #[test]
    fn a_refused_token_is_kept_apart_and_an_account_is_renamed_and_touched_hourly() {
        let book = MemoryAccounts::default();
        let a = Account::new("nas", "NAS", &[]);
        book.save(&a).unwrap();
        let marked = needs_attention(&book, "nas-1", "refused").unwrap();
        assert!(matches!(
            marked.state,
            AccountState::NeedsAttention { refused: true, .. }
        ));
        let doc = book.account("nas-1").unwrap().unwrap().to_document();
        assert_eq!(doc["refused"], true);

        assert_eq!(rename(&book, "nas-1", "  Home NAS ").unwrap(), "NAS");
        assert_eq!(book.account("nas-1").unwrap().unwrap().label, "Home NAS");
        assert!(rename(&book, "nas-1", " ").is_err());
        assert!(rename(&book, "nas-9", "x").is_err());

        touch(&book, "nas-1").unwrap();
        let used = book.account("nas-1").unwrap().unwrap().last_used.unwrap();
        assert!(used + 5 >= now());
        // Within the hour, nothing is written again.
        let mut old = book.account("nas-1").unwrap().unwrap();
        old.last_used = Some(now() - 10);
        book.save(&old).unwrap();
        touch(&book, "nas-1").unwrap();
        assert_eq!(
            book.account("nas-1").unwrap().unwrap().last_used,
            old.last_used
        );
        old.last_used = Some(now() - TOUCH_EVERY - 1);
        book.save(&old).unwrap();
        touch(&book, "nas-1").unwrap();
        assert!(book.account("nas-1").unwrap().unwrap().last_used.unwrap() + 5 >= now());
        touch(&book, "nas-9").unwrap();
    }
}
