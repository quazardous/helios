//! A connected node's jobs in the desktop's jobs centre: relayed from
//! the node's inspect endpoint with ids `node:<name>/<id>`, a batch as a
//! parent with its exports as children, kept consistent when the link
//! drops and comes back, forgotten when the node stays away.
//!
//! Its other events reach the shell bus's listeners by kind, resumed
//! after a reconnection, resynced after a hole (`node_events`); its
//! relayable facts reach the listeners of their point on the desktop's
//! application bus, resynced when the node restarted.
//!
//! The node here is the real protocol over real sockets: a bus whose
//! jobs an inspector shows and streams (as `helios-node` serves them),
//! and a control endpoint answering `job.cancel`.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_pop::api::bus::bus_contract::ListenOpts;
use egui_pop::api::bus::egui_pop_hooks::Kernel;
use egui_pop::api::jobs as centre_jobs;
use egui_pop::api::jobs::JOBS;
use egui_pop::model::jobs::{JobState, Request};
use helios_core::jobs::{Jobs, Progress, Start};
use helios_core::{Bus, BusJournal, Envelope, Event, FactPoint};
use helios_gui::node_events::{Listener, NodeEvent};
use helios_gui::relay::{self, NodeLink, Timing};
use helios_gui::{Shell, headless, helios_panels};
use helios_inspect::control::ControlRequest;
use helios_inspect::{Address, Inspector, Listen};
use helios_node_core::settings::LayerChanged;
use serde_json::json;

const LONG: Duration = Duration::from_secs(20);

/// Attempts close together, so the tests wait little.
const FAST: Timing = Timing {
    retry: Duration::from_millis(20),
    forget_after: Duration::from_mins(10),
    silence: Duration::from_secs(5),
};

/// A node: its bus and jobs, its inspect and control endpoints.
struct Node {
    dir: PathBuf,
    bus: Arc<Bus>,
    jobs: Jobs,
    inspector: Inspector,
    /// Its application bus, when it relays its facts.
    _kernel: Option<Arc<Kernel>>,
}

impl Node {
    fn start(name: &str) -> Node {
        Node::with_journal(name, helios_core::bus::JOURNAL)
    }

    /// A node whose bus keeps `journal` events.
    fn with_journal(name: &str, journal: usize) -> Node {
        Node::serving(name, journal, false)
    }

    /// A node serving its relayable facts, as `helios-node` does: its
    /// application bus has its fact bus as journal and relay.
    fn relaying(name: &str) -> Node {
        Node::serving(name, helios_core::bus::JOURNAL, true)
    }

    fn serving(name: &str, journal: usize, relays: bool) -> Node {
        let dir =
            std::env::temp_dir().join(format!("helios-node-jobs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bus = Arc::new(Bus::with_journal(journal));
        let jobs = Jobs::new(bus.clone());
        let mut inspector = Inspector::new(bus.clone()).with_jobs();
        let kernel = relays.then(|| journaled(bus.clone()));
        if let Some(kernel) = &kernel {
            inspector = inspector.with_relay(kernel.clone());
        }
        helios_inspect::serve(inspector.clone(), &dir.join("node.sock")).unwrap();
        let cancels = jobs.clone();
        helios_inspect::serve_at(
            &Listen::Unix(dir.join("control.sock")),
            None,
            move |request: ControlRequest| match request {
                ControlRequest::JobCancel { id } => match cancels.cancel(&id) {
                    Ok(()) => json!({ "ok": true, "cancelling": id }),
                    Err(e) => json!({ "ok": false, "error": e }),
                },
                _ => json!({ "ok": false, "error": "not served here" }),
            },
        )
        .unwrap();
        Node {
            dir,
            bus,
            jobs,
            inspector,
            _kernel: kernel,
        }
    }

    fn link(&self, name: &str) -> NodeLink {
        NodeLink {
            name: name.into(),
            inspect: Address::Unix(self.dir.join("node.sock")),
            control: Some(Address::Unix(self.dir.join("control.sock"))),
        }
    }

    /// Out of reach: its inspect endpoint can no longer be connected to.
    fn go_away(&self) {
        std::fs::remove_file(self.dir.join("node.sock")).unwrap();
    }

    /// Back, the same process.
    fn come_back(&self) {
        helios_inspect::serve(self.inspector.clone(), &self.dir.join("node.sock")).unwrap();
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Waits for the shell's bus to carry an event `kind` for job `id`.
fn wait_for(events: &Receiver<Envelope>, kind: &str, id: &str) -> Envelope {
    let deadline = Instant::now() + LONG;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(e) if e.kind == kind && e.payload["id"] == id => return e,
            Ok(e) => seen.push(format!("{} {}", e.kind, e.payload["id"])),
            Err(e) => panic!("no {kind} for {id} ({e}); seen: {seen:#?}"),
        }
    }
}

/// An application bus with `facts` as its journal and relay, the
/// desktop's fact kinds declared on it, as `main` builds it.
fn journaled(facts: Arc<Bus>) -> Arc<Kernel> {
    let (kernel, journal) = BusJournal::attach(
        facts,
        |j| Kernel::new().with_journal(j.clone()).with_relay(j),
        Kernel::declaration,
    );
    let refused = journal.declare(&*kernel, helios_gui::FACTS.iter().copied());
    assert!(refused.is_empty(), "{refused:?}");
    kernel
}

fn harness<'a>() -> egui_kittest::Harness<'a, Shell> {
    headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0))
}

