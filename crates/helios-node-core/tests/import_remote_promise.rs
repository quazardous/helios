//! `library.import {remote, rule}`: photos chosen in a service's page,
//! handed to the node by the process holding their connector, downloaded
//! and imported as a promise. Against a fake service: the promise
//! progresses item by item and settles with each one's outcome; a rate
//! limit and an expired download address are waited out and gone on
//! from; the pick imported again downloads nothing; a pick unknown to the
//! process is refused at once.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helios_connect::{
    Access, AccountRef, Cancel, Caps, Connector, ConnectorError, ConnectorInfo, Cursor, FetchSink,
    Fetched, Item, ItemId, ItemKind, ItemRef, Page, SourceRef, Text,
};
use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_inspect::client::Client;
use helios_node_core::endpoint::VaultService;
use helios_node_core::import_promise::{self, Handed};
use helios_vault::promises::PROGRESS;
use helios_vault::{MemoryStore, Vault};
use serde_json::{Value, json};

/// How long a test waits for anything at most.
const DEADLINE: Duration = Duration::from_mins(1);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-import-remote-{name}-{}-{}",
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

/// A service in memory: its items' bytes, and the answers it gives
/// before serving them (a rate limit, an expired address).
#[derive(Default)]
struct Service {
    files: BTreeMap<String, Vec<u8>>,
    /// Per item, the errors its next fetches answer, in order.
    refusals: Mutex<HashMap<String, Vec<ConnectorError>>>,
    /// Each fetch asked, by item.
    calls: Mutex<Vec<String>>,
    picks_ended: AtomicU32,
}

impl Service {
    fn new(n: usize) -> Self {
        let files = (0..n)
            .map(|i| {
                let mut bytes = vec![9u8; 3_000];
                bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
                (format!("g{i:02}"), bytes)
            })
            .collect();
        Self {
            files,
            ..Self::default()
        }
    }

    fn items(&self) -> Vec<Item> {
        self.files
            .iter()
            .map(|(id, bytes)| Item {
                id: ItemId(id.clone()),
                path: format!("{id}.raf"),
                size: Some(bytes.len() as u64),
                modified: Some(1_700_000_000),
                taken: None,
                kind: ItemKind::Raw,
                mime: None,
                metadata: BTreeMap::new(),
                claim: None,
                revision: None,
                group: None,
            })
            .collect()
    }

    fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl Connector for Service {
    fn describe(&self) -> ConnectorInfo {
        ConnectorInfo {
            id: "fake-photos".into(),
            label: Text::new("connect-fake-photos", "Fake photos"),
            caps: Caps {
                accounts: true,
                sizes: true,
                ..Caps::default()
            },
        }
    }

    fn sources(&self, _account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        Ok(Vec::new())
    }

    fn list(
        &self,
        _source: &SourceRef,
        _from: Option<&Cursor>,
        _page: usize,
        _cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        Err(ConnectorError::Unsupported)
    }

    fn end_pick(&self) -> Result<(), ConnectorError> {
        self.picks_ended.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn access(&self, _item: &ItemRef) -> Access {
        Access::Fetch
    }

    fn fetch(
        &self,
        item: &ItemRef,
        offset: u64,
        sink: &mut dyn FetchSink,
        _cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        self.calls.lock().unwrap().push(item.id.0.clone());
        if let Some(next) = self
            .refusals
            .lock()
            .unwrap()
            .get_mut(&item.id.0)
            .filter(|r| !r.is_empty())
        {
            return Err(next.remove(0));
        }
        let bytes = self.files.get(&item.id.0).ok_or(ConnectorError::NotFound)?;
        let from = (offset as usize).min(bytes.len());
        sink.accept(&bytes[from..])
            .map_err(|e| ConnectorError::Fatal(e.to_string()))?;
        Ok(Fetched {
            bytes: (bytes.len() - from) as u64,
            mime: None,
        })
    }
}

fn source() -> SourceRef {
    SourceRef {
        connector: "fake-photos".into(),
        id: "account-1".into(),
        label: "Fake photos".into(),
        root: None,
        capacity: None,
    }
}

fn hand(service: &Arc<Service>) -> String {
    import_promise::hand(Handed {
        connector: service.clone(),
        source: source(),
        items: service.items(),
        end_pick: true,
    })
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

fn node() -> (
    Arc<Vault<MemoryStore>>,
    Client,
    std::sync::mpsc::Receiver<helios_core::bus::Envelope>,
) {
    let bus = Arc::new(Bus::new());
    let heard = bus.subscribe();
    let vault = Arc::new(Vault::new(MemoryStore::default()).with_bus(bus.clone()));
    let service = VaultService::new(vault.clone(), bus.clone(), Jobs::new(bus.clone()), "test");
    let client = Client::in_process(Arc::new(service)).with_origin("agent:test");
    (vault, client, heard)
}

#[test]
fn a_pick_handed_to_the_node_is_downloaded_as_a_promise_waiting_out_limits() {
    let dir = scratch("pick");
    let (vault, client, heard) = node();
    let service = Arc::new(Service::new(6));
    {
        let mut refusals = service.refusals.lock().unwrap();
        // The service asks to wait (a quota or a rate limit), then serves.
        refusals.insert(
            "g01".into(),
            vec![ConnectorError::RateLimited {
                retry_after: Some(Duration::from_millis(50)),
            }],
        );
        // The download address expired mid-import: fetched once more.
        refusals.insert("g03".into(), vec![ConnectorError::Expired]);
    }
    let answer = client
        .request(
            "library.import",
            json!({ "remote": hand(&service), "rule": "all", "into": dir.join("lib"),
                    "layout": "flat", "signals": false }),
        )
        .unwrap();
    let promise = &answer["promise"];
    assert_eq!(promise["kind"], "import", "{answer}");
    assert!(
        promise["label"]
            .as_str()
            .unwrap()
            .contains("6 photos from Fake photos"),
        "{promise}"
    );
    let settled = wait(&client, &promise["id"]);
    assert_eq!(settled["state"], "done", "{settled}");
    let result = &settled["result"];
    assert_eq!(result["report"]["added"], 6, "{result}");
    assert_eq!(result["failed"], json!([]), "{result}");
    // Its progress was heard item by item, each with its name.
    let progress: Vec<Value> = std::iter::from_fn(|| heard.try_recv().ok())
        .filter(|e| e.kind == PROGRESS && e.payload["id"] == promise["id"])
        .map(|e| e.payload)
        .collect();
    for n in 1..=6 {
        assert!(
            progress.iter().any(|p| p["done"] == n && p["total"] == 6),
            "{n}: {progress:?}"
        );
    }
    assert!(
        progress.iter().any(|p| p["note"] == "g03.raf"),
        "{progress:?}"
    );
    assert_eq!(vault.images().unwrap().len(), 6);
    // Six items, two of them asked twice.
    assert_eq!(service.calls(), 8);
    assert_eq!(service.picks_ended.load(Ordering::SeqCst), 1);

    // The same photos chosen again (after a restart, say): nothing is
    // downloaded, each is skipped by its id.
    let again = client
        .request(
            "library.import",
            json!({ "remote": hand(&service), "rule": "all", "into": dir.join("lib"),
                    "layout": "flat", "signals": false }),
        )
        .unwrap();
    let settled = wait(&client, &again["promise"]["id"]);
    let result = &settled["result"];
    assert_eq!(result["report"]["added"], 0, "{result}");
    assert_eq!(result["report"]["skipped"], 6, "{result}");
    assert_eq!(service.calls(), 8, "nothing fetched again");
    assert_eq!(vault.images().unwrap().len(), 6);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_item_that_keeps_failing_is_reported_and_the_others_imported() {
    let dir = scratch("failing");
    let (vault, client, _heard) = node();
    let service = Arc::new(Service::new(3));
    service.refusals.lock().unwrap().insert(
        "g02".into(),
        vec![ConnectorError::Fatal("the service refused it".into())],
    );
    let answer = client
        .request(
            "library.import",
            json!({ "remote": hand(&service), "into": dir.join("lib"), "signals": false }),
        )
        .unwrap();
    let settled = wait(&client, &answer["promise"]["id"]);
    let result = &settled["result"];
    assert_eq!(result["report"]["added"], 2, "{result}");
    assert_eq!(result["failed"][0]["item"], "g02", "{result}");
    assert_eq!(vault.images().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pick_the_process_does_not_hold_is_refused_at_once() {
    let (_vault, client, _heard) = node();
    let refused = client
        .request(
            "library.import",
            json!({ "remote": "pick-0-0", "into": "/tmp/nowhere" }),
        )
        .unwrap_err();
    assert!(refused.message.contains("choose them again"), "{refused:?}");
}
