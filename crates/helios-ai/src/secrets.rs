//! Where provider keys live. A key is never written to a Helios file: a
//! provider names an environment variable or a keyring entry
//! ([`KeySource`]), read when a call is made.
//!
//! The keyring is the system's, reached through its own tool:
//! `secret-tool` (the Secret Service: GNOME Keyring, `KWallet`) on Linux and
//! the BSDs, `security` (the login keychain) on macOS. Going through the
//! tools keeps Helios free of a native D-Bus dependency. `secret-tool`
//! reads the secret on its standard input; `security` takes it as an
//! argument, briefly visible to the user's own processes. Windows has no
//! keyring support yet: use an environment variable there.

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The keyring service name every Helios entry is stored under.
pub const SERVICE: &str = "helios-ai";

/// Where a provider's key is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// An environment variable, read at each call.
    Env(String),
    /// A keyring entry, by name (under [`SERVICE`]).
    Keyring(String),
}

impl KeySource {
    /// The key, or why it cannot be had.
    pub fn resolve(&self, secrets: &dyn Secrets) -> Result<String, String> {
        match self {
            KeySource::Env(var) => {
                std::env::var(var).map_err(|_| format!("the key variable `{var}` is not set"))
            }
            KeySource::Keyring(name) => secrets
                .get(name)?
                .ok_or_else(|| format!("no key `{name}` in the {}", secrets.describe())),
        }
    }

    /// Where it is, for people (never the key itself).
    pub fn describe(&self) -> String {
        match self {
            KeySource::Env(var) => format!("environment variable `{var}`"),
            KeySource::Keyring(name) => format!("keyring entry `{SERVICE}/{name}`"),
        }
    }
}

/// A store of secrets by name.
pub trait Secrets: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    fn set(&self, name: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
    /// What it is, for messages: "system keyring (secret-tool)".
    fn describe(&self) -> &'static str;
}

/// The system keyring, through its command-line tool.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemKeyring;

fn run(program: &str, args: &[&str], input: Option<&str>) -> Result<(bool, String), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run `{program}` (the system keyring's tool): {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("`{program}`: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("`{program}`: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    if !out.status.success() && !out.stderr.is_empty() {
        return Ok((
            false,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok((out.status.success(), stdout))
}

impl Secrets for SystemKeyring {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        let (ok, out) = if cfg!(target_os = "macos") {
            run(
                "security",
                &["find-generic-password", "-s", SERVICE, "-a", name, "-w"],
                None,
            )?
        } else if cfg!(windows) {
            return Err("no keyring support on Windows yet: use an environment variable".into());
        } else {
            run(
                "secret-tool",
                &["lookup", "service", SERVICE, "account", name],
                None,
            )?
        };
        Ok((ok && !out.is_empty()).then_some(out))
    }

    fn set(&self, name: &str, secret: &str) -> Result<(), String> {
        let label = format!("Helios AI key: {name}");
        let (ok, out) = if cfg!(target_os = "macos") {
            run(
                "security",
                &[
                    "add-generic-password",
                    "-U",
                    "-s",
                    SERVICE,
                    "-a",
                    name,
                    "-l",
                    &label,
                    "-w",
                    secret,
                ],
                None,
            )?
        } else if cfg!(windows) {
            return Err("no keyring support on Windows yet: use an environment variable".into());
        } else {
            run(
                "secret-tool",
                &[
                    "store", "--label", &label, "service", SERVICE, "account", name,
                ],
                Some(secret),
            )?
        };
        if ok {
            Ok(())
        } else {
            Err(format!("the system keyring refused the key: {out}"))
        }
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        let (ok, out) = if cfg!(target_os = "macos") {
            run(
                "security",
                &["delete-generic-password", "-s", SERVICE, "-a", name],
                None,
            )?
        } else if cfg!(windows) {
            return Ok(());
        } else {
            run(
                "secret-tool",
                &["clear", "service", SERVICE, "account", name],
                None,
            )?
        };
        if ok || out.is_empty() {
            Ok(())
        } else {
            Err(format!("the system keyring did not remove the key: {out}"))
        }
    }

    fn describe(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            "system keyring (security)"
        } else {
            "system keyring (secret-tool)"
        }
    }
}

/// Secrets in memory, for tests and sandboxes.
#[derive(Debug, Default)]
pub struct MemorySecrets {
    entries: Mutex<BTreeMap<String, String>>,
}

impl Secrets for MemorySecrets {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self
            .entries
            .lock()
            .expect("secrets lock")
            .get(name)
            .cloned())
    }

    fn set(&self, name: &str, secret: &str) -> Result<(), String> {
        self.entries
            .lock()
            .expect("secrets lock")
            .insert(name.into(), secret.into());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        self.entries.lock().expect("secrets lock").remove(name);
        Ok(())
    }

    fn describe(&self) -> &'static str {
        "in-memory keyring"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keyring_key_is_read_and_a_missing_one_named() {
        let secrets = MemorySecrets::default();
        secrets.set("openai", "sk-1").unwrap();
        let source = KeySource::Keyring("openai".into());
        assert_eq!(source.resolve(&secrets).unwrap(), "sk-1");
        let missing = KeySource::Keyring("other".into()).resolve(&secrets);
        assert!(missing.unwrap_err().contains("`other`"));
        let unset = KeySource::Env("HELIOS_AI_TEST_UNSET_KEY".into()).resolve(&secrets);
        assert!(unset.unwrap_err().contains("HELIOS_AI_TEST_UNSET_KEY"));
        assert!(!source.describe().contains("sk-1"));
    }

    #[test]
    fn key_sources_read_as_toml() {
        #[derive(Deserialize)]
        struct P {
            key: KeySource,
        }
        let p: P = toml::from_str("key = { env = \"OPENAI_API_KEY\" }").unwrap();
        assert_eq!(p.key, KeySource::Env("OPENAI_API_KEY".into()));
    }
}
