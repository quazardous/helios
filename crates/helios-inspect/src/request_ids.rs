//! Request ids: a call sent again is applied once.
//!
//! A client may give a call that changes something a key of its own,
//! `_meta.request_id`. The endpoint remembers the recent keys with their
//! answers: when a key comes again, for the same call, the remembered
//! answer is given back, marked `already_applied`, and the call does not
//! run twice. So after an "outcome unknown" (the connection closed before
//! the answer, `docs/protocol.md`), the client may send the call again
//! with the same key.
//!
//! The store is bounded ([`CAPACITY`] keys, the oldest answered forgotten
//! first) and time-limited (a key is kept [`KEPT`]). A call that failed
//! is not remembered: it changed nothing, and sent again it runs again.
//! A key sent again while its call still runs waits for that call.

use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::rpc::{Call, Error, Service, code};

/// How many keys an endpoint remembers at most.
pub const CAPACITY: usize = 1000;

/// How long an endpoint remembers a key.
pub const KEPT: Duration = Duration::from_secs(10 * 60);

/// The longest key taken, in bytes.
pub const LONGEST: usize = 128;

/// The field of an answer given again.
pub const ALREADY_APPLIED: &str = "already_applied";

/// The key `params` carries (`_meta.request_id`): `None` without one, an
/// error for one that is not a short non-empty string.
pub fn request_id(params: &Value) -> Result<Option<String>, Error> {
    match &params["_meta"]["request_id"] {
        Value::Null => Ok(None),
        Value::String(key) if !key.is_empty() && key.len() <= LONGEST => Ok(Some(key.clone())),
        _ => Err(Error::new(
            code::INVALID_PARAMS,
            format!("`_meta.request_id` is a string of 1 to {LONGEST} bytes"),
        )),
    }
}

/// A new key, for a client that sends a change.
pub fn new_request_id() -> String {
    crate::transport::new_token()[..32].to_string()
}

/// `answer` given again: marked [`ALREADY_APPLIED`] (an answer that is
/// not an object is held in `result`).
pub fn applied(answer: Value) -> Value {
    let mut fields = match answer {
        Value::Object(fields) => fields,
        other => [("result".to_string(), other)].into_iter().collect(),
    };
    fields.insert(ALREADY_APPLIED.into(), json!(true));
    Value::Object(fields)
}

/// The error of a key sent with another call than its own.
pub fn reused(key: &str) -> Error {
    Error::new(
        code::INVALID_REQUEST,
        format!(
            "the request id `{key}` was given to another call: a key names one call, use a new one"
        ),
    )
}

/// What came of a call with a key.
#[derive(Debug, PartialEq)]
pub enum Once<T, E> {
    /// It ran now.
    Ran(Result<T, E>),
    /// It ran before: its remembered answer.
    Applied(Value),
    /// The key is another call's.
    Other,
}

/// One key: the call it names, its answer once it ran, and when.
struct Entry {
    call: String,
    answer: Option<Value>,
    at: Instant,
}

#[derive(Default)]
struct Keys {
    entries: HashMap<String, Entry>,
    /// The keys, oldest first.
    order: VecDeque<String>,
}

impl Keys {
    fn forget(&mut self, key: &str) {
        self.entries.remove(key);
        self.order.retain(|k| k != key);
    }

    /// Forgets the answered keys older than `kept`.
    fn forget_expired(&mut self, now: Instant, kept: Duration) {
        let entries = &mut self.entries;
        self.order.retain(|key| {
            let expired = entries
                .get(key)
                .is_some_and(|e| e.answer.is_some() && now.saturating_duration_since(e.at) >= kept);
            if expired {
                entries.remove(key);
            }
            !expired
        });
    }

