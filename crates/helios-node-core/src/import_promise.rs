//! `library.import {source, rule}`: an import answered at once with a
//! promise (see `docs/protocol.md`, "Promises", and
//! `docs/design/import-slow-sources.md`, slice 3).
//!
//! The promise's work, of kind [`KIND`], lists the source itself through
//! its connector (the progress says how many files were found so far),
//! then imports them, each file a step of the promise's progress
//! (`done` of `total`, the file's name as its note). It settles with the
//! outcome: the import's report, the files that failed with their
//! reasons, the import history's record id and the camera looks put on.
//!
//! - **Rules:** `all` (every image file of the source, duplicates as
//!   `duplicates` says), `new` (what the library knows is left out: the
//!   image map makes it cheap, a copied file is not read again) and
//!   `{"items": [...]}` (the files and folders named, a window's
//!   selection).
//! - **Cancel** (`promises.cancel`): the import stops before its next
//!   file; what it imported stays, and the promise settles `cancelled`.
//! - **After a stop or a crash** the kind is resumable: the node takes
//!   the promise up again and runs the same import, which skips what is
//!   already in the library (dedupe makes reading again safe).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use helios_connect::{Cancel, ConnectorProvider, ConnectorServices};
use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_vault::{Store, Vault};
use serde_json::{Value, json};

use crate::endpoint::{Imported, SOURCE};
use crate::promises::{Handle, Promises};

/// The kind of the import's promises.
pub const KIND: &str = "import";

/// Items a listing page holds.
const PAGE: usize = 500;

/// How often a running import looks whether a cancel was asked.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// What an import by rule takes in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// Every image file of the source.
    All,
    /// Only what the library does not know.
    New,
    /// The files and folders named.
    Items(Vec<PathBuf>),
}

impl Rule {
    /// The rule of `library.import`'s params: `all` when not given.
    ///
    /// # Errors
    /// A rule that is neither `all`, `new` nor `{"items": [...]}`.
    pub fn of(params: &Value) -> Result<Self, String> {
        match &params["rule"] {
            Value::Null => Ok(Self::All),
            Value::String(s) if s == "all" => Ok(Self::All),
            Value::String(s) if s == "new" => Ok(Self::New),
            Value::Object(o) => {
                let items: Vec<PathBuf> = o
                    .get("items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect();
                if items.is_empty() {
                    return Err("the rule's `items` names no file".into());
                }
                Ok(Self::Items(items))
            }
            other => Err(format!("`{other}` is not a rule: all, new or {{items}}")),
        }
    }
}

/// Whether `library.import`'s params ask for an import by rule (answered
/// with a promise) rather than of `paths` (answered with its report).
pub fn by_rule(params: &Value) -> bool {
    !params["source"].is_null() || !params["rule"].is_null()
}

/// Starts the import `params` ask for (`source`, `rule` and the import's
/// choices) as a promise of `promises`: answered at once, pending.
///
/// # Errors
/// No source and no items, a rule refused, or no runner for imports.
pub fn start<S: Store + Send + 'static>(
    promises: &Promises<S>,
    params: &Value,
    origin: &str,
) -> Result<helios_vault::promises::Promise, String> {
    let rule = Rule::of(params)?;
    let source = params["source"].as_str().map(PathBuf::from);
    if source.is_none() && !matches!(rule, Rule::Items(_)) {
        return Err("`library.import` needs a `source`, or a rule naming its `items`".into());
    }
    // Its choices are checked now, not once the work runs.
    crate::library::import_options(params, None)?;
    let label = match (&source, &rule) {
        (_, Rule::Items(items)) => format!("Import {} items", items.len()),
        (Some(source), _) => format!("Import {}", name_of(source)),
        (None, _) => "Import".into(),
    };
    let mut request = params.clone();
    if let Some(fields) = request.as_object_mut() {
        fields.remove("_meta");
        fields.insert("origin".into(), json!(origin));
    }
    promises.start(KIND, &label, request)
}

fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// `promises` running imports too, each announced on `bus` once done
/// (`library.imported`, as an import by `paths`).
#[must_use]
pub fn register<S: Store + Send + 'static>(promises: Promises<S>, bus: Arc<Bus>) -> Promises<S> {
    let vault = promises.vault().clone();
    let jobs = promises.jobs().clone();
    promises.kind(KIND, true, move |handle| {
        run(&vault, &jobs, &bus, &handle);
    })
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The files of `source`, listed through its connector page by page, the
/// count so far told on the promise.
fn listed<S: Store>(source: &Path, handle: &Handle<S>, cancel: &Cancel) -> Result<u64, String> {
    let connector = helios_connect::folder::FolderProvider
        .create(&Value::Null, &ConnectorServices::default())
        .map_err(|e| format!("{}: {e:?}", source.display()))?;
    let source_ref = helios_connect::folder::Folder::source(source);
    let mut from = None;
    let mut found = 0u64;
    loop {
        let page = connector
            .list(&source_ref, from.as_ref(), PAGE, cancel)
            .map_err(|e| format!("{}: {e:?}", source.display()))?;
        found += page.items.len() as u64;
        handle.progress(0, None, Some(&format!("listing: {found} found")));
        match page.next {
            Some(next) if !page.complete => from = Some(next),
            _ => return Ok(found),
        }
    }
}

/// The import of `handle`'s request, settling its promise.
fn run<S: Store + Send + 'static>(
    vault: &Arc<Vault<S>>,
    jobs: &Jobs,
    bus: &Arc<Bus>,
    handle: &Handle<S>,
) {
    let request = handle.request().clone();
    let stop = Arc::new(AtomicBool::new(false));
    // A cancel asked of the promise stops the import before its next
    // file.
    let ended = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (stop, ended) = (stop.clone(), ended.clone());
        let (vault, id) = (vault.clone(), handle.id().to_string());
        std::thread::Builder::new()
            .name("helios-import-cancel".into())
            .spawn(move || {
                while !ended.load(Ordering::SeqCst) {
                    let asked = vault
                        .promise(&id)
                        .ok()
                        .flatten()
                        .is_none_or(|p| p.cancel_asked);
                    if asked {
                        stop.store(true, Ordering::SeqCst);
                        return;
                    }
                    std::thread::sleep(CANCEL_POLL);
                }
            })
            .ok()
    };
    let outcome = import(vault, jobs, bus, handle, &request, &stop);
    ended.store(true, Ordering::SeqCst);
    if let Some(watcher) = watcher {
        let _ = watcher.join();
    }
    match outcome {
        Ok(_) if stop.load(Ordering::SeqCst) || handle.cancel_asked() => handle.cancelled(),
        Ok(result) => handle.done(result),
        Err(_) if handle.cancel_asked() => handle.cancelled(),
        Err(e) => handle.failed(&e, true),
    }
}

fn import<S: Store + Send + 'static>(
    vault: &Arc<Vault<S>>,
    jobs: &Jobs,
    bus: &Arc<Bus>,
    handle: &Handle<S>,
    request: &Value,
    stop: &Arc<AtomicBool>,
) -> Result<Value, String> {
    let rule = Rule::of(request)?;
    let mut options = crate::library::import_options(request, Some(stop.clone()))?;
    options.origin = request["origin"].as_str().map(Into::into);
    if rule == Rule::New {
        options.duplicates = helios_library::import::Duplicates::Skip;
    }
    let cancel = Cancel::from_flag(stop.clone());
    let (sources, total) = match &rule {
        Rule::Items(items) => {
            let total = items
                .iter()
                .map(|i| helios_connect::local::files_of(i).map_or(0, |f| f.len() as u64))
                .sum();
            (items.clone(), total)
        }
        Rule::All | Rule::New => {
            let source = PathBuf::from(request["source"].as_str().unwrap_or_default());
            let total = listed(&source, handle, &cancel)?;
            (vec![source], total)
        }
    };
    if stop.load(Ordering::SeqCst) {
        return Ok(Value::Null);
    }
    handle.progress(0, Some(total), None);
    let mut done = 0u64;
    let mut failed = Vec::new();
    let (report, record) = helios_library::import::import_recorded(
        vault,
        jobs,
        &sources,
        &options,
        now(),
        &mut |line| {
            if line["status"] == "cancelled" {
                return;
            }
            done += 1;
            if line["status"] == "failed" {
                failed.push(json!({ "path": line["path"], "error": line["error"] }));
            }
            let name = line["path"]
                .as_str()
                .map(|p| name_of(Path::new(p)))
                .unwrap_or_default();
            handle.progress(done, Some(total.max(done)), Some(&name));
        },
    );
    let mut report = report?;
    let looks = if request["camera_look"] == false {
        0
    } else {
        crate::calibrate::look::default_on_import(
            vault,
            &mut report.changes,
            now(),
            &crate::calibrate::look::raw_camera,
        )
        .map_err(|m| m.english)?
    };
    bus.publish(SOURCE, None, &Imported { report: &report });
    Ok(json!({
        "report": report,
        "failed": failed,
        "history": record,
        "camera_looks": looks,
    }))
}