fn children(app: &egui_pop::api::AppHandle, id: &str) -> Vec<(String, JobState)> {
    let mut c: Vec<(String, JobState)> = app
        .service(&JOBS)
        .unwrap()
        .children(id)
        .into_iter()
        .map(|j| (j.id, j.state))
        .collect();
    c.sort_by(|a, b| a.0.cmp(&b.0));
    c
}

#[test]
fn a_node_batch_shows_as_a_parent_with_its_children() {
    let node = Node::start("batch");
    let mut h = harness();
    let app = h.state().app().handle().clone();
    let events = h.state().jobs().bus().subscribe();
    h.state_mut()
        .follow_node_with(node.link("box"), FAST)
        .unwrap();

    let batch = node
        .jobs
        .start(Start::new("batch-1", "Batch of 2 images", "batch").total(2));
    let first = node
        .jobs
        .start(Start::new("export-1", "Export a.png", "export").parent("batch-1", 1.0));
    first.progress(Progress::new().step("render"));
    let second = node
        .jobs
        .start(Start::new("export-2", "Export b.png", "export").parent("batch-1", 1.0));
    wait_for(&events, "helios.job.started", "node:box/export-2");
    h.run();

    let centre = app.service(&JOBS).unwrap();
    let parent = centre
        .job("node:box/batch-1")
        .expect("the batch is relayed");
    assert_eq!(parent.title, "Batch of 2 images");
    assert_eq!(parent.kind, "batch");
    assert!(parent.state.active(), "{:?}", parent.state);
    assert_eq!(
        children(&app, "node:box/batch-1"),
        [
            ("node:box/export-1".to_string(), JobState::Running),
            ("node:box/export-2".to_string(), JobState::Running)
        ]
    );
    let roots: Vec<String> = centre.roots().into_iter().map(|j| j.id).collect();
    assert_eq!(roots, ["node:box/batch-1"], "the exports fold under it");

    first.finished();
    batch.progress(Progress::new().counts(1, 2));
    second.finished();
    batch.finished();
    wait_for(&events, "helios.job.finished", "node:box/batch-1");
    h.run();
    assert_eq!(
        centre.job("node:box/batch-1").unwrap().state,
        JobState::Done
    );
    assert_eq!(
        children(&app, "node:box/batch-1"),
        [
            ("node:box/export-1".to_string(), JobState::Done),
            ("node:box/export-2".to_string(), JobState::Done)
        ]
    );
}

