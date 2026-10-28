//! `helios batch`: a network applied to many files, each exported to a file.
//!
//! A client of a node's control and inspect endpoints: the images are
//! submitted as one **run** of the workflow level (`workflow.submit`, see
//! `docs/workflow-level.md`), which the node works itself, an item at a
//! time under a lease, retried when another attempt may pass, each output
//! path decided once at submission. The batch then follows the run's rows
//! at its export (`workflow.rows`, and the run read whole every two
//! seconds) and prints an image's line when its export settles. Up to
//! `jobs` exports of the run are out at once; the node's scheduler runs
//! them side by side.
//!
//! It starts its own node in a temporary sandbox, serving the batch
//! endpoint (every build) on a private socket, or attaches to a running
//! node's batch or debug control endpoint. With `--vault FILE`, its own
//! node keeps the run in that vault: the same batch run again (after a
//! crash, or an interrupt) extends the same run, and what was written is
//! not written again.
//!
//! Each image gets one JSON line on stdout: its path, the file written, how
//! long it took, and the error if it failed, in English (`error`) and by
//! key when it has one (`message`, a [`Message`] to show translated). The
//! others go on after a failure.
//!
//! With `--sidecar`, each image's base darktable sidecar is applied: its
//! edit becomes the item's own parameters. Its folder is listed once to
//! find it and its duplicates ([`helios_xmp::files`]). With `--vault FILE`
//! as well, every sidecar of the image is kept in that vault
//! ([`keep_sidecar`]): the image, each darktable edit (base and duplicates)
//! as a version with origin `import:xmp`, and the base's rating when the
//! vault has none.
//!
//! On an interrupt, the run is cancelled (its exports stopped: a file is
//! written whole or not at all, never in part), except a run the batch's
//! own node keeps in a vault: that node stops, and the same batch run
//! again finishes the run. A client that dies leaves a run on a node it
//! attached to going.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use helios_core::{Message, Params};
use helios_inspect::control::{ControlRequest, FileFormat, RunItem};
use helios_inspect::{Address, Request, request_at};
use serde_json::{Value, json};

pub use crate::darktable::{IMPORT_XMP, keep_sidecar, read_sidecar};
use crate::darktable::{Listings, keep_file};

/// What `helios batch` was asked.
#[derive(Clone, Debug)]
pub struct Options {
    /// The network file; required unless attached to a node.
    pub network: Option<PathBuf>,
    pub out: PathBuf,
    pub format: FileFormat,
    pub quality: u8,
    /// Exports of the run out at once.
    pub jobs: usize,
    /// The output file name: `{stem}` and `{ext}` are replaced.
    pub name: String,
    /// Apply each image's darktable sidecar on top of the network.
    pub sidecar: bool,
    /// Keep each sidecar applied in this vault (see [`keep_sidecar`]);
    /// the batch's own node keeps its run there too.
    pub vault: Option<PathBuf>,
    /// The source given each image; the network's only source by default.
    pub source: Option<String>,
    /// The node exported; the network's first export, else its only sink.
    pub node: Option<String>,
    /// A running node to attach to, instead of starting one.
    pub attach: Option<(Address, Address)>,
    /// A new run, even if the same batch has one.
    pub again: bool,
    pub images: Vec<PathBuf>,
}

/// The extension of a format's files.
pub fn extension(format: FileFormat) -> &'static str {
    match format {
        FileFormat::Jpeg => "jpg",
        FileFormat::Png => "png",
        FileFormat::Tiff => "tif",
    }
}

