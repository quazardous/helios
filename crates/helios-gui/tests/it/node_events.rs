//! The desktop's own node, in process, publishes on the fact bus that is
//! the application bus's journal: the node listeners that follow it are
//! named listeners of the application bus's points, each hearing a fact
//! once, with no relayed copy (`helios_gui::node_events`).

use std::sync::Arc;
use std::time::Duration;

use egui_pop::api::bus::bus_contract;
use helios_gui::node_events::{LOCAL, NodeEvent};
use helios_gui::{Shell, helios_panels};
use helios_node_core::settings::LayerChanged;

/// The events a listener heard within a while, the first waited for.
fn heard(rx: &std::sync::mpsc::Receiver<NodeEvent>) -> Vec<NodeEvent> {
    let mut all = Vec::new();
    if let Ok(first) = rx.recv_timeout(Duration::from_secs(10)) {
        all.push(first);
    }
    std::thread::sleep(Duration::from_millis(300));
    all.extend(rx.try_iter());
    all
}

#[test]
fn a_local_node_s_fact_is_heard_once_by_a_named_listener_of_its_point() {
    let (bus, facts) = super::conformance::journaled();
    let shell = Shell::builder(helios_panels(), None)
        .bus(bus.clone())
        .build();
    let events = shell.node_events().clone();
    // A listener of the table, as `reads` is: a remote node's facts come
    // through the table, the local node's through the point's listener.
    let table = events.listen("test", &["settings.*"]);
    let (tx, rx) = std::sync::mpsc::channel();
    let hand = table.hand();
    let refused = events.listen_local(
        &*bus,
        "test.local",
        "test",
        &["settings.*"],
        helios_gui::FACTS.iter().copied(),
        move |e| {
            tx.send(e.clone()).unwrap();
            hand(e);
        },
    );
    assert!(refused.is_empty(), "{refused:?}");
    let order = bus_contract::order_dump(&*bus, &["settings.layerChanged"]);
    assert!(order.contains("test.local"), "{order}");

    facts.publish(
        "node.settings",
        None,
        &LayerChanged {
            layer: "user".into(),
        },
    );
    let by_point = heard(&rx);
    assert_eq!(by_point.len(), 1, "{by_point:?}");
    assert_eq!(by_point[0].node, LOCAL);
    assert_eq!(by_point[0].event.kind, "settings.layerChanged");
    assert_eq!(by_point[0].event.payload["layer"], "user");
    // The table's listener, once too: no second path copies it.
    let mut by_table = Vec::new();
    if let Ok(e) = table.recv_timeout(Duration::from_secs(10)) {
        by_table.push(e);
    }
    std::thread::sleep(Duration::from_millis(300));
    by_table.extend(table.try_iter());
    assert_eq!(by_table.len(), 1, "{by_table:?}");
    drop(shell);
}

#[test]
fn the_desktop_s_node_listeners_are_named_on_the_application_bus() {
    let (bus, _facts) = super::conformance::journaled();
    let mut shell = Shell::builder(helios_panels(), None)
        .bus(bus.clone())
        .build();
    shell.follow_import_sources(|| Ok(None));
    shell.watch_cards(Arc::new(Vec::<helios_storage::Card>::new()));
    let points = [
        "settings.layerChanged",
        "library.imported",
        "volume.mounted",
        "volume.unmounted",
    ];
    let order = bus_contract::order_dump(&*bus, &points);
    for listener in ["helios.import-sources", "helios.reads", "import.cards"] {
        assert!(order.contains(listener), "{listener} missing:\n{order}");
    }
    drop(shell);
}
