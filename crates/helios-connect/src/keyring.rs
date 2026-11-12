//! The system keyring as an account's [`Secrets`], reached through the
//! system's own tool as `helios-ai` does for provider keys (`docs/ai-models.md`):
//! `secret-tool` (the Secret Service: GNOME Keyring, `KWallet`) on Linux
//! and the BSDs, `security` (the login keychain) on macOS. Going through
//! the tools keeps Helios free of a native D-Bus dependency. Windows has
//! no keyring support yet: a secret there cannot be kept, and the step
//! that needs one says so.
//!
//! Every entry is under the service [`SERVICE`], its account
//! `<account id>/<secret name>`.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::account::Secrets;

/// The keyring service import accounts' secrets are kept under.
pub const SERVICE: &str = "helios-import";

/// The system keyring.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemSecrets;

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
    if !out.status.success() && !out.stderr.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Ok((false, err));
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim_end().to_owned();
    Ok((out.status.success(), stdout))
}

const NO_WINDOWS: &str = "no system keyring support on Windows yet";

impl Secrets for SystemSecrets {
    fn get(&self, account: &str, name: &str) -> Result<Option<String>, String> {
        let entry = format!("{account}/{name}");
        let (ok, out) = if cfg!(target_os = "macos") {
            let args = ["find-generic-password", "-s", SERVICE, "-a", &entry, "-w"];
            run("security", &args, None)?
        } else if cfg!(windows) {
            return Err(NO_WINDOWS.into());
        } else {
            let args = ["lookup", "service", SERVICE, "account", &entry];
            run("secret-tool", &args, None)?
        };
        Ok((ok && !out.is_empty()).then_some(out))
    }

    fn set(&self, account: &str, name: &str, value: &str) -> Result<(), String> {
        let entry = format!("{account}/{name}");
        let label = format!("Helios import: {entry}");
        let (ok, out) = if cfg!(target_os = "macos") {
            let args = [
                "add-generic-password",
                "-U",
                "-s",
                SERVICE,
                "-a",
                &entry,
                "-l",
                &label,
                "-w",
                value,
            ];
            run("security", &args, None)?
        } else if cfg!(windows) {
            return Err(NO_WINDOWS.into());
        } else {
            let args = [
                "store", "--label", &label, "service", SERVICE, "account", &entry,
            ];
            run("secret-tool", &args, Some(value))?
        };
        if ok {
            Ok(())
        } else {
            Err(format!("the system keyring refused the secret: {out}"))
        }
    }

    fn delete(&self, account: &str, name: &str) -> Result<(), String> {
        let entry = format!("{account}/{name}");
        let (ok, out) = if cfg!(target_os = "macos") {
            let args = ["delete-generic-password", "-s", SERVICE, "-a", &entry];
            run("security", &args, None)?
        } else if cfg!(windows) {
            return Ok(());
        } else {
            let args = ["clear", "service", SERVICE, "account", &entry];
            run("secret-tool", &args, None)?
        };
        if ok || out.is_empty() || out.contains("could not be found") {
            Ok(())
        } else {
            Err(format!(
                "the system keyring did not remove the secret: {out}"
            ))
        }
    }
}