#[test]
fn a_reconnection_keeps_the_jobs_consistent() {
    let node = Node::start("reconnect");
    let mut h = harness();
    let app = h.state().app().handle().clone();
    let events = h.state().jobs().bus().subscribe();
    let batch = node
        .jobs
        .start(Start::new("batch-1", "Batch of 2 images", "batch").total(2));
    let first = node
        .jobs
        .start(Start::new("export-1", "Export a.png", "export").parent("batch-1", 1.0));
    // Started before the desktop follows: caught up from the node's jobs.
    h.state_mut()
        .follow_node_with(node.link("box"), FAST)
        .unwrap();
    wait_for(&events, "helios.job.started", "node:box/export-1");
    h.run();

    // The link drops and the node is out of reach: its jobs are lost,
    // not ended.
    node.go_away();
    assert!(h.state().drop_node_link("box"));
    wait_for(&events, "helios.job.paused", "node:box/batch-1");
    wait_for(&events, "helios.job.paused", "node:box/export-1");
    h.run();
    let centre = app.service(&JOBS).unwrap();
    let lost = centre.job("node:box/batch-1").unwrap();
    assert_eq!(lost.state, JobState::Paused);
    assert!(
        centre
            .log_from("node:box/batch-1", 0)
            .iter()
            .any(|l| l.contains("lost: node box is out of reach")),
        "its console says why"
    );

    // Meanwhile on the node: an export ends, another starts.
    first.finished();
    batch.progress(Progress::new().counts(1, 2));
    let second = node
        .jobs
        .start(Start::new("export-2", "Export b.png", "export").parent("batch-1", 1.0));

    // Back: the desktop catches up.
    node.come_back();
    wait_for(&events, "helios.job.started", "node:box/export-2");
    h.run();
    let parent = centre.job("node:box/batch-1").unwrap();
    assert_eq!(parent.state, JobState::Running);
    assert_eq!(parent.done, 1, "its progress while away");
    assert_eq!(
        children(&app, "node:box/batch-1"),
        [
            ("node:box/export-1".to_string(), JobState::Done),
            ("node:box/export-2".to_string(), JobState::Running)
        ],
        "the export that ended away ended as it did"
    );

    // And follows on.
    second.finished();
    batch.finished();
    wait_for(&events, "helios.job.finished", "node:box/batch-1");
    h.run();
    assert_eq!(
        centre.job("node:box/batch-1").unwrap().state,
        JobState::Done
    );
    assert!(centre.idle());
}

#[test]
fn a_node_lost_for_too_long_has_its_jobs_forgotten() {
    let node = Node::start("forget");
    let mut h = harness();
    let app = h.state().app().handle().clone();
    let events = h.state().jobs().bus().subscribe();
    let timing = Timing {
        forget_after: Duration::from_millis(100),
        ..FAST
    };
    h.state_mut()
        .follow_node_with(node.link("box"), timing)
        .unwrap();
    let _batch = node
        .jobs
        .start(Start::new("batch-1", "Batch of 2 images", "batch").total(2));
    let _first = node
        .jobs
        .start(Start::new("export-1", "Export a.png", "export").parent("batch-1", 1.0));
    wait_for(&events, "helios.job.started", "node:box/export-1");
    h.run();
    assert!(
        app.service(&JOBS)
            .unwrap()
            .job("node:box/batch-1")
            .is_some()
    );

    node.go_away();
    h.state().drop_node_link("box");
    let failed = wait_for(&events, "helios.job.failed", "node:box/batch-1");
    assert!(
        failed.payload["error"]
            .as_str()
            .unwrap()
            .contains("out of reach"),
        "{:?}",
        failed.payload
    );
    let deadline = Instant::now() + LONG;
    loop {
        let e = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        if e.kind == relay::FORGOTTEN {
            assert_eq!(e.payload["node"], "box");
            break;
        }
    }
    // A frame folds the jobs' ends, the next at the latest takes them
    // out.
    h.run();
    h.run();
    let centre = app.service(&JOBS).unwrap();
    assert!(centre.job("node:box/batch-1").is_none(), "forgotten");
    assert!(
        centre.job("node:box/export-1").is_none(),
        "with its children"
    );
}

