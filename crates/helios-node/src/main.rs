//! `helios-node`: a headless Helios node.
//!
//! Loads a network from a JSON description, serves the read-only inspection
//! endpoint on a socket, prints `ready <socket>` on stdout, renders what the
//! description asks for, then stays up so it can be inspected. Stop it with
//! SIGTERM or SIGINT (Ctrl-C): it exits with 0, its vault closed cleanly.
//!
//!     helios-node --graph network.json --socket /path/to/node.sock
//!                 [--global FILE] [--project DIR] [--user FILE] [--vault FILE] [--runs-vault FILE]
//!                 [--set KEY=VALUE]... [--control /path/to/control.sock] [--batch-control PATH]
//!                 [--token-file FILE]
//!
//! An endpoint is a Unix socket path or `tcp://127.0.0.1:PORT` (port 0 picks
//! one); with TCP the node writes a session token to `--token-file` (readable
//! by its user only) and prints the addresses it bound: `ready ADDRESS`, then
//! `control ADDRESS`.
//!
//! Settings come from the files given (global layer, `DIR/helios.toml` for
//! the project layer, user layer) and from `--set` (run layer); nothing is
//! read from a default location behind the caller's back. With `--vault
//! FILE` (a SQLite vault), the user and project layers live in the vault:
//! a layer file given and not yet in the vault is imported into it once,
//! and from then on the vault's copy is the one read.
//!
//! With `--vault`, the node owns the vault (`helios_node::vault_node`): it
//! opens it once and lends it to the settings, the edit and the runs, saves
//! it on its own schedule, and runs what the desktop's node runs without a
//! window: it watches the machine's volumes, runs the backups (a backup
//! disk of the strategy plugged in starts one) and the health check on its
//! schedule, each a job on its bus. An import through its control
//! endpoints (`library.import`) is announced on the bus and starts the
//! strategy's after-import backup. The settings layers it keeps (`user`,
//! `project/<folder>`) are read and written through them too
//! (`settings.layer.get`, `settings.layer.set`), each change announced on
//! the bus (`settings.layerChanged`). A signal cancels the running backups
//! (the copy under way is recorded), then closes the vault cleanly.
//!
//! `--control` serves the debug control endpoint (render, set a parameter,
//! cancel, pause, resume, inject a fault). It exists only in debug builds and
//! only when `HELIOS_DEBUG_CONTROL` is set.
//!
//! With `--vault FILE` and a network whose first source is a file, the node
//! keeps the edit of that file's image in the vault: on start it continues
//! the image's newest version (parameters, then the decision book
//! replayed), and each change made through control (a parameter set, a
//! decision answered) is saved at once as a version, announced as
//! `edit.saved`. A restart, even after `kill -9`, resumes from the last
//! change answered.
//!
//! `--batch-control` serves the batch endpoint in every build: only
//! `load-source`, `export`, `cancel`, `job.cancel`, the batch job's
//! `batch_start` and `batch_end`, and the runs' `workflow.*` requests
//! (`submit`, `runs`, `items`, `forget`, `cancel`, `retry`,
//! `prioritize`, `pause`, `resume`), `library.import` and
//! `settings.layer.*` (with `--vault`), on a Unix socket only its user can
//! open. `helios batch` starts its own node with it, on a socket in a
//! private directory made for that run.
//!
//! Its exports and batches are jobs people follow (`helios_core::jobs`,
//! `job.*` on its bus): `job.cancel` with a job's id cancels it, a batch
//! with its exports.
//!
//! A batch submitted with `workflow.submit` is a run of the workflow level
//! (`helios_node::workflow`): one journal row per item and queueing node,
//! worked with leases and retries, kept in the vault with `--vault` and in
//! memory without; `describe workflow` shows the runs per queueing node,
//! and `workflow.*` events announce their rows. With a vault, a node
//! resumes at its start the runs a dead node left (itself before a crash),
//! and removes finished runs after `system.workflow.keep_days`.
//! `--runs-vault FILE` keeps the runs in that vault instead, and nothing
//! else: no settings layer and no edit (what `helios batch --vault` gives
//! the node it starts).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use helios_config::{Config, Layer, Schema};
use helios_core::jobs::{Jobs, Report as JobReport, Start};
use helios_core::{
    Bus, Context, Describe, Engine, Event, Graph, JobId, JobState, Message, NodeId, Region,
    Scheduler,
};
use helios_decision::Book;
use helios_inspect::control::ControlRequest;
use helios_inspect::{Inspector, Listen, new_token, serve_inspector, write_token};
use helios_node::ExportDesc;
use helios_node::vault_node::{Options as VaultNodeOptions, VaultNode};
use helios_node::workflow::runs::{Context as RunsContext, SharedVault, Workflows};
use helios_resolve::{Report, Resolver, Settings};
use serde::Serialize;
use serde_json::{Value, json};

const SOURCE: &str = "node";

/// The node is serving and about to render.
#[derive(Serialize)]
struct Ready {
    socket: String,
    renders: usize,
    exports: usize,
}

impl Event for Ready {
    const KIND: &'static str = "node.ready";
}

/// A render is finished.
#[derive(Serialize)]
struct RenderDone {
    node: String,
    port: String,
    region: Region,
    hash: Option<String>,
    error: Option<String>,
}

impl Event for RenderDone {
    const KIND: &'static str = "node.render_done";
}

/// Every render and export asked for by the description is finished.
#[derive(Serialize)]
struct Idle {
    renders: usize,
    exports: usize,
    failed: usize,
}

impl Event for Idle {
    const KIND: &'static str = "node.idle";
}

/// The modules this node knows, with their manifests, and the composite
/// files Helios ships (a workshop's block included): what a surface or
/// an agent needs to build and edit a network.
struct Modules;

impl Describe for Modules {
    fn describe(&self) -> Value {
        let modules: serde_json::Map<String, Value> = helios_node::registry()
            .into_iter()
            .map(|(name, module)| {
                (
                    name.to_string(),
                    serde_json::to_value(module.manifest()).expect("manifests serialize"),
                )
            })
            .collect();
        let mut catalogue = helios_node::mcp::workspace_catalogue().describe();
        let composites = catalogue["composites"].take();
        json!({ "modules": modules, "composites": composites })
    }
}

/// The graph of the node, shared by renders, inspection and control.
struct SharedGraph(RwLock<Graph>);

impl Describe for SharedGraph {
    fn describe(&self) -> Value {
        self.0.read().expect("graph lock").describe()
    }
}

/// The vault `--vault` names.
type Vault = helios_vault::Vault<helios_vault_sqlite::SqliteStore>;