/// The output paths of the images, in order: the name template filled in,
/// and `-1`, `-2`... before the extension when the name is taken, by an
/// earlier image of the batch or by a file already there.
pub fn output_paths(
    images: &[PathBuf],
    out: &Path,
    template: &str,
    ext: &str,
    exists: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut taken = BTreeSet::new();
    images
        .iter()
        .map(|image| {
            let stem = image
                .file_stem()
                .map_or_else(|| "image".into(), |s| s.to_string_lossy().into_owned());
            let name = template.replace("{stem}", &stem).replace("{ext}", ext);
            let (base, dot_ext) = match name.rsplit_once('.') {
                Some((b, e)) if !b.is_empty() => (b.to_string(), format!(".{e}")),
                _ => (name.clone(), String::new()),
            };
            let mut n = 0;
            loop {
                let candidate = if n == 0 {
                    out.join(&name)
                } else {
                    out.join(format!("{base}-{n}{dot_ext}"))
                };
                if !taken.contains(&candidate) && !exists(&candidate) {
                    taken.insert(candidate.clone());
                    break candidate;
                }
                n += 1;
            }
        })
        .collect()
}

/// What a sidecar changes in the network: parameters by node name, and
/// what was left out. A module of the edit goes to the one node of the
/// network running that module; one the network does not run, or runs
/// more than once, is left out and named.
pub fn sidecar_params(
    imported: &helios_xmp::Imported,
    modules: &BTreeMap<String, String>,
) -> (BTreeMap<String, Params>, Value) {
    let mut params: BTreeMap<String, Params> = BTreeMap::new();
    let (mut applied, mut skipped) = (Vec::new(), Vec::new());
    for n in &imported.nodes {
        // Its mask is nodes the network does not have: the module alone
        // would change the whole image.
        if n.mask.is_some() {
            skipped.push(json!({ "operation": n.operation, "reason": "a masked module: the network has no node for its mask" }));
            continue;
        }
        let mut running = modules.iter().filter(|(_, m)| **m == n.module);
        match (running.next(), running.next()) {
            (Some((node, _)), None) if !params.contains_key(node) => {
                let target = params.entry(node.clone()).or_default();
                for (k, v) in n.params.iter() {
                    target.set(k, v.clone());
                }
                applied.push(json!({ "operation": n.operation, "node": node }));
            }
            (None, _) => skipped.push(json!({ "operation": n.operation, "reason": format!("the network has no `{}` node", n.module) })),
            _ => skipped.push(json!({ "operation": n.operation, "reason": format!("several `{}` nodes or instances", n.module) })),
        }
    }
    let mut report = json!({ "applied": applied, "skipped": skipped, "import": imported.report });
    if let Some(warning) = imported.report.summary() {
        report["warning"] = json!(warning.english);
        report["warning_message"] = json!(warning);
    }
    (params, report)
}

/// The network a batch's own node runs: files made absolute (the network
/// is copied into the sandbox), its own renders and exports left out.
fn sandbox_network(path: &Path) -> Result<(String, Option<String>), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut network: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let base = std::path::absolute(path)
        .map_err(|e| e.to_string())?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if let Some(sources) = network["sources"].as_array_mut() {
        for s in sources {
            if let Some(file) = s["file"].as_str() {
                s["file"] = json!(base.join(file));
            }
        }
    }
    let first_export = network["exports"][0]["node"].as_str().map(str::to_string);
    if let Some(map) = network.as_object_mut() {
        map.remove("renders");
        map.remove("exports");
    }
    Ok((
        serde_json::to_string_pretty(&network).expect("JSON values serialize"),
        first_export,
    ))
}

/// A node the batch talks to; its own sandbox is removed when dropped,
/// which stops its node.
struct Node {
    inspect: Address,
    control: Address,
    sandbox: Option<PathBuf>,
}

impl Drop for Node {
    fn drop(&mut self) {
        if let Some(dir) = &self.sandbox {
            let _ = crate::sandbox::clean(dir);
        }
    }
}

impl Node {
    /// Stops the batch's own node now, and removes its sandbox.
    fn stop(&mut self) {
        if let Some(dir) = self.sandbox.take() {
            let _ = crate::sandbox::clean(&dir);
        }
    }

