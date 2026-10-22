//! The contract every [`Store`] must pass, after grampy's driver contract:
//! the journal's rules hold whatever keeps the rows.
//!
//! ```ignore
//! helios_journal::contract::check(MemoryStore::default);
//! helios_journal::contract::check_lasting(MemoryStore::default, MemoryStore::share);
//! ```

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::{Error, Journal, Node, Outcome, Row, Status, Store, Workflow};

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
    a_renewed_lease_does_not_expire(journal(&make, linear()));
    a_claim_reads_only_its_candidates(&make);
}

/// The scenarios of a journal that outlives its process: `backing` makes
/// what keeps the rows (a vault, a shared map), `open` opens a store over
/// it, as a process does when it starts. A crash is a store dropped
/// without its commit, then opened again.
pub fn check_lasting<B, S: Store + 'static>(backing: impl Fn() -> B, open: impl Fn(&B) -> S) {
    a_crash_between_two_puts_leaves_nothing(&backing(), &open);
    reopened_tokens_never_collide(&backing(), &open);
    reopened_running_rows_are_given_back(&backing(), &open);
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

/// A store that panics at its `n`-th put, as a process dying in the middle
/// of a write.
struct DiesAtPut<S> {
    store: S,
    puts_left: usize,
}

impl<S: Store> Store for DiesAtPut<S> {
    fn get(&self, item: &str, node: &str) -> Option<Row> {
        self.store.get(item, node)
    }
    fn put(&mut self, item: &str, node: &str, row: Row) {
        self.puts_left -= 1;
        assert!(self.puts_left > 0, "the process died mid-transaction");
        self.store.put(item, node, row);
    }
    fn remove(&mut self, item: &str, node: &str) {
        self.store.remove(item, node);
    }
    fn rows(&self) -> Vec<(String, String, Row)> {
        self.store.rows()
    }
    fn begin(&mut self) {
        self.store.begin();
    }
    fn commit(&mut self) {
        self.store.commit();
    }
    fn last_token(&self) -> u64 {
        self.store.last_token()
    }
    fn set_last_token(&mut self, token: u64) {
        self.store.set_last_token(token);
    }
}

fn a_crash_between_two_puts_leaves_nothing<B, S: Store + 'static>(
    backing: &B,
    open: &impl Fn(&B) -> S,
) {
    let dying = Journal::new(
        linear(),
        DiesAtPut {
            store: open(backing),
            puts_left: 2,
        },
    );
    let crashed = catch_unwind(AssertUnwindSafe(|| {
        dying.claim("fetch", 2, &["i1", "i2"], 0)
    }));
    assert!(crashed.is_err(), "the second put must panic");
    drop(dying);
    let j = Journal::new(linear(), open(backing));
    assert_eq!(
        (j.row("i1", "fetch"), j.row("i2", "fetch")),
        (None, None),
        "a crash between two puts leaves nothing"
    );
    let lease = j.claim("fetch", 2, &["i1", "i2"], 1).unwrap();
    assert_eq!(lease.items, ["i1", "i2"], "both still claimable");
    j.conclude(&lease, Outcome::Done, 2).unwrap();
    drop(j);
    let j = Journal::new(linear(), open(backing));
    assert_eq!(
        j.status("i2", "fetch"),
        Some(Status::Done),
        "a committed write outlives its journal"
    );
}

fn reopened_tokens_never_collide<B, S: Store + 'static>(backing: &B, open: &impl Fn(&B) -> S) {
    let first = Journal::new(linear(), open(backing));
    let concluded = first.claim("fetch", 1, &["a"], 0).unwrap();
    first.conclude(&concluded, Outcome::Done, 1).unwrap();
    let left_running = first.claim("fetch", 1, &["b"], 1).unwrap();
    drop(first);
    let again = Journal::new(linear(), open(backing));
    let lease = again.claim("fetch", 1, &["c"], 2).unwrap();
    assert_eq!(lease.items, ["c"]);
    assert!(
        lease.token > concluded.token && lease.token > left_running.token,
        "reopened, the tokens never collide ({} after {} and {})",
        lease.token,
        concluded.token,
        left_running.token
    );
    // No row carries a token any more: only the kept counter remembers.
    again.conclude(&lease, Outcome::Done, 3).unwrap();
    again.conclude(&left_running, Outcome::Done, 3).unwrap();
    drop(again);
    let third = Journal::new(linear(), open(backing));
    let last = third.claim("fetch", 1, &["d"], 4).unwrap();
    assert!(
        last.token > lease.token,
        "the store keeps the counter ({} after {})",
        last.token,
        lease.token
    );
}

/// Counts the reads of the store it wraps.
struct Counting<S> {
    store: S,
    gets: Arc<AtomicUsize>,
    scans: Arc<AtomicUsize>,
}

