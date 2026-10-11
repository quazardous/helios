//! The contract every [`Store`] must pass, after grampy's driver contract:
//! the journal's rules hold whatever keeps the rows.
//!
//! ```ignore
//! helios_journal::contract::check(MemoryStore::default);
//! ```

use std::collections::BTreeSet;
use std::sync::Arc;
use std::thread;

use crate::{Error, Journal, Node, Outcome, Status, Store, Workflow};

/// Runs every scenario against fresh stores made by `make`; panics naming
/// the first scenario that fails.
pub fn check<S: Store + 'static>(make: impl Fn() -> S) {
    a_child_waits_for_its_parent(journal(&make, linear()));
    need_k_of_n(journal(&make, need_one_of_two()));
    a_failed_parent_satisfies_nobody(journal(&make, linear()));
    a_failure_is_retried_when_due_then_stands(journal(&make, retried()));
    a_slow_worker_whose_lease_expired_writes_nothing(journal(&make, linear()));
    a_choice_omits_the_other_branches_and_what_only_they_lead_to(journal(&make, choice()));
    concurrent_claimers_never_take_an_item_twice(Arc::new(journal(&make, linear())));
    unknown_nodes_and_bad_branches_are_refused(journal(&make, choice()));
}

fn journal<S: Store>(make: &impl Fn() -> S, workflow: Workflow) -> Journal<S> {
    Journal::new(workflow, make())
}

fn linear() -> Workflow {
    Workflow::new(vec![
        Node::new("fetch").lease(100),
        Node::new("develop").after(&["fetch"]),
    ])
    .unwrap()
}

fn need_one_of_two() -> Workflow {
    Workflow::new(vec![
        Node::new("a"),
        Node::new("b"),
        Node::new("first").after(&["a", "b"]).need(1),
    ])
    .unwrap()
}

fn retried() -> Workflow {
    Workflow::new(vec![Node::new("call").retry(3, 10, true)]).unwrap()
}

/// cull (choice) → keep | reject; keep → edit; edit and reject → archive.
fn choice() -> Workflow {
    Workflow::new(vec![
        Node::new("cull").choice(),
        Node::new("keep").after(&["cull"]),
        Node::new("reject").after(&["cull"]),
        Node::new("edit").after(&["keep"]),
        Node::new("archive").after(&["edit", "reject"]),
    ])
    .unwrap()
}

fn a_child_waits_for_its_parent<S: Store>(j: Journal<S>) {
    assert!(
        j.claim("develop", 10, &["i1"], 0).unwrap().items.is_empty(),
        "develop before fetch"
    );
    let lease = j.claim("fetch", 10, &["i1", "i2"], 0).unwrap();
    assert_eq!(lease.items, ["i1", "i2"]);
    assert!(
        j.claim("fetch", 10, &["i1"], 0).unwrap().items.is_empty(),
        "claimed twice"
    );
    j.conclude(&lease, Outcome::Done, 1).unwrap();
    assert_eq!(j.claim("develop", 10, &["i1"], 2).unwrap().items, ["i1"]);
}

fn need_k_of_n<S: Store>(j: Journal<S>) {
    let a = j.claim("a", 1, &["i"], 0).unwrap();
    assert!(j.claim("first", 1, &["i"], 0).unwrap().items.is_empty());
    j.conclude(&a, Outcome::Skipped, 1).unwrap();
    assert_eq!(
        j.claim("first", 1, &["i"], 1).unwrap().items,
        ["i"],
        "one of two is enough"
    );
}

fn a_failed_parent_satisfies_nobody<S: Store>(j: Journal<S>) {
    let lease = j.claim("fetch", 1, &["i"], 0).unwrap();
    j.conclude(&lease, Outcome::Failed("404".into()), 1)
        .unwrap();
    assert_eq!(
        j.status("i", "fetch"),
        Some(Status::Failed),
        "no retry declared: it stands"
    );
    assert!(j.claim("develop", 1, &["i"], 2).unwrap().items.is_empty());
    assert_eq!(j.row("i", "fetch").unwrap().error.as_deref(), Some("404"));
}

fn a_failure_is_retried_when_due_then_stands<S: Store>(j: Journal<S>) {
    let fail = |now: u64| {
        let lease = j.claim("call", 1, &["i"], now).unwrap();
        assert_eq!(lease.items, ["i"], "claimable at {now}");
        j.conclude(&lease, Outcome::Failed("timeout".into()), now)
            .unwrap();
    };
    fail(0);
    assert_eq!(j.status("i", "call"), Some(Status::Scheduled));
    assert!(
        j.claim("call", 1, &["i"], 9).unwrap().items.is_empty(),
        "first retry due at 10"
    );
    fail(10);
    assert!(
        j.claim("call", 1, &["i"], 29).unwrap().items.is_empty(),
        "second retry due at 30 (doubled)"
    );
    fail(30);
    let row = j.row("i", "call").unwrap();
    assert_eq!(
        (row.status, row.attempts),
        (Status::Failed, 3),
        "past the limit it stands"
    );
    assert!(j.claim("call", 1, &["i"], 1_000).unwrap().items.is_empty());
}