#[test]
fn cancel_from_the_centre_stops_it_on_the_node() {
    let node = Node::start("cancel");
    let mut h = harness();
    let app = h.state().app().handle().clone();
    let events = h.state().jobs().bus().subscribe();
    h.state_mut()
        .follow_node_with(node.link("box"), FAST)
        .unwrap();
    let batch = Arc::new(
        node.jobs
            .start(Start::new("batch-1", "Batch of 2 images", "batch").total(2)),
    );
    let (asked, cancelled) = channel();
    let asked = Mutex::new(asked);
    batch.on_cancel(move || {
        let _ = asked.lock().unwrap().send(());
    });
    wait_for(&events, "helios.job.started", "node:box/batch-1");
    h.run();

    // The jobs centre's Cancel: `job.cancel` sent to the node.
    centre_jobs::ask(&app, "node:box/batch-1", Request::Cancel);
    h.run();
    cancelled
        .recv_timeout(LONG)
        .expect("the node's job was asked to stop");
    wait_for(&events, "helios.job.cancelling", "node:box/batch-1");
    batch.cancelled();
    wait_for(&events, "helios.job.cancelled", "node:box/batch-1");
    h.run();
    let centre = app.service(&JOBS).unwrap();
    let job = centre.job("node:box/batch-1").unwrap();
    assert_eq!(job.state, JobState::Cancelled);
    assert_eq!(job.request_error, None);

    // A job the node no longer runs: refused there, and said so.
    centre_jobs::ask(&app, "node:box/export-9", Request::Cancel);
    h.run();
    assert!(
        node.jobs.cancellable().is_empty(),
        "nothing else was cancelled"
    );
}

/// The next event `listener` hears, or a panic after [`LONG`].
fn next_heard(listener: &Listener) -> NodeEvent {
    listener
        .recv_timeout(LONG)
        .expect("a node event within the deadline")
}

#[test]
fn a_node_s_events_reach_listeners_resumed_and_resynced() {
    let node = Node::with_journal("events", 16);
    let mut h = harness();
    let shell_events = h.state().jobs().bus().subscribe();
    let heard = h
        .state()
        .node_events()
        .listen("test", &["storage.*", "import.*"]);
    h.state_mut()
        .follow_node_with(node.link("box"), FAST)
        .unwrap();
    // Followed once a job of the node shows.
    let job = node.jobs.start(Start::new("scan-1", "Scan", "scan"));
    wait_for(&shell_events, "helios.job.started", "node:box/scan-1");
    let says = |kind: &'static str, n: u64, cause: Option<u64>| {
        node.bus
            .publish_value("test", cause, kind, json!({ "n": n }))
    };

    // Several kinds: the ones asked for, in order, with their node and
    // cause; the others not.
    says("other.thing", 0, None);
    let asked = says("storage.changed", 1, None);
    says("import.done", 2, Some(asked));
    let e = next_heard(&heard);
    assert_eq!((e.node.as_str(), e.event.kind), ("box", "storage.changed"));
    assert_eq!(e.event.payload["n"], 1);
    let e = next_heard(&heard);
    assert_eq!(e.event.kind, "import.done");
    assert_eq!(e.event.cause, Some(asked), "the round trip's cause is kept");

    // A reconnection resumes where it left off: what was said while away
    // comes once, nothing comes twice, no resync.
    node.go_away();
    assert!(h.state().drop_node_link("box"));
    says("storage.changed", 3, None);
    node.come_back();
    let e = next_heard(&heard);
    assert!(!e.is_resync(), "resumed, not resynced: {e:?}");
    assert_eq!(e.event.payload["n"], 3);
    says("storage.changed", 4, None);
    assert_eq!(next_heard(&heard).event.payload["n"], 4);

    // A hole the node's journal no longer holds: a resync, then on.
    node.go_away();
    assert!(h.state().drop_node_link("box"));
    for n in 10..50 {
        says("storage.changed", n, None);
    }
    node.come_back();
    let e = next_heard(&heard);
    assert!(e.is_resync(), "a hole says resync: {e:?}");
    assert_eq!(e.node, "box");
    says("storage.changed", 100, None);
    assert_eq!(next_heard(&heard).event.payload["n"], 100);
    job.finished();
}

