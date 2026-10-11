//! `helios-node`: a headless Helios node.
//!
//! Loads a network from a JSON description, serves the read-only inspection
//! endpoint on a socket, prints `ready <socket>` on stdout, renders what the
//! description asks for, then stays up so it can be inspected. Stop it with
//! a signal.
//!
//!     helios-node --graph network.json --socket /path/to/node.sock
//!                 [--global FILE] [--project DIR] [--user FILE] [--set KEY=VALUE]...
//!                 [--control /path/to/control.sock]
//!
//! Settings come from the files given (global layer, `DIR/helios.toml` for
//! the project layer, user layer) and from `--set` (run layer); nothing is
//! read from a default location behind the caller's back.
//!
//! `--control` serves the debug control endpoint (render, set a parameter,
//! cancel, pause, resume, inject a fault). It exists only in debug builds and
//! only when `HELIOS_DEBUG_CONTROL` is set.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, RwLock};

use helios_config::{Config, Layer, Schema};
use helios_core::{Bus, Describe, Engine, Event, Graph, JobId, NodeId, Region, Scheduler};
use helios_inspect::{Inspector, serve};
use serde::Serialize;
use serde_json::{Value, json};

const SOURCE: &str = "node";

/// The node is serving and about to render.
#[derive(Serialize)]
struct Ready {
    socket: String,
    renders: usize,
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

/// Every render asked for by the description is finished.
#[derive(Serialize)]
struct Idle {
    renders: usize,
    failed: usize,
}

impl Event for Idle {
    const KIND: &'static str = "node.idle";
}

/// The graph of the node, shared by renders, inspection and control.
struct SharedGraph(RwLock<Graph>);

impl Describe for SharedGraph {
    fn describe(&self) -> Value {
        self.0.read().expect("graph lock").describe()
    }
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
    idle: Mutex<bool>,
}

impl Node {
    fn id(&self, name: &str) -> Result<NodeId, String> {
        self.names
            .get(name)
            .copied()
            .ok_or_else(|| format!("no node named `{name}`"))
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
            "headers": headers,
        })
    }
}

#[cfg(debug_assertions)]
mod control {
    //! The debug control endpoint: every action goes to the owner of what
    //! it changes, and is announced on the bus.

    use std::sync::Arc;

    use helios_core::{Event, JobId};
    use helios_inspect::control::ControlRequest;
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

    pub fn answer(node: &Arc<Node>, request: ControlRequest) -> Value {
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
        let result: Result<Value, String> = match &request {
            ControlRequest::Render {
                node: name,
                port,
                region,
            } => node
                .submit_render(name, port, *region, Some(seq))
                .map(|job| json!({ "job": job.0 })),
            ControlRequest::SetParam {
                node: name,
                param,
                value,
            } => node.id(name).and_then(|id| {
                let mut graph = node.graph.0.write().expect("graph lock");
                graph
                    .set_param(id, param, value.clone())
                    .map(|()| json!({}))
                    .map_err(|e| e.to_string())
            }),
            ControlRequest::Cancel { job } => {
                if node.scheduler.cancel(JobId(*job)) {
                    Ok(json!({}))
                } else {
                    Err(format!("job {job} is unknown or already ended"))
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
        };
        match result {
            Ok(extra) => done(extra),
            Err(error) => json!({ "ok": false, "seq": seq, "error": error }),
        }
    }
}

/// Serves the control endpoint, if this build and environment allow it.
fn serve_control(node: &Arc<Node>, path: &std::path::Path) -> Result<(), String> {
    #[cfg(debug_assertions)]
    {
        if std::env::var_os("HELIOS_DEBUG_CONTROL").is_none() {
            return Err(
                "the control endpoint is served only when HELIOS_DEBUG_CONTROL is set".into(),
            );
        }
        let node = node.clone();
        helios_inspect::serve_lines(path, move |request| control::answer(&node, request))
            .map(|_| ())
            .map_err(|e| format!("cannot serve control on {}: {e}", path.display()))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (node, path);
        Err("the control endpoint exists only in debug builds".into())
    }
}

struct Args {
    graph: PathBuf,
    socket: PathBuf,
    control: Option<PathBuf>,
    layers: Vec<(Layer, PathBuf)>,
    sets: Vec<(String, String)>,
}

fn parse_args() -> Result<Args, String> {
    let mut graph = None;
    let mut socket = None;
    let mut control = None;
    let mut layers = Vec::new();
    let mut sets = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--graph" => graph = Some(PathBuf::from(value()?)),
            "--socket" => socket = Some(PathBuf::from(value()?)),
            "--control" => control = Some(PathBuf::from(value()?)),
            "--global" => layers.push((Layer::Global, PathBuf::from(value()?))),
            "--project" => {
                layers.push((Layer::Project, PathBuf::from(value()?).join("helios.toml")))
            }
            "--user" => layers.push((Layer::User, PathBuf::from(value()?))),
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
        layers,
        sets,
    })
}