fn a_slow_worker_whose_lease_expired_writes_nothing<S: Store>(j: Journal<S>) {
    let slow = j.claim("fetch", 1, &["i"], 0).unwrap();
    assert_eq!(j.expire(99), 0, "not yet");
    assert_eq!(j.expire(100), 1);
    let fresh = j.claim("fetch", 1, &["i"], 100).unwrap();
    assert_eq!(fresh.items, ["i"]);
    assert_eq!(
        j.row("i", "fetch").unwrap().attempts,
        1,
        "an expired lease is not an attempt"
    );
    let err = j.conclude(&slow, Outcome::Done, 101).unwrap_err();
    assert_eq!(
        err,
        Error::StaleLease {
            node: "fetch".into(),
            item: "i".into()
        }
    );
    assert_eq!(
        j.status("i", "fetch"),
        Some(Status::Running),
        "the stale conclude wrote nothing"
    );
    j.conclude(&fresh, Outcome::Done, 102).unwrap();
}

fn a_choice_omits_the_other_branches_and_what_only_they_lead_to<S: Store>(j: Journal<S>) {
    let cull = j.claim("cull", 2, &["kept", "dropped"], 0).unwrap();
    j.conclude(
        &Lease1::only(&cull, "kept"),
        Outcome::Branch("keep".into()),
        1,
    )
    .unwrap();
    j.conclude(
        &Lease1::only(&cull, "dropped"),
        Outcome::Branch("reject".into()),
        1,
    )
    .unwrap();
    assert_eq!(j.status("kept", "reject"), Some(Status::Omitted));
    assert_eq!(j.status("dropped", "keep"), Some(Status::Omitted));
    assert_eq!(
        j.status("dropped", "edit"),
        Some(Status::Omitted),
        "only `keep` led to `edit`"
    );
    assert_eq!(
        j.status("dropped", "archive"),
        None,
        "`reject` still leads to `archive`"
    );
    // `archive` joins `edit` and `reject`: omitted satisfies a join.
    let reject = j.claim("reject", 1, &["dropped"], 2).unwrap();
    j.conclude(&reject, Outcome::Done, 3).unwrap();
    assert_eq!(
        j.claim("archive", 2, &["kept", "dropped"], 3)
            .unwrap()
            .items,
        ["dropped"]
    );
}

fn concurrent_claimers_never_take_an_item_twice<S: Store + 'static>(j: Arc<Journal<S>>) {
    let items: Vec<String> = (0..200).map(|i| format!("i{i}")).collect();
    let taken: Vec<Vec<String>> = thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (j, items) = (&j, &items);
                s.spawn(move || {
                    let candidates: Vec<&str> = items.iter().map(String::as_str).collect();
                    let mut mine = Vec::new();
                    loop {
                        let lease = j.claim("fetch", 7, &candidates, 0).unwrap();
                        if lease.items.is_empty() {
                            return mine;
                        }
                        mine.extend(lease.items);
                    }
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let all: Vec<&String> = taken.iter().flatten().collect();
    let unique: BTreeSet<&String> = all.iter().copied().collect();
    assert_eq!(
        (all.len(), unique.len()),
        (200, 200),
        "every item once, none twice"
    );
}

fn unknown_nodes_and_bad_branches_are_refused<S: Store>(j: Journal<S>) {
    assert_eq!(
        j.claim("nope", 1, &["i"], 0).unwrap_err(),
        Error::UnknownNode("nope".into())
    );
    let cull = j.claim("cull", 1, &["i"], 0).unwrap();
    assert_eq!(
        j.conclude(&cull, Outcome::Branch("edit".into()), 1)
            .unwrap_err(),
        Error::UnknownBranch {
            node: "cull".into(),
            branch: "edit".into()
        },
        "`edit` is not a child of `cull`"
    );
    assert_eq!(
        j.status("i", "cull"),
        Some(Status::Running),
        "a refused conclude writes nothing"
    );
}

/// A lease narrowed to one of its items.
struct Lease1;

impl Lease1 {
    fn only(lease: &crate::Lease, item: &str) -> crate::Lease {
        crate::Lease {
            items: vec![item.to_string()],
            ..lease.clone()
        }
    }
}