#[test]
fn a_node_s_relayable_facts_reach_the_desktop_s_listeners_and_a_restart_resyncs() {
    let node = Node::relaying("relay");
    // The desktop's application bus, as `main` builds it, and a listener
    // of a relayable kind on it.
    let desktop = journaled(helios_gui::bus());
    let (heard_tx, heard) = channel();
    let heard_tx = Mutex::new(heard_tx);
    desktop
        .listen::<FactPoint<LayerChanged>>("test", ListenOpts::new(), move |fact, ()| {
            let n = fact.payload["n"].as_u64().unwrap_or(0);
            let _ = heard_tx.lock().unwrap().send((fact.source.clone(), n));
            Ok(())
        })
        .unwrap();
    let shell = Shell::builder(helios_panels(), None)
        .bus(desktop.clone())
        .build();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    let resyncs = h.state().node_events().listen("test", &["nothing.*"]);
    h.state_mut()
        .follow_node_with(node.link("box"), FAST)
        .unwrap();
    let layer = |node: &Node, n: u64| {
        node.bus.publish_value(
            "settings",
            None,
            LayerChanged::KIND,
            json!({ "layer": "user", "n": n }),
        )
    };
    // The link starts from the node's present: said until it is up.
    let deadline = Instant::now() + LONG;
    let first = loop {
        layer(&node, 1);
        if let Ok(got) = heard.recv_timeout(Duration::from_millis(50)) {
            break got;
        }
        assert!(Instant::now() < deadline, "no relayed fact within {LONG:?}");
    };
    assert_eq!(first, ("relayed:box".to_string(), 1));
    while heard.try_recv().is_ok() {}
    layer(&node, 2);
    assert_eq!(heard.recv_timeout(LONG).unwrap().1, 2, "taken in once");

    // The node restarts: another process at the same address, its
    // numbers from 1 again. The desktop resyncs, then hears it.
    node.go_away();
    assert!(h.state().drop_node_link("box"));
    drop(node);
    let node = Node::relaying("relay");
    let deadline = Instant::now() + LONG;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let e = resyncs
            .recv_timeout(left)
            .expect("a resync after the node restarted");
        if e.is_resync() && e.event.payload["reason"] == "the node restarted" {
            break;
        }
    }
    layer(&node, 3);
    let deadline = Instant::now() + LONG;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let (_, n) = heard
            .recv_timeout(left)
            .expect("the restarted node's fact is heard");
        if n == 3 {
            break;
        }
        assert_eq!(n, 2, "only what the old run said may come before");
    }
}

