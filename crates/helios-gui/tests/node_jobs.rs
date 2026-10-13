//! A connected node's jobs in the desktop's jobs centre: relayed from
//! the node's inspect endpoint with ids `node:<name>/<id>`, a batch as a
//! parent with its exports as children, kept consistent when the link
//! drops and comes back, forgotten when the node stays away.
//!
//! The node here is the real protocol over real sockets: a bus whose
//! jobs an inspector shows and streams (as `helios-node` serves them),
//! and a control endpoint answering `job.cancel`.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::vec2;
use egui_pop::api::jobs::JOBS;
use egui_pop::model::jobs::JobState;
use helios_core::jobs::{Jobs, Progress, Start};
use helios_core::{Bus, Envelope};
use helios_gui::relay::{self, NodeLink, Timing};
use helios_gui::{Shell, headless, helios_panels};
use helios_inspect::control::ControlRequest;
use helios_inspect::{Address, Inspector, Listen};
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
    jobs: Jobs,
    inspector: Inspector,
}

impl Node {
    fn start(name: &str) -> Node {
        let dir =
            std::env::temp_dir().join(format!("helios-node-jobs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bus = Arc::new(Bus::new());
        let jobs = Jobs::new(bus.clone());
        let inspector = Inspector::new(bus).with_jobs();
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
            jobs,
            inspector,
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
    wait_for(&events, "job.started", "node:box/export-2");
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
    wait_for(&events, "job.finished", "node:box/batch-1");
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
    wait_for(&events, "job.started", "node:box/export-1");
    h.run();

    // The link drops and the node is out of reach: its jobs are lost,
    // not ended.
    node.go_away();
    assert!(h.state().drop_node_link("box"));
    wait_for(&events, "job.paused", "node:box/batch-1");
    wait_for(&events, "job.paused", "node:box/export-1");
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
    wait_for(&events, "job.started", "node:box/export-2");
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
    wait_for(&events, "job.finished", "node:box/batch-1");
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
    wait_for(&events, "job.started", "node:box/export-1");
    h.run();
    assert!(
        app.service(&JOBS)
            .unwrap()
            .job("node:box/batch-1")
            .is_some()
    );

    node.go_away();
    h.state().drop_node_link("box");
    let failed = wait_for(&events, "job.failed", "node:box/batch-1");
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
