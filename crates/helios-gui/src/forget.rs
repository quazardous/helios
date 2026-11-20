//! Settings > Danger zone: forgetting the accounts, the secrets Helios
//! keeps in the system keyring removed (import accounts' tokens under
//! `helios-import`, AI provider keys under `helios-ai`), never an entry
//! of another application's.
//!
//! - [`COMMAND`] (`helios.app.forgetAccounts`) runs only with the word
//!   [`WORD`] typed (`confirm`), is destructive and is **kept from
//!   agents** (`crate::destructive::NOT_FOR_AGENTS`). The accounts stay
//!   listed; each signs in again.
//! - The Danger zone's "Forget accounts…" opens the [`WIZARD`]: the word,
//!   checked as it is typed ([`CHECK_WORD`]).
//! - An "everything" reset forgets them too when its wizard's
//!   "Also forget the accounts" is ticked (`crate::reset`), once Helios
//!   has quit, with the reset's own typed word.
//!
//! The keyring is reached through a [`Keyring`]: the desktop's is the
//! system's ([`SystemKeyring`], through `secret-tool` or `security`);
//! tests use [`MemoryKeyring`] and never touch the system's.

use std::collections::BTreeSet;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use egui_pop::api::admin::model::hooks::Hook;
use egui_pop::api::admin::model::{Confirmation, Field, FieldKind, Wizard};
use helios_core::Message;
use helios_shell::commands::CommandSpec;
use serde_json::{Value, json};

use crate::shell::Shell;

/// Forgets the accounts: `confirm`, the word [`WORD`] typed.
pub const COMMAND: &str = "helios.app.forgetAccounts";

/// The word typed to confirm.
pub const WORD: &str = "FORGET";

/// The wizard, a single step: the word typed.
pub const WIZARD: &str = "helios.forget";

/// The wizard's check of the word typed.
pub const CHECK_WORD: &str = "helios.forget.word";

/// The keyring services Helios keeps secrets under: the import accounts'
/// (`helios_connect::keyring::SERVICE`) and the AI providers' keys
/// (`helios_ai::secrets::SERVICE`). Nothing else is ever removed.
pub const SERVICES: &[&str] = &[helios_connect::keyring::SERVICE, "helios-ai"];

/// A keyring whose entries can be removed by service.
pub trait Keyring: Send + Sync {
    /// Removes every entry of `service`.
    ///
    /// # Errors
    /// The keyring cannot be reached or refused.
    fn clear(&self, service: &str) -> Result<(), String>;
}

/// The system keyring, through its own tool: `secret-tool` (the Secret
/// Service) on Linux and the BSDs, `security` (the login keychain) on
/// macOS. Windows has no keyring support yet: nothing is kept there.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemKeyring;

fn tool(program: &str, args: &[&str]) -> Result<(bool, String), String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("cannot run `{program}` (the system keyring's tool): {e}"))?;
    let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    Ok((out.status.success(), err))
}

impl Keyring for SystemKeyring {
    fn clear(&self, service: &str) -> Result<(), String> {
        if cfg!(windows) {
            return Ok(());
        }
        if cfg!(target_os = "macos") {
            // One entry per call: until none of the service is left.
            for _ in 0..10_000 {
                let (ok, _) = tool("security", &["delete-generic-password", "-s", service])?;
                if !ok {
                    return Ok(());
                }
            }
            return Err(format!("the keychain still holds `{service}` entries"));
        }
        // Every item whose `service` attribute is this one.
        let (ok, err) = tool("secret-tool", &["clear", "service", service])?;
        if ok || err.is_empty() || err.contains("could not be found") {
            Ok(())
        } else {
            Err(format!(
                "the system keyring did not remove `{service}`: {err}"
            ))
        }
    }
}

/// A keyring in memory: entries by service and account. For tests.
#[derive(Debug, Default)]
pub struct MemoryKeyring {
    entries: Mutex<BTreeSet<(String, String)>>,
}

impl MemoryKeyring {
    /// Adds an entry.
    pub fn put(&self, service: &str, account: &str) {
        self.lock().insert((service.into(), account.into()));
    }

    /// Every entry, as `(service, account)`.
    pub fn entries(&self) -> Vec<(String, String)> {
        self.lock().iter().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeSet<(String, String)>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Keyring for MemoryKeyring {
    fn clear(&self, service: &str) -> Result<(), String> {
        self.lock().retain(|(s, _)| s != service);
        Ok(())
    }
}

/// Removes every entry of Helios's [`SERVICES`] from `keyring`, logged.
///
/// # Errors
/// A service could not be cleared; the others are still tried.
pub fn forget(keyring: &dyn Keyring) -> Result<(), String> {
    let mut failed = Vec::new();
    for service in SERVICES {
        match keyring.clear(service) {
            Ok(()) => log::warn!(target: "helios::reset", "keyring: `{service}` entries removed"),
            Err(e) => {
                log::error!(target: "helios::reset", "keyring: `{service}`: {e}");
                failed.push(e);
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

/// Runs [`COMMAND`]: the word checked, then [`forget`].
///
/// # Errors
/// The word is not [`WORD`], or the keyring refused.
pub fn run(keyring: &dyn Keyring, args: &Value) -> Result<Value, String> {
    let typed = args.get("confirm").and_then(Value::as_str).unwrap_or("");
    if typed.trim() != WORD {
        return Err(format!(
            "forgetting the accounts needs `confirm`: the word {WORD} typed by the person"
        ));
    }
    forget(keyring)?;
    Ok(json!({ "forgotten": SERVICES }))
}

/// Registers [`COMMAND`] on the shell, over `keyring`.
///
/// # Errors
/// The command cannot be registered.
pub fn register(shell: &mut Shell, keyring: Arc<dyn Keyring>) -> Result<(), String> {
    shell.register(
        CommandSpec::new(
            COMMAND,
            "Forget the accounts (`confirm`: the word FORGET): Helios's secrets in the system keyring are removed; each account signs in again",
            "Help",
        ),
        move |args, _| run(keyring.as_ref(), args),
    )
}

/// The [`WIZARD`]: the word typed; its finish runs [`COMMAND`].
pub fn wizard(t: &dyn Fn(Message) -> String) -> Wizard {
    let word = Field::draft(
        WIZARD,
        "confirm",
        &t(Message::new("gui-reset-type", "Type { $word } to confirm").with("word", WORD)),
        FieldKind::Text,
    )
    .required()
    .validate(CHECK_WORD);
    Wizard::new(
        WIZARD,
        &t(Message::new("gui-forget-wizard", "Forget the accounts")),
        COMMAND,
    )
    .description(&t(Message::new(
        "gui-forget-wizard-about",
        "The accounts' sign-ins and the AI keys Helios keeps in the system keyring are removed. Each account signs in again. Photos and the library stay.",
    )))
    .step(
        "confirm",
        &t(Message::new("gui-reset-step-confirm", "Confirm")),
        |s| {
            s.field(word);
        },
    )
    .args(json!({ "confirm": "${confirm}" }))
    .no_summary()
    .cancel(Confirmation::new(
        &t(Message::new("gui-forget-cancel", "Leave without forgetting the accounts?")),
        "",
        &t(Message::new("gui-forget-cancel-verb", "Leave")),
    ))
    .verb(&t(Message::new("gui-forget-verb", "Forget the accounts")))
}

/// The [`CHECK_WORD`] hook: a word typed that is not [`WORD`] is a
/// problem, said in the field.
pub fn check_word(t: impl Fn(Message) -> String + Send + Sync + 'static) -> Hook {
    crate::reset::typed_word(WORD, t)
}
