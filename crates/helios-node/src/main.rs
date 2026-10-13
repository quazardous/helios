//! `helios-node`: a headless Helios node.
//!
//! Loads a network from a JSON description, serves the read-only inspection
//! endpoint on a socket, prints `ready <socket>` on stdout, renders what the
//! description asks for, then stays up so it can be inspected. Stop it with
//! a signal.
//!
//!     helios-node --graph network.json --socket /path/to/node.sock
//!                 [--global FILE] [--project DIR] [--user FILE] [--vault FILE] [--set KEY=VALUE]...
//!                 [--control /path/to/control.sock] [--batch-control PATH] [--token-file FILE]
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
//! `load-source`, `export` and `cancel`, on a Unix socket only its user can
//! open. `helios batch` starts its own node with it, on a socket in a
//! private directory made for that run.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, RwLock};

use helios_config::{Config, Layer, Schema};
use helios_core::{Bus, Describe, Engine, Event, Graph, JobId, Message, NodeId, Region, Scheduler};
use helios_decision::Book;
use helios_inspect::{Inspector, Listen, new_token, serve_inspector, write_token};
use helios_node::ExportDesc;
use helios_photo::CameraInfo;
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

/// The modules this node knows, with their manifests: what a surface or an
/// agent needs to build and edit a network.
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
        json!({ "modules": modules })
    }
}

/// The graph of the node, shared by renders, inspection and control.
struct SharedGraph(RwLock<Graph>);

impl Describe for SharedGraph {
    fn describe(&self) -> Value {
        self.0.read().expect("graph lock").describe()
    }
}

/// The edit of the image a node with a vault works on, kept as versions.
struct NodeEdit {
    vault: helios_vault::Vault<helios_vault_sqlite::SqliteStore>,
    session: Mutex<helios_vault::EditSession>,
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
    vault_file: &Path,
    file: &Path,
    bus: &Arc<Bus>,
    graph: &mut Graph,
    book: &mut Book,
    declared: &BTreeMap<String, helios_decision::Mode>,
) -> Result<NodeEdit, String> {
    let store = helios_vault_sqlite::SqliteStore::open(vault_file)
        .map_err(|e| format!("{}: {e}", vault_file.display()))?;
    let vault = helios_vault::Vault::new(store).with_bus(bus.clone());
    let image = vault.import_file(file, unix_now())?;
    let (session, head) = helios_vault::EditSession::open(&vault, image, "control")?;
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
        vault,
        session: Mutex::new(session),
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
    /// The camera of each source holding a raw.
    cameras: Mutex<BTreeMap<NodeId, CameraInfo>>,
    /// The vault keeping the edit, when the node has one.
    edit: Option<NodeEdit>,
}

