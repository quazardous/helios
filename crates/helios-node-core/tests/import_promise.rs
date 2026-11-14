//! `library.import {source, rule}` through the node: answered at once
//! with a promise, which progresses and settles with the import's
//! outcome; cancelled; taken up after a stop without importing twice.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_inspect::client::Client;
use helios_node_core::endpoint::VaultService;
use helios_node_core::import_promise::{self, KIND};
use helios_node_core::promises::Promises;
use helios_vault::promises::{PROGRESS, State};
use helios_vault::{MemoryStore, Store, Vault};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

/// How long a test waits for anything at most.
const DEADLINE: Duration = Duration::from_mins(1);

/// A fresh folder of the test's own, emptied.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-import-promise-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A roll of `n` raws, each of its own content, in two folders.
fn roll(dir: &Path, n: usize) -> PathBuf {
    let roll = dir.join("roll");
    for i in 0..n {
        let sub = roll.join(if i % 2 == 0 { "a" } else { "b" });
        std::fs::create_dir_all(&sub).unwrap();
        let mut bytes = vec![7u8; 4_000];
        bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
        std::fs::write(sub.join(format!("p{i:03}.raf")), bytes).unwrap();
    }
    roll
}

fn images<S: Store>(vault: &Vault<S>) -> usize {
    vault.images().unwrap().len()
}

fn wait(client: &Client, id: &Value) -> Value {
    let waited = client
        .request(
            "promises.wait",
            json!({ "id": id, "timeout_ms": DEADLINE.as_millis() as u64 }),
        )
        .unwrap();
    assert_eq!(waited["settled"], true, "{waited}");
    waited["promise"].clone()
}