    fn call(address: &Address, body: &Value) -> Result<Value, Message> {
        let answer = request_at(address, body).map_err(|e| {
            Message::new(
                "batch-node-unreachable",
                "cannot reach the node at { $address }: { $error }",
            )
            .with("address", address.to_string())
            .with("error", e.to_string())
        })?;
        if answer["ok"] == true {
            Ok(answer)
        } else {
            // The node's message when it has a key, else its English.
            let keyed = serde_json::from_value::<Message>(answer["message"].clone()).ok();
            Err(match (keyed, answer["error"].as_str()) {
                (Some(m), _) => m,
                (None, Some(e)) => Message::plain(e),
                (None, None) => Message::new("batch-node-refused", "the node refused"),
            })
        }
    }

    fn control(&self, request: &ControlRequest) -> Result<Value, Message> {
        Self::call(
            &self.control,
            &serde_json::to_value(request).expect("requests serialize"),
        )
    }

    fn inspect(&self, request: &Request) -> Result<Value, Message> {
        Self::call(
            &self.inspect,
            &serde_json::to_value(request).expect("requests serialize"),
        )
    }
}

/// Starts a node in a fresh sandbox (`node_bin` is `helios-node`), serving
/// the batch endpoint on a socket in a directory private to this run: the
/// debug control endpoint is not needed, so this works in release builds.
/// With `runs_vault`, the node keeps its runs there.
fn own_node(
    network: &Path,
    jobs: usize,
    runs_vault: Option<&Path>,
    node_bin: &Path,
) -> Result<(Node, Option<String>), String> {
    let (text, first_export) = sandbox_network(network)?;
    let dir = std::env::temp_dir().join(format!("helios-batch-{}", std::process::id()));
    if dir.join("sandbox.json").exists() {
        crate::sandbox::clean(&dir)?;
    }
    crate::sandbox::create(&dir, Some(&text))?;
    let started = std::fs::write(
        dir.join("global.toml"),
        format!("[system.scheduler]\nworkers = {jobs}\n"),
    )
    .map_err(|e| e.to_string())
    .and_then(|()| crate::sandbox::start_batch(&dir, node_bin, runs_vault));
    match started {
        Ok(state) => Ok((
            Node {
                inspect: Address::Unix(state.inspect),
                control: Address::Unix(state.control),
                sandbox: Some(dir),
            },
            first_export,
        )),
        Err(e) => {
            let _ = crate::sandbox::clean(&dir);
            Err(e)
        }
    }
}

/// Removes what a node stopped in the middle of writing `output` left
/// (`helios_export::write_atomic`'s temporary file, `.NAME.helios-PID`
/// next to it): an interrupted export leaves no part of a file.
fn remove_parts(output: &Path) {
    let (Some(dir), Some(name)) = (output.parent(), output.file_name()) else {
        return;
    };
    let prefix = format!(".{}.helios-", name.to_string_lossy());
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The module each node of the node's graph runs, by node name: where a
/// sidecar's edit goes.
fn modules(graph: &Value) -> BTreeMap<String, String> {
    graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            Some((
                n["name"].as_str()?.to_string(),
                n["module"].as_str()?.to_string(),
            ))
        })
        .collect()
}

/// One image on its way.
struct Pending {
    image: PathBuf,
    output: PathBuf,
    started: Instant,
    sidecar: Value,
}

fn line(p: &Pending, error: Option<&Message>) -> Value {
    let mut line = json!({
        "path": p.image,
        "output": if error.is_none() { json!(p.output) } else { Value::Null },
        "duration_ms": p.started.elapsed().as_millis() as u64,
        "error": error.map(|e| &e.english),
        "sidecar": p.sidecar,
    });
    if let Some(e) = error.filter(|e| e.is_keyed()) {
        line["message"] = json!(e);
    }
    line
}

/// The key of a node that cannot be reached (as `Node::call` writes it):
/// it stops the whole batch.
pub const UNREACHABLE: &str = "batch-node-unreachable";

