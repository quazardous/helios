//! Accounts saved twice on one identity, merged once
//! (`docs/design/import-connectors.md`, section 2.4.1). A connector that
//! takes one source per account (`Setup::one_source_per_account`) refuses
//! a second source on an identity; a library saved before that rule can
//! still hold two accounts signed in as one address, each with its remote
//! volume and its sources. At start, for those connectors only:
//!
//! - the accounts are grouped by identity ([`Account::identity`]); the one
//!   used last is kept ([`helios_connect::account::duplicates`]);
//! - their sources become one, on the kept account, with the profile and
//!   choices of the most recently used ([`import_sources::merge_accounts`]);
//! - what the others imported moves to the kept account's volume: its
//!   sightings, its import history, its volume record (the `library`
//!   step, `helios_library::fetch::merge_sources` on the desktop);
//! - the other accounts are forgotten, their secrets deleted from the
//!   keyring;
//! - the Log panel says so.
//!
//! Nothing here knows a connector: only the flag its setup declares. Run
//! again, nothing is left to merge.

use helios_connect::SourceRef;
use helios_connect::account::{self, Account};
use helios_core::Message;

use super::Shell;
use super::import_sources;
use crate::dialogs::Level;

/// What moves an account's imports to another's: its sightings, history
/// and volume record, from the first source to the second.
pub type MergeLibrary<'a> = &'a dyn Fn(&SourceRef, &SourceRef) -> Result<usize, String>;

fn source_of(a: &Account) -> SourceRef {
    SourceRef {
        connector: a.connector.clone(),
        id: a.id.clone(),
        label: a.label.clone(),
        root: None,
        capacity: None,
        camera: None,
    }
}

impl Shell {
    /// Merges the import accounts saved twice on one identity, for the
    /// connectors that take one source per account (see the module):
    /// their sources, their imports (through `library`) and the accounts
    /// themselves. Returns how many accounts were merged away; an account
    /// whose imports could not move stays, to merge at the next start.
    pub fn merge_duplicate_accounts(&mut self, library: MergeLibrary<'_>) -> usize {
        let mut w = self.import.get_mut();
        let Some(book) = w.accounts.book.clone() else {
            return 0;
        };
        let secrets = w.accounts.secrets.clone();
        let accounts = match book.accounts() {
            Ok(a) => a,
            Err(e) => {
                log::warn!("import accounts: not merged: {e}");
                return 0;
            }
        };
        let merges = {
            let connectors = &w.connectors;
            let one = |c: &str| {
                connectors
                    .provider(c)
                    .and_then(|p| p.manifest().setup())
                    .is_some_and(|s| s.one_source_per_account)
            };
            account::duplicates(&accounts, &one)
        };
        if merges.is_empty() {
            return 0;
        }
        let mut merged = 0;
        let mut notes = Vec::new();
        for merge in &merges {
            // The sources first: an account is forgotten only once no
            // source names it.
            if import_sources::merge_accounts(&mut w.sources.list, merge) {
                if w.sources
                    .account
                    .as_ref()
                    .is_some_and(|a| merge.dropped.iter().any(|d| &d.id == a))
                {
                    w.sources.account = Some(merge.kept.id.clone());
                }
                w.sources.store();
            }
            let to = source_of(&merge.kept);
            let mut gone = 0;
            for dropped in &merge.dropped {
                let moved = library(&source_of(dropped), &to);
                let removed = moved.and_then(|n| {
                    account::remove(book.as_ref(), secrets.as_ref(), &dropped.id).map(|()| n)
                });
                match removed {
                    Ok(n) => {
                        log::info!(
                            "import accounts: {} merged into {} ({n} items moved)",
                            dropped.id,
                            merge.kept.id
                        );
                        gone += 1;
                    }
                    Err(e) => log::warn!(
                        "import accounts: {} not merged into {}: {e}",
                        dropped.id,
                        merge.kept.id
                    ),
                }
            }
            if gone > 0 {
                let connector = w.connectors.provider(&merge.kept.connector).map_or_else(
                    || merge.kept.connector.clone(),
                    |p| super::import_accounts::translated(w.texts.as_ref(), &p.manifest().label),
                );
                notes.push(
                    Message::new(
                        "gui-import-accounts-merged",
                        "{ $count } { $connector } accounts signed in as one address merged into one: { $address }",
                    )
                    .with("count", (gone + 1) as u64)
                    .with("connector", connector)
                    .with("address", merge.kept.email().unwrap_or_default().to_owned()),
                );
                merged += gone;
            }
        }
        drop(w);
        for note in notes {
            crate::log_panel::write(Level::Info, &note);
        }
        merged
    }
}