#[test]
fn a_relayed_fact_lands_once_and_a_restart_resyncs_once() {
    let node = Node::relaying("once");
    let facts = helios_gui::bus();
    let desktop = journaled(facts.clone());
    let (point_tx, by_point) = channel();
    let point_tx = Mutex::new(point_tx);
    desktop
        .listen::<FactPoint<LayerChanged>>("test", ListenOpts::new(), move |fact, ()| {
            let n = fact.payload["n"].as_u64().unwrap_or(0);
            let _ = point_tx.lock().unwrap().send(n);
            Ok(())
        })
        .unwrap();
    let shell = Shell::builder(helios_panels(), None)
        .bus(desktop.clone())
        .build();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    let shell_bus = h.state().jobs().bus().subscribe();
    let by_table = h
        .state()
        .node_events()
        .listen("test", &["helios.settings.*"]);
    h.state_mut()
        .follow_node_with(node.link("box"), FAST)
        .unwrap();
    let layer = |node: &Node, n: u64| {
        node.bus.publish_value(
            "settings",
            None,
            LayerChanged::KIND,
            json!({ "layer": "user", "n": n }),
        )
    };
    // Said until both links are up: the relay link takes it in.
    let deadline = Instant::now() + LONG;
    while by_point.recv_timeout(Duration::from_millis(50)).is_err() {
        layer(&node, 1);
        assert!(Instant::now() < deadline, "no relayed fact within {LONG:?}");
    }
    wait_for_link(&shell_bus);

    // One fact: on the desktop's fact bus once, heard once by the point's
    // listener and once by the node table's, never copied on the shell
    // bus by the event stream.
    layer(&node, 2);
    assert_eq!(heard_n(&by_point, 2), 1, "the point's listener");
    let table: Vec<NodeEvent> = std::iter::from_fn(|| by_table.recv_timeout(QUIET).ok())
        .filter(|e| e.event.payload["n"] == 2)
        .collect();
    assert_eq!(table.len(), 1, "the node table's listener: {table:#?}");
    assert_eq!(table[0].node, "box");
    assert_eq!(table[0].event.source, "relayed:box");
    let landed = facts
        .since(0)
        .iter()
        .filter(|e| e.kind == LayerChanged::KIND && e.payload["n"] == 2)
        .count();
    assert_eq!(landed, 1, "on the desktop's fact bus once");
    let copied: Vec<Envelope> = shell_bus
        .try_iter()
        .filter(|e| e.kind == LayerChanged::KIND && e.payload["event"]["payload"]["n"] == 2)
        .collect();
    assert!(copied.is_empty(), "copied on the shell bus: {copied:#?}");

    // The node restarts: one resync, not one per link.
    node.go_away();
    assert!(h.state().drop_node_link("box"));
    drop(node);
    let _node = Node::relaying("once");
    let deadline = Instant::now() + LONG;
    let first = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let e = by_table
            .recv_timeout(left)
            .expect("a resync after the node restarted");
        if e.is_resync() {
            break e;
        }
    };
    let more: Vec<NodeEvent> = std::iter::from_fn(|| by_table.recv_timeout(QUIET).ok())
        .filter(NodeEvent::is_resync)
        .collect();
    assert!(
        more.is_empty(),
        "one resync per hole: {first:?} then {more:#?}"
    );
}

/// A node of 0.92 or before, which names its fact kinds without their
/// `helios.` prefix: a proxy in front of `node`'s inspect endpoint that
/// drops the `prefixed` each request of the desktop says, so the node
/// writes every kind under its old name, as such a node does
/// (`helios_core::legacy_kinds`). It keeps the kinds it hands on.
struct OldNode {
    path: PathBuf,
    kinds: Arc<Mutex<Vec<String>>>,
}

impl OldNode {
    fn in_front_of(node: &Node) -> OldNode {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::{UnixListener, UnixStream};
        let path = node.dir.join("old.sock");
        let upstream = node.dir.join("node.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let kinds: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = kinds.clone();
        std::thread::spawn(move || {
            for desktop in listener.incoming() {
                let Ok(desktop) = desktop else { return };
                let Ok(node) = UnixStream::connect(&upstream) else {
                    continue;
                };
                let (mut to_node, from_node) = (node.try_clone().unwrap(), node);
                let mut to_desktop = desktop.try_clone().unwrap();
                std::thread::spawn(move || {
                    for line in BufReader::new(desktop).lines() {
                        let Ok(line) = line else { break };
                        let mut request: serde_json::Value =
                            serde_json::from_str(&line).unwrap_or_default();
                        if let Some(r) = request.as_object_mut() {
                            r.remove("prefixed");
                        }
                        if writeln!(to_node, "{request}").is_err() {
                            break;
                        }
                    }
                    let _ = to_node.shutdown(std::net::Shutdown::Both);
                });
                let seen = seen.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(from_node).lines() {
                        let Ok(line) = line else { break };
                        let value: serde_json::Value =
                            serde_json::from_str(&line).unwrap_or_default();
                        for at in ["event", "relayed"] {
                            if let Some(kind) = value[at]["kind"].as_str() {
                                seen.lock().unwrap().push(kind.to_string());
                            }
                        }
                        if writeln!(to_desktop, "{line}").is_err() {
                            break;
                        }
                    }
                    let _ = to_desktop.shutdown(std::net::Shutdown::Both);
                });
            }
        });
        OldNode { path, kinds }
    }

    fn link(&self, name: &str) -> NodeLink {
        NodeLink {
            name: name.into(),
            inspect: Address::Unix(self.path.clone()),
            control: None,
        }
    }
}

