//! Reads at scale through the node: a cursor paged window by window, and
//! a client cache kept true by the node's events, on both stores.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_inspect::Inspector;
use helios_inspect::cache::ReadCache;
use helios_inspect::client::Client;
use helios_inspect::rpc::{Call, Error, Service, code};
use helios_node_core::cursor::GONE;
use helios_node_core::endpoint::VaultService;
use helios_vault::{Image, ImageId, MemoryStore, Store, Vault, parse_hash};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

/// How long a test waits for an event at most.
const DEADLINE: Duration = Duration::from_secs(10);

fn id(n: usize) -> ImageId {
    ImageId(parse_hash(&format!("{n:064x}")).unwrap())
}

/// A store holding `n` photos in its Library.
fn with_photos<S: Store>(mut store: S, n: usize) -> S {
    for i in 1..=n {
        let image = Image {
            id: id(i),
            size: 100,
            partial: parse_hash(&format!("{:064x}", i + 1_000_000)).unwrap(),
        };
        store.add_to_library(&image).unwrap();
    }
    store
}

struct Node<S: Store + Send + 'static> {
    vault: Arc<Vault<S>>,
    bus: Arc<Bus>,
    service: Arc<VaultService<S>>,
}

impl<S: Store + Send + 'static> Node<S> {
    fn new(store: S) -> Self {
        let bus = Arc::new(Bus::new());
        let vault = Arc::new(Vault::new(store).with_bus(bus.clone()));
        let service = Arc::new(VaultService::new(
            vault.clone(),
            bus.clone(),
            Jobs::new(bus.clone()),
            "test",
        ));
        Self {
            vault,
            bus,
            service,
        }
    }

    fn client(&self, origin: &str) -> Client {
        Client::in_process(self.service.clone()).with_origin(origin)
    }

    /// A client of the node's events (its inspect endpoint).
    fn events(&self) -> Client {
        Client::in_process(Arc::new(Inspector::new(self.bus.clone())))
    }
}

fn rate(client: &Client, image: &ImageId, stars: i64) -> Value {
    client
        .request(
            "library.fields.set",
            json!({ "changes": [{ "image": image.hex(), "field": "rating", "value": stars }] }),
        )
        .unwrap()
}