fn cancelled_message() -> Message {
    Message::new("batch-cancelled", "cancelled")
}

/// How a batch ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every image was written.
    Done,
    /// At least one image failed, or waits for what the batch cannot
    /// give (a person's answer, a paused run); the others were written.
    Failed,
    /// Interrupted: what was asked was cancelled, or, in a run kept in a
    /// vault by the batch's own node, stopped to be finished by the same
    /// batch run again.
    Interrupted,
}

/// How often the run is read whole besides its events: what the node's
/// bounded event journal let go of is caught up then.
const RECONCILE: Duration = Duration::from_secs(2);

/// How long a cancelled batch waits for the node to cancel its items.
const CANCEL_WAIT: Duration = Duration::from_secs(30);

/// Items read at once from the run.
const PAGE: usize = 500;

/// What became of an item at the run's export.
enum Settled {
    Written,
    Failed(Message),
    Cancelled,
}

/// What a state of an item at the export (a row status, or where the
/// run places an item without a row) says: settled, or still on its way
/// (`None`).
fn settled(state: &str, error: Option<&str>, message: &Value) -> Option<Settled> {
    let failure = || {
        serde_json::from_value::<Message>(message.clone())
            .ok()
            .filter(Message::is_keyed)
            .unwrap_or_else(|| Message::plain(error.unwrap_or(state)))
    };
    match state {
        "done" => Some(Settled::Written),
        "cancelled" => Some(Settled::Cancelled),
        "failed" | "skipped" | "omitted" => Some(Settled::Failed(failure())),
        "held" => Some(Settled::Failed(Message::new(
            "batch-held",
            "not exported: a step before the export failed or was cancelled",
        ))),
        _ => None,
    }
}

/// The items of the batch still on their way, by item id, and how the
/// settled ones ended.
struct Follow {
    pending: BTreeMap<String, Pending>,
    failed: u64,
    cancelled: bool,
    /// The last progress written on stderr.
    shown: String,
}

impl Follow {
    /// Settles `item` when `state` says it is, with its line.
    fn apply(
        &mut self,
        item: &str,
        state: &str,
        error: Option<&str>,
        message: &Value,
        emit: &mut impl FnMut(&Value),
    ) {
        if state == "running"
            && let Some(p) = self.pending.get_mut(item)
        {
            // Its time counts from when its export started.
            p.started = Instant::now();
            return;
        }
        let Some(how) = settled(state, error, message) else {
            return;
        };
        let Some(p) = self.pending.remove(item) else {
            return;
        };
        match how {
            Settled::Written => emit(&line(&p, None)),
            Settled::Cancelled => {
                self.cancelled = true;
                emit(&line(&p, Some(&cancelled_message())));
            }
            Settled::Failed(why) => {
                self.failed += 1;
                emit(&line(&p, Some(&why)));
            }
        }
    }

    /// Ends every item still on its way with `why`.
    fn end_all(&mut self, why: &Message, emit: &mut impl FnMut(&Value)) {
        for p in std::mem::take(&mut self.pending).into_values() {
            emit(&line(&p, Some(why)));
        }
    }

