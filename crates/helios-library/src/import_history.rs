//! The import history: one record per import, kept in the vault
//! (library documents `import/history`), written by [`crate::import`]
//! itself so every runner leaves one: the desktop, the headless node
//! (`helios-node --vault`), the CLI, an importer plugin, an automatic
//! import.
//!
//! A record says what was imported (the sources), how (the choices, as
//! `library.import` takes them), what came of it (the counts, or the
//! error of an import that could not start), when, and who asked (the
//! origin: `user`, `cli`, `agent:NAME`…). "Import again" runs the same
//! import from it ([`Record::params`]): a card's new photos come in, the
//! ones the library has are skipped as duplicates.
//!
//! The newest [`KEPT`] records are kept; older ones are dropped as new
//! ones are written.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use helios_vault::{Store, Vault};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::import::{Options, Placement, Report};

/// The library documents' namespace the records are kept in.
pub const NS: &str = "import/history";

/// How many records are kept.
pub const KEPT: usize = 200;

/// Records written by this process: the key's last part, so two imports
/// ending in the same second keep a record each.
static NEXT: AtomicU64 = AtomicU64::new(1);

/// What an import did, in numbers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Counts {
    pub files: u64,
    pub added: u64,
    pub located: u64,
    pub unchanged: u64,
    pub skipped: u64,
    pub asked: u64,
    pub failed: u64,
    pub copied: u64,
    pub cancelled: bool,
}

impl From<&Report> for Counts {
    fn from(r: &Report) -> Self {
        Counts {
            files: r.files,
            added: r.added,
            located: r.located,
            unchanged: r.unchanged,
            skipped: r.skipped,
            asked: r.asked,
            failed: r.failed,
            copied: r.copied,
            cancelled: r.cancelled,
        }
    }
}

/// One import.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// Its key: the time it started, then the process and a number, so
    /// keys sort by time.
    pub id: String,
    /// When it started, in seconds since the epoch.
    pub when: u64,
    /// Who asked for it: `user`, `cli`, `control`, `agent:NAME`,
    /// `script:NAME`; empty when the caller did not say.
    pub origin: String,
    /// The folders, cards and files imported. Empty for an importer
    /// plugin's import, whose files its own scan listed.
    pub sources: Vec<PathBuf>,
    /// The importer plugin that listed the files, if one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub importer: Option<String>,
    /// The choices, as `library.import` takes them: `copy`, `into`,
    /// `rename`, `layout`, `duplicates`, `signals`, `darktable`.
    pub choices: Value,
    pub counts: Counts,
    /// Why it could not start (a source that cannot be listed), if so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Record {
    /// The params of `library.import` that run this import again, or why
    /// it cannot be: an importer plugin's import is run again from the
    /// import window, its plugin listing the files anew.
    ///
    /// # Errors
    /// An importer's import, or a record with no source.
    pub fn params(&self) -> Result<Value, String> {
        if let Some(importer) = &self.importer {
            return Err(format!(
                "this import came through the importer `{importer}`: run it again from the import window"
            ));
        }
        if self.sources.is_empty() {
            return Err("this import has no source to import again".into());
        }
        let mut params = match &self.choices {
            Value::Object(o) => Value::Object(o.clone()),
            _ => json!({}),
        };
        params["paths"] = json!(self.sources);
        Ok(params)
    }

    /// Its sources as a person reads them: the first one's name, and how
    /// many more.
    pub fn source_label(&self) -> String {
        let name = |p: &PathBuf| {
            p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        };
        match (self.sources.first(), &self.importer) {
            (_, Some(importer)) => importer.clone(),
            (None, None) => String::new(),
            (Some(first), None) if self.sources.len() == 1 => name(first),
            (Some(first), None) => format!("{} (+{})", name(first), self.sources.len() - 1),
        }
    }
}

/// An import's choices, as `library.import` takes them.
pub fn choices(options: &Options) -> Value {
    let mut c = json!({
        "layout": options.layout.as_str(),
        "duplicates": options.duplicates.as_str(),
        "signals": options.signals,
        "darktable": options.darktable,
    });
    if let Placement::Copy { into } = &options.placement {
        c["copy"] = json!(true);
        if let Some(into) = into {
            c["into"] = json!(into);
        }
    }
    if let Some(rename) = &options.rename {
        c["rename"] = json!(rename.text());
    }
    c
}

/// Records an import that ended (`outcome`: its report, or the error it
/// could not start for), then drops the records past [`KEPT`].
///
/// # Errors
/// The vault's.
pub fn record<S: Store>(
    vault: &Vault<S>,
    sources: &[PathBuf],
    options: &Options,
    now: u64,
    outcome: Result<&Report, &str>,
) -> Result<Record, String> {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let importer = options.listed.as_ref().map(|l| l.by.clone());
    let record = Record {
        id: format!("{now:011}-{:07}-{n:09}", std::process::id()),
        when: now,
        origin: options.origin.clone().unwrap_or_default(),
        // An importer's files are its scan's: not kept, as many as they
        // may be.
        sources: if importer.is_some() {
            Vec::new()
        } else {
            sources.to_vec()
        },
        importer,
        choices: choices(options),
        counts: outcome.map(Counts::from).unwrap_or_default(),
        error: outcome.err().map(String::from),
    };
    let doc = serde_json::to_value(&record).map_err(|e| e.to_string())?;
    vault.set_library_document(NS, &record.id, Some(&doc))?;
    let mut keys: Vec<String> = vault
        .library_documents(NS)?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    if keys.len() > KEPT {
        keys.sort_unstable();
        for key in &keys[..keys.len() - KEPT] {
            vault.set_library_document(NS, key, None)?;
        }
    }
    Ok(record)
}

