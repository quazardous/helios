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
//! - **Watched in the process** ([`Watch`]): the desktop that owns the
//!   node hears each file's line and the report before the promise
//!   settles, for what only it keeps (its undo step, its Duplicates and
//!   Log panels, the backups run after an import).
//! - **Photos chosen in a service's page** (Google Photos): the process
//!   that holds the connector and its picking hands them to the node
//!   ([`hand`]), then asks `library.import {remote: <key>, rule}`. The
//!   promise downloads them one by one (`helios_library::fetch`): each
//!   item a step of the progress with its outcome; a rate limit or a
//!   quota answer waits and goes on, an expired download address is
//!   listed again, the account's token is renewed per call by the
//!   connector's host. A pick lives in its process only: after a restart
//!   the promise fails, not retryable, and the photos chosen again skip
//!   those imported before (their item ids are known to the Library).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, RwLock};
use std::time::Duration;

use helios_connect::{Cancel, Connector, ConnectorProvider, ConnectorServices, Item, SourceRef};
use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_vault::{Store, Vault};
use serde_json::{Value, json};

use crate::endpoint::{Imported, SOURCE};
use crate::promises::{Handle, Promises};

/// What a process holding the node hears of its imports by rule, on the
/// import's thread: each file's line as `helios_library::import` gives
/// it, then the end. The request is the promise's (`library.import`'s
/// params, with `origin`).
pub trait ImportWatch: Send + Sync {
    /// A file's line.
    fn line(&self, request: &Value, line: &Value);
    /// The import ended (cancelled too): `report.changes` may be taken,
    /// the promise settles after; `camera_looks` were put on its raws.
    fn ended(
        &self,
        request: &Value,
        report: &mut helios_library::import::Report,
        camera_looks: usize,
    );
    /// The import failed before its end.
    fn failed(&self, request: &Value, error: &str);
}

/// Where the watch of a node's imports is set: none at first.
pub type Watch = Arc<RwLock<Option<Arc<dyn ImportWatch>>>>;

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
    !params["source"].is_null() || !params["rule"].is_null() || !params["remote"].is_null()
}

/// Photos chosen in a service's page, handed to the node by the process
/// holding their connector, for `library.import {remote: <key>}`.
pub struct Handed {
    /// The connector that listed them, its picking still open: it
    /// fetches them.
    pub connector: Arc<dyn Connector>,
    pub source: SourceRef,
    pub items: Vec<Item>,
    /// Whether the service's picking is ended once they are imported (a
    /// picking yes, an album listed no).
    pub end_pick: bool,
}

/// The picks handed in this process, by key, until imported.
static HANDED: LazyLock<Mutex<HashMap<String, Arc<Handed>>>> = LazyLock::new(Mutex::default);
static NEXT_PICK: AtomicU64 = AtomicU64::new(1);

fn picks() -> std::sync::MutexGuard<'static, HashMap<String, Arc<Handed>>> {
    HANDED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Hands `pick` to the node's imports: the key `library.import`'s
/// `remote` names it by. It stays until an import of it ends (a failed or
/// cancelled one keeps it, for Retry).
pub fn hand(pick: Handed) -> String {
    let key = format!(
        "pick-{}-{}",
        std::process::id(),
        NEXT_PICK.fetch_add(1, Ordering::Relaxed)
    );
    picks().insert(key.clone(), Arc::new(pick));
    key
}

fn handed(key: &str) -> Option<Arc<Handed>> {
    picks().get(key).cloned()
}

/// Why an import of a pick cannot run: the pick is not in this process.
fn pick_gone(key: &str) -> String {
    format!(
        "the photos chosen ({key}) are gone with the session that chose them: choose them again, those already imported are skipped"
    )
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
    if let Some(key) = params["remote"].as_str() {
        return start_remote(promises, params, origin, key, &rule);
    }
    let source = params["source"].as_str().map(PathBuf::from);
    if source.is_none() && !matches!(rule, Rule::Items(_)) {
        return Err("`library.import` needs a `source`, or a rule naming its `items`".into());
    }
    // Its choices and sources are checked now, not once the work runs:
    // a refusal is the answer, not a failed promise.
    let options = crate::library::import_options(params, None)?;
    match &options.placement {
        helios_library::import::Placement::Reference if options.rename.is_some() => {
            return Err("renaming needs a copy: a referenced file stays as it is".into());
        }
        helios_library::import::Placement::Copy { into: None }
            if helios_library::import::library_root(promises.vault())?.is_none() =>
        {
            return Err(
                "copying needs the library folder: give it once (`into`) and it is remembered"
                    .into(),
            );
        }
        _ => {}
    }
    let named: Vec<&Path> = match &rule {
        Rule::Items(items) => items.iter().map(PathBuf::as_path).collect(),
        Rule::All | Rule::New => source.iter().map(PathBuf::as_path).collect(),
    };
    if let Some(missing) = named.iter().find(|p| !p.exists()) {
        return Err(format!("{}: no such file or folder", missing.display()));
    }
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

/// [`start`] for a pick handed to the node ([`hand`]).
fn start_remote<S: Store + Send + 'static>(
    promises: &Promises<S>,
    params: &Value,
    origin: &str,
    key: &str,
    rule: &Rule,
) -> Result<helios_vault::promises::Promise, String> {
    let pick = handed(key).ok_or_else(|| pick_gone(key))?;
    if matches!(rule, Rule::Items(_)) {
        return Err("a pick is imported `all` or `new`: its items are the ones chosen".into());
    }
    let options = crate::library::import_options(params, None)?;
    // Always a copy: into the folder given, else the library folder.
    let into = matches!(
        &options.placement,
        helios_library::import::Placement::Copy { into: Some(_) }
    );
    if !into && helios_library::import::library_root(promises.vault())?.is_none() {
        return Err(
            "copying needs the library folder: give it once (`into`) and it is remembered".into(),
        );
    }
    let label = format!(
        "Import {} photos from {}",
        pick.items.len(),
        pick.source.label
    );
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
    register_watched(promises, bus, Watch::default())
}

