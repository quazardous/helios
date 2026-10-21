//! The Queue panel: it follows the selected node and lists its work items
//! (the jobs that name it, a connected node's included) with their state;
//! Cancel goes through `job.cancel`; empty, it shows an empty state, or
//! closes when `interface.panels.queue_when_empty` is off.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_pop::model::ids::PanelId;
use helios_core::Value;
use helios_core::jobs::{Progress, Start};
use helios_gui::queue::{Action, ItemState, PANEL, Place};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_values::{Change, Set};
use serde_json::json;

fn harness(when_empty: bool) -> Harness<'static, Shell> {
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_queue_when_empty(when_empty);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

/// Selects a node of the graph, as the graph editor does.
fn select(h: &mut Harness<'_, Shell>, node: &str) {
    h.state()
        .values
        .lock()
        .unwrap()
        .set(Set::new(
            helios_gui::params::SELECTION,
            Change::To(Value::Text(node.into())),
            Origin::User,
        ))
        .unwrap();
    h.run();
}

/// Brings the Queue panel to the front of its area.
fn show_queue(h: &mut Harness<'_, Shell>) {
    h.state_mut()
        .run("helios.queue.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
}

fn visible(h: &Harness<'_, Shell>) -> bool {
    h.state()
        .app()
        .handle()
        .panel_visible(&PanelId::new(PANEL).unwrap())
}

/// The ids and states of the followed node's items.
fn items(h: &Harness<'_, Shell>) -> Vec<(String, ItemState)> {
    h.state()
        .queue_panel()
        .queue()
        .map(|q| q.items.iter().map(|i| (i.id.clone(), i.state)).collect())
        .unwrap_or_default()
}

#[test]
fn the_selected_node_s_jobs_are_its_queue_and_cancel_goes_through_the_jobs() {
    let mut h = harness(true);
    let jobs = h.state().jobs();
    // Two exports of `grade`: one waiting, one rendering; one of another
    // node, one of no node.
    let waiting = jobs.start(
        Start::new("export-a", "Export a.png", "export")
            .total(1)
            .node("grade"),
    );
    let asked = Arc::new(AtomicBool::new(false));
    {
        let asked = asked.clone();
        waiting.on_cancel(move || asked.store(true, Ordering::SeqCst));
    }
    let running = jobs.start(
        Start::new("export-b", "Export b.png", "export")
            .total(1)
            .node("grade"),
    );
    running.progress(Progress::new().step("render"));
    let _other = jobs.start(Start::new("export-c", "Export c.png", "export").node("sigmoid"));
    let _none = jobs.start(Start::new("import-1", "Import 3 photos", "import"));
    select(&mut h, "grade");
    show_queue(&mut h);

    assert_eq!(h.state().queue_panel().node(), Some("grade"));
    assert_eq!(
        items(&h),
        [
            ("export-a".to_string(), ItemState::Waiting),
            ("export-b".to_string(), ItemState::Running),
        ]
    );
    {
        let panel = h.state().queue_panel();
        let queue = panel.queue().unwrap();
        let summary = queue.summary();
        assert_eq!((summary.waiting, summary.running), (1, 1));
        assert!(summary.oldest_wait.is_some());
        assert!(queue.items.iter().all(|i| i.place == Place::Here));
        assert_eq!(queue.items[0].attempts, 1);
        assert_eq!(queue.items[0].actions, [Action::Cancel]);
    }
    h.get_by_label("Queue of grade");
    h.get_by_label("1 waiting, 1 running, 0 done, 0 failed");

    // Cancel, from the panel: `job.cancel`, as the jobs centre asks it.
    h.get_by_label("Cancel Export a.png").click();
    h.run();
    assert!(asked.load(Ordering::SeqCst), "the job was asked to stop");
    waiting.cancelled();
    running.finished();
    h.run();
    assert_eq!(
        items(&h),
        [
            ("export-a".to_string(), ItemState::Cancelled),
            ("export-b".to_string(), ItemState::Done),
        ]
    );
    assert!(h.query_by_label("Cancel Export a.png").is_none());

    // Another node selected: its own queue.
    select(&mut h, "sigmoid");
    assert_eq!(items(&h), [("export-c".to_string(), ItemState::Waiting)]);
}

#[test]
fn a_connected_node_s_jobs_say_where_they_run() {
    let mut h = harness(true);
    let bus = h.state().jobs().bus().clone();
    // As a relay republishes a node's export.
    bus.publish_value(
        "relay",
        None,
        helios_core::jobs::STARTED,
        json!({ "id": "node:n1/export-1", "title": "Export d.png", "kind": "export",
                "total": 1, "node": "grade" }),
    );
    bus.publish_value(
        "relay",
        None,
        helios_core::jobs::PROGRESS,
        json!({ "id": "node:n1/export-1", "step": "render" }),
    );
    select(&mut h, "grade");
    show_queue(&mut h);
    let panel = h.state().queue_panel();
    let queue = panel.queue().expect("the node's queue");
    assert_eq!(queue.items.len(), 1);
    assert_eq!(queue.items[0].state, ItemState::Running);
    assert_eq!(queue.items[0].place, Place::Node("n1".into()));
    drop(panel);
    h.get_by_label("Node n1");
}

#[test]
fn an_empty_queue_shows_an_empty_state_by_default() {
    let mut h = harness(true);
    show_queue(&mut h);
    assert!(visible(&h));
    h.get_by_label("Select a node to see its queue");
    select(&mut h, "grade");
    assert!(h.state().queue_panel().is_empty());
    assert!(visible(&h), "shown, empty, as the setting is on");
    h.get_by_label("Nothing queued at grade");
}

#[test]
fn an_empty_queue_closes_the_panel_when_the_setting_is_off() {
    let mut h = harness(false);
    assert!(!visible(&h), "nothing selected: closed");
    select(&mut h, "grade");
    assert!(!visible(&h), "nothing queued at the node: still closed");
    // Work at the node opens it again.
    let job = h.state().jobs().start(
        Start::new("export-a", "Export a.png", "export")
            .total(1)
            .node("grade"),
    );
    h.run();
    assert!(visible(&h), "the node has work: shown");
    assert_eq!(items(&h).len(), 1);
    // Another node, without work: closed again.
    select(&mut h, "sigmoid");
    assert!(!visible(&h));
    job.finished();
}