impl<S: Store> Store for Counting<S> {
    fn get(&self, item: &str, node: &str) -> Option<Row> {
        self.gets.fetch_add(1, Ordering::Relaxed);
        self.store.get(item, node)
    }
    fn put(&mut self, item: &str, node: &str, row: Row) {
        self.store.put(item, node, row);
    }
    fn remove(&mut self, item: &str, node: &str) {
        self.store.remove(item, node);
    }
    fn rows(&self) -> Vec<(String, String, Row)> {
        self.scans.fetch_add(1, Ordering::Relaxed);
        self.store.rows()
    }
    fn begin(&mut self) {
        self.store.begin();
    }
    fn commit(&mut self) {
        self.store.commit();
    }
    fn last_token(&self) -> u64 {
        self.store.last_token()
    }
    fn set_last_token(&mut self, token: u64) {
        self.store.set_last_token(token);
    }
}

/// A batch of 3,200 items: claiming one reads the rows of the candidates
/// it looks at, never every row; expiring and watching read none.
fn a_claim_reads_only_its_candidates<S: Store>(make: &impl Fn() -> S) {
    let gets = Arc::new(AtomicUsize::new(0));
    let scans = Arc::new(AtomicUsize::new(0));
    let workflow = Workflow::new(vec![
        Node::new("submit"),
        Node::new("export").after(&["submit"]),
    ])
    .unwrap();
    let j = Journal::new(
        workflow,
        Counting {
            store: make(),
            gets: Arc::clone(&gets),
            scans: Arc::clone(&scans),
        },
    );
    let items: Vec<String> = (0..3_200).map(|i| format!("i{i:04}")).collect();
    let candidates: Vec<&str> = items.iter().map(String::as_str).collect();
    let submitted = j.claim("submit", items.len(), &candidates, 0).unwrap();
    j.conclude(&submitted, Outcome::Done, 0).unwrap();
    let reads = || {
        (
            gets.swap(0, Ordering::Relaxed),
            scans.swap(0, Ordering::Relaxed),
        )
    };
    reads();

    let first = j.claim("export", 1, &candidates, 1).unwrap();
    assert_eq!(first.items, ["i0000"]);
    let (got, scanned) = reads();
    assert!(
        got <= 2 && scanned == 0,
        "one claim of 3,200 candidates read {got} rows and scanned {scanned} times: \
         its row and its parent's only"
    );
    j.conclude(&first, Outcome::Done, 2).unwrap();
    reads();
    let second = j.claim("export", 1, &candidates, 3).unwrap();
    assert_eq!(second.items, ["i0001"]);
    let (got, scanned) = reads();
    assert!(
        got <= 3 && scanned == 0,
        "past one done item, a claim read {got} rows and scanned {scanned} times"
    );

    assert_eq!(j.expire(4), 0);
    let watched = j.monitor(4);
    assert_eq!(
        reads(),
        (0, 0),
        "expiring and watching read no row: the journal keeps the counts"
    );
    assert_eq!(watched["nodes"]["submit"]["done"], 3_200);
    assert_eq!(watched["nodes"]["export"]["done"], 1);
    assert_eq!(watched["nodes"]["export"]["running"], 1);
}

fn a_renewed_lease_does_not_expire<S: Store>(j: Journal<S>) {
    let lease = j.claim("fetch", 1, &["i"], 0).unwrap();
    j.renew(&lease, 90).unwrap();
    assert_eq!(j.expire(150), 0, "renewed at 90, due at 190");
    assert_eq!(j.row("i", "fetch").unwrap().due, Some(190));
    assert_eq!(j.expire(190), 1, "not renewed again: given back");
    assert_eq!(
        j.renew(&lease, 191).unwrap_err(),
        Error::StaleLease {
            node: "fetch".into(),
            item: "i".into()
        },
        "a lease given back cannot be renewed"
    );
    let fresh = j.claim("fetch", 1, &["i"], 191).unwrap();
    j.renew(&fresh, 250).unwrap();
    j.conclude(&fresh, Outcome::Done, 340).unwrap();
}

fn reopened_running_rows_are_given_back<B, S: Store + 'static>(
    backing: &B,
    open: &impl Fn(&B) -> S,
) {
    let dead = Journal::new(retried(), open(backing));
    let first = dead.claim("call", 2, &["a", "b"], 0).unwrap();
    dead.conclude(&Lease1::only(&first, "b"), Outcome::Failed("x".into()), 1)
        .unwrap();
    let second = dead.claim("call", 1, &["b"], 50).unwrap();
    assert_eq!(second.items, ["b"]);
    drop(dead);
    let j = Journal::new(retried(), open(backing));
    assert_eq!(j.recover(60), 2, "every running row, due or not");
    assert_eq!(j.status("a", "call"), None, "back to claimable");
    let b = j.row("b", "call").unwrap();
    assert_eq!(
        (b.status, b.attempts, b.due),
        (Status::Scheduled, 1, Some(60)),
        "the attempt cut short does not count"
    );
    assert_eq!(j.recover(61), 0);
    let lease = j.claim("call", 2, &["a", "b"], 61).unwrap();
    assert_eq!(lease.items, ["a", "b"]);
    assert_eq!(j.row("b", "call").unwrap().attempts, 2);
}