    /// Reads the run whole: each item where it stands, then whether the
    /// run can still move on its own. A run that only waits for a person
    /// (a decision nobody answered), or is paused, ends the items left
    /// with why.
    fn reconcile(
        &mut self,
        node: &Node,
        run: &str,
        emit: &mut impl FnMut(&Value),
    ) -> Result<(), Message> {
        let mut offset = 0;
        loop {
            let page = node.control(&ControlRequest::WorkflowItems {
                run: run.to_string(),
                node: None,
                state: None,
                offset,
                limit: PAGE,
            })?;
            let items = page["items"].as_array().cloned().unwrap_or_default();
            for i in &items {
                if let (Some(id), Some(state)) = (i["item"].as_str(), i["state"].as_str()) {
                    self.apply(id, state, i["error"].as_str(), &i["message"], emit);
                }
            }
            offset += items.len();
            if items.is_empty() || offset as u64 >= page["total"].as_u64().unwrap_or(0) {
                break;
            }
        }
        if self.pending.is_empty() {
            return Ok(());
        }
        let runs = node.control(&ControlRequest::WorkflowRuns)?;
        let Some(summary) = runs["runs"]
            .as_array()
            .and_then(|all| all.iter().find(|r| r["id"] == run))
        else {
            self.failed += self.pending.len() as u64;
            let gone =
                Message::new("batch-run-gone", "the run { $run } left the node").with("run", run);
            self.end_all(&gone, emit);
            return Ok(());
        };
        self.show(run, summary);
        let nodes: Vec<&Value> = summary["nodes"]
            .as_object()
            .map(|n| n.values().collect())
            .unwrap_or_default();
        let count = |n: &Value, key: &str| n[key].as_u64().unwrap_or(0);
        let moving = nodes.iter().any(|n| {
            ["running", "scheduled", "waiting", "leases_out"]
                .iter()
                .any(|k| count(n, k) > 0)
        });
        let why = if summary["state"] == "paused" {
            Some(Message::new(
                "batch-run-paused",
                "not exported: the run { $run } is paused",
            ))
        } else if !moving && nodes.iter().any(|n| count(n, "asking") > 0) {
            Some(Message::new(
                "batch-asking",
                "not exported: the run { $run } waits for a person to answer a decision",
            ))
        } else {
            None
        };
        if let Some(why) = why {
            self.failed += self.pending.len() as u64;
            self.end_all(&why.with("run", run), emit);
        }
        Ok(())
    }

    /// Writes the run's progress per queueing node on stderr, when it is
    /// a terminal and the progress changed.
    fn show(&mut self, run: &str, summary: &Value) {
        if !std::io::IsTerminal::is_terminal(&std::io::stderr()) {
            return;
        }
        let Some(nodes) = summary["nodes"].as_object() else {
            return;
        };
        let parts: Vec<String> = nodes
            .iter()
            .map(|(name, n)| {
                let counts: Vec<String> = [
                    "done",
                    "failed",
                    "running",
                    "scheduled",
                    "waiting",
                    "asking",
                    "held",
                ]
                .iter()
                .filter_map(|k| {
                    let v = n[*k].as_u64().unwrap_or(0);
                    (v > 0).then(|| format!("{v} {k}"))
                })
                .collect();
                format!("{name}: {}", counts.join(", "))
            })
            .collect();
        let shown = format!("helios batch: run {run}: {}", parts.join("; "));
        if shown != self.shown {
            eprintln!("{shown}");
            self.shown = shown;
        }
    }
}

