//! A client's cache of a node's reads, kept true by the node's events
//! (see `docs/protocol.md`, "A client cache").
//!
//! A read the cache knows ([`area_of`]) is answered from it when it holds
//! the same method with the same params; otherwise the node answers and
//! the cache keeps the answer, with the bus sequence it was read at (the
//! answer's `seq`). Events say what changed, not the new values: the
//! cache drops what an event names ([`touched_by`]) and the next read
//! asks the node again. So a reader never sees a value older than a
//! change it has heard of, whoever made it.
//!
//! - An event dropped before a read's answer was kept still applies to
//!   it: the cache remembers its last invalidations and checks an answer
//!   read at an older sequence against them.
//! - A gap in the events (the subscription's `gap`), a lost subscription
//!   or a node of another instance (restarted) drops everything.
//! - Read your writes: a write's answer carries the sequence of its
//!   events (`seq`); once [`ReadCache::seen`] reaches it, the cache shows
//!   the write ([`ReadCache::wait_seen`]).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::client::Client;
use crate::rpc::Error;

/// What part of a node's data a cached read shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Area {
    /// Photos' fields, read through a cursor.
    Photos,
    /// An image's edit.
    Edits,
    /// The stacks and their suggestions.
    Stacks,
    /// The settings layers.
    Settings,
}

/// The reads the cache keeps, by method, and the area each shows; any
/// other method goes to the node every time (a write, a cursor opened or
/// refreshed, a wait).
pub fn area_of(method: &str) -> Option<Area> {
    match method {
        "cursor.window" | "cursor.rank" => Some(Area::Photos),
        "library.edit.head" => Some(Area::Edits),
        "library.stacks.list" | "library.stacks.decisions" => Some(Area::Stacks),
        "settings.layer.get" | "settings.layers" => Some(Area::Settings),
        _ => None,
    }
}

/// What an event makes stale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Touched {
    /// Nothing the cache keeps.
    Nothing,
    /// Every read naming one of these images (by id).
    Images(BTreeSet<String>),
    /// Every read of an area.
    Area(Area),
    /// Everything: the library changed as a whole.
    All,
}

/// What an event of kind `kind` with `payload` makes stale: the
/// invalidation map.
///
/// | Event | Drops |
/// |---|---|
/// | `vault.field_changed`, `vault.version_added`, `edit.saved` | the reads naming its `image` |
/// | `vault.stack_changed`, `library.decision` | the stacks' reads |
/// | `settings.layerChanged` | the settings' reads |
/// | `library.imported`, `vault.locations_changed`, `vault.library_changed`, `vault.captures_set`, `vault.changed_outside`, `node.ready` | everything |
pub fn touched_by(kind: &str, payload: &Value) -> Touched {
    match kind {
        "vault.field_changed" | "vault.version_added" | "edit.saved" => {
            payload["image"].as_str().map_or(Touched::All, |image| {
                Touched::Images(BTreeSet::from([image.to_string()]))
            })
        }
        "vault.stack_changed" | "library.decision" => Touched::Area(Area::Stacks),
        "settings.layerChanged" => Touched::Area(Area::Settings),
        "library.imported"
        | "vault.locations_changed"
        | "vault.library_changed"
        | "vault.captures_set"
        | "vault.changed_outside"
        | "node.ready" => Touched::All,
        _ => Touched::Nothing,
    }
}

/// How many invalidations the cache remembers for answers still on their
/// way.
const RECENT: usize = 256;

struct Entry {
    value: Value,
    area: Area,
    /// The images the answer names.
    images: BTreeSet<String>,
    /// The cursor it reads, if any.
    cursor: Option<u64>,
}

impl Entry {
    fn hit_by(&self, touched: &Touched) -> bool {
        match touched {
            Touched::Nothing => false,
            Touched::Images(ids) => !self.images.is_disjoint(ids),
            Touched::Area(area) => self.area == *area,
            Touched::All => true,
        }
    }
}

