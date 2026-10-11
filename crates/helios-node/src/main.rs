//! `helios-node`: a headless Helios node.
//!
//! Loads a network from a JSON description, serves the read-only inspection
//! endpoint on a socket, prints `ready <socket>` on stdout, renders what the
//! description asks for, then stays up so it can be inspected. Stop it with
//! a signal.
//!
//!     helios-node --graph network.json --socket /path/to/node.sock
//!                 [--global FILE] [--project DIR] [--user FILE] [--set KEY=VALUE]...
//!
//! Settings come from the files given (global layer, `DIR/helios.toml` for
//! the project layer, user layer) and from `--set` (run layer); nothing is
//! read from a default location behind the caller's back.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use helios_config::{Config, Layer, Schema};
use helios_core::{Bus, Describe, Engine, Event, Graph, NodeId, Region, Scheduler};
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

/// A render asked for by the description is finished.
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

/// Every render asked for is finished; nothing runs any more.
#[derive(Serialize)]
struct Idle {
    renders: usize,
    failed: usize,
}

impl Event for Idle {
    const KIND: &'static str = "node.idle";
}

/// What the node reports about itself, with the header of every node
/// involved in each render it was asked for.
struct Status {
    graph: Arc<Graph>,
    engine: Arc<Engine>,
    targets: Vec<(String, NodeId, Region)>,
    renders: Mutex<Vec<Value>>,
    idle: Mutex<bool>,
}

impl Describe for Status {
    fn describe(&self) -> Value {
        let headers: Vec<Value> = self
            .targets
            .iter()
            .map(
                |(name, id, region)| match self.engine.status(&self.graph, *id, *region) {
                    Ok(nodes) => json!({ "target": name, "region": region, "nodes": nodes }),
                    Err(e) => json!({ "target": name, "region": region, "error": e.to_string() }),
                },
            )
            .collect();
        json!({
            "idle": *self.idle.lock().expect("status lock"),
            "renders": *self.renders.lock().expect("status lock"),
            "headers": headers,
        })
    }
}

struct Args {
    graph: PathBuf,
    socket: PathBuf,
    layers: Vec<(Layer, PathBuf)>,
    sets: Vec<(String, String)>,
}

fn parse_args() -> Result<Args, String> {
    let mut graph = None;
    let mut socket = None;
    let mut layers = Vec::new();
    let mut sets = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--graph" => graph = Some(PathBuf::from(value()?)),
            "--socket" => socket = Some(PathBuf::from(value()?)),
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
                "helios-node: {e}\nusage: helios-node --graph FILE --socket PATH [--global FILE] [--project DIR] [--user FILE] [--set KEY=VALUE]..."
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
    let engine = Arc::new(Engine::with_budget(bus.clone(), budget));
    let scheduler = Arc::new(Scheduler::new(
        bus.clone(),
        config.number("system.scheduler.workers") as usize,
    ));
    let graph = Arc::new(loaded.graph);
    let status = Arc::new(Status {
        graph: graph.clone(),
        engine: engine.clone(),
        targets: loaded
            .renders
            .iter()
            .map(|r| (r.node.clone(), loaded.nodes[&r.node], r.region))
            .collect(),
        renders: Mutex::new(Vec::new()),
        idle: Mutex::new(false),
    });
    let inspector = Inspector::new(bus.clone())
        .with("graph", graph.clone())
        .with("engine", engine.clone())
        .with("node", status.clone())
        .with("config", config.clone())
        .with("scheduler", scheduler.clone())
        .with_scheduler(scheduler.clone());
    if let Err(e) = serve(inspector, &args.socket) {
        eprintln!(
            "helios-node: cannot serve on {}: {e}",
            args.socket.display()
        );
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
    for r in loaded.renders.iter().cloned() {
        let id = loaded.nodes[&r.node];
        let (engine, graph, status, bus) =
            (engine.clone(), graph.clone(), status.clone(), bus.clone());
        scheduler.submit(&format!("render {}", r.node), move |_| {
            let result = engine.render(&graph, id, &r.port, r.region);
            let (hash, error) = match &result {
                Ok(image) => (Some(image.content_hash().to_string()), None),
                Err(e) => (None, Some(e.to_string())),
            };
            let done = RenderDone {
                node: r.node.clone(),
                port: r.port.clone(),
                region: r.region,
                hash,
                error: error.clone(),
            };
            status
                .renders
                .lock()
                .expect("status lock")
                .push(serde_json::to_value(&done).expect("events serialize"));
            bus.publish(SOURCE, Some(ready), &done);
            error.map_or(Ok(()), Err)
        });
    }
    scheduler.wait_until_idle(std::time::Duration::MAX);
    let failed = status
        .renders
        .lock()
        .expect("status lock")
        .iter()
        .filter(|r| !r["error"].is_null())
        .count();
    *status.idle.lock().expect("status lock") = true;
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