/// Runs a batch, printing a JSON line per image through `emit`. Errors
/// that stop the whole batch (no node, a network that does not fit) come
/// back as `Err`.
///
/// The images are submitted as one run of the workflow level
/// (`workflow.submit`), which the node works with leases and retries; the
/// batch then follows the run's rows (`workflow.rows` events, and the run
/// read whole every two seconds) and prints each image's line when its
/// export settles. The same batch run again extends the same run: what it
/// wrote is not written again (`again` starts a new run).
pub fn run(
    options: &Options,
    node_bin: &Path,
    interrupted: &Arc<AtomicBool>,
    mut emit: impl FnMut(&Value),
) -> Result<Outcome, Message> {
    std::fs::create_dir_all(&options.out).map_err(|e| {
        Message::new("batch-cannot-create", "cannot create { $path }: { $error }")
            .with("path", &options.out)
            .with("error", e.to_string())
    })?;
    let out = std::path::absolute(&options.out).map_err(|e| Message::plain(e.to_string()))?;
    if options.attach.is_some() && options.vault.is_some() && !options.sidecar {
        return Err(Message::new(
            "batch-vault-attached",
            "--vault with a running node only keeps sidecars: it needs --sidecar (the node keeps its runs in its own vault)",
        ));
    }
    let (mut node, first_export) = match (&options.attach, &options.network) {
        (Some((inspect, control)), network) => {
            let first = match network {
                Some(path) => sandbox_network(path)?.1,
                None => None,
            };
            let node = Node {
                inspect: inspect.clone(),
                control: control.clone(),
                sandbox: None,
            };
            (node, first)
        }
        (None, Some(network)) => {
            own_node(network, options.jobs, options.vault.as_deref(), node_bin)?
        }
        (None, None) => {
            return Err(Message::new(
                "batch-needs-network",
                "batch needs --network FILE, or a node to attach to",
            ));
        }
    };
    let graph = node.inspect(&Request::Describe {
        entity: "graph".into(),
    })?["describe"]
        .clone();
    let modules = modules(&graph);
    let images: Vec<PathBuf> = options
        .images
        .iter()
        .map(|i| std::path::absolute(i).unwrap_or_else(|_| i.clone()))
        .collect();

    let vault = match (&options.vault, options.sidecar) {
        (Some(path), true) => Some(helios_vault::Vault::new(
            helios_vault_sqlite::SqliteStore::open(path)
                .map_err(|e| Message::plain(format!("{}: {e}", path.display())))?,
        )),
        _ => None,
    };
    let mut sidecars = Sidecars {
        vault: vault.as_ref(),
        listings: Listings::default(),
    };
    let mut follow = Follow {
        pending: BTreeMap::new(),
        failed: 0,
        cancelled: false,
        shown: String::new(),
    };
    // Each image's sidecar first: what it changes is part of its item.
    let mut items = Vec::with_capacity(images.len());
    let mut reports: BTreeMap<PathBuf, Value> = BTreeMap::new();
    let mut refused: BTreeSet<&PathBuf> = BTreeSet::new();
    for image in &images {
        if interrupted.load(Ordering::SeqCst) {
            break;
        }
        match sidecar_of(options, &mut sidecars, &modules, image) {
            Ok((params, report)) => {
                reports.insert(image.clone(), report);
                items.push(if params.is_empty() {
                    RunItem::Path(image.clone())
                } else {
                    RunItem::With {
                        path: image.clone(),
                        params,
                    }
                });
            }
            Err(e) => {
                follow.failed += 1;
                refused.insert(image);
                let p = Pending {
                    image: image.clone(),
                    output: PathBuf::new(),
                    started: Instant::now(),
                    sidecar: Value::Null,
                };
                emit(&line(&p, Some(&e)));
            }
        }
    }
    drop(sidecars);
    drop(vault);
    if interrupted.load(Ordering::SeqCst) {
        // Nothing was asked of the node yet.
        for image in images.iter().filter(|i| !refused.contains(i)) {
            let p = Pending {
                image: image.clone(),
                output: PathBuf::new(),
                started: Instant::now(),
                sidecar: Value::Null,
            };
            emit(&line(&p, Some(&cancelled_message())));
        }
        return Ok(Outcome::Interrupted);
    }
    if items.is_empty() {
        return Ok(if follow.failed > 0 {
            Outcome::Failed
        } else {
            Outcome::Done
        });
    }

    let answer = node.control(&ControlRequest::WorkflowSubmit {
        title: Some(format!("Batch of {} images", images.len())),
        items,
        out,
        format: options.format,
        quality: Some(options.quality),
        name: options.name.clone(),
        node: options.node.clone().or(first_export),
        source: options.source.clone(),
        again: options.again,
        concurrency: Some(options.jobs.max(1)),
    })?;
    let run = answer["run"].as_str().unwrap_or_default().to_string();
    let job = answer["job"].as_str().map(str::to_string);
    // The batch's own node stops with it; a run it keeps in a vault is
    // finished by the same batch run again.
    let resumable = options.attach.is_none() && answer["durable"] == true;
    let export = answer["workflow"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|n| n["kind"] == "export"))
        .and_then(|n| n["name"].as_str())
        .unwrap_or_default()
        .to_string();
    if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        eprintln!(
            "helios batch: run {run}: {} new, {} already in it{}",
            answer["new"],
            answer["already"],
            if answer["durable"] == true {
                ", kept in the vault"
            } else {
                ""
            }
        );
    }
    let started = Instant::now();
    for i in answer["items"].as_array().into_iter().flatten() {
        let (Some(id), Some(path), Some(output)) =
            (i["item"].as_str(), i["path"].as_str(), i["output"].as_str())
        else {
            continue;
        };
        let image = PathBuf::from(path);
        follow.pending.insert(
            id.to_string(),
            Pending {
                sidecar: reports.get(&image).cloned().unwrap_or(Value::Null),
                image,
                output: PathBuf::from(output),
                started,
            },
        );
    }

    let mut cursor = answer["seq"].as_u64().unwrap_or(0);
    let mut cancel_asked: Option<Instant> = None;
    let mut read_whole: Option<Instant> = None;
    while !follow.pending.is_empty() {
        if interrupted.load(Ordering::SeqCst) && cancel_asked.is_none() {
            if resumable {
                follow.cancelled = true;
                node.stop();
                for p in follow.pending.values() {
                    remove_parts(&p.output);
                }
                let stopped = Message::new(
                    "batch-stopped",
                    "stopped before it was written: the same batch run again finishes it",
                );
                follow.end_all(&stopped, &mut emit);
                break;
            }
            // The run's job cancelled: its exports stop, its rows not
            // concluded are cancelled.
            // A job already ended has nothing left to cancel.
            if let Some(id) = &job
                && let Err(e) = node.control(&ControlRequest::JobCancel { id: id.clone() })
                && e.key == UNREACHABLE
            {
                return Err(e);
            }
            cancel_asked = Some(Instant::now());
        }
        if cancel_asked.is_some_and(|at| at.elapsed() > CANCEL_WAIT) {
            follow.cancelled = true;
            follow.end_all(&cancelled_message(), &mut emit);
            break;
        }
        if read_whole.is_none_or(|at| at.elapsed() >= RECONCILE) {
            read_whole = Some(Instant::now());
            follow.reconcile(&node, &run, &mut emit)?;
            continue;
        }
        // Bounded, so an interrupt is seen soon.
        let woken = node.inspect(&Request::WaitEvent {
            kind: "workflow.rows".into(),
            since: cursor,
            timeout_ms: 250,
        });
        match woken {
            Ok(_) => {}
            Err(e) if e.key == UNREACHABLE => return Err(e),
            Err(_) => continue,
        }
        let events = node.inspect(&Request::Events { since: cursor })?;
        for event in events["events"].as_array().into_iter().flatten() {
            cursor = cursor.max(event["seq"].as_u64().unwrap_or(cursor));
            let payload = &event["payload"];
            if event["kind"] != "workflow.rows" || payload["run"] != run.as_str() {
                continue;
            }
            if payload["node"] != export.as_str() {
                continue;
            }
            for row in payload["items"].as_array().into_iter().flatten() {
                if let (Some(id), Some(status)) = (row["item"].as_str(), row["status"].as_str()) {
                    follow.apply(
                        id,
                        status,
                        row["error"].as_str(),
                        &row["message"],
                        &mut emit,
                    );
                }
            }
        }
    }
    Ok(if follow.cancelled {
        Outcome::Interrupted
    } else if follow.failed > 0 {
        Outcome::Failed
    } else {
        Outcome::Done
    })
}