/// The edit of the image a node with a vault works on, kept as versions.
struct NodeEdit {
    vault: SharedVault,
    session: Mutex<helios_vault::EditSession>,
    /// The head's `marks` (the desktop's labels and locks), which a node
    /// does not change: carried over to each version it saves.
    marks: Value,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What a version keeps of a node's edit: its modules' parameters, and its
/// decisions when it has any.
fn edit_of(graph: &Graph, book: &Book) -> Value {
    let mut edit = helios_vault::edit::of_graph(graph);
    let decisions = book.to_json();
    if decisions["nodes"]
        .as_object()
        .is_some_and(|n| !n.is_empty())
    {
        edit["decisions"] = decisions;
    }
    edit
}

/// Binds a node to the vault image of its first source's file: the edit
/// saved there is loaded into `graph` and `book`, and changes are saved
/// from now on.
fn open_edit(
    vault: &SharedVault,
    file: &Path,
    bus: &Arc<Bus>,
    graph: &mut Graph,
    book: &mut Book,
    declared: &BTreeMap<String, helios_decision::Mode>,
) -> Result<NodeEdit, String> {
    let image = vault.import_file(file, unix_now())?;
    let (session, head) = helios_vault::EditSession::open(vault, image)?;
    let marks = head
        .as_ref()
        .map_or(Value::Null, |h| h.edit["marks"].clone());
    if let Some(head) = head {
        for skipped in helios_vault::edit::apply_to_graph(graph, &head.edit)? {
            eprintln!("helios-node: the saved edit does not fit the network: {skipped}");
        }
        if !head.edit["decisions"].is_null() {
            let mut kept = Book::from_json(head.edit["decisions"].clone())?.with_bus(bus.clone());
            // A decision node the description adds since takes its mode.
            for (name, mode) in declared {
                if kept.mode(name).is_err() {
                    kept.declare(name, *mode);
                }
            }
            kept.replay(graph)?;
            *book = kept;
        }
        eprintln!(
            "helios-node: the edit of {} continues version {}",
            file.display(),
            head.id.hex()
        );
    }
    Ok(NodeEdit {
        vault: vault.clone(),
        session: Mutex::new(session),
        marks,
    })
}

/// Everything a node holds.
struct Node {
    bus: Arc<Bus>,
    engine: Arc<Engine>,
    scheduler: Arc<Scheduler>,
    graph: Arc<SharedGraph>,
    names: BTreeMap<String, NodeId>,
    /// Renders asked for, by description or control: their node headers are reported.
    targets: Mutex<Vec<(String, NodeId, Region)>>,
    renders: Mutex<Vec<Value>>,
    /// Export jobs, by destination.
    exports: Mutex<Vec<(PathBuf, JobId)>>,
    idle: Mutex<bool>,
    /// The decisions of the network, and the model that answers them.
    book: Mutex<Book>,
    resolver: Option<Arc<dyn Resolver>>,
    brief: Option<String>,
    /// The first source: decisions look at its region.
    frame: Option<NodeId>,
    /// What each source loaded from a file gives the modules reading it.
    contexts: Mutex<BTreeMap<NodeId, Context>>,
    /// The vault keeping the edit, when the node has one.
    edit: Option<NodeEdit>,
    /// Where its exports and batches report (`job.*` on its bus), and
    /// what `job.cancel` cancels.
    jobs: Jobs,
    /// The batch jobs running, by id.
    batches: Mutex<BTreeMap<String, Batch>>,
    /// The runs of the workflow level.
    workflows: Arc<Workflows>,
    /// Every decision node with its mode: what a run derives its workflow from.
    decisions: BTreeMap<String, helios_decision::Mode>,
    /// The network's first export: what a run exports unless told.
    first_export: Option<String>,
    /// Imports into the vault the node owns (`--vault`).
    importer: Option<helios_node::vault_node::Importer>,
    /// The cursors its clients hold on the vault.
    cursors: helios_node::cursor::Cursors,
    /// The settings layers the node keeps in that vault (`user`,
    /// `project/<folder>`).
    settings: Option<helios_node_core::settings::NodeSettings<helios_vault_sqlite::SqliteStore>>,
}

/// A batch job: the parent of the exports a batch asks for.
struct Batch {
    report: JobReport,
    total: u64,
    /// Cancelled: its exports are cancelled, new ones refused.
    stopped: Arc<AtomicBool>,
    /// Its exports' scheduler jobs; locked while one is added, so a cancel
    /// never misses it.
    children: Arc<Mutex<Vec<JobId>>>,
    /// When an export was last asked for it, or one of its exports seen
    /// running.
    touched: Mutex<Instant>,
    /// How long it may go untouched before it is taken as orphaned.
    idle_timeout: Duration,
}

/// How long a batch may go without an export asked for it, and none of
/// its exports running, before it ends as orphaned.
const BATCH_IDLE_TIMEOUT: Duration = Duration::from_mins(10);

/// The batches' job ids in this node: `batch-1`, `batch-2`...
static NEXT_BATCH: AtomicU64 = AtomicU64::new(1);

impl Node {
    /// Saves the edit as it is now as a version continuing the last one,
    /// made by `origin`; its id, or `None` when the node keeps no edit or
    /// nothing changed. The session is locked first, so two changes
    /// answered at once are saved in the order they were made.
    fn save_edit(&self, label: &str, origin: &str) -> Result<Option<String>, String> {
        let Some(edit) = &self.edit else {
            return Ok(None);
        };
        let mut session = edit.session.lock().expect("edit lock");
        let mut value = {
            let book = self.book.lock().expect("book lock");
            let graph = self.graph.0.read().expect("graph lock");
            edit_of(&graph, &book)
        };
        if !edit.marks.is_null() {
            value["marks"] = edit.marks.clone();
        }
        session
            .save(&edit.vault, value, label, origin, unix_now())
            .map(|saved| saved.map(|v| v.id.hex()))
    }
}

impl Node {
    fn id(&self, name: &str) -> Result<NodeId, Message> {
        self.names.get(name).copied().ok_or_else(|| {
            Message::new("node-no-node", "no node named `{ $name }`").with("name", name)
        })
    }

    /// Renders a node's port over a region as a scheduler job.
    fn submit_render(
        self: &Arc<Self>,
        name: &str,
        port: &str,
        region: Region,
        cause: Option<u64>,
    ) -> Result<JobId, String> {
        let id = self.id(name)?;
        {
            let mut targets = self.targets.lock().expect("targets lock");
            if !targets.iter().any(|(n, _, r)| n == name && *r == region) {
                targets.push((name.to_string(), id, region));
            }
        }
        let (node, name, port) = (self.clone(), name.to_string(), port.to_string());
        Ok(self.scheduler.submit(&format!("render {name}"), move |_| {
            let result = {
                let graph = node.graph.0.read().expect("graph lock");
                node.engine.render(&graph, id, &port, region)
            };
            let (hash, error) = match &result {
                Ok(image) => (Some(image.content_hash().to_string()), None),
                Err(e) => (None, Some(e.to_string())),
            };
            let done = RenderDone {
                node: name,
                port,
                region,
                hash,
                error: error.clone(),
            };
            node.renders
                .lock()
                .expect("renders lock")
                .push(serde_json::to_value(&done).expect("events serialize"));
            node.bus.publish(SOURCE, cause, &done);
            error.map_or(Ok(()), Err)
        }))
    }
}

impl Node {
    /// Exports a node's port as a scheduler job holding the destination.
    fn submit_export(self: &Arc<Self>, export: &ExportDesc) -> Result<JobId, String> {
        let id = self.id(&export.node)?;
        let (node, port, region) = (self.clone(), export.port.clone(), export.region);
        let job = helios_export::submit_reported(
            &self.scheduler,
            &self.jobs,
            Some(&export.node),
            &export.path,
            export.format()?,
            move || {
                let graph = node.graph.0.read().expect("graph lock");
                node.engine
                    .render(&graph, id, &port, region)
                    .map_err(|e| e.to_string())
            },
        )?;
        self.exports
            .lock()
            .expect("exports lock")
            .push((export.path.clone(), job));
        Ok(job)
    }
}

/// What only the control endpoints ask for.
impl Node {
    /// Exports a node's port as it is now: the job renders a copy of the
    /// graph taken here, with `params` changed on that copy only, so later
    /// changes (another source, another parameter) never reach it.
    #[allow(
        clippy::too_many_arguments,
        reason = "the fields of the control request, one by one"
    )]
    fn submit_export_now(
        self: &Arc<Self>,
        name: &str,
        port: &str,
        region: Option<Region>,
        path: &Path,
        format: helios_export::Format,
        params: &BTreeMap<String, helios_core::Params>,
        batch: Option<&str>,
        as_mask: bool,
    ) -> Result<(JobId, Region), Message> {
        absolute(path)?;
        let id = self.id(name)?;
        let mut graph = self.graph.0.read().expect("graph lock").clone();
        for (node, values) in params {
            let target = self.id(node)?;
            for (param, value) in values.iter() {
                graph.set_param(target, param, value.clone()).map_err(|e| {
                    Message::new(
                        "node-param-refused",
                        "cannot set { $node }.{ $param }: { $error }",
                    )
                    .with("node", node)
                    .with("param", param)
                    .with("error", e.to_string())
                })?;
            }
        }
        let region = match region {
            Some(r) => r,
            None => graph
                .extent(id)
                .map_err(|e| Message::plain(e.to_string()))?
                .ok_or_else(|| {
                    Message::new("node-no-extent", "`{ $name }` has no extent: give a region")
                        .with("name", name)
                })?,
        };
        let (engine, port) = (self.engine.clone(), port.to_string());
        let render = move || {
            let image = engine
                .render(&graph, id, &port, region)
                .map_err(|e| e.to_string())?;
            // A mask written as its values (the mask display's grey).
            if as_mask {
                helios_export::mask_as_grey(&image).map_err(|m| m.english)
            } else {
                Ok(image)
            }
        };
        let job = match batch {
            None => helios_export::submit_reported(
                &self.scheduler,
                &self.jobs,
                Some(name),
                path,
                format,
                render,
            )?,
            Some(batch) => {
                let (stopped, children) = {
                    let batches = self.batches.lock().expect("batches lock");
                    let b = batches
                        .get(batch)
                        .ok_or_else(|| Message::plain(format!("no running batch `{batch}`")))?;
                    *b.touched.lock().expect("touched lock") = Instant::now();
                    (b.stopped.clone(), b.children.clone())
                };
                // Held until the export is listed: a cancel of the batch
                // waits for it, then cancels it too.
                let mut children = children.lock().expect("children lock");
                if stopped.load(Ordering::SeqCst) {
                    return Err(Message::new("batch-cancelled", "cancelled"));
                }
                let job = helios_export::submit_child(
                    &self.scheduler,
                    &self.jobs,
                    batch,
                    Some(name),
                    path,
                    format,
                    render,
                )?;
                children.push(job);
                job
            }
        };
        self.exports
            .lock()
            .expect("exports lock")
            .push((path.to_path_buf(), job));
        Ok((job, region))
    }

    /// Starts a batch job: its id. Cancelled (`job.cancel`), it cancels
    /// its exports, queued or running, and refuses new ones.
    fn batch_start(&self, title: &str, total: u64, idle_timeout: Duration) -> String {
        let id = format!("batch-{}", NEXT_BATCH.fetch_add(1, Ordering::Relaxed));
        let report = self
            .jobs
            .start(Start::new(&id, title, "batch").total(total));
        let stopped = Arc::new(AtomicBool::new(false));
        let children: Arc<Mutex<Vec<JobId>>> = Arc::default();
        {
            let (stopped, children) = (stopped.clone(), children.clone());
            let canceller = self.scheduler.canceller();
            report.on_cancel(move || {
                stopped.store(true, Ordering::SeqCst);
                for job in children.lock().expect("children lock").iter() {
                    canceller.cancel(*job);
                }
            });
        }
        self.batches.lock().expect("batches lock").insert(
            id.clone(),
            Batch {
                report,
                total,
                stopped,
                children,
                touched: Mutex::new(Instant::now()),
                idle_timeout,
            },
        );
        id
    }

    /// Ends a batch job: cancelled when it was (here or by `job.cancel`),
    /// failed when some of its images failed, else finished. Whether it
    /// was cancelled.
    fn batch_end(&self, id: &str, failed: u64, cancelled: bool) -> Result<bool, Message> {
        let batch = self
            .batches
            .lock()
            .expect("batches lock")
            .remove(id)
            .ok_or_else(|| Message::plain(format!("no running batch `{id}`")))?;
        let report = &batch.report;
        let cancelled = cancelled || batch.stopped.load(Ordering::SeqCst);
        if cancelled {
            report.cancelled();
        } else if failed > 0 {
            report.failed(&format!("{failed} of {} images failed", batch.total));
        } else {
            report.finished();
        }
        Ok(cancelled)
    }

    /// Ends a batch its client left without ending it: failed, saying
    /// why; the exports already asked for go on, new ones are refused.
    /// Nothing when it already ended.
    fn abandon_batch(&self, id: &str, why: &str) {
        let Some(batch) = self.batches.lock().expect("batches lock").remove(id) else {
            return;
        };
        batch.stopped.store(true, Ordering::SeqCst);
        batch.report.failed(&format!("abandoned: {why}"));
    }

    /// Ends the batches untouched for longer than their idle timeout at
    /// `now`: no export asked for, none running.
    fn end_idle_batches(&self, now: Instant) {
        let idle: Vec<(String, Duration)> = {
            let batches = self.batches.lock().expect("batches lock");
            batches
                .iter()
                .filter_map(|(id, b)| {
                    let busy = b.children.lock().expect("children lock").iter().any(|j| {
                        matches!(
                            self.scheduler.job(*j),
                            Some(JobState::Queued | JobState::Running)
                        )
                    });
                    let mut touched = b.touched.lock().expect("touched lock");
                    if busy {
                        *touched = now;
                    }
                    (now.saturating_duration_since(*touched) >= b.idle_timeout)
                        .then(|| (id.clone(), b.idle_timeout))
                })
                .collect()
        };
        for (id, timeout) in idle {
            self.abandon_batch(
                &id,
                &format!(
                    "nothing asked for it in {} s, its client is gone",
                    timeout.as_secs()
                ),
            );
        }
    }

    /// Gives a source the image of a file: read and decoded with no lock
    /// held, then swapped in under the graph's write lock.
    fn load_source(&self, name: &str, path: &Path) -> Result<helios_node::SourceLoaded, Message> {
        absolute(path)?;
        let id = self.id(name)?;
        // The loader's English names the file and says what is wrong.
        let loaded = helios_load::load(path).map_err(|e| {
            Message::new("node-load-refused", "cannot load the image: { $error }").with("error", e)
        })?;
        let mut graph = self.graph.0.write().expect("graph lock");
        let mut contexts = self.contexts.lock().expect("contexts lock");
        helios_node::load_source(&mut graph, &mut contexts, id, loaded).map_err(|e| {
            Message::new(
                "node-source-refused",
                "`{ $name }` cannot take the image: { $error }",
            )
            .with("name", name)
            .with("error", e)
        })
    }
}

