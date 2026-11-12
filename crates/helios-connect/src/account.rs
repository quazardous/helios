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
        }
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
    fn a_state_needing_attention_keeps_its_step_and_reason() {
        let mut a = Account::new("nas", "NAS", &[]);
        a.state = AccountState::NeedsAttention {
            step: Some("probe".into()),
            why: "refused".into(),
        };
        let again = Account::from_document(&a.to_document()).unwrap();
        assert_eq!(again, a);
        assert_eq!(again.state.name(), "needs-attention");
    }
}