/// Where a batch finds sidecars, and the vault it keeps them in.
struct Sidecars<'a> {
    vault: Option<&'a helios_vault::Vault<helios_vault_sqlite::SqliteStore>>,
    listings: Listings,
}

/// What an image's sidecars change, with `--sidecar`: the parameters of
/// its base sidecar by node name, and the report of its line (`null`
/// without a sidecar); each sidecar kept in the vault when there is one.
fn sidecar_of(
    options: &Options,
    sidecars: &mut Sidecars,
    modules: &BTreeMap<String, String>,
    image: &Path,
) -> Result<(BTreeMap<String, Params>, Value), Message> {
    let mut params = BTreeMap::new();
    if !options.sidecar {
        return Ok((params, Value::Null));
    }
    let vault = sidecars.vault;
    let files = sidecars.listings.sidecars(image).map_err(Message::plain)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut warn = |path: &Path, warning: &str| {
        // Once per import, beside the image's JSON line.
        eprintln!("helios batch: {}: {warning}", path.display());
    };
    // The base sidecar is the edit exported; the others are kept only.
    let (base, others) = match files.split_first() {
        Some((first, rest)) if first.version == 0 => (Some(first), rest),
        _ => (None, &files[..]),
    };
    let mut report = Value::Null;
    if let Some(base) = base {
        let path = &base.path;
        let read = std::fs::read_to_string(path)
            .map_err(|e| {
                Message::new("batch-cannot-read", "cannot read { $path }: { $error }")
                    .with("path", path)
                    .with("error", e.to_string())
            })
            .and_then(|xml| {
                let (sidecar, imported) = read_sidecar(&xml).map_err(Message::plain)?;
                if let Some(warning) = imported.report.summary() {
                    warn(path, &warning.english);
                }
                let (params, mut report) = sidecar_params(&imported, modules);
                if let Some(vault) = vault {
                    report["vault"] = keep_sidecar(vault, image, 0, true, &sidecar, &imported, now)
                        .map_err(Message::plain)?;
                }
                Ok((params, report))
            });
        match read {
            Ok((p, mut r)) => {
                r["path"] = json!(path);
                report = r;
                params = p;
            }
            Err(e) if e.is_keyed() => return Err(e),
            Err(e) => return Err(Message::plain(format!("{}: {e}", path.display()))),
        }
    }
    if !others.is_empty() {
        let others: Vec<Value> = others
            .iter()
            .map(|file| match vault {
                Some(vault) => keep_file(vault, image, file, false, now, &mut warn),
                None => {
                    json!({ "path": file.path, "version": file.version, "exact": file.exact })
                }
            })
            .collect();
        if report.is_null() {
            report = json!({});
        }
        report["duplicates"] = json!(others);
    }
    Ok((params, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_image_says_why_in_english_and_by_key() {
        let p = Pending {
            image: "a.png".into(),
            output: "out/a.jpg".into(),
            started: Instant::now(),
            sidecar: Value::Null,
        };
        let failed = line(&p, Some(&cancelled_message()));
        assert_eq!(failed["error"], "cancelled");
        assert_eq!(failed["message"]["key"], "batch-cancelled");
        assert!(failed["output"].is_null());
        let plain = line(&p, Some(&Message::plain("broken.png: not a picture")));
        assert_eq!(plain["error"], "broken.png: not a picture");
        assert!(plain.get("message").is_none());
        assert!(line(&p, None)["error"].is_null());
    }

    #[test]
    fn names_that_collide_get_a_number() {
        let images: Vec<PathBuf> = ["a/x.cr2", "b/x.png", "c/x.nef", "y.png"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let out = Path::new("/out");
        let on_disk = |p: &Path| p == Path::new("/out/y.jpg");
        let paths = output_paths(&images, out, "{stem}.{ext}", "jpg", on_disk);
        let names: Vec<_> = paths.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(
            names,
            ["/out/x.jpg", "/out/x-1.jpg", "/out/x-2.jpg", "/out/y-1.jpg"]
        );
        let plain = output_paths(&images[..2], out, "{stem}", "jpg", |_| false);
        assert_eq!(plain, [out.join("x"), out.join("x-1")]);
    }
}