fn ids_of(window: &Value) -> Vec<String> {
    window["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

/// A cursor over `n` photos paged by `page`: every photo read once, in
/// order, though another client writes in the middle of the paging; the
/// windows after the write say the snapshot is stale, and a refresh
/// names what the write changed.
fn paging_reads_each_photo_once<S: Store + Send + 'static>(store: S, n: usize, page: usize) {
    let node = Node::new(with_photos(store, n));
    let reader = node.client("agent:reader");
    let writer = node.client("cli");
    rate(&writer, &id(1), 3);
    let open = reader
        .request("cursor.open", json!({ "query": { "min_rating": 1 } }))
        .unwrap();
    assert_eq!(open["count"], 1, "{open}");
    let all = reader.request("cursor.open", json!({})).unwrap();
    assert_eq!(all["count"], n, "{all}");
    let (rated, cursor) = (open["cursor"].clone(), all["cursor"].clone());

    let mut seen = BTreeSet::new();
    let mut offset = 0;
    let mut stale = Vec::new();
    let mut written = false;
    while offset < n {
        if offset >= n / 2 && !written {
            rate(&writer, &id(2), 5);
            written = true;
        }
        let window = reader
            .request(
                "cursor.window",
                json!({ "cursor": cursor, "offset": offset, "len": page, "fields": ["rating"] }),
            )
            .unwrap();
        let ids = ids_of(&window);
        assert_eq!(ids.len(), page.min(n - offset));
        for got in ids {
            assert!(seen.insert(got.clone()), "{got} read twice");
        }
        stale.push(window["stale"] == true);
        offset += page;
    }
    assert_eq!(seen.len(), n);
    assert!(!stale[0], "the first window is current");
    assert!(*stale.last().unwrap(), "the write made the snapshot stale");

    let delta = reader
        .request("cursor.refresh", json!({ "cursor": rated }))
        .unwrap();
    assert_eq!(delta["changed"], true, "{delta}");
    assert_eq!(delta["inserted"], json!([id(2).hex()]));
    let rank = reader
        .request(
            "cursor.rank",
            json!({ "cursor": rated, "image": id(2).hex() }),
        )
        .unwrap();
    assert!(rank["rank"].is_u64(), "{rank}");

    let closed = reader
        .request("cursor.close", json!({ "cursor": cursor }))
        .unwrap();
    assert_eq!(closed["closed"], true);
    let gone = reader
        .request(
            "cursor.window",
            json!({ "cursor": cursor, "offset": 0, "len": 1 }),
        )
        .unwrap_err();
    assert_eq!(gone.code, GONE, "{gone:?}");
    assert_eq!(node.service.cursors().len(), 1);
}

#[test]
fn a_hundred_thousand_photos_are_paged_each_once_in_memory() {
    paging_reads_each_photo_once(MemoryStore::default(), 100_000, 1_000);
}

#[test]
fn photos_are_paged_each_once_in_sqlite() {
    paging_reads_each_photo_once(SqliteStore::in_memory().unwrap(), 2_000, 300);
}

/// A repeated read comes from the cache without a store query; another
/// client's write drops exactly the page showing the photo it changed,
/// and the reader's next read shows the write.
fn the_cache_drops_what_another_client_changed<S: Store + Send + 'static>(store: S) {
    let node = Node::new(with_photos(store, 20));
    let reader = node.client("agent:reader");
    let writer = node.client("cli");
    let cache = Arc::new(ReadCache::new());
    let events = node.events();
    cache.follow(&events).unwrap();
    let cursor = reader.request("cursor.open", json!({})).unwrap()["cursor"].clone();
    let page = |offset: usize| json!({ "cursor": cursor, "offset": offset, "len": 10, "fields": ["rating"] });
    let first = cache.read(&reader, "cursor.window", page(0)).unwrap();
    cache.read(&reader, "cursor.window", page(10)).unwrap();
    let queries = node.vault.reads().queries;
    assert_eq!(
        cache.read(&reader, "cursor.window", page(0)).unwrap(),
        first
    );
    assert_eq!(
        node.vault.reads().queries,
        queries,
        "a repeated read never reaches the store"
    );
    assert_eq!((cache.stats().hits, cache.stats().misses), (1, 2));

    // Photo 12 is on the second page.
    let written = rate(&writer, &id(12), 4);
    let seq = written["seq"].as_u64().unwrap();
    assert!(cache.wait_seen(seq, DEADLINE), "the write's event came");
    assert!(cache.get("cursor.window", &page(0)).is_some(), "untouched");
    assert!(cache.get("cursor.window", &page(10)).is_none(), "dropped");
    let again = cache.read(&reader, "cursor.window", page(10)).unwrap();
    let row = again["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id(12).hex())
        .unwrap();
    assert_eq!(row["fields"]["rating"], 4, "the reader sees the write");

    // A stack made elsewhere drops the stacks' reads, not the pages.
    cache
        .read(&reader, "library.stacks.list", json!({}))
        .unwrap();
    let made = writer
        .request(
            "library.stacks.create",
            json!({ "images": [id(1).hex(), id(2).hex()] }),
        )
        .unwrap();
    assert!(cache.wait_seen(made["seq"].as_u64().unwrap(), DEADLINE));
    assert!(cache.get("library.stacks.list", &json!({})).is_none());
    assert!(cache.get("cursor.window", &page(0)).is_some());

    // A restarted node (another instance) drops everything.
    let restarted = Client::in_process(Arc::new(Inspector::new(Arc::new(Bus::new()))));
    cache.follow(&restarted).unwrap();
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(cache.stats().reset.as_deref(), Some("the node restarted"));
}

#[test]
fn the_cache_follows_the_node_in_memory() {
    the_cache_drops_what_another_client_changed(MemoryStore::default());
}

#[test]
fn the_cache_follows_the_node_in_sqlite() {
    the_cache_drops_what_another_client_changed(SqliteStore::in_memory().unwrap());
}

/// Holds `cursor.window` until its call is cancelled, then lets the node
/// answer it: a page cancelled while it reads.
struct Gate(Arc<VaultService<MemoryStore>>);

impl Service for Gate {
    fn call(&self, method: &str, params: Value, call: &Call) -> Result<Value, Error> {
        if method == "cursor.window" && params["len"] != 1 {
            let until = std::time::Instant::now() + DEADLINE;
            while !call.is_cancelled() && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        self.0.call(method, params, call)
    }

    fn cancellable(&self, method: &str) -> bool {
        self.0.cancellable(method)
    }
}

#[test]
fn a_page_cancelled_answers_cancelled_and_the_cursor_stays() {
    let node = Node::new(with_photos(MemoryStore::default(), 50));
    let client = Client::in_process(Arc::new(Gate(node.service.clone())));
    let cursor = client.request("cursor.open", json!({})).unwrap()["cursor"].clone();
    let page = client.call(
        "cursor.window",
        json!({ "cursor": cursor, "offset": 0, "len": 50 }),
    );
    client.cancel(&page);
    assert_eq!(page.wait().unwrap_err().code, code::CANCELLED);
    let next = client
        .request(
            "cursor.window",
            json!({ "cursor": cursor, "offset": 10, "len": 1 }),
        )
        .unwrap();
    assert_eq!(ids_of(&next), vec![id(11).hex()]);
}

#[test]
fn a_cursor_left_unread_expires_and_the_cache_forgets_it() {
    let node = Node::new(with_photos(MemoryStore::default(), 5));
    let client = node.client("cli");
    let cache = ReadCache::new();
    let cursor = client
        .request("cursor.open", json!({ "lease": 1 }))
        .unwrap()["cursor"]
        .clone();
    let page = json!({ "cursor": cursor, "offset": 0, "len": 5 });
    cache.read(&client, "cursor.window", page.clone()).unwrap();
    std::thread::sleep(Duration::from_millis(1_100));
    let rank = json!({ "cursor": cursor, "image": id(1).hex() });
    let gone = cache.read(&client, "cursor.rank", rank).unwrap_err();
    assert_eq!(gone.code, GONE, "{gone:?}");
    assert!(cache.get("cursor.window", &page).is_none(), "forgotten");
    assert!(node.service.cursors().is_empty());
}

/// `library.fields.set` puts a name in and out of a set field, keeping
/// the others, and `library.fields.get` reads the fields back as stored.
#[test]
fn fields_are_set_by_op_and_read_back() {
    let node = Node::new(with_photos(MemoryStore::default(), 2));
    let client = node.client("user");
    let set = |changes: Value| {
        client
            .request("library.fields.set", json!({ "changes": changes }))
            .unwrap()
    };
    let image = id(1).hex();
    set(json!([
        { "image": image, "field": "labels", "op": "add", "value": "red" },
        { "image": image, "field": "labels", "op": "add", "value": "blue" },
        { "image": image, "field": "rating", "value": 4 },
    ]));
    set(json!([
        { "image": image, "field": "labels", "op": "remove", "value": "red" },
        { "image": image, "field": "rating", "op": "set_if_absent", "value": 1 },
    ]));
    let read = client
        .request(
            "library.fields.get",
            json!({ "images": [image, id(2).hex()], "fields": ["labels", "rating"] }),
        )
        .unwrap();
    let rows = read["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{read}");
    assert_eq!(rows[0]["id"], image);
    assert_eq!(rows[0]["fields"]["labels"], json!(["blue"]), "{read}");
    assert_eq!(rows[0]["fields"]["rating"], 4, "{read}");
    assert!(rows[1]["fields"].get("rating").is_none(), "{read}");
    let refused = client.request(
        "library.fields.set",
        json!({ "changes": [{ "image": image, "field": "labels", "op": "toggle", "value": "red" }] }),
    );
    assert!(refused.is_err(), "an unknown op is refused");
}