/// The records, newest first, at most `limit` of them.
///
/// # Errors
/// The vault's.
pub fn list<S: Store>(vault: &Vault<S>, limit: Option<usize>) -> Result<Vec<Record>, String> {
    let mut records: Vec<Record> = vault
        .library_documents(NS)?
        .into_iter()
        .filter_map(|(key, doc)| {
            let mut r: Record = serde_json::from_value(doc).ok()?;
            r.id = key;
            Some(r)
        })
        .collect();
    records.sort_by(|a, b| b.id.cmp(&a.id));
    records.truncate(limit.unwrap_or(KEPT));
    Ok(records)
}

/// One record, by its id.
///
/// # Errors
/// The vault's, or no record of that id.
pub fn get<S: Store>(vault: &Vault<S>, id: &str) -> Result<Record, String> {
    let doc = vault
        .library_document(NS, id)?
        .ok_or_else(|| format!("no import `{id}` in the history"))?;
    let mut record: Record = serde_json::from_value(doc).map_err(|e| e.to_string())?;
    record.id = id.to_string();
    Ok(record)
}

/// The `library.import` params that run import `id` again.
///
/// # Errors
/// No such record, or one that cannot run again ([`Record::params`]).
pub fn again<S: Store>(vault: &Vault<S>, id: &str) -> Result<Value, String> {
    get(vault, id)?.params()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::{Duplicates, Layout, Listed};
    use crate::rename::Pattern;
    use helios_vault::MemoryStore;

    fn report(added: u64) -> Report {
        Report {
            files: added + 1,
            added,
            skipped: 1,
            ..Report::default()
        }
    }

    #[test]
    fn an_import_is_recorded_with_its_choices_and_runs_again_with_them() {
        let vault = Vault::new(MemoryStore::default());
        let options = Options {
            placement: Placement::Copy {
                into: Some("/photos".into()),
            },
            rename: Some(Pattern::parse("{date}-{seq:3}").unwrap()),
            layout: Layout::Month,
            duplicates: Duplicates::Keep,
            origin: Some("agent:tester".into()),
            ..Options::default()
        };
        let card = PathBuf::from("/media/EOS_DIGITAL");
        let r = record(&vault, &[card.clone()], &options, 1000, Ok(&report(3))).unwrap();
        let listed = list(&vault, None).unwrap();
        assert_eq!(listed, [r.clone()]);
        assert_eq!(r.origin, "agent:tester");
        assert_eq!(
            (r.counts.added, r.counts.skipped, r.counts.files),
            (3, 1, 4)
        );
        assert_eq!(r.source_label(), "EOS_DIGITAL");
        let params = again(&vault, &r.id).unwrap();
        assert_eq!(params["paths"], json!([card]));
        assert_eq!(params["copy"], true);
        assert_eq!(params["into"], "/photos");
        assert_eq!(params["rename"], "{date}-{seq:3}");
        assert_eq!(params["layout"], "month");
        assert_eq!(params["duplicates"], "keep");
        // The params make the same choices again.
        let rename = Pattern::parse(params["rename"].as_str().unwrap()).unwrap();
        assert_eq!(Some(rename), options.rename);
    }

    #[test]
    fn the_history_is_newest_first_and_bounded() {
        let vault = Vault::new(MemoryStore::default());
        let options = Options::default();
        for when in 0..(KEPT as u64 + 5) {
            record(&vault, &["/a".into()], &options, when, Ok(&report(1))).unwrap();
        }
        let listed = list(&vault, None).unwrap();
        assert_eq!(listed.len(), KEPT);
        assert_eq!(listed[0].when, KEPT as u64 + 4, "newest first");
        assert_eq!(listed[KEPT - 1].when, 5, "the oldest dropped");
        assert_eq!(list(&vault, Some(2)).unwrap().len(), 2);
    }

    #[test]
    fn a_failed_import_is_recorded_and_an_importers_is_not_run_again() {
        let vault = Vault::new(MemoryStore::default());
        let failed = record(
            &vault,
            &["/gone".into()],
            &Options::default(),
            10,
            Err("/gone: not found"),
        )
        .unwrap();
        assert_eq!(failed.error.as_deref(), Some("/gone: not found"));
        assert!(
            again(&vault, &failed.id).is_ok(),
            "a failed import is tried again"
        );
        let options = Options {
            listed: Some(Listed {
                by: "lightroom".into(),
                files: Default::default(),
            }),
            ..Options::default()
        };
        let through = record(&vault, &["/x.jpg".into()], &options, 11, Ok(&report(1))).unwrap();
        assert!(through.sources.is_empty());
        assert!(
            again(&vault, &through.id)
                .unwrap_err()
                .contains("lightroom")
        );
        assert!(again(&vault, "nope").is_err());
    }
}