impl Node {
    /// Saves the edit as it is now as a version continuing the last one;
    /// its id, or `None` when the node keeps no edit or nothing changed.
    /// The session is locked first, so two changes answered at once are
    /// saved in the order they were made.
    fn save_edit(&self, label: &str) -> Result<Option<String>, String> {
        let Some(edit) = &self.edit else {
            return Ok(None);
        };
        let mut session = edit.session.lock().expect("edit lock");
        let value = {
            let book = self.book.lock().expect("book lock");
            let graph = self.graph.0.read().expect("graph lock");
            edit_of(&graph, &book)
        };
        session
            .save(&edit.vault, value, label, unix_now())
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
        let job = helios_export::submit_with(
            &self.scheduler,
            self.bus.clone(),
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
    fn submit_export_now(
        self: &Arc<Self>,
        name: &str,
        port: &str,
        region: Option<Region>,
        path: &Path,
        format: helios_export::Format,
        params: &BTreeMap<String, helios_core::Params>,
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
        let job = helios_export::submit_with(
            &self.scheduler,
            self.bus.clone(),
            path,
            format,
            move || {
                engine
                    .render(&graph, id, &port, region)
                    .map_err(|e| e.to_string())
            },
        )?;
        self.exports
            .lock()
            .expect("exports lock")
            .push((path.to_path_buf(), job));
        Ok((job, region))
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
        let mut cameras = self.cameras.lock().expect("cameras lock");
        helios_node::load_source(&mut graph, &mut cameras, id, loaded).map_err(|e| {
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
    //! export needs: give a source a file, export, cancel a job.

    use std::sync::Arc;

    use helios_core::{Event, JobId, Message};
    use helios_export::Format;
    use helios_inspect::control::{ControlRequest, FileFormat};
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
        /// `load-source`, `export` and `cancel`: the batch endpoint.
        Batch,
    }

    impl Scope {
        fn allows(self, request: &ControlRequest) -> bool {
            self == Self::Debug
                || matches!(
                    request,
                    ControlRequest::LoadSource { .. }
                        | ControlRequest::Export { .. }
                        | ControlRequest::Cancel { .. }
                )
        }
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

    pub fn answer(node: &Arc<Node>, scope: Scope, request: ControlRequest) -> Value {
        if !scope.allows(&request) {
            return refused(
                None,
                &Message::new(
                    "node-batch-only",
                    "the batch endpoint only answers load-source, export and cancel",
                ),
            );
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
            } => {
                let format = match format {
                    FileFormat::Jpeg => Format::Jpeg {
                        quality: quality.unwrap_or(92),
                    },
                    FileFormat::Png => Format::Png { bits: 8 },
                    FileFormat::Tiff => Format::Tiff,
                };
                node.submit_export_now(name, port, *region, path, format, params)
                    .map(|(job, region)| json!({ "job": job.0, "region": region }))
            }
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
            ControlRequest::Resume => {
                node.scheduler.resume();
                Ok(json!({}))
            }
            ControlRequest::Fault { node: name, kind } => node.id(name).map(|id| {
                let graph = node.graph.0.read().expect("graph lock");
                node.engine.inject_fault(&graph, id, *kind);
                json!({})
            }),
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
        // A change to the edit is saved before it is answered.
        let label = match &request {
            ControlRequest::SetParam {
                node: name, param, ..
            } => Some(format!("set {name}.{param}")),
            ControlRequest::DecisionResolve => Some("decisions resolved".into()),
            ControlRequest::DecisionAccept { node: name, .. } => {
                Some(format!("decision {name} accepted"))
            }
            ControlRequest::DecisionDecide { node: name, .. } => {
                Some(format!("decision {name} decided"))
            }
            ControlRequest::DecisionReask { node: name } => {
                Some(format!("decision {name} asked again"))
            }
            _ => None,
        };
        let result = match (result, label) {
            (Ok(mut extra), Some(label)) => match node.save_edit(&label) {
                Ok(saved) => {
                    if let (Some(id), Some(map)) = (saved, extra.as_object_mut()) {
                        map.insert("saved".into(), json!(id));
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

/// Serves the batch endpoint: `load-source`, `export` and `cancel` only,
/// in every build. A Unix socket only, readable and writable by this user
/// alone: whoever reaches it can read and write the user's files through
/// the node, as the user could anyway.
fn serve_batch_control(node: &Arc<Node>, listen: &Listen) -> Result<String, String> {
    let Listen::Unix(path) = listen else {
        return Err("--batch-control takes a Unix socket path, not a TCP address".into());
    };
    let node = node.clone();
    let bound = helios_inspect::serve_at(listen, None, move |request| {
        control::answer(&node, control::Scope::Batch, request)
    })
    .map_err(|e| format!("cannot serve the batch endpoint on {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(bound.address)
}

/// Serves the control endpoint, if this build and environment allow it.
fn serve_control(node: &Arc<Node>, listen: &Listen, token: Option<&str>) -> Result<String, String> {
    #[cfg(debug_assertions)]
    {
        if std::env::var_os("HELIOS_DEBUG_CONTROL").is_none() {
            return Err(
                "the control endpoint is served only when HELIOS_DEBUG_CONTROL is set".into(),
            );
        }
        let node = node.clone();
        helios_inspect::serve_at(listen, token, move |request| {
            control::answer(&node, control::Scope::Debug, request)
        })
        .map(|bound| bound.address)
        .map_err(|e| format!("cannot serve control on {listen:?}: {e}"))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (node, listen, token);
        Err("the control endpoint exists only in debug builds".into())
    }
}

struct Args {
    graph: PathBuf,
    socket: Listen,
    control: Option<Listen>,
    batch_control: Option<Listen>,
    token_file: Option<PathBuf>,
    layers: Vec<(Layer, PathBuf)>,
    sets: Vec<(String, String)>,
    vault: Option<PathBuf>,
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
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--graph" => graph = Some(PathBuf::from(value()?)),
            "--socket" => socket = Some(Listen::parse(&value()?)?),
            "--control" => control = Some(Listen::parse(&value()?)?),
            "--batch-control" => batch_control = Some(Listen::parse(&value()?)?),
            "--token-file" => token_file = Some(PathBuf::from(value()?)),
            "--global" => layers.push((Layer::Global, PathBuf::from(value()?))),
            "--project" => {
                layers.push((Layer::Project, PathBuf::from(value()?).join("helios.toml")));
            }
            "--user" => layers.push((Layer::User, PathBuf::from(value()?))),
            "--vault" => vault = Some(PathBuf::from(value()?)),
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
        graph: graph.ok_or("missing --graph FILE")?,
        socket: socket.ok_or("missing --socket PATH")?,
        control,
        batch_control,
        token_file,
        layers,
        sets,
        vault,
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

/// The node's settings, from the layers it was given.
fn config(args: &Args) -> Result<Config, String> {
    let mut config = Config::new(Schema::core());
    // Lowest layer first: a value equal to what it inherits leaves its layer,
    // so what it inherits must be known when it is set.
    let vault = args
        .vault
        .as_ref()
        .map(|path| {
            helios_vault_sqlite::SqliteStore::open(path)
                .map(helios_vault::Vault::new)
                .map_err(|e| format!("{}: {e}", path.display()))
        })
        .transpose()?;
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
        let kept = vault.as_ref().zip(vault_layer(*layer, path));
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
    if let Some(vault) = &vault {
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

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "helios-node: {e}\nusage: helios-node --graph FILE --socket PATH [--global FILE] [--project DIR] [--user FILE] [--vault FILE] [--set KEY=VALUE]... [--control PATH] [--batch-control PATH] [--token-file FILE]"
            );
            return ExitCode::from(2);
        }
    };
    let loaded = std::fs::read_to_string(&args.graph)
        .map_err(|e| format!("cannot read {}: {e}", args.graph.display()))
        .and_then(|text| {
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", args.graph.display()))
        })
        // Files the network names are relative to it.
        .and_then(|desc| helios_node::load_in(desc, args.graph.parent().unwrap_or(Path::new("."))));
    let mut loaded = match loaded {
        Ok(l) => l,
        Err(e) => {
            eprintln!("helios-node: {e}");
            return ExitCode::from(1);
        }
    };
    let config = match config(&args) {
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
    let edit = match (&args.vault, &loaded.frame_file) {
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
    let node = Arc::new(Node {
        bus: bus.clone(),
        engine: Arc::new(Engine::with_budget(bus.clone(), budget)),
        scheduler: Arc::new(Scheduler::with_resources(
            bus.clone(),
            config.number("system.scheduler.workers") as usize,
            &[(
                helios_resolve::AI_CALLS,
                config.number("system.ai.calls") as u64,
            )],
        )),
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
        cameras: Mutex::new(loaded.cameras),
        edit,
    });
    let inspector = Inspector::new(bus.clone())
        .with("graph", node.graph.clone())
        .with("engine", node.engine.clone())
        .with("node", Arc::new(Status(node.clone())))
        .with("config", config.clone())
        .with("scheduler", node.scheduler.clone())
        .with("modules", Arc::new(Modules))
        .with_scheduler(node.scheduler.clone());
    // A TCP endpoint needs a session token, written where only this user reads it.
    let tcp = [Some(&args.socket), args.control.as_ref()]
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
    let address = match serve_inspector(inspector, &args.socket, token.as_deref()) {
        Ok(bound) => bound.address,
        Err(e) => {
            eprintln!("helios-node: cannot serve on {:?}: {e}", args.socket);
            return ExitCode::from(1);
        }
    };
    let control_address = match &args.control {
        None => None,
        Some(listen) => match serve_control(&node, listen, token.as_deref()) {
            Ok(address) => Some(address),
            Err(e) => {
                eprintln!("helios-node: {e}");
                return ExitCode::from(1);
            }
        },
    };
    let batch_address = match &args.batch_control {
        None => None,
        Some(listen) => match serve_batch_control(&node, listen) {
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