/// The node's settings, from the layers it was given.
fn config(args: &Args) -> Result<Config, String> {
    let mut config = Config::new(Schema::core());
    // Lowest layer first: a value equal to what it inherits leaves its layer,
    // so what it inherits must be known when it is set.
    let mut layers: Vec<_> = args.layers.iter().collect();
    layers.sort_by_key(|(layer, _)| *layer);
    for (layer, path) in layers {
        config.load_file(*layer, path).map_err(|problems| {
            let lines: Vec<String> = problems
                .iter()
                .map(|p| format!("{}: {p}", path.display()))
                .collect();
            lines.join("\n")
        })?;
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
                "helios-node: {e}\nusage: helios-node --graph FILE --socket PATH [--global FILE] [--project DIR] [--user FILE] [--set KEY=VALUE]... [--control PATH]"
            );
            return ExitCode::from(2);
        }
    };
    let loaded = std::fs::read_to_string(&args.graph)
        .map_err(|e| format!("cannot read {}: {e}", args.graph.display()))
        .and_then(|text| {
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", args.graph.display()))
        })
        .and_then(helios_node::load);
    let loaded = match loaded {
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
    let budget = match config.number("system.cache.budget") as usize {
        0 => None,
        mib => Some(mib * 1024 * 1024),
    };
    let node = Arc::new(Node {
        bus: bus.clone(),
        engine: Arc::new(Engine::with_budget(bus.clone(), budget)),
        scheduler: Arc::new(Scheduler::new(
            bus.clone(),
            config.number("system.scheduler.workers") as usize,
        )),
        graph: Arc::new(SharedGraph(RwLock::new(loaded.graph))),
        names: loaded.nodes,
        targets: Mutex::new(Vec::new()),
        renders: Mutex::new(Vec::new()),
        idle: Mutex::new(false),
    });
    let inspector = Inspector::new(bus.clone())
        .with("graph", node.graph.clone())
        .with("engine", node.engine.clone())
        .with("node", Arc::new(Status(node.clone())))
        .with("config", config.clone())
        .with("scheduler", node.scheduler.clone())
        .with_scheduler(node.scheduler.clone());
    if let Err(e) = serve(inspector, &args.socket) {
        eprintln!(
            "helios-node: cannot serve on {}: {e}",
            args.socket.display()
        );
        return ExitCode::from(1);
    }
    if let Some(path) = &args.control
        && let Err(e) = serve_control(&node, path)
    {
        eprintln!("helios-node: {e}");
        return ExitCode::from(1);
    }
    println!("ready {}", args.socket.display());
    let _ = std::io::stdout().flush();
    let ready = bus.publish(
        SOURCE,
        None,
        &Ready {
            socket: args.socket.display().to_string(),
            renders: loaded.renders.len(),
        },
    );

    // Every render is a job of the scheduler; the node is idle when it is.
    for r in &loaded.renders {
        if let Err(e) = node.submit_render(&r.node, &r.port, r.region, Some(ready)) {
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
        .count();
    *node.idle.lock().expect("idle lock") = true;
    bus.publish(
        SOURCE,
        Some(ready),
        &Idle {
            renders: loaded.renders.len(),
            failed,
        },
    );

    loop {
        std::thread::park();
    }
}