    /// Room for one more key, the oldest answered forgotten first; false
    /// when every key held is a call still running.
    fn make_room(&mut self, capacity: usize) -> bool {
        while self.entries.len() >= capacity {
            let Some(oldest) = self
                .order
                .iter()
                .find(|k| self.entries.get(*k).is_some_and(|e| e.answer.is_some()))
                .cloned()
            else {
                return false;
            };
            self.forget(&oldest);
        }
        true
    }
}

/// The keys an endpoint remembers.
pub struct Remembered {
    keys: Mutex<Keys>,
    ended: Condvar,
    capacity: usize,
    kept: Duration,
}

impl Default for Remembered {
    fn default() -> Self {
        Self::new(CAPACITY, KEPT)
    }
}

/// Forgets a key whose call ended without an answer to keep (failed,
/// or its thread panicked), and wakes the calls waiting on it.
struct Running<'a> {
    remembered: &'a Remembered,
    key: &'a str,
    done: bool,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.remembered.lock().forget(self.key);
            self.remembered.ended.notify_all();
        }
    }
}

impl Remembered {
    /// At most `capacity` keys, each kept `kept`.
    pub fn new(capacity: usize, kept: Duration) -> Self {
        Self {
            keys: Mutex::default(),
            ended: Condvar::new(),
            capacity: capacity.max(1),
            kept,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Keys> {
        self.keys.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `run`, the call `call` (its method and params, a fingerprint)
    /// keyed `key`, unless that key's answer is remembered.
    pub fn once<E>(
        &self,
        key: &str,
        call: &str,
        run: impl FnOnce() -> Result<Value, E>,
    ) -> Once<Value, E> {
        self.once_at(key, call, Instant::now(), run)
    }

    /// As [`Remembered::once`], at `now`.
    pub fn once_at<E>(
        &self,
        key: &str,
        call: &str,
        now: Instant,
        run: impl FnOnce() -> Result<Value, E>,
    ) -> Once<Value, E> {
        let mut keys = self.lock();
        loop {
            keys.forget_expired(now, self.kept);
            let running = match keys.entries.get(key) {
                None => false,
                Some(entry) if entry.call != call => return Once::Other,
                Some(Entry {
                    answer: Some(answer),
                    ..
                }) => return Once::Applied(answer.clone()),
                Some(_) => true,
            };
            if !running {
                break;
            }
            // Its call still runs: its answer is waited for.
            keys = self
                .ended
                .wait(keys)
                .unwrap_or_else(PoisonError::into_inner);
        }
        // With every key held by a call still running, this one runs
        // without being remembered.
        if !keys.make_room(self.capacity) {
            drop(keys);
            return Once::Ran(run());
        }
        keys.entries.insert(
            key.to_string(),
            Entry {
                call: call.to_string(),
                answer: None,
                at: now,
            },
        );
        keys.order.push_back(key.to_string());
        drop(keys);
        let mut running = Running {
            remembered: self,
            key,
            done: false,
        };
        let started = Instant::now();
        let outcome = run();
        if let Ok(answer) = &outcome {
            if let Some(entry) = self.lock().entries.get_mut(key) {
                entry.answer = Some(answer.clone());
                entry.at = now + started.elapsed();
            }
            running.done = true;
            self.ended.notify_all();
        }
        Once::Ran(outcome)
    }

    /// How many keys are held.
    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    /// Whether no key is held.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A [`Service`] whose calls with a key are applied once.
pub(crate) struct Keyed {
    pub(crate) service: std::sync::Arc<dyn Service>,
    pub(crate) remembered: std::sync::Arc<Remembered>,
}

impl Service for Keyed {
    fn call(&self, method: &str, params: Value, call: &Call) -> Result<Value, Error> {
        let Some(key) = request_id(&params)? else {
            return self.service.call(method, params, call);
        };
        let fingerprint = crate::cache::key(method, &params);
        match self.remembered.once(&key, &fingerprint, || {
            self.service.call(method, params, call)
        }) {
            Once::Ran(outcome) => outcome,
            Once::Applied(answer) => Ok(applied(answer)),
            Once::Other => Err(reused(&key)),
        }
    }

    fn cancellable(&self, method: &str) -> bool {
        self.service.cancellable(method)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn ok(runs: &Cell<u32>) -> impl FnOnce() -> Result<Value, ()> + '_ {
        move || {
            runs.set(runs.get() + 1);
            Ok(json!({ "n": runs.get() }))
        }
    }

    #[test]
    fn a_key_sent_again_runs_once_and_gives_its_answer_back() {
        let store = Remembered::default();
        let runs = Cell::new(0);
        assert_eq!(
            store.once("k", "set", ok(&runs)),
            Once::Ran(Ok(json!({ "n": 1 })))
        );
        assert_eq!(
            store.once("k", "set", ok(&runs)),
            Once::Applied(json!({ "n": 1 }))
        );
        assert_eq!(runs.get(), 1);
        assert_eq!(store.once("k", "other", ok(&runs)), Once::Other);
        assert_eq!(runs.get(), 1);
    }

    #[test]
    fn a_failed_call_is_not_remembered() {
        let store = Remembered::default();
        let failed: Once<Value, &str> = store.once("k", "set", || Err("refused"));
        assert_eq!(failed, Once::Ran(Err("refused")));
        assert!(store.is_empty());
        let runs = Cell::new(0);
        assert!(matches!(
            store.once("k", "set", ok(&runs)),
            Once::Ran(Ok(_))
        ));
        assert_eq!(runs.get(), 1);
    }

    #[test]
    fn a_key_is_forgotten_once_its_time_is_up() {
        let kept = Duration::from_secs(60);
        let store = Remembered::new(10, kept);
        let runs = Cell::new(0);
        let start = Instant::now();
        assert!(matches!(
            store.once_at("k", "set", start, ok(&runs)),
            Once::Ran(_)
        ));
        let before = start + kept - Duration::from_secs(1);
        assert!(matches!(
            store.once_at("k", "set", before, ok(&runs)),
            Once::Applied(_)
        ));
        let after = start + kept + Duration::from_secs(1);
        assert!(matches!(
            store.once_at("k", "set", after, ok(&runs)),
            Once::Ran(_)
        ));
        assert_eq!(runs.get(), 2);
    }

    #[test]
    fn the_oldest_key_goes_when_the_store_is_full() {
        let store = Remembered::new(2, KEPT);
        let runs = Cell::new(0);
        for key in ["a", "b", "c"] {
            assert!(matches!(store.once(key, "set", ok(&runs)), Once::Ran(_)));
        }
        assert_eq!(store.len(), 2);
        assert!(matches!(
            store.once("c", "set", ok(&runs)),
            Once::Applied(_)
        ));
        assert!(matches!(
            store.once("b", "set", ok(&runs)),
            Once::Applied(_)
        ));
        // "a" was forgotten: it runs again (and "b" goes in its turn).
        assert!(matches!(store.once("a", "set", ok(&runs)), Once::Ran(_)));
        assert_eq!((runs.get(), store.len()), (4, 2));
    }

    #[test]
    fn keys_are_checked_and_answers_marked() {
        assert_eq!(request_id(&json!({})), Ok(None));
        assert_eq!(
            request_id(&json!({ "_meta": { "request_id": "r1" } })),
            Ok(Some("r1".into()))
        );
        for bad in [json!(""), json!(3), json!("x".repeat(LONGEST + 1))] {
            let error = request_id(&json!({ "_meta": { "request_id": bad } })).unwrap_err();
            assert_eq!(error.code, code::INVALID_PARAMS);
        }
        assert_eq!(
            applied(json!({ "ok": true })),
            json!({ "ok": true, "already_applied": true })
        );
        assert_eq!(
            applied(json!(3)),
            json!({ "result": 3, "already_applied": true })
        );
        assert_eq!(new_request_id().len(), 32);
        assert_ne!(new_request_id(), new_request_id());
    }
}