#[derive(Default)]
struct Inner {
    instance: Option<String>,
    entries: HashMap<String, Entry>,
    /// The highest event sequence applied.
    seen: u64,
    /// The last invalidations, by sequence.
    recent: VecDeque<(u64, Touched)>,
    /// Every invalidation after this sequence is in `recent`.
    recent_from: u64,
    /// The sequence the cache was last emptied at: an answer read before
    /// it is not kept.
    reset_at: u64,
    /// Moves at every reset: a read sent before one is not kept.
    generation: u64,
    hits: u64,
    misses: u64,
    reset: Option<String>,
}

/// What the cache did, for tests and `describe`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub seen: u64,
    /// Why it was last emptied whole.
    pub reset: Option<String>,
}

/// A cache of a node's reads; share it ([`Arc`]) between the readers and
/// the thread that feeds it events ([`ReadCache::follow`]).
#[derive(Default)]
pub struct ReadCache {
    inner: Mutex<Inner>,
    moved: Condvar,
}

/// The cache's key for a call: its method and its params, `_meta` left
/// out and object keys in order.
pub(crate) fn key(method: &str, params: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(fields) => {
                let mut sorted: Vec<_> = fields
                    .iter()
                    .filter(|(k, _)| k.as_str() != "_meta")
                    .collect();
                sorted.sort_by(|a, b| a.0.cmp(b.0));
                Value::Object(
                    sorted
                        .into_iter()
                        .map(|(k, v)| (k.clone(), canonical(v)))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }
    format!("{method} {}", canonical(params))
}

/// The images an answer names: a window's rows, a rank's or an edit's
/// image.
fn images_of(answer: &Value) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for row in answer["rows"].as_array().into_iter().flatten() {
        if let Some(id) = row["id"].as_str() {
            ids.insert(id.to_string());
        }
    }
    if let Some(id) = answer["image"].as_str() {
        ids.insert(id.to_string());
    }
    ids
}