/// [`register`], each import heard by what `watch` holds when it runs.
#[must_use]
pub fn register_watched<S: Store + Send + 'static>(
    promises: Promises<S>,
    bus: Arc<Bus>,
    watch: Watch,
) -> Promises<S> {
    let vault = promises.vault().clone();
    let jobs = promises.jobs().clone();
    promises.kind(KIND, true, move |handle| {
        let watcher = watch.read().unwrap_or_else(PoisonError::into_inner).clone();
        run(&vault, &jobs, &bus, &handle, watcher.as_deref());
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
    watch: Option<&dyn ImportWatch>,
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
    let outcome = import(vault, jobs, bus, handle, &request, &stop, watch);
    if let (Err(e), Some(watch)) = (&outcome, watch) {
        watch.failed(&request, e);
    }
    ended.store(true, Ordering::SeqCst);
    if let Some(watcher) = watcher {
        let _ = watcher.join();
    }
    match outcome {
        Ok(_) if stop.load(Ordering::SeqCst) || handle.cancel_asked() => handle.cancelled(),
        Ok(result) => handle.done(result),
        Err(_) if handle.cancel_asked() => handle.cancelled(),
        Err(e) => {
            // A pick gone with its process cannot be tried again.
            let retryable = request["remote"]
                .as_str()
                .is_none_or(|key| handed(key).is_some());
            handle.failed(&e, retryable);
        }
    }
}

fn import<S: Store + Send + 'static>(
    vault: &Arc<Vault<S>>,
    jobs: &Jobs,
    bus: &Arc<Bus>,
    handle: &Handle<S>,
    request: &Value,
    stop: &Arc<AtomicBool>,
    watch: Option<&dyn ImportWatch>,
) -> Result<Value, String> {
    let rule = Rule::of(request)?;
    let mut options = crate::library::import_options(request, Some(stop.clone()))?;
    options.origin = request["origin"].as_str().map(Into::into);
    if rule == Rule::New {
        options.duplicates = helios_library::import::Duplicates::Skip;
    }
    if let Some(key) = request["remote"].as_str() {
        let pick = handed(key).ok_or_else(|| pick_gone(key))?;
        let (mut report, failed) =
            fetched(vault, jobs, handle, request, &pick, &options, &rule, watch)?;
        let result = ended(
            vault,
            bus,
            request,
            &mut report,
            failed,
            &Value::Null,
            watch,
        );
        if !report.cancelled {
            // Imported: the service forgets the picking, the node the pick.
            if pick.end_pick
                && let Err(e) = pick.connector.end_pick()
            {
                log::debug!(target: "helios::node", "end of a picking: {e:?}");
            }
            picks().remove(key);
        }
        return result;
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
            if let Some(watch) = watch {
                watch.line(request, line);
            }
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
    ended(
        vault,
        bus,
        request,
        &mut report,
        failed,
        &json!(record),
        watch,
    )
}

/// What every import by rule does once its files are in: the cameras'
/// looks put on its raws, the watch told, the import announced; the
/// promise's result.
fn ended<S: Store + Send + 'static>(
    vault: &Arc<Vault<S>>,
    bus: &Arc<Bus>,
    request: &Value,
    report: &mut helios_library::import::Report,
    failed: Vec<Value>,
    history: &Value,
    watch: Option<&dyn ImportWatch>,
) -> Result<Value, String> {
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
    if let Some(watch) = watch {
        watch.ended(request, report, looks);
    }
    bus.publish(SOURCE, None, &Imported { report });
    Ok(json!({
        "report": report,
        "failed": failed,
        "history": history,
        "camera_looks": looks,
    }))
}

/// The photos of `pick` downloaded and imported
/// (`helios_library::fetch::import_remote`), each item a step of the
/// promise's progress with its name: the report and the items that
/// failed, with their reasons.
#[allow(clippy::too_many_arguments)]
fn fetched<S: Store + Send + 'static>(
    vault: &Arc<Vault<S>>,
    jobs: &Jobs,
    handle: &Handle<S>,
    request: &Value,
    pick: &Handed,
    options: &helios_library::import::Options,
    rule: &Rule,
    watch: Option<&dyn ImportWatch>,
) -> Result<(helios_library::import::Report, Vec<Value>), String> {
    let total = pick.items.len() as u64;
    handle.progress(0, Some(total), None);
    let remote = helios_library::fetch::Remote {
        connector: pick.connector.as_ref(),
        source: pick.source.clone(),
        items: pick.items.clone(),
        new_only: *rule == Rule::New,
    };
    let mut done = 0u64;
    let mut failed = Vec::new();
    let report =
        helios_library::fetch::import_remote(vault, jobs, &remote, options, now(), &mut |line| {
            if let Some(watch) = watch {
                watch.line(request, line);
            }
            if line["status"] == "cancelled" {
                return;
            }
            done += 1;
            if line["status"] == "failed" {
                failed.push(
                    json!({ "path": line["path"], "item": line["item"], "error": line["error"] }),
                );
            }
            let name = line["path"]
                .as_str()
                .map(|p| name_of(Path::new(p)))
                .unwrap_or_default();
            handle.progress(done, Some(total.max(done)), Some(&name));
        })?;
    Ok((report, failed))
}