/// A path the node is given must be absolute: it does not share the
/// caller's directory.
fn absolute(path: &Path) -> Result<(), Message> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(Message::new(
            "node-path-not-absolute",
            "{ $path }: give an absolute path, the node does not know where you are",
        )
        .with("path", path))
    }
}

impl Node {
    /// Asks the resolver about every decision waiting on a model: inputs
    /// read under the graph's read lock, models called with no lock held,
    /// answers recorded under the write lock.
    fn resolve_decisions(&self) -> Result<Report, String> {
        let resolver = self
            .resolver
            .clone()
            .ok_or("the network names no `resolver`")?;
        let region = {
            let graph = self.graph.0.read().expect("graph lock");
            self.frame.and_then(|id| graph.extent(id).ok().flatten())
        }
        .ok_or("the network has no source to decide on")?;
        let settings = Settings {
            region,
            scale: proxy_scale(region),
            brief: self.brief.clone(),
            timeout: std::time::Duration::from_mins(10),
            now: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        };
        let (requests, failed) = {
            let mut book = self.book.lock().expect("book lock");
            let graph = self.graph.0.read().expect("graph lock");
            book.follow(&graph);
            helios_resolve::ask(&book, &graph, &self.engine, &settings)
        };
        let called = helios_resolve::call(&self.scheduler, resolver, requests, settings.timeout)?;
        let mut report = Report {
            failed,
            ..Report::default()
        };
        let mut book = self.book.lock().expect("book lock");
        let mut graph = self.graph.0.write().expect("graph lock");
        helios_resolve::record(&mut book, &mut graph, called, settings.now, &mut report);
        Ok(report)
    }

    fn export_states(&self) -> Vec<Value> {
        self.exports
            .lock()
            .expect("exports lock")
            .iter()
            .map(|(path, job)| json!({ "path": path, "job": job, "state": self.scheduler.job(*job) }))
            .collect()
    }
}

/// The scale a model sees an image at: its long side near 1024 pixels.
fn proxy_scale(region: Region) -> f64 {
    let long = f64::from(region.width.max(region.height)).max(1.0);
    (1024.0 / long).min(1.0)
}

/// What the node reports about itself, with the header of every node
/// involved in each render it was asked for.
struct Status(Arc<Node>);

impl Describe for Status {
    fn describe(&self) -> Value {
        let node = &self.0;
        let targets = node.targets.lock().expect("targets lock").clone();
        let graph = node.graph.0.read().expect("graph lock");
        let headers: Vec<Value> = targets
            .iter()
            .map(
                |(name, id, region)| match node.engine.status(&graph, *id, *region) {
                    Ok(nodes) => json!({ "target": name, "region": region, "nodes": nodes }),
                    Err(e) => json!({ "target": name, "region": region, "error": e.to_string() }),
                },
            )
            .collect();
        json!({
            "idle": *node.idle.lock().expect("idle lock"),
            "renders": *node.renders.lock().expect("renders lock"),
            "exports": node.export_states(),
            "headers": headers,
        })
    }
}

mod control {
    //! The control endpoints: every action goes to the owner of what it
    //! changes, and is announced on the bus.
    //!
    //! Two scopes share this code. The debug endpoint (`--control`, debug
    //! builds with `HELIOS_DEBUG_CONTROL` set) answers every verb. The batch
    //! endpoint (`--batch-control`, every build) answers only what a batch
    //! export needs: give a source a file, export, cancel a job, and the
    //! batch's own job.
    //!
    //! Both also answer `commands.list` and `command.run`: every verb as a
    //! command (`helios_node::commands`), listed with whether it can run on
    //! this endpoint and node and why not, and run by id with its params
    //! checked against its schema.
    //!
    //! Both speak JSON-RPC 2.0 too ([`Rpc`]): the method is a command's id,
    //! the params its schema's object, run as `command.run` would.

    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use helios_core::{Event, JobId, Message};
    use helios_export::Format;
    use helios_inspect::control::{ControlRequest, FileFormat};
    use helios_inspect::rpc;
    use helios_node::commands;
    use helios_node::workflow::runs::Submit;
    use helios_node_core::commands::{CommandSpec, Need, check_origin, validate};
    use serde::Serialize;
    use serde_json::{Value, json};

    use super::{Node, SOURCE};