impl ReadCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What it holds and did.
    pub fn stats(&self) -> Stats {
        let inner = self.lock();
        Stats {
            entries: inner.entries.len(),
            hits: inner.hits,
            misses: inner.misses,
            seen: inner.seen,
            reset: inner.reset.clone(),
        }
    }

    /// The answer kept for `method` with `params`, if any.
    pub fn get(&self, method: &str, params: &Value) -> Option<Value> {
        area_of(method)?;
        let mut inner = self.lock();
        let value = inner.entries.get(&key(method, params))?.value.clone();
        inner.hits += 1;
        Some(value)
    }

    /// Keeps the node's answer to `method` with `params`, unless an event
    /// heard since it was read (its `seq`) makes it stale.
    pub fn put(&self, method: &str, params: &Value, answer: &Value) {
        let Some(area) = area_of(method) else {
            return;
        };
        let read_at = answer["seq"].as_u64().unwrap_or(0);
        let entry = Entry {
            value: answer.clone(),
            area,
            images: images_of(answer),
            cursor: params["cursor"].as_u64(),
        };
        let mut inner = self.lock();
        if read_at < inner.reset_at {
            return;
        }
        if read_at < inner.seen {
            // Events after the read were heard already: the answer is kept
            // only if every one of them is remembered and none touches it.
            let covered = read_at >= inner.recent_from;
            let stale = inner
                .recent
                .iter()
                .any(|(seq, touched)| *seq > read_at && entry.hit_by(touched));
            if !covered || stale {
                return;
            }
        }
        inner.entries.insert(key(method, params), entry);
    }

    /// Reads `method` with `params`: from the cache when it holds the
    /// answer, else from the node through `client` (waiting for it), the
    /// answer kept. A read of a cursor the node no longer holds drops
    /// what the cache kept of it.
    ///
    /// # Errors
    /// The node's refusal.
    pub fn read(&self, client: &Client, method: &str, params: Value) -> Result<Value, Error> {
        if let Some(value) = self.get(method, &params) {
            return Ok(value);
        }
        let generation = {
            let mut inner = self.lock();
            if area_of(method).is_some() {
                inner.misses += 1;
            }
            inner.generation
        };
        match client.request(method, params.clone()) {
            Ok(answer) => {
                if self.lock().generation == generation {
                    self.put(method, &params, &answer);
                }
                Ok(answer)
            }
            Err(error) => {
                if let Some(cursor) = params["cursor"].as_u64() {
                    self.forget_cursor(cursor);
                }
                Err(error)
            }
        }
    }

    /// Drops what the cache kept of cursor `cursor` (closed, or gone).
    pub fn forget_cursor(&self, cursor: u64) {
        self.lock().entries.retain(|_, e| e.cursor != Some(cursor));
    }

    /// Applies an event, as the bus carries it (`seq`, `kind`, `payload`).
    pub fn event(&self, event: &Value) {
        let seq = event["seq"].as_u64().unwrap_or(0);
        let kind = event["kind"].as_str().unwrap_or_default();
        let touched = touched_by(kind, &event["payload"]);
        let mut inner = self.lock();
        if touched != Touched::Nothing {
            inner.entries.retain(|_, e| !e.hit_by(&touched));
            if inner.recent.len() == RECENT
                && let Some((oldest, _)) = inner.recent.pop_front()
            {
                inner.recent_from = oldest;
            }
            inner.recent.push_back((seq, touched));
        }
        inner.seen = inner.seen.max(seq);
        drop(inner);
        self.moved.notify_all();
    }

    /// Empties the cache: an event gap, a lost subscription, a restarted
    /// node. An answer read before now is no longer kept.
    pub fn reset(&self, why: &str) {
        let mut inner = self.lock();
        inner.entries.clear();
        inner.recent.clear();
        inner.recent_from = inner.seen;
        inner.reset_at = inner.seen;
        inner.generation += 1;
        inner.reset = Some(why.to_string());
        drop(inner);
        self.moved.notify_all();
    }

    /// The node's instance, from its `node.hello` or a subscription: a
    /// different one than before (the node restarted, its sequence
    /// started over) empties the cache.
    pub fn instance(&self, instance: &str) {
        let mut inner = self.lock();
        let before = inner.instance.replace(instance.to_string());
        if before.is_some_and(|b| b != instance) {
            inner.entries.clear();
            inner.recent.clear();
            inner.seen = 0;
            inner.recent_from = 0;
            inner.reset_at = 0;
            inner.generation += 1;
            inner.reset = Some("the node restarted".into());
        }
    }

    /// The highest event sequence the cache applied.
    pub fn seen(&self) -> u64 {
        self.lock().seen
    }

    /// Waits at most `timeout` for the cache to have applied the events up
    /// to `seq` (a write's answer): true once it shows that write.
    pub fn wait_seen(&self, seq: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut inner = self.lock();
        while inner.seen < seq {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            inner = self
                .moved
                .wait_timeout(inner, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Feeds the cache the events of the node `events` reaches (its
    /// inspect endpoint, `events.subscribe`), on a thread of its own,
    /// until that client's connection closes; the cache is emptied then,
    /// as on a gap or a node of another instance.
    ///
    /// # Errors
    /// The node refused the subscription.
    pub fn follow(self: &Arc<Self>, events: &Client) -> Result<(), Error> {
        let notifications = events.notifications();
        let since = match self.seen() {
            0 => Value::Null,
            seen => json!(seen),
        };
        let ack = events.request("events.subscribe", json!({ "prefix": "", "since": since }))?;
        if let Some(instance) = ack["instance"].as_str() {
            self.instance(instance);
        }
        if ack["gap"] == true {
            self.reset("events were missed");
        }
        let subscription = ack["subscription"].clone();
        let cache = self.clone();
        std::thread::Builder::new()
            .name("helios-cache".into())
            .spawn(move || {
                for note in notifications {
                    if note["params"]["subscription"] != subscription {
                        continue;
                    }
                    if note["method"] == "events.event" {
                        cache.event(&note["params"]["event"]);
                    }
                }
                cache.reset("the node's events stopped");
            })
            .map_err(|e| Error::new(crate::rpc::code::INTERNAL_ERROR, e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(seq: u64, ids: &[&str]) -> Value {
        json!({ "seq": seq, "rows": ids.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>() })
    }

    fn changed(seq: u64, image: &str) -> Value {
        json!({ "seq": seq, "kind": "vault.field_changed", "payload": { "image": image } })
    }

    #[test]
    fn an_event_drops_the_reads_naming_its_image_only() {
        let cache = ReadCache::new();
        let page = |o: u64| json!({ "cursor": 1, "offset": o, "len": 2 });
        cache.put("cursor.window", &page(0), &window(1, &["a", "b"]));
        cache.put("cursor.window", &page(2), &window(1, &["c", "d"]));
        cache.put("library.stacks.list", &json!({}), &json!({ "seq": 1 }));
        assert!(cache.get("cursor.window", &page(0)).is_some());
        cache.event(&changed(2, "c"));
        assert!(cache.get("cursor.window", &page(0)).is_some());
        assert!(cache.get("cursor.window", &page(2)).is_none());
        assert!(cache.get("library.stacks.list", &json!({})).is_some());
        cache.event(&json!({ "seq": 3, "kind": "vault.stack_changed", "payload": {} }));
        assert!(cache.get("library.stacks.list", &json!({})).is_none());
        cache.event(&json!({ "seq": 4, "kind": "library.imported", "payload": {} }));
        assert_eq!(cache.stats().entries, 0);
        assert!(cache.get("cursor.open", &json!({})).is_none(), "never kept");
    }

    #[test]
    fn an_answer_read_before_an_event_heard_is_checked_against_it() {
        let cache = ReadCache::new();
        let page = json!({ "cursor": 1, "offset": 0, "len": 2 });
        cache.event(&changed(5, "a"));
        // Read at 4, the change at 5 heard first: not kept.
        cache.put("cursor.window", &page, &window(4, &["a", "b"]));
        assert!(cache.get("cursor.window", &page).is_none());
        // Read at 4, the change at 5 does not touch it: kept.
        cache.put("cursor.window", &page, &window(4, &["x", "y"]));
        assert!(cache.get("cursor.window", &page).is_some());
    }

    #[test]
    fn a_gap_or_another_instance_drops_everything() {
        let cache = ReadCache::new();
        let page = json!({ "cursor": 1, "offset": 0, "len": 1 });
        cache.instance("one");
        cache.event(&json!({ "seq": 3, "kind": "job.started", "payload": {} }));
        cache.put("cursor.window", &page, &window(3, &["a"]));
        assert_eq!(cache.stats().entries, 1, "an event of nothing kept");
        cache.reset("events were missed");
        assert_eq!(cache.stats().entries, 0);
        cache.put("cursor.window", &page, &window(2, &["a"]));
        assert_eq!(cache.stats().entries, 0, "read before the reset");
        cache.put("cursor.window", &page, &window(3, &["a"]));
        assert_eq!(cache.stats().entries, 1);
        cache.instance("one");
        assert_eq!(cache.stats().entries, 1);
        cache.instance("two");
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(cache.stats().reset.as_deref(), Some("the node restarted"));
    }

    #[test]
    fn keys_ignore_meta_and_key_order() {
        assert_eq!(
            key(
                "m",
                &json!({ "b": 1, "a": { "y": 2, "x": 1 }, "_meta": { "origin": "cli" } })
            ),
            key("m", &json!({ "a": { "x": 1, "y": 2 }, "b": 1 }))
        );
    }
}
