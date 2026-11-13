//! The import history through the node: an import run by `library.import`
//! leaves its record, `import.history.list` lists it, and
//! `import.history.again` runs it again, as whoever asks.

use std::sync::Arc;

use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_inspect::client::Client;
use helios_node_core::endpoint::VaultService;
use helios_vault::{MemoryStore, Vault};
use serde_json::json;

#[test]
fn an_import_is_recorded_listed_and_run_again_through_the_node() {
    let dir = std::env::temp_dir().join(format!("helios-import-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let roll = dir.join("roll");
    std::fs::create_dir_all(&roll).unwrap();
    std::fs::write(roll.join("p0.raf"), vec![9u8; 20_000]).unwrap();

    let bus = Arc::new(Bus::new());
    let vault = Arc::new(Vault::new(MemoryStore::default()).with_bus(bus.clone()));
    let service = Arc::new(VaultService::new(
        vault.clone(),
        bus.clone(),
        Jobs::new(bus.clone()),
        "test",
    ));
    let agent = Client::in_process(service.clone()).with_origin("agent:importer");
    let cli = Client::in_process(service).with_origin("cli");

    let first = agent
        .request(
            "library.import",
            json!({ "paths": [roll], "duplicates": "skip", "signals": false }),
        )
        .unwrap();
    assert_eq!(first["report"]["added"], 1, "{first}");

    let listed = cli.request("import.history.list", json!({})).unwrap();
    let imports = listed["imports"].as_array().unwrap();
    assert_eq!(imports.len(), 1, "{listed}");
    let record = &imports[0];
    assert_eq!(record["origin"], "agent:importer", "{record}");
    assert_eq!(record["sources"], json!([roll]), "{record}");
    assert_eq!(record["counts"]["added"], 1, "{record}");
    assert_eq!(record["choices"]["duplicates"], "skip", "{record}");
    assert_eq!(record["choices"]["signals"], false, "{record}");

    // Again, by the CLI: the same source and choices; the photo is in
    // the library already.
    let again = cli
        .request("import.history.again", json!({ "id": record["id"] }))
        .unwrap();
    assert_eq!(again["report"]["added"], 0, "{again}");
    assert_eq!(again["report"]["files"], 1, "{again}");
    let listed = cli
        .request("import.history.list", json!({ "limit": 5 }))
        .unwrap();
    let imports = listed["imports"].as_array().unwrap();
    assert_eq!(imports.len(), 2, "{listed}");
    assert_eq!(imports[0]["origin"], "cli", "newest first: {listed}");
    assert_eq!(imports[0]["sources"], json!([roll]), "{listed}");

    let unknown = cli.request("import.history.again", json!({ "id": "nope" }));
    assert!(unknown.is_err(), "{unknown:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