#[test]
fn an_import_by_rule_answers_a_promise_that_progresses_and_settles_with_its_outcome() {
    let dir = scratch("folder");
    let roll = roll(&dir, 12);
    let bus = Arc::new(Bus::new());
    let heard = bus.subscribe();
    let vault = Arc::new(Vault::new(MemoryStore::default()).with_bus(bus.clone()));
    let service = VaultService::new(vault.clone(), bus.clone(), Jobs::new(bus.clone()), "test");
    let client = Client::in_process(Arc::new(service)).with_origin("agent:test");

    let answer = client
        .request(
            "library.import",
            json!({ "source": roll, "rule": "all", "signals": false }),
        )
        .unwrap();
    let promise = &answer["promise"];
    assert_eq!(promise["kind"], KIND, "{answer}");
    assert!(
        ["pending", "running"].contains(&promise["state"].as_str().unwrap()),
        "answered before the import ends: {answer}"
    );
    assert_eq!(promise["label"], "Import roll", "{answer}");
    let settled = wait(&client, &promise["id"]);
    assert_eq!(settled["state"], "done", "{settled}");
    let result = &settled["result"];
    assert_eq!(result["report"]["added"], 12, "{settled}");
    assert_eq!(result["failed"], json!([]), "{settled}");
    assert!(result["history"].is_string(), "{settled}");
    assert_eq!(images(&vault), 12);
    // The history keeps the import, with who asked.
    let listed = client.request("import.history.list", json!({})).unwrap();
    assert_eq!(listed["imports"][0]["id"], result["history"], "{listed}");
    assert_eq!(listed["imports"][0]["origin"], "agent:test", "{listed}");

    // Its progress was heard: the listing's count, then n of 12.
    let progress: Vec<Value> = std::iter::from_fn(|| heard.try_recv().ok())
        .filter(|e| e.kind == PROGRESS && e.payload["id"] == promise["id"])
        .map(|e| e.payload)
        .collect();
    assert!(
        progress
            .iter()
            .any(|p| p["note"].as_str().is_some_and(|n| n.starts_with("listing"))),
        "{progress:?}"
    );
    assert!(
        progress.iter().any(|p| p["done"] == 12 && p["total"] == 12),
        "{progress:?}"
    );

    // Run again with `new`: everything is known, nothing is added.
    let again = client
        .request(
            "library.import",
            json!({ "source": roll, "rule": "new", "signals": false }),
        )
        .unwrap();
    let settled = wait(&client, &again["promise"]["id"]);
    assert_eq!(settled["result"]["report"]["added"], 0, "{settled}");
    assert_eq!(images(&vault), 12);

    // Explicit items: one file of a new roll.
    let other = dir.join("other.raf");
    std::fs::write(&other, vec![3u8; 4_000]).unwrap();
    let items = client
        .request(
            "library.import",
            json!({ "rule": { "items": [other] }, "signals": false }),
        )
        .unwrap();
    assert_eq!(items["promise"]["label"], "Import 1 items", "{items}");
    let settled = wait(&client, &items["promise"]["id"]);
    assert_eq!(settled["result"]["report"]["added"], 1, "{settled}");
    assert_eq!(images(&vault), 13);

    // Refused at once: no source, a rule that is none.
    assert!(
        client
            .request("library.import", json!({ "rule": "all" }))
            .is_err()
    );
    assert!(
        client
            .request("library.import", json!({ "source": roll, "rule": "some" }))
            .is_err()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_import_promise_stops_and_keeps_what_it_imported() {
    let dir = scratch("cancel");
    let roll = roll(&dir, 400);
    let bus = Arc::new(Bus::new());
    let vault = Arc::new(Vault::new(MemoryStore::default()).with_bus(bus.clone()));
    let service = VaultService::new(vault.clone(), bus.clone(), Jobs::new(bus.clone()), "test");
    let client = Client::in_process(Arc::new(service)).with_origin("cli");

    let answer = client
        .request(
            "library.import",
            json!({ "source": roll, "signals": false, "copy": true, "into": dir.join("library") }),
        )
        .unwrap();
    let id = answer["promise"]["id"].clone();
    let cancelled = client
        .request("promises.cancel", json!({ "id": id }))
        .unwrap();
    assert_eq!(cancelled["promise"]["cancel_asked"], true, "{cancelled}");
    let settled = wait(&client, &id);
    // It may have ended before the cancel was seen on a fast machine;
    // when cancelled, it stopped before the last file.
    match settled["state"].as_str().unwrap() {
        "cancelled" => assert!(images(&vault) < 400, "{settled}"),
        "done" => assert_eq!(images(&vault), 400, "{settled}"),
        other => panic!("settled {other}: {settled}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_import_cut_by_a_stop_is_taken_up_and_finishes_without_duplicates() {
    let dir = scratch("resume");
    let roll = roll(&dir, 10);
    let file = dir.join("vault.db");
    let request = json!({ "source": roll, "rule": "all", "signals": false, "origin": "cli" });
    // A node imports part of the roll, then stops with its promise
    // running.
    let cut = {
        let vault = Vault::new(SqliteStore::open(&file).unwrap());
        let bus = Arc::new(Bus::new());
        let options = helios_node_core::library::import_options(&request, None).unwrap();
        let first: Vec<PathBuf> = helios_connect::local::files_of(&roll)
            .unwrap()
            .into_iter()
            .take(4)
            .collect();
        let report = helios_library::import::import(
            &vault,
            &Jobs::new(bus),
            &first,
            &options,
            1,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(report.added, 4);
        let p = vault
            .create_promise(KIND, "import-{id}", "Import roll", request.clone(), 1)
            .unwrap();
        vault.progress_promise(&p.id, 4, Some(10), None, 2).unwrap();
        p.id
    };
    // The next start takes it up and finishes the roll.
    let bus = Arc::new(Bus::new());
    let vault = Arc::new(Vault::new(SqliteStore::open(&file).unwrap()).with_bus(bus.clone()));
    assert_eq!(images(&vault), 4);
    let promises = import_promise::register(
        Promises::new(vault.clone(), Jobs::new(bus.clone())),
        bus.clone(),
    );
    assert_eq!(promises.recover().unwrap(), vec![cut.clone()]);
    let start = Instant::now();
    let p = vault.wait_promise(&cut, DEADLINE, &|| false).unwrap();
    assert!(start.elapsed() < DEADLINE);
    let State::Done { result } = &p.state else {
        panic!("not done: {p:?}");
    };
    assert_eq!(p.attempts, 2);
    assert_eq!(result["report"]["added"], 6, "{result}");
    assert_eq!(images(&vault), 10, "no photo imported twice");
    drop(promises);
    drop(vault);
    let _ = std::fs::remove_dir_all(&dir);
}