    /// A control action was received.
    #[derive(Serialize)]
    struct Action<'a> {
        #[serde(flatten)]
        request: &'a ControlRequest,
    }

    impl Event for Action<'_> {
        const KIND: &'static str = "control.action";
    }

    /// A command is run, by whom: the `control.action` of its request
    /// follows.
    use helios_node_core::endpoint::CommandRan;

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }

    /// Which verbs an endpoint answers.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Scope {
        /// Every verb: the debug control endpoint.
        #[cfg_attr(
            not(debug_assertions),
            allow(
                dead_code,
                reason = "the debug endpoint is not served in release builds"
            )
        )]
        Debug,
        /// `load-source`, `export`, `cancel`, `job.cancel`, `batch_start`,
        /// `batch_end`, the runs' `workflow.*` requests, `library.import`,
        /// `settings.layer.*`, `commands.list` and `command.run`: the batch
        /// endpoint.
        Batch,
    }

    /// The verbs the batch endpoint answers.
    const BATCH_VERBS: &[&str] = &[
        "load_source",
        "export",
        "cancel",
        "job.cancel",
        "batch_start",
        "batch_end",
        "workflow.submit",
        "workflow.runs",
        "workflow.items",
        "workflow.forget",
        "workflow.cancel",
        "workflow.retry",
        "workflow.prioritize",
        "workflow.pause",
        "workflow.resume",
        "library.import",
        "settings.layer.get",
        "settings.layer.set",
        "commands.list",
        "command.run",
    ];

    impl Scope {
        fn allows(self, verb: &str) -> bool {
            self == Self::Debug
                || BATCH_VERBS.contains(&verb)
                || helios_node::library::handles(verb)
                || helios_node::calibrate::handles(verb)
        }
    }

    /// Why a command cannot run on this endpoint and node, if it cannot.
    fn disabled(node: &Node, scope: Scope, spec: &CommandSpec) -> Option<Message> {
        if !scope.allows(&spec.id) {
            return Some(Message::new(
                "node-command-debug-only",
                "only the debug control endpoint runs it (helios-node --control, debug builds)",
            ));
        }
        if spec.needs.contains(&Need::Vault) && node.importer.is_none() {
            return Some(Message::new(
                "node-command-needs-vault",
                "the node keeps no vault: start it with --vault",
            ));
        }
        if spec.needs.contains(&Need::Window) {
            return Some(Message::new(
                "node-command-needs-window",
                "no desktop window holds this workspace",
            ));
        }
        None
    }

    /// `commands.list`: each command with whether it can run here.
    fn list(node: &Node, scope: Scope, filter: Option<&str>, enabled_only: bool) -> Value {
        let commands: Vec<Value> = commands::registry()
            .specs()
            .filter(|spec| filter.is_none_or(|f| spec.matches(f)))
            .filter_map(|spec| {
                let reason = disabled(node, scope, spec);
                if enabled_only && reason.is_some() {
                    return None;
                }
                let mut entry = json!(spec);
                entry["enabled"] = json!(reason.is_none());
                entry["scope"] = json!("node");
                if let Some(reason) = reason {
                    entry["reason"] = json!(reason.english);
                    entry["disabledReason"] = json!(reason.english);
                    entry["message"] = json!(reason);
                }
                Some(entry)
            })
            .collect();
        json!({ "ok": true, "commands": commands })
    }

    /// `command.run`: the command's params checked against its schema, then
    /// the request it names answered as the endpoint would, its change to
    /// the edit saved under `origin`.
    fn run(
        node: &Arc<Node>,
        scope: Scope,
        id: &str,
        params: &Value,
        origin: &str,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Value {
        let Some(spec) = commands::registry().spec(id) else {
            return refused(
                None,
                &Message::new("node-command-unknown", "no command `{ $id }`").with("id", id),
            );
        };
        if check_origin(origin).is_err() {
            return refused(
                None,
                &Message::new(
                    "node-command-origin",
                    "`{ $origin }` is not an origin: user, cli, control, agent:NAME or script:NAME",
                )
                .with("origin", origin),
            );
        }
        if let Err(error) = validate(&spec.params, params) {
            return refused(
                None,
                &Message::new(
                    "node-command-params",
                    "`{ $id }` refused its params: { $error }",
                )
                .with("id", id)
                .with("error", error),
            );
        }
        if let Some(reason) = disabled(node, scope, spec) {
            return refused(None, &reason);
        }
        if helios_node::cursor::handles(id) {
            return refused(
                None,
                &Message::plain(format!("`{id}` is served over JSON-RPC only, as a method")),
            );
        }
        if helios_node::calibrate::PROFILE_IDS.contains(&id) {
            node.bus.publish(SOURCE, None, &CommandRan { id, origin });
            let vault = node.importer.as_ref().map(|i| i.vault().as_ref());
            let mut answer = match helios_node::calibrate::run_as(vault, id, params, origin, now())
            {
                Ok(done) => {
                    let mut answer = done.answer;
                    answer["ok"] = json!(true);
                    answer
                }
                Err(e) => refused(None, &e),
            };
            answer["command"] = json!(id);
            answer["origin"] = json!(origin);
            return answer;
        }
        if id == helios_node::library::AGAIN {
            // The import of the history run again: `library.import` with
            // its record's params, as whoever asks now.
            let Some(importer) = &node.importer else {
                return refused(
                    None,
                    &Message::new(
                        "node-command-needs-vault",
                        "the node keeps no vault: start it with --vault",
                    ),
                );
            };
            let mut answer = match helios_node::library::again_params(importer.vault(), params) {
                Ok(again) => run(
                    node,
                    scope,
                    helios_node::library::IMPORT,
                    &again,
                    origin,
                    cancel,
                ),
                Err(e) => refused(None, &Message::plain(e)),
            };
            answer["command"] = json!(id);
            return answer;
        }
        if helios_node::library::handles(id) {
            node.bus.publish(SOURCE, None, &CommandRan { id, origin });
            let Some(importer) = &node.importer else {
                return refused(
                    None,
                    &Message::new(
                        "node-command-needs-vault",
                        "the node keeps no vault: start it with --vault",
                    ),
                );
            };
            let mut answer = match helios_node::library::run(
                importer.vault(),
                importer.bus(),
                id,
                params,
                origin,
            ) {
                Ok(mut answer) => {
                    answer["ok"] = json!(true);
                    answer
                }
                Err(e) => refused(None, &Message::plain(e)),
            };
            answer["command"] = json!(id);
            answer["origin"] = json!(origin);
            return answer;
        }
        let mut params = params.clone();
        // A decision taken through a command is taken by whoever ran it.
        if spec.params["properties"].get("who").is_some()
            && let Some(fields) = params.as_object_mut()
        {
            fields.entry("who").or_insert_with(|| json!(origin));
        }
        let request = match serde_json::from_value::<ControlRequest>(commands::request(id, &params))
        {
            Ok(request) => request,
            Err(error) => {
                return refused(
                    None,
                    &Message::new(
                        "node-command-params",
                        "`{ $id }` refused its params: { $error }",
                    )
                    .with("id", id)
                    .with("error", error.to_string()),
                );
            }
        };
        node.bus.publish(SOURCE, None, &CommandRan { id, origin });
        let mut answer = answer_as(node, scope, request, origin, cancel);
        answer["command"] = json!(id);
        answer["origin"] = json!(origin);
        answer
    }

    /// An import's options, from `library.import`'s params.
    fn import_options(
        copy: bool,
        into: Option<&std::path::PathBuf>,
        rename: Option<&str>,
        layout: Option<&str>,
        duplicates: Option<&str>,
        signals: Option<bool>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<helios_library::import::Options, String> {
        use helios_library::import::{Duplicates, Layout, Options, Placement};
        let mut options = Options::default();
        if copy || into.is_some() {
            options.placement = Placement::Copy {
                into: into.cloned(),
            };
        }
        if let Some(pattern) = rename {
            options.rename = Some(helios_library::rename::Pattern::parse(pattern)?);
        }
        if let Some(layout) = layout {
            options.layout = Layout::parse(layout)?;
        }
        if let Some(duplicates) = duplicates {
            options.duplicates = Duplicates::parse(duplicates)?;
        }
        if let Some(signals) = signals {
            options.signals = signals;
        }
        options.cancel = cancel;
        Ok(options)
    }

    /// A refusal: its English in `error`, and the message itself in
    /// `message` when it has a key, for a client to show translated.
    fn refused(seq: Option<u64>, error: &Message) -> Value {
        let mut answer = json!({ "ok": false, "error": error.english });
        if let Some(seq) = seq {
            answer["seq"] = json!(seq);
        }
        if error.is_keyed() {
            answer["message"] = json!(error);
        }
        answer
    }

    /// The refusal of a settings layer request by a node with no vault.
    fn no_settings() -> Message {
        Message::new(
            "node-no-settings",
            "the node keeps no settings: start it with --vault",
        )
    }

    /// The origin of an edit saved for a control client.
    const CONTROL: &str = "control";

    /// A control endpoint's JSON-RPC methods: `node.hello`,
    /// `commands.list`, and every command by its id, its params checked
    /// against its schema and run under the origin `_meta.origin` names
    /// (`control` when not given).
    pub struct Rpc {
        pub node: Arc<Node>,
        pub scope: Scope,
        /// The node instance, as the inspect endpoint names it.
        pub instance: String,
    }

    impl Rpc {
        /// A cursor command (`helios_node::cursor`), on the node's vault.
        fn cursor(&self, id: &str, params: &Value, call: &rpc::Call) -> Result<Value, rpc::Error> {
            let mut params = params.clone();
            if let Some(fields) = params.as_object_mut() {
                fields.remove("_meta");
            }
            let spec = commands::registry()
                .spec(id)
                .ok_or_else(|| rpc::Error::method_not_found(id))?;
            validate(&spec.params, &params).map_err(|e| {
                rpc::Error::invalid_params(format!("`{id}` refused its params: {e}"))
            })?;
            let Some(importer) = &self.node.importer else {
                return Err(rpc::Error::new(
                    rpc::code::NEEDS_VAULT,
                    "the node keeps no vault: start it with --vault",
                ));
            };
            let mut answer = self.node.cursors.run(
                importer.vault(),
                id,
                &params,
                call,
                std::time::Instant::now(),
            )?;
            answer["command"] = json!(id);
            answer["seq"] = json!(self.node.bus.last_seq());
            Ok(answer)
        }
    }

    /// An answer stamped with the bus's sequence once it ran, unless it
    /// carries one: a client's cache shows a write once it saw it.
    fn with_seq(node: &Node, mut answer: Value) -> Value {
        if answer.is_object() && answer.get("seq").is_none() {
            answer["seq"] = json!(node.bus.last_seq());
        }
        answer
    }

    impl rpc::Service for Rpc {
        fn call(&self, method: &str, params: Value, call: &rpc::Call) -> Result<Value, rpc::Error> {
            let mut params = params;
            let meta = params.as_object_mut().and_then(|p| p.remove("_meta"));
            let origin = meta
                .as_ref()
                .and_then(|m| m["origin"].as_str())
                .unwrap_or(CONTROL);
            match method {
                "node.hello" => Ok(match self.scope {
                    Scope::Debug => rpc::hello(&self.instance, "debug", &["batch", "debug"]),
                    Scope::Batch => rpc::hello(&self.instance, "batch", &["batch"]),
                }),
                "commands.list" => {
                    let filter = params["filter"].as_str();
                    let enabled_only = params["enabled_only"].as_bool().unwrap_or(false);
                    rpc::from_answer(list(&self.node, self.scope, filter, enabled_only))
                }
                // The same run by its id, as the vault endpoints take it:
                // `{id, params, origin}`, the origin else the call's.
                "command.run" => {
                    let id = params["id"]
                        .as_str()
                        .ok_or_else(|| rpc::Error::invalid_params("`command.run` takes an `id`"))?;
                    let origin = params["origin"].as_str().unwrap_or(origin);
                    let args = match &params["params"] {
                        Value::Null => json!({}),
                        v => v.clone(),
                    };
                    if helios_node::cursor::handles(id) {
                        return self.cursor(id, &args, call);
                    }
                    rpc::from_answer(with_seq(
                        &self.node,
                        run(
                            &self.node,
                            self.scope,
                            id,
                            &args,
                            origin,
                            Some(call.cancel_flag()),
                        ),
                    ))
                }
                id if helios_node::cursor::handles(id) => self.cursor(id, &params, call),
                id => {
                    let answer = run(
                        &self.node,
                        self.scope,
                        id,
                        &params,
                        origin,
                        Some(call.cancel_flag()),
                    );
                    // A batch held by its client ends when the connection does.
                    if id == "batch_start"
                        && params["hold"] == true
                        && let Some(batch) = answer["id"].as_str()
                    {
                        let (node, batch) = (self.node.clone(), batch.to_string());
                        call.on_close(move || node.abandon_batch(&batch, "its client went away"));
                    }
                    rpc::from_answer(with_seq(&self.node, answer))
                }
            }
        }

        /// An import stops before its next file, a cursor window between
        /// chunks of rows.
        fn cancellable(&self, method: &str) -> bool {
            method == "library.import" || method == "cursor.window"
        }
    }

    pub fn answer(node: &Arc<Node>, scope: Scope, request: ControlRequest) -> Value {
        answer_as(node, scope, request, CONTROL, None)
    }

    /// Answers a request; a change to the edit is saved under `origin`
    /// unless the request names who made it.
    fn answer_as(
        node: &Arc<Node>,
        scope: Scope,
        request: ControlRequest,
        origin: &str,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Value {
        if !scope.allows(request.verb()) {
            return refused(
                None,
                &Message::new(
                    "node-batch-only",
                    "the batch endpoint only answers load-source, export, cancel, job.cancel, batch_start, batch_end, the workflow.* requests, library.import, settings.layer.*, commands.list and command.run",
                ),
            );
        }
        match &request {
            ControlRequest::CommandsList {
                filter,
                enabled_only,
            } => return list(node, scope, filter.as_deref(), *enabled_only),
            ControlRequest::CommandRun {
                id,
                params,
                origin: by,
            } => return run(node, scope, id, params, by, cancel),
            _ => {}
        }
        let seq = node
            .bus
            .publish(SOURCE, None, &Action { request: &request });
        let done = |extra: Value| {
            let mut answer = json!({ "ok": true, "seq": seq });
            if let (Some(a), Some(e)) = (answer.as_object_mut(), extra.as_object()) {
                a.extend(e.clone());
            }
            answer
        };
        let result: Result<Value, Message> = match &request {
            ControlRequest::CommandsList { .. } | ControlRequest::CommandRun { .. } => {
                Err(Message::plain("a command request is answered above"))
            }
            ControlRequest::Render {
                node: name,
                port,
                region,
            } => node
                .submit_render(name, port, *region, Some(seq))
                .map(|job| json!({ "job": job.0 }))
                .map_err(Message::plain),
            ControlRequest::SetParam {
                node: name,
                param,
                value,
            } => node.id(name).and_then(|id| {
                let mut graph = node.graph.0.write().expect("graph lock");
                graph
                    .set_param(id, param, value.clone())
                    .map(|()| json!({}))
                    .map_err(|e| Message::plain(e.to_string()))
            }),
            ControlRequest::NodeBypass { node: name, on } => node.id(name).and_then(|id| {
                let mut graph = node.graph.0.write().expect("graph lock");
                graph
                    .set_bypass(id, *on)
                    .map(|()| json!({ "node": name, "bypassed": on }))
                    .map_err(|e| Message::plain(e.to_string()))
            }),
            ControlRequest::LoadSource { node: name, path } => node
                .load_source(name, path)
                .map(|loaded| json!({ "source": loaded })),
            ControlRequest::Export {
                node: name,
                port,
                region,
                path,
                format,
                quality,
                params,
                batch,
                as_mask,
            } => {
                let format = match format {
                    FileFormat::Jpeg => Format::Jpeg {
                        quality: quality.unwrap_or(92),
                    },
                    FileFormat::Png => Format::Png { bits: 8 },
                    FileFormat::Tiff => Format::Tiff,
                };
                node.submit_export_now(
                    name,
                    port,
                    *region,
                    path,
                    format,
                    params,
                    batch.as_deref(),
                    *as_mask,
                )
                .map(|(job, region)| json!({ "job": job.0, "region": region }))
            }
            ControlRequest::JobCancel { id } => node
                .jobs
                .cancel(id)
                .map(|()| json!({ "cancelling": id }))
                .map_err(Message::plain),
            ControlRequest::BatchStart {
                title,
                total,
                idle_timeout_s,
                ..
            } => Ok(json!({
                "id": node.batch_start(
                    title,
                    *total,
                    idle_timeout_s.map_or(super::BATCH_IDLE_TIMEOUT, Duration::from_secs),
                )
            })),
            ControlRequest::BatchEnd {
                id,
                failed,
                cancelled,
            } => node
                .batch_end(id, *failed, *cancelled)
                .map(|cancelled| json!({ "cancelled": cancelled })),
            ControlRequest::Cancel { job } => {
                if node.scheduler.cancel(JobId(*job)) {
                    Ok(json!({}))
                } else {
                    Err(Message::new(
                        "node-job-unknown",
                        "job { $job } is unknown or already ended",
                    )
                    .with("job", job.to_string()))
                }
            }
            ControlRequest::Pause => {
                node.scheduler.pause();
                Ok(json!({}))
            }
            ControlRequest::CacheStats => Ok(helios_cache::Service::global().snapshot()),
            ControlRequest::CalibrateChart { .. } => {
                let params = serde_json::to_value(&request).unwrap_or_default();
                let vault = node.importer.as_ref().map(|i| i.vault().as_ref());
                helios_node::calibrate::run(vault, helios_node::calibrate::CHART, &params, now())
                    .map(|done| done.answer)
            }
            ControlRequest::CacheClear { kind } => Ok(json!({
                "freed": helios_cache::Service::global().clear(kind.as_deref()),
            })),
            ControlRequest::Resume => {
                node.scheduler.resume();
                Ok(json!({}))
            }
            ControlRequest::Fault { node: name, kind } => node.id(name).map(|id| {
                let graph = node.graph.0.read().expect("graph lock");
                node.engine.inject_fault(&graph, id, *kind);
                // A run's export renders a copy of the network: it takes
                // the fault for its next render.
                node.workflows.inject_fault(name, *kind);
                json!({})
            }),
            ControlRequest::WorkflowSubmit {
                title,
                items,
                out,
                format,
                quality,
                name,
                node: export,
                source,
                again,
                concurrency,
            } => {
                let graph = node.graph.0.read().expect("graph lock");
                node.workflows.submit(
                    &graph,
                    &node.decisions,
                    node.first_export.as_deref(),
                    &Submit {
                        title: title.as_deref(),
                        items,
                        out,
                        format: *format,
                        quality: *quality,
                        name,
                        node: export.as_deref(),
                        source: source.as_deref(),
                        again: *again,
                        concurrency: *concurrency,
                    },
                )
            }
            ControlRequest::WorkflowRuns => Ok(node.workflows.runs()),
            ControlRequest::WorkflowItems {
                run,
                node: at,
                state,
                offset,
                limit,
            } => node
                .workflows
                .items(run, at.as_deref(), state.as_deref(), *offset, *limit),
            ControlRequest::WorkflowForget { run } => node.workflows.forget(run),
            ControlRequest::WorkflowCancel {
                run,
                items,
                node: at,
            } => node.workflows.cancel(run, items.as_deref(), at.as_deref()),
            ControlRequest::WorkflowRetry {
                run,
                items,
                node: at,
            } => node.workflows.retry(run, items.as_deref(), at.as_deref()),
            ControlRequest::WorkflowPrioritize { run, items, to } => {
                node.workflows.prioritize(run, items, *to)
            }
            ControlRequest::WorkflowPause { run } => node.workflows.pause(run),
            ControlRequest::WorkflowResume { run } => node.workflows.resume(run),
            ControlRequest::SettingsLayerGet { layer } => match &node.settings {
                None => Err(no_settings()),
                Some(settings) => settings
                    .read(layer)
                    .map(|text| json!({ "layer": layer, "text": text }))
                    .map_err(Message::plain),
            },
            ControlRequest::SettingsLayerSet { layer, text } => match &node.settings {
                None => Err(no_settings()),
                Some(settings) => settings
                    .write(layer, text)
                    .map(|()| json!({ "layer": layer }))
                    .map_err(Message::plain),
            },
            ControlRequest::LibraryImport {
                paths,
                copy,
                into,
                rename,
                layout,
                duplicates,
                signals,
            } => match &node.importer {
                None => Err(Message::new(
                    "node-no-vault",
                    "the node has no vault to import into: start it with --vault",
                )),
                Some(importer) => import_options(
                    *copy,
                    into.as_ref(),
                    rename.as_deref(),
                    layout.as_deref(),
                    duplicates.as_deref(),
                    *signals,
                    cancel,
                )
                .and_then(|mut options| {
                    options.origin = Some(origin.to_string());
                    let mut files = Vec::new();
                    importer
                        .import(paths, &options, &mut |line| files.push(line.clone()))
                        .map(|report| json!({ "report": report, "files": files }))
                })
                .map_err(Message::plain),
            },
            ControlRequest::Decisions => {
                let book = node.book.lock().expect("book lock");
                Ok(json!({ "decisions": helios_core::Describe::describe(&*book)["nodes"] }))
            }
            ControlRequest::DecisionResolve => node
                .resolve_decisions()
                .map(|report| json!({ "report": report }))
                .map_err(Message::plain),
            ControlRequest::DecisionAccept {
                node: name,
                option,
                who,
            } => {
                let mut book = node.book.lock().expect("book lock");
                let mut graph = node.graph.0.write().expect("graph lock");
                book.accept(&mut graph, name, who, *option, now())
                    .map(|version| json!({ "version": version }))
                    .map_err(Message::plain)
            }
            ControlRequest::DecisionDecide {
                node: name,
                values,
                who,
            } => {
                let mut book = node.book.lock().expect("book lock");
                let mut graph = node.graph.0.write().expect("graph lock");
                let inputs = match node.frame.and_then(|id| graph.extent(id).ok().flatten()) {
                    Some(region) => helios_decision::input_keys(&node.engine, &graph, name, region),
                    None => Ok(Vec::new()),
                };
                inputs
                    .and_then(|inputs| {
                        book.decide(&mut graph, name, who, values.clone(), inputs, now())
                            .map(|version| json!({ "version": version }))
                    })
                    .map_err(Message::plain)
            }
            ControlRequest::DecisionReask { node: name } => {
                let mut book = node.book.lock().expect("book lock");
                let mut graph = node.graph.0.write().expect("graph lock");
                book.reask(&mut graph, name)
                    .map(|()| json!({}))
                    .map_err(Message::plain)
            }
        };
        // A change to the edit is saved before it is answered, under who
        // made it: the request's `who` when it names one, else the origin
        // it came from (the control client, or a command run's). Its label
        // is the step's undo group, answered as `group`.
        let label = match &request {
            ControlRequest::SetParam {
                node: name, param, ..
            } => Some((format!("set {name}.{param}"), origin)),
            ControlRequest::NodeBypass { node: name, on } => Some((
                if *on {
                    format!("bypass {name}")
                } else {
                    format!("restore {name}")
                },
                origin,
            )),
            ControlRequest::DecisionResolve => Some(("decisions resolved".into(), origin)),
            ControlRequest::DecisionAccept {
                node: name, who, ..
            } => Some((format!("decision {name} accepted"), who.as_str())),
            ControlRequest::DecisionDecide {
                node: name, who, ..
            } => Some((format!("decision {name} decided"), who.as_str())),
            ControlRequest::DecisionReask { node: name } => {
                Some((format!("decision {name} asked again"), origin))
            }
            _ => None,
        };
        let result = match (result, label) {
            (Ok(mut extra), Some((label, origin))) => match node.save_edit(&label, origin) {
                Ok(saved) => {
                    if let Some(map) = extra.as_object_mut() {
                        if let Some(id) = saved {
                            map.insert("saved".into(), json!(id));
                        }
                        map.insert("group".into(), json!(label));
                    }
                    Ok(extra)
                }
                Err(e) => Err(Message::new(
                    "node-edit-not-saved",
                    "done, but the edit was not saved: { $error }",
                )
                .with("error", e)),
            },
            (result, _) => result,
        };
        match result {
            Ok(extra) => done(extra),
            Err(error) => refused(Some(seq), &error),
        }
    }
}

/// Answers a control endpoint's requests; a batch started with `hold`
/// is abandoned when the connection that asked for it closes before
/// the batch ends (its client died).
fn control_handler(
    node: Arc<Node>,
    scope: control::Scope,
) -> impl Fn(ControlRequest, &mut helios_inspect::Connection<'_>) -> Option<Value> + Send + Sync + 'static
{
    move |request: ControlRequest, connection: &mut helios_inspect::Connection<'_>| {
        let hold = matches!(request, ControlRequest::BatchStart { hold: true, .. });
        let answer = control::answer(&node, scope, request);
        if hold && let Some(id) = answer["id"].as_str() {
            let (node, id) = (node.clone(), id.to_string());
            connection.on_close(move || node.abandon_batch(&id, "its client went away"));
        }
        Some(answer)
    }
}

/// Where Unix sockets exist, the batch endpoint is one: never TCP.
const BATCH_TCP_REFUSED: &str = "--batch-control takes a Unix socket path, not a TCP address";

/// Serves the batch endpoint: `load-source`, `export` and `cancel` only,
/// in every build. A Unix socket readable and writable by this user alone,
/// or, where Unix sockets do not exist, loopback TCP behind the session
/// token: whoever reaches it can read and write the user's files through
/// the node, as the user could anyway.
fn serve_batch_control(
    node: &Arc<Node>,
    instance: &str,
    listen: &Listen,
    token: Option<&str>,
) -> Result<String, String> {
    if matches!(listen, Listen::Tcp(_)) && cfg!(unix) {
        return Err(BATCH_TCP_REFUSED.into());
    }
    let bound = helios_inspect::serve_rpc_at(
        listen,
        token,
        control_handler(node.clone(), control::Scope::Batch),
        Arc::new(control::Rpc {
            node: node.clone(),
            scope: control::Scope::Batch,
            instance: instance.to_string(),
        }),
    )
    .map_err(|e| format!("cannot serve the batch endpoint on {listen:?}: {e}"))?;
    #[cfg(unix)]
    if let Listen::Unix(path) = listen {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(bound.address)
}

/// Serves the control endpoint, if this build and environment allow it.
fn serve_control(
    node: &Arc<Node>,
    instance: &str,
    listen: &Listen,
    token: Option<&str>,
) -> Result<String, String> {
    #[cfg(debug_assertions)]
    {
        if std::env::var_os("HELIOS_DEBUG_CONTROL").is_none() {
            return Err(
                "the control endpoint is served only when HELIOS_DEBUG_CONTROL is set".into(),
            );
        }
        helios_inspect::serve_rpc_at(
            listen,
            token,
            control_handler(node.clone(), control::Scope::Debug),
            Arc::new(control::Rpc {
                node: node.clone(),
                scope: control::Scope::Debug,
                instance: instance.to_string(),
            }),
        )
        .map(|bound| bound.address)
        .map_err(|e| format!("cannot serve control on {listen:?}: {e}"))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (node, instance, listen, token);
        Err("the control endpoint exists only in debug builds".into())
    }
}

struct Args {
    /// The network; none: an empty one (a node serving a vault only).
    graph: Option<PathBuf>,
    socket: Listen,
    control: Option<Listen>,
    batch_control: Option<Listen>,
    token_file: Option<PathBuf>,
    layers: Vec<(Layer, PathBuf)>,
    sets: Vec<(String, String)>,
    vault: Option<PathBuf>,
    /// Where runs are kept, when not in `vault`.
    runs_vault: Option<PathBuf>,
    /// Exit once no client has been connected and no job has run for this
    /// long.
    idle_exit: Option<Duration>,
}

fn parse_args() -> Result<Args, String> {
    let mut graph = None;
    let mut socket = None;
    let mut control = None;
    let mut batch_control = None;
    let mut token_file = None;
    let mut layers = Vec::new();
    let mut sets = Vec::new();
    let mut vault = None;
    let mut runs_vault = None;
    let mut idle_exit = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--graph" => graph = Some(PathBuf::from(value()?)),
            "--socket" => socket = Some(Listen::parse(&value()?)?),
            "--control" => control = Some(Listen::parse(&value()?)?),
            "--batch-control" => {
                let listen = Listen::parse(&value()?)?;
                // Refused here, before any other check (a TCP endpoint's
                // token file), so the refusal names its real reason.
                if matches!(listen, Listen::Tcp(_)) && cfg!(unix) {
                    return Err(BATCH_TCP_REFUSED.into());
                }
                batch_control = Some(listen);
            }
            "--token-file" => token_file = Some(PathBuf::from(value()?)),
            "--global" => layers.push((Layer::Global, PathBuf::from(value()?))),
            "--project" => {
                layers.push((Layer::Project, PathBuf::from(value()?).join("helios.toml")));
            }
            "--user" => layers.push((Layer::User, PathBuf::from(value()?))),
            "--vault" => vault = Some(PathBuf::from(value()?)),
            "--runs-vault" => runs_vault = Some(PathBuf::from(value()?)),
            "--idle-exit" => {
                let v = value()?;
                let seconds: u64 = v
                    .strip_suffix('s')
                    .unwrap_or(&v)
                    .parse()
                    .map_err(|_| format!("--idle-exit takes seconds, got `{v}`"))?;
                idle_exit = Some(Duration::from_secs(seconds));
            }
            "--set" => {
                let v = value()?;
                let (k, val) = v
                    .split_once('=')
                    .ok_or(format!("--set takes KEY=VALUE, got `{v}`"))?;
                sets.push((k.to_string(), val.to_string()));
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Args {
        graph,
        socket: socket.ok_or("missing --socket PATH")?,
        control,
        batch_control,
        token_file,
        layers,
        sets,
        vault,
        runs_vault,
        idle_exit,
    })
}

/// The vault's name for a layer kept in it: `user`, or `project/<dir>`.
fn vault_layer(layer: Layer, path: &Path) -> Option<String> {
    match layer {
        Layer::User => Some("user".into()),
        Layer::Project => {
            let dir = path.parent()?;
            let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
            Some(format!("project/{}", dir.display()))
        }
        _ => None,
    }
}

/// The node's settings, from the layers it was given; with a `vault`,
/// the user and project layers are kept in it.
fn config(args: &Args, vault: Option<&Vault>) -> Result<Config, String> {
    let mut config = Config::new(Schema::core());
    // Lowest layer first: a value equal to what it inherits leaves its layer,
    // so what it inherits must be known when it is set.
    let mut layers: Vec<_> = args.layers.iter().collect();
    layers.sort_by_key(|(layer, _)| *layer);
    let problems = |source: String| {
        move |problems: Vec<helios_config::Problem>| {
            let lines: Vec<String> = problems.iter().map(|p| format!("{source}: {p}")).collect();
            lines.join("\n")
        }
    };
    let mut from_vault = Vec::new();
    for (layer, path) in layers {
        let kept = vault.zip(vault_layer(*layer, path));
        let Some((vault, name)) = kept else {
            config
                .load_file(*layer, path)
                .map_err(problems(path.display().to_string()))?;
            continue;
        };
        if vault.config_layer(&name)?.is_none() {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            vault.set_config_layer(&name, Some(&text))?;
            eprintln!(
                "helios-node: {} imported into the vault as `{name}`; the vault's copy is read from now on",
                path.display()
            );
        }
        from_vault.push((*layer, name));
    }
    // Layers kept in the vault, given as files or not.
    if let Some(vault) = vault {
        if !from_vault.iter().any(|(l, _)| *l == Layer::User) {
            from_vault.push((Layer::User, "user".into()));
        }
        from_vault.sort_by_key(|(layer, _)| *layer);
        for (layer, name) in from_vault {
            if let Some(text) = vault.config_layer(&name)? {
                config
                    .load_str(layer, &text)
                    .map_err(problems(format!("vault layer `{name}`")))?;
            }
        }
    }
    for (key, value) in &args.sets {
        config
            .set_str(Layer::Run, key, value)
            .map_err(|p| p.to_string())?;
    }
    Ok(config)
}

/// The node of `--vault`, stopped once: by a signal, as `main` ends, or
/// idle; its rendezvous file removed first.
#[derive(Clone, Default)]
struct Stopper(Arc<Mutex<Option<VaultNode>>>, Arc<Mutex<Option<PathBuf>>>);

impl Stopper {
    fn hold(&self, node: VaultNode) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(node);
    }

    /// The rendezvous file to remove as the node stops.
    fn rendezvous(&self, file: PathBuf) {
        *self
            .1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(file);
    }

    /// Stops the node, if it is running: its jobs settled, the vault
    /// closed cleanly.
    fn stop(&self) {
        // No client finds the node any more once it starts stopping.
        let rendezvous = self
            .1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(file) = rendezvous {
            let _ = std::fs::remove_file(file);
        }
        let node = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(node) = node {
            node.stop();
            eprintln!("helios-node: the vault is closed");
        }
    }

    /// SIGTERM or SIGINT stops the node, then ends the process (code 0).
    #[cfg(unix)]
    fn on_signals(&self) {
        let signalled = Arc::new(AtomicBool::new(false));
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            if let Err(e) = signal_hook::flag::register(signal, signalled.clone()) {
                eprintln!("helios-node: signal {signal} not heard: {e}");
            }
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("helios-node-signals".into())
            .spawn(move || {
                while !signalled.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                eprintln!("helios-node: stopping");
                me.stop();
                std::process::exit(0);
            });
        if let Err(e) = spawned {
            eprintln!("helios-node: no thread to hear signals on: {e}");
        }
    }

    #[cfg(not(unix))]
    fn on_signals(&self) {}
}

/// Takes the lock of the vault at `file`; `None`, said on stderr, when
/// another node holds it.
fn owner_lock(file: &Path) -> Result<Option<helios_inspect::discovery::Lock>, String> {
    use helios_inspect::discovery::{Lock, Rendezvous, files};
    let files = files(file);
    match Lock::try_take(&files.lock) {
        Ok(Some(lock)) => Ok(Some(lock)),
        Ok(None) => {
            let holder = Rendezvous::read(&files.rendezvous)
                .ok()
                .flatten()
                .map_or_else(String::new, |r| format!(" (process {})", r.pid));
            eprintln!(
                "helios-node: {}: another node holds this vault{holder}; this one is not advertised to clients",
                file.display()
            );
            Ok(None)
        }
        Err(e) => Err(format!("{}: {e}", files.lock.display())),
    }
}

/// Stops the node and ends the process once, for `idle`, no client has
/// been connected to any endpoint and no job has run.
fn exit_when_idle(idle: Duration, jobs: Jobs, stopper: Stopper) {
    let started = Instant::now();
    let spawned = std::thread::Builder::new()
        .name("helios-node-idle".into())
        .spawn(move || {
            let mut quiet_since = started;
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let activity = helios_inspect::activity();
                if activity.open > 0 || !jobs.cancellable().is_empty() {
                    quiet_since = Instant::now();
                    continue;
                }
                let last = activity.last.map_or(quiet_since, |l| l.max(quiet_since));
                if last.elapsed() >= idle {
                    eprintln!("helios-node: idle for {} s, stopping", idle.as_secs());
                    stopper.stop();
                    std::process::exit(0);
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("helios-node: no thread to watch for idleness: {e}");
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "helios-node: {e}\nusage: helios-node [--graph FILE] --socket PATH [--global FILE] [--project DIR] [--user FILE] [--vault FILE] [--runs-vault FILE] [--set KEY=VALUE]... [--control PATH] [--batch-control PATH] [--token-file FILE] [--idle-exit SECONDS]"
            );
            return ExitCode::from(2);
        }
    };
    let stopper = Stopper::default();
    stopper.on_signals();
    let code = serve(&args, &stopper);
    // A start that failed past the vault's opening still closes it cleanly.
    stopper.stop();
    code
}

/// Runs the node `args` describe; returns only when it cannot start.
fn serve(args: &Args, stopper: &Stopper) -> ExitCode {
    let loaded = match &args.graph {
        None => helios_node::load_in(
            serde_json::from_str("{}").expect("an empty network reads"),
            Path::new("."),
        ),
        Some(graph) => std::fs::read_to_string(graph)
            .map_err(|e| format!("cannot read {}: {e}", graph.display()))
            .and_then(|text| {
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", graph.display()))
            })
            // Files the network names are relative to it.
            .and_then(|desc| helios_node::load_in(desc, graph.parent().unwrap_or(Path::new(".")))),
    };
    let mut loaded = match loaded {
        Ok(l) => l,
        Err(e) => {
            eprintln!("helios-node: {e}");
            return ExitCode::from(1);
        }
    };
    // The node owning a vault holds its lock while the process lives
    // (released by the system even if it is killed) and advertises
    // itself; another node on the same vault (one taking over the runs of
    // a dead one) works, but clients do not find it.
    let owned = match args.vault.as_deref().map(owner_lock) {
        None => None,
        Some(Ok(lock)) => lock,
        Some(Err(e)) => {
            eprintln!("helios-node: {e}");
            return ExitCode::from(1);
        }
    };
    // The vault is opened once: read for the settings first, then owned by
    // the node, which the other users borrow it from.
    let vault = match args.vault.as_ref().map(|path| {
        helios_vault_sqlite::SqliteStore::open(path)
            .map(Vault::new)
            .map_err(|e| format!("{}: {e}", path.display()))
    }) {
        None => None,
        Some(Ok(v)) => Some(v),
        Some(Err(e)) => {
            eprintln!("helios-node: {e}");
            return ExitCode::from(1);
        }
    };
    let config = match config(args, vault.as_ref()) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("helios-node: {e}");
            return ExitCode::from(1);
        }
    };

    let bus = Arc::new(Bus::with_journal(
        config.number("system.events.journal") as usize
    ));
    let mut book = Book::new().with_bus(bus.clone());
    for (name, mode) in &loaded.decisions {
        book.declare(name, *mode);
    }
    let resolver = match helios_node::build_resolvers(loaded.resolver.as_ref(), &loaded.resolvers) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("helios-node: resolver: {e}");
            return ExitCode::from(1);
        }
    };
    let jobs = Jobs::new(bus.clone());
    // The node owns the vault and runs its jobs: the volume watch, the
    // backups, the health check.
    let held = vault.zip(args.vault.as_ref()).map(|(vault, file)| {
        let node = VaultNode::start(vault, file, bus.clone(), &jobs, VaultNodeOptions::system());
        let held = (
            node.vault().clone(),
            node.importer().clone(),
            node.settings(),
        );
        stopper.hold(node);
        held
    });
    let (vault, importer, settings) = match held {
        Some((vault, importer, settings)) => (Some(vault), Some(importer), Some(settings)),
        None => (None, None, None),
    };
    let edit = match (&vault, &loaded.frame_file) {
        (Some(vault), Some(file)) => match open_edit(
            vault,
            file,
            &bus,
            &mut loaded.graph,
            &mut book,
            &loaded.decisions,
        ) {
            Ok(edit) => Some(edit),
            Err(e) => {
                eprintln!("helios-node: the edit cannot be kept in the vault: {e}");
                return ExitCode::from(1);
            }
        },
        _ => None,
    };
    let budget = config.cache_budget(helios_core::engine::physical_memory());
    let workers = config.number("system.scheduler.workers") as usize;
    let engine = Arc::new(Engine::with_budget(bus.clone(), budget));
    let scheduler = Arc::new(Scheduler::with_resources(
        bus.clone(),
        workers,
        &[(
            helios_resolve::AI_CALLS,
            config.number("system.ai.calls") as u64,
        )],
    ));
    // Built first: its instance names the owner of the runs.
    let inspector = Inspector::new(bus.clone());
    // The runs last in the node's vault, or in `--runs-vault`'s.
    let runs_vault = match &args.runs_vault {
        None => vault,
        Some(path) => match helios_vault_sqlite::SqliteStore::open(path) {
            Ok(store) => Some(Arc::new(Vault::new(store))),
            Err(e) => {
                eprintln!("helios-node: {}: {e}", path.display());
                return ExitCode::from(1);
            }
        },
    };
    let workflows = Arc::new(Workflows::new(RunsContext {
        bus: bus.clone(),
        engine: engine.clone(),
        scheduler: scheduler.clone(),
        jobs: jobs.clone(),
        vault: runs_vault,
        // 0 is one per CPU, as the scheduler counts it.
        workers: if workers == 0 {
            std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
        } else {
            workers
        },
        owner: inspector.instance().to_string(),
        keep_ms: (config.number("system.workflow.keep_days") * 86_400_000.0) as u64,
    }));
    workflows.start(&loaded.graph);
    let node = Arc::new(Node {
        bus: bus.clone(),
        engine,
        scheduler,
        graph: Arc::new(SharedGraph(RwLock::new(loaded.graph))),
        names: loaded.nodes,
        targets: Mutex::new(Vec::new()),
        renders: Mutex::new(Vec::new()),
        exports: Mutex::new(Vec::new()),
        idle: Mutex::new(false),
        book: Mutex::new(book),
        resolver,
        brief: loaded.brief.clone(),
        frame: loaded.frame,
        contexts: Mutex::new(loaded.contexts),
        edit,
        jobs,
        batches: Mutex::new(BTreeMap::new()),
        workflows: workflows.clone(),
        decisions: loaded.decisions.clone(),
        first_export: loaded.exports.first().map(|e| e.node.clone()),
        importer,
        cursors: helios_node::cursor::Cursors::default(),
        settings,
    });
    // A batch whose client went without ending it ends on its own.
    {
        let node = node.clone();
        std::thread::Builder::new()
            .name("helios-node-batches".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(250));
                    node.end_idle_batches(Instant::now());
                }
            })
            .expect("a thread for the batches' watch");
    }
    let inspector = inspector
        .with("graph", node.graph.clone())
        .with("workflow", workflows)
        .with("engine", node.engine.clone())
        .with("node", Arc::new(Status(node.clone())))
        .with("config", config.clone())
        .with("scheduler", node.scheduler.clone())
        .with("modules", Arc::new(Modules))
        .with_jobs()
        .with_scheduler(node.scheduler.clone());
    // A TCP endpoint needs a session token, written where only this user reads it.
    let tcp = [
        Some(&args.socket),
        args.control.as_ref(),
        args.batch_control.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|l| matches!(l, Listen::Tcp(_)));
    let token = tcp.then(new_token);
    if let Some(token) = &token {
        let Some(file) = &args.token_file else {
            eprintln!("helios-node: a tcp:// endpoint needs --token-file FILE");
            return ExitCode::from(2);
        };
        if let Err(e) = write_token(file, token) {
            eprintln!("helios-node: cannot write {}: {e}", file.display());
            return ExitCode::from(1);
        }
    }
    let instance = inspector.instance().to_string();
    let address = match serve_inspector(inspector, &args.socket, token.as_deref()) {
        Ok(bound) => bound.address,
        Err(e) => {
            eprintln!("helios-node: cannot serve on {:?}: {e}", args.socket);
            return ExitCode::from(1);
        }
    };
    let control_address = match &args.control {
        None => None,
        Some(listen) => match serve_control(&node, &instance, listen, token.as_deref()) {
            Ok(address) => Some(address),
            Err(e) => {
                eprintln!("helios-node: {e}");
                return ExitCode::from(1);
            }
        },
    };
    let batch_address = match &args.batch_control {
        None => None,
        Some(listen) => match serve_batch_control(&node, &instance, listen, token.as_deref()) {
            Ok(address) => Some(address),
            Err(e) => {
                eprintln!("helios-node: {e}");
                return ExitCode::from(1);
            }
        },
    };
    // Whoever started the node may stop reading once it knows the node
    // serves: a closed stdout must not kill the node (`println!` panics).
    let mut out = std::io::stdout();
    let _ = writeln!(out, "ready {address}");
    if let Some(control) = &control_address {
        let _ = writeln!(out, "control {control}");
    }
    if let Some(batch) = &batch_address {
        let _ = writeln!(out, "batch-control {batch}");
    }
    let _ = out.flush();
    // Clients of the vault find the node by its rendezvous file.
    if let (Some(file), Some(batch), Some(_)) = (&args.vault, &batch_address, &owned) {
        let rendezvous = helios_inspect::discovery::Rendezvous {
            address: batch.clone(),
            inspect: address.clone(),
            token_file: token
                .as_ref()
                .and(args.token_file.as_ref())
                .map(|f| std::path::absolute(f).unwrap_or_else(|_| f.clone())),
            instance: instance.clone(),
            protocol: helios_inspect::rpc::PROTOCOL.into(),
            pid: std::process::id(),
            vault: std::path::absolute(file).unwrap_or_else(|_| file.clone()),
        };
        let path = helios_inspect::discovery::files(file).rendezvous;
        if let Err(e) = rendezvous.write(&path) {
            eprintln!("helios-node: cannot write {}: {e}", path.display());
            return ExitCode::from(1);
        }
        stopper.rendezvous(path);
    }
    if let Some(idle) = args.idle_exit {
        exit_when_idle(idle, node.jobs.clone(), stopper.clone());
    }
    let ready = bus.publish(
        SOURCE,
        None,
        &Ready {
            socket: address.clone(),
            renders: loaded.renders.len(),
            exports: loaded.exports.len(),
        },
    );

    // Every render is a job of the scheduler; the node is idle when it is.
    for r in &loaded.renders {
        if let Err(e) = node.submit_render(&r.node, &r.port, r.region, Some(ready)) {
            eprintln!("helios-node: {e}");
            return ExitCode::from(1);
        }
    }
    for e in &loaded.exports {
        if let Err(e) = node.submit_export(e) {
            eprintln!("helios-node: {e}");
            return ExitCode::from(1);
        }
    }
    node.scheduler.wait_until_idle(std::time::Duration::MAX);
    let failed = node
        .renders
        .lock()
        .expect("renders lock")
        .iter()
        .filter(|r| !r["error"].is_null())
        .count()
        + node
            .export_states()
            .iter()
            .filter(|e| e["state"] != "done")
            .count();
    *node.idle.lock().expect("idle lock") = true;
    bus.publish(
        SOURCE,
        Some(ready),
        &Idle {
            renders: loaded.renders.len(),
            exports: loaded.exports.len(),
            failed,
        },
    );

    loop {
        std::thread::park();
    }
}