/// A node of an older release says its facts without their prefix
/// (`settings.layerChanged`, `job.started`): the desktop takes each in
/// under its new name, once, and no bus holds the old one.
#[test]
fn an_older_node_s_fact_lands_under_its_new_name_once() {
    let node = Node::relaying("legacy");
    let old = OldNode::in_front_of(&node);
    let facts = helios_gui::bus();
    let desktop = journaled(facts.clone());
    let (point_tx, by_point) = channel();
    let point_tx = Mutex::new(point_tx);
    desktop
        .listen::<FactPoint<LayerChanged>>("test", ListenOpts::new(), move |fact, ()| {
            let n = fact.payload["n"].as_u64().unwrap_or(0);
            let _ = point_tx.lock().unwrap().send(n);
            Ok(())
        })
        .unwrap();
    let shell = Shell::builder(helios_panels(), None)
        .bus(desktop.clone())
        .build();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    let shell_bus = h.state().jobs().bus().subscribe();
    let by_table = h
        .state()
        .node_events()
        .listen("test", &["helios.settings.*"]);
    h.state_mut()
        .follow_node_with(old.link("box"), FAST)
        .unwrap();
    let layer = |n: u64| {
        node.bus.publish_value(
            "settings",
            None,
            LayerChanged::KIND,
            json!({ "layer": "user", "n": n }),
        )
    };
    let deadline = Instant::now() + LONG;
    while by_point.recv_timeout(Duration::from_millis(50)).is_err() {
        layer(1);
        assert!(Instant::now() < deadline, "no relayed fact within {LONG:?}");
    }
    wait_for_link(&shell_bus);

    layer(2);
    assert_eq!(heard_n(&by_point, 2), 1, "the point's listener, once");
    let table: Vec<NodeEvent> = std::iter::from_fn(|| by_table.recv_timeout(QUIET).ok())
        .filter(|e| e.event.payload["n"] == 2)
        .collect();
    assert_eq!(table.len(), 1, "the node table's listener: {table:#?}");
    assert_eq!(table[0].event.kind, LayerChanged::KIND);
    let landed: Vec<&str> = facts
        .since(0)
        .iter()
        .filter(|e| e.payload["n"] == 2)
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        landed,
        [LayerChanged::KIND],
        "on the desktop's fact bus once"
    );

    // A job's events, by the event stream: under their new names too.
    let job = node.jobs.start(Start::new("legacy-1", "Scan", "scan"));
    wait_for(&shell_bus, helios_core::jobs::STARTED, "node:box/legacy-1");
    job.finished();
    wait_for(&shell_bus, helios_core::jobs::FINISHED, "node:box/legacy-1");

    // The node did say them by their old names.
    let said = old.kinds.lock().unwrap().clone();
    assert!(
        said.iter().any(|k| k == "settings.layerChanged"),
        "{said:?}"
    );
    assert!(said.iter().any(|k| k == "job.started"), "{said:?}");
    assert!(!said.iter().any(|k| k.starts_with("helios.")), "{said:?}");
}

/// How long a test waits to be sure nothing more comes.
const QUIET: Duration = Duration::from_millis(500);

/// How many times `n` comes on `heard`: the first awaited at most
/// [`LONG`], then counted until nothing comes for [`QUIET`].
fn heard_n(heard: &Receiver<u64>, n: u64) -> usize {
    let deadline = Instant::now() + LONG;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match heard.recv_timeout(left) {
            Ok(got) if got == n => break,
            Ok(_) => {}
            Err(_) => return 0,
        }
    }
    1 + std::iter::from_fn(|| heard.recv_timeout(QUIET).ok())
        .filter(|got| *got == n)
        .count()
}

/// Waits for the shell's bus to say the node's link is up.
fn wait_for_link(events: &Receiver<Envelope>) {
    let deadline = Instant::now() + LONG;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(e) if e.kind == relay::LINK && e.payload["up"] == true => return,
            Ok(_) => {}
            Err(e) => panic!("the node's link did not come up: {e}"),
        }
    }
}
