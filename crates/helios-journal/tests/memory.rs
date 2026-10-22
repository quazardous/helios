//! The in-memory store passes the journal contract.

use helios_core::Describe;
use helios_journal::{Journal, MemoryStore, Node, Outcome, Workflow, contract};

#[test]
fn the_memory_store_passes_the_contract() {
    contract::check(MemoryStore::default);
}

#[test]
fn the_memory_store_passes_the_lasting_contract() {
    contract::check_lasting(MemoryStore::default, MemoryStore::share);
}

#[test]
fn monitoring_counts_rows_per_node() {
    let workflow = Workflow::new(vec![
        Node::new("fetch").retry(2, 5, false),
        Node::new("develop").after(&["fetch"]),
    ])
    .unwrap();
    let j = Journal::new(workflow, MemoryStore::default());
    let lease = j.claim("fetch", 3, &["a", "b", "c"], 10).unwrap();
    j.conclude(
        &helios_journal::Lease {
            items: vec!["a".into()],
            ..lease.clone()
        },
        Outcome::Failed("x".into()),
        12,
    )
    .unwrap();
    let m = j.monitor(20);
    assert_eq!(m["nodes"]["fetch"]["running"], 2);
    assert_eq!(m["nodes"]["fetch"]["scheduled"], 1);
    assert_eq!(m["nodes"]["fetch"]["retries_due"], 1);
    assert_eq!(m["nodes"]["fetch"]["oldest_running_since"], 10);
    assert_eq!(j.describe()["nodes"]["develop"]["done"], 0);
}

#[test]
fn a_workflow_is_checked_when_built() {
    assert!(
        Workflow::new(vec![Node::new("b").after(&["a"]), Node::new("a")]).is_err(),
        "parents first"
    );
    assert!(Workflow::new(vec![Node::new("a"), Node::new("a")]).is_err());
    assert!(Workflow::new(vec![Node::new("a"), Node::new("b").after(&["a"]).need(2)]).is_err());
}

/// Loses every write: the contract must reject it.
#[derive(Default)]
struct Forgetful;

impl helios_journal::Store for Forgetful {
    fn get(&self, _: &str, _: &str) -> Option<helios_journal::Row> {
        None
    }
    fn put(&mut self, _: &str, _: &str, _: helios_journal::Row) {}
    fn remove(&mut self, _: &str, _: &str) {}
    fn rows(&self) -> Vec<(String, String, helios_journal::Row)> {
        Vec::new()
    }
}

#[test]
#[should_panic(expected = "claimed twice")]
fn a_store_that_loses_writes_fails_the_contract() {
    contract::check(Forgetful::default);
}
