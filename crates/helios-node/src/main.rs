//! `helios-node`: a headless Helios node.
//!
//! Loads a network from a JSON description, serves the read-only inspection
//! endpoint on a socket, prints `ready <socket>` on stdout, renders what the
//! description asks for, then stays up so it can be inspected. Stop it with
//! a signal.
//!
//!     helios-node --graph network.json --socket /path/to/node.sock

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use helios_core::{Bus, Describe, Engine, Event, Graph, NodeId, Region};
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
}

fn parse_args() -> Result<Args, String> {
    let mut graph = None;
    let mut socket = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--graph" => graph = args.next().map(PathBuf::from),
            "--socket" => socket = args.next().map(PathBuf::from),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Args {
        graph: graph.ok_or("missing --graph FILE")?,
        socket: socket.ok_or("missing --socket PATH")?,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("helios-node: {e}\nusage: helios-node --graph FILE --socket PATH");
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

    let bus = Arc::new(Bus::new());
    let engine = Arc::new(Engine::new(bus.clone()));
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
        .with("node", status.clone());
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

    let mut failed = 0;
    for r in &loaded.renders {
        let id = loaded.nodes[&r.node];
        let result = engine.render(&graph, id, &r.port, r.region);
        let (hash, error) = match &result {
            Ok(image) => (Some(image.content_hash().to_string()), None),
            Err(e) => {
                failed += 1;
                (None, Some(e.to_string()))
            }
        };
        let done = RenderDone {
            node: r.node.clone(),
            port: r.port.clone(),
            region: r.region,
            hash,
            error,
        };
        status
            .renders
            .lock()
            .expect("status lock")
            .push(serde_json::to_value(&done).expect("events serialize"));
        bus.publish(SOURCE, Some(ready), &done);
    }
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
