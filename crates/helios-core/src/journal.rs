//! Helios's fact bus as the journal and relay of the application's bus.
//!
//! `bus-contract` leaves two capabilities to the implementation: a
//! journal (events numbered, read again from a number, delivered to their
//! listeners after the publisher moved on) and a relay (the events of
//! relayable points served to another process from a cursor, and taken
//! in from one). [`Bus`], the fact bus every Helios layer publishes on,
//! already numbers and keeps its facts: [`BusJournal`] offers it to the
//! application's bus as both.
//!
//! - **Numbers.** A journal entry is a fact of the bus: its `seq`, its
//!   `cause` and its window ([`Bus::since`], [`Bus::missed`]) are the
//!   journal's numbers, causes and gaps.
//! - **Typed facts.** A typed fact kind ([`crate::FactDecl::of`]) is the
//!   journaled event point [`FactPoint<E>`], its input a [`Fact`]: the
//!   fact's JSON and where it came from. A listener reads its own typed
//!   view of the payload ([`Fact::read`]).
//! - **The pump.** One thread per journal follows the bus and hands each
//!   fact of a journaled point to its listeners, in `seq` order, as a
//!   collect emit of the application's bus ([`bus_contract::Bus::collect_any`]):
//!   every rule of the bus holds (order, deadlines, failures named, local
//!   listeners queued for their thread). Publishing never waits on it.
//! - **The relay.** A cursor's run is this journal's, one per process: a
//!   node restarted answers an old cursor with [`Lost::Restarted`], an
//!   evicted range with [`Lost::Gap`]. Facts taken in from another
//!   process ([`Relay::ingest`]) are republished here with their source
//!   and never served again.
//!
//! A kind's point, its codec and its name are made in one place,
//! [`crate::FactDecl`]: renaming a kind, or reading an old name relayed by
//! an older node, changes that line and [`FactPoint`]'s codec only.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bus_contract::{
    AnyInput, AnyValue, Cursor, FreshFn, IngestError, Journal, JournalEntry, Lost, Point,
    PointDecl, PointKey, PointKind, PointSpec, Refused, Relay, Relayed, RelayedEntry, Resumed,
    Since, kind_matches,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::bus::{Bus, Envelope, Event, FactDecl};

/// A fact of the bus as a listener of its event point reads it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Fact {
    /// Its number on the bus that delivers it.
    pub seq: u64,
    /// Its kind (`layer.event`).
    pub kind: String,
    /// Who published it (a layer, `node:<name>` for a node's).
    pub source: String,
    /// The number of the fact that caused it, if any: on this bus, or on
    /// its source's for a fact relayed in.
    pub cause: Option<u64>,
    /// What it says, as published.
    pub payload: Value,
}

impl Fact {
    /// The payload read as `T`: a listener's own typed view, which
    /// accepts fields it does not know (a newer node) and fills the
    /// ones it misses with `#[serde(default)]` (an older one).
    pub fn read<T: DeserializeOwned>(&self) -> Result<T, String> {
        T::deserialize(&self.payload).map_err(|e| format!("`{}`: {e}", self.kind))
    }

    fn of(envelope: &Envelope) -> Fact {
        Fact {
            seq: envelope.seq,
            kind: envelope.kind.to_string(),
            source: envelope.source.clone(),
            cause: envelope.cause,
            payload: envelope.payload.clone(),
        }
    }
}

/// The journaled event point of the fact kind `E` (`E::KIND`): its
/// listeners hear each fact of the kind, in order, as a [`Fact`].
pub struct FactPoint<E>(PhantomData<fn() -> E>);

impl<E: Event + 'static> Point for FactPoint<E> {
    const ID: &'static str = E::KIND;
    type Input = Fact;
    type Value = ();
    const KIND: PointKind = PointKind::Event;
}

/// The codec of every fact point: the payload's JSON, read back whatever
/// fields it holds (two node versions read each other); only a text that
/// is not JSON is refused.
impl<E: Event + 'static> Relayed for FactPoint<E> {
    fn encode(input: &Fact) -> String {
        input.payload.to_string()
    }

    fn decode(text: &str) -> Result<Fact, String> {
        let payload = serde_json::from_str(text).map_err(|e| e.to_string())?;
        Ok(Fact {
            kind: E::KIND.to_string(),
            payload,
            ..Fact::default()
        })
    }
}

/// The event points of `tables`' typed kinds ([`FactDecl::point`]), in
/// table order: what [`BusJournal::declare`] declares.
pub fn points<'a>(tables: impl IntoIterator<Item = &'a [FactDecl]>) -> Vec<PointDecl> {
    tables
        .into_iter()
        .flatten()
        .filter_map(|d| d.point)
        .collect()
}

/// Relays the facts of `from`'s journaled kinds among `tables` (a typed
/// kind with its event point, [`FactDecl::of`]; the `fact_only` ones
/// stay on `from`) to the listeners of their [`FactPoint`] on the
/// application's bus `into`: recorded in its journal, delivered by its
/// pump, when it has one; emitted to them at once otherwise. Each is
/// handed on as it is published on `from`, so no burst of other kinds
/// there pushes one out unread. Ends once `into` is gone. The kinds
/// relayed.
///
/// For a bus of its own whose rare facts the application listens to
/// (the viewport engine's notices), its per-render kinds left on it.
pub fn relay_points<'a>(
    from: &Bus,
    tables: impl IntoIterator<Item = &'a [FactDecl]>,
    into: Weak<dyn bus_contract::Bus>,
) -> Vec<&'static str> {
    let keys: BTreeMap<&'static str, PointKey> = tables
        .into_iter()
        .flatten()
        .filter_map(|d| Some((d.kind, d.point?.key())))
        .collect();
    let kinds: Vec<&'static str> = keys.keys().copied().collect();
    if kinds.is_empty() {
        return kinds;
    }
    from.tap_kinds(
        &kinds,
        Arc::new(move |e: &Envelope| {
            let Some(bus) = into.upgrade() else {
                return false;
            };
            let Some(&key) = keys.get(e.kind) else {
                return true;
            };
            let mut fact = Fact::of(e);
            fact.cause = None;
            match bus.journal() {
                Some(journal) => {
                    if let Err(refused) = journal.publish_any(key, Arc::new(fact), fresh_unit, None)
                    {
                        log::warn!(target: "helios", "bus: `{}` not relayed: {refused}", e.kind);
                    }
                }
                None => {
                    let _ = bus.collect_any(key, Arc::new(fact), fresh_unit);
                }
            }
            true
        }),
    );
    kinds
}

/// A kind as the bus names it (`&'static`): each distinct kind is kept
/// once for the process.
pub fn intern(kind: &str) -> &'static str {
    static KINDS: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut kinds = KINDS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(k) = kinds.get(kind) {
        return k;
    }
    let k: &'static str = Box::leak(kind.to_owned().into_boxed_str());
    kinds.insert(k);
    k
}

/// The source of facts published through the journal with none named.
const SOURCE: &str = "journal";

/// How often an idle pump looks whether its bus is gone.
const IDLE: Duration = Duration::from_millis(200);

/// The application's bus, seen from its journal: upgraded per delivery,
/// so the journal never keeps it alive.
type Kernel = Arc<dyn Fn() -> Option<Arc<dyn bus_contract::Bus>> + Send + Sync>;
/// A point's declaration on the application's bus.
type Declaration = Arc<dyn Fn(&str) -> Option<(PointKey, PointSpec)> + Send + Sync>;

/// What the journal and its pump share.
struct Shared {
    bus: Arc<Bus>,
    kernel: Kernel,
    declaration: Declaration,
    /// The fact points declared through [`BusJournal::declare`]: their
    /// input is a [`Fact`], made from the fact itself.
    facts: Mutex<HashSet<&'static str>>,
    /// The typed inputs published through [`Journal::publish_any`] for a
    /// point that is not a fact point, until the pump delivers them. Held
    /// across the publish, so the pump never reads a fact before its
    /// input is kept.
    inputs: Mutex<BTreeMap<u64, AnyInput>>,
    /// The facts taken in from another process: never served again.
    ingested: Mutex<BTreeSet<u64>>,
    /// The last fact the pump handed on, and its wake.
    delivered: Mutex<u64>,
    progress: Condvar,
}

/// Each journal a run of its own: a process restarted, or a second bus
/// in one process, never answers another's cursor.
fn new_run() -> u64 {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
    let mixed = nanos
        ^ (u64::from(std::process::id()) << 32)
        ^ COUNT.fetch_add(1, Ordering::Relaxed).rotate_left(48);
    mixed.max(1)
}

fn fresh_unit() -> AnyValue {
    Box::new(())
}

/// Helios's [`Bus`] as the `Journal` and `Relay` of the application's
/// bus (the hook kernel it hands to `AppBuilder::bus`). Built with the
/// kernel by [`BusJournal::attach`]; its fact points declared by
/// [`BusJournal::declare`] once the kernel holds it.
pub struct BusJournal {
    shared: Arc<Shared>,
    run: u64,
    /// The pump's subscription, taken when the journal is made (no fact
    /// published after is missed) and handed to the pump at
    /// [`BusJournal::attach`]'s end.
    pending: Mutex<Option<Receiver<Envelope>>>,
}

impl BusJournal {
    /// Builds the application's bus `K` with `bus` as its journal and
    /// relay: `make` receives the journal (`Kernel::new().with_journal(j.clone())
    /// .with_relay(j)`), `declaration` reads a point's declaration on `K`
    /// (`Kernel::declaration`). The pump starts once `K` exists and stops
    /// when it is dropped.
    pub fn attach<K>(
        bus: Arc<Bus>,
        make: impl FnOnce(Arc<BusJournal>) -> K,
        declaration: fn(&K, &str) -> Option<(PointKey, PointSpec)>,
    ) -> (Arc<K>, Arc<BusJournal>)
    where
        K: bus_contract::Bus + 'static,
    {
        let mut journal = None;
        let kernel = Arc::new_cyclic(|me: &Weak<K>| {
            let upgrade = me.clone();
            let declared = me.clone();
            let j = Arc::new(BusJournal::new(
                bus,
                Arc::new(move || upgrade.upgrade().map(|k| k as Arc<dyn bus_contract::Bus>)),
                Arc::new(move |id: &str| declaration(&*declared.upgrade()?, id)),
            ));
            journal = Some(j.clone());
            make(j)
        });
        let journal = journal.expect("the kernel was made with its journal");
        journal.start();
        (kernel, journal)
    }

    fn new(bus: Arc<Bus>, kernel: Kernel, declaration: Declaration) -> BusJournal {
        let rx = bus.subscribe();
        let last = bus.last_seq();
        BusJournal {
            shared: Arc::new(Shared {
                bus,
                kernel,
                declaration,
                facts: Mutex::default(),
                inputs: Mutex::default(),
                ingested: Mutex::default(),
                delivered: Mutex::new(last),
                progress: Condvar::new(),
            }),
            run: new_run(),
            pending: Mutex::new(Some(rx)),
        }
    }

    fn start(&self) {
        let Some(rx) = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
        };
        let shared = Arc::downgrade(&self.shared);
        let spawned = std::thread::Builder::new()
            .name("helios-journal".into())
            .spawn(move || pump(&shared, &rx));
        if let Err(e) = spawned {
            log::error!(target: "helios", "the bus journal has no pump: {e}");
        }
    }

    /// The fact bus it reads.
    pub fn bus(&self) -> &Arc<Bus> {
        &self.shared.bus
    }

    /// Declares the event points of `tables`' typed kinds on `kernel`
    /// (the bus built by [`BusJournal::attach`]); what it refused, a point
    /// already declared there (by its crate's own table) left out.
    pub fn declare<'a>(
        &self,
        kernel: &dyn bus_contract::Bus,
        tables: impl IntoIterator<Item = &'a [FactDecl]>,
    ) -> Vec<Refused> {
        let decls = points(tables);
        let refused = bus_contract::declare_all(kernel, &decls);
        let mut facts = self
            .shared
            .facts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for d in &decls {
            let ours = !refused.iter().any(|r| refused_point(r) == Some(d.id));
            if ours {
                facts.insert(d.id);
            }
        }
        refused
            .into_iter()
            .filter(|r| !matches!(r, Refused::AlreadyDeclared { .. }))
            .collect()
    }

    fn entry(&self, e: &Envelope, ingested: &BTreeSet<u64>) -> JournalEntry {
        let mut entry = JournalEntry::new(e.seq, e.kind);
        entry.cause = e.cause;
        if ingested.contains(&e.seq) {
            entry.source = Some(e.source.clone());
        }
        entry.input = self.shared.input_of(e);
        entry.text = Some(e.payload.to_string());
        entry
    }
}

fn refused_point(r: &Refused) -> Option<&'static str> {
    match r {
        Refused::AlreadyDeclared { point }
        | Refused::Unsupported { point, .. }
        | Refused::NotEvent { point }
        | Refused::TypeMismatch { point }
        | Refused::NotJournaled { point } => Some(point),
        _ => None,
    }
}

impl Shared {
    /// The typed input of `e`, when this process holds its point: a
    /// [`Fact`] for a fact point, else the input published, else its
    /// payload read by the point's codec.
    fn input_of(&self, e: &Envelope) -> Option<AnyInput> {
        if self
            .facts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(e.kind)
        {
            return Some(Arc::new(Fact::of(e)));
        }
        if let Some(input) = self
            .inputs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&e.seq)
        {
            return Some(input.clone());
        }
        let (_, spec) = (self.declaration)(e.kind)?;
        (spec.relay?.decode)(&e.payload.to_string()).ok()
    }

    /// Hands `e` to its point's listeners, when it is a journaled point
    /// of the application's bus.
    fn deliver(&self, kernel: &dyn bus_contract::Bus, e: &Envelope) {
        let Some((key, spec)) = (self.declaration)(e.kind) else {
            return;
        };
        if !spec.journaled {
            return;
        }
        let input = self.input_of(e);
        self.inputs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&e.seq);
        let Some(input) = input else {
            log::warn!(target: "helios", "journal: fact {} `{}` has no input its point reads", e.seq, e.kind);
            return;
        };
        let fresh: FreshFn = fresh_unit;
        let collected = kernel.collect_any(key, input, fresh);
        for f in &collected.failures {
            log::warn!(target: "helios", "journal: fact {} `{}`: {f}", e.seq, e.kind);
        }
    }

    fn mark(&self, seq: u64) {
        let mut delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *delivered = (*delivered).max(seq);
        self.progress.notify_all();
    }
}

/// The pump: each fact, in order, to its point's listeners, until the
/// journal or the application's bus is gone.
fn pump(shared: &Weak<Shared>, rx: &Receiver<Envelope>) {
    loop {
        let next = rx.recv_timeout(IDLE);
        let Some(me) = shared.upgrade() else { return };
        let Some(kernel) = (me.kernel)() else { return };
        match next {
            Ok(e) => {
                me.deliver(&*kernel, &e);
                me.mark(e.seq);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

impl Journal for BusJournal {
    fn last_seq(&self) -> u64 {
        self.shared.bus.last_seq()
    }

    fn since(&self, seq: u64, limit: usize) -> Since {
        let last = self.shared.bus.last_seq();
        let gap = self.shared.bus.missed(seq) > 0;
        let ingested = self
            .shared
            .ingested
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let entries = self
            .shared
            .bus
            .since(seq)
            .iter()
            .take(limit)
            .map(|e| self.entry(e, &ingested))
            .collect();
        let mut out = Since::new(entries, last);
        out.gap = gap;
        out
    }

    fn wait(&self, patterns: &[&str], seq: u64, timeout: Duration) -> Option<JournalEntry> {
        let wanted =
            |kind: &str| patterns.is_empty() || patterns.iter().any(|p| kind_matches(p, kind));
        // Listened to before the journal is read, so a fact published in
        // between is seen by one or the other.
        let rx = self.shared.bus.subscribe();
        let until = Instant::now() + timeout;
        let found = |e: &Envelope| e.seq > seq && wanted(e.kind);
        let hit = self
            .shared
            .bus
            .since(seq)
            .into_iter()
            .find(found)
            .or_else(|| {
                loop {
                    let left = until.checked_duration_since(Instant::now())?;
                    match rx.recv_timeout(left) {
                        Ok(e) if found(&e) => break Some(e),
                        Ok(_) => {}
                        Err(_) => break None,
                    }
                }
            })?;
        let ingested = self
            .shared
            .ingested
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Some(self.entry(&hit, &ingested))
    }

    fn publish_any(
        &self,
        point: PointKey,
        input: AnyInput,
        _fresh: FreshFn,
        cause: Option<u64>,
    ) -> Result<u64, Refused> {
        // The kernel checked the point is declared journaled, with this
        // type, before handing it here.
        let shared = &self.shared;
        let fact = (&*input as &dyn Any).downcast_ref::<Fact>();
        let (source, payload) = if let Some(f) = fact {
            let source = if f.source.is_empty() {
                SOURCE
            } else {
                f.source.as_str()
            };
            (source, f.payload.clone())
        } else {
            // Another point's input, written as JSON by its codec when it
            // has one; the pump hands the input itself on.
            let text = (shared.declaration)(point.id)
                .and_then(|(_, spec)| spec.relay)
                .and_then(|codec| (codec.encode)(&*input));
            let payload = text
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null);
            (SOURCE, payload)
        };
        let mut inputs = shared.inputs.lock().unwrap_or_else(PoisonError::into_inner);
        let seq = shared
            .bus
            .publish_journaled(source, cause, point.id, payload);
        if fact.is_none() {
            inputs.insert(seq, input);
        }
        Ok(seq)
    }

    fn flush(&self, timeout: Duration) -> bool {
        let target = self.shared.bus.last_seq();
        let delivered = self
            .shared
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (delivered, _) = self
            .shared
            .progress
            .wait_timeout_while(delivered, timeout, |d| *d < target)
            .unwrap_or_else(PoisonError::into_inner);
        *delivered >= target
    }
}

impl Relay for BusJournal {
    fn cursor(&self) -> Cursor {
        Cursor {
            run: self.run,
            seq: self.shared.bus.last_seq(),
        }
    }

    fn resume(&self, from: Cursor, limit: usize) -> Resumed {
        let bus = &self.shared.bus;
        let (lost, after) = if from == Cursor::START {
            (None, 0)
        } else if from.run != self.run {
            (Some(Lost::Restarted), 0)
        } else {
            ((bus.missed(from.seq) > 0).then_some(Lost::Gap), from.seq)
        };
        let read = bus.since(after);
        let ingested = self
            .shared
            .ingested
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut entries = Vec::new();
        let mut next = read.last().map_or(after, |e| e.seq);
        for e in &read {
            if entries.len() == limit {
                next = e.seq - 1;
                break;
            }
            if ingested.contains(&e.seq) {
                continue;
            }
            let Some((_, spec)) = (self.shared.declaration)(e.kind) else {
                continue;
            };
            let Some(codec) = spec.relay else { continue };
            let Some(input) = self.shared.input_of(e) else {
                continue;
            };
            let Some(text) = (codec.encode)(&*input) else {
                continue;
            };
            let mut out = RelayedEntry::new(e.seq, e.kind, text);
            out.cause = e.cause;
            entries.push(out);
        }
        let mut out = Resumed::new(
            entries,
            Cursor {
                run: self.run,
                seq: next,
            },
        );
        out.lost = lost;
        out
    }

    fn ingest(&self, source: &str, entry: RelayedEntry) -> Result<u64, IngestError> {
        let shared = &self.shared;
        let declared = (shared.declaration)(&entry.kind);
        let (kind, payload) = match declared {
            None => (
                intern(&entry.kind),
                serde_json::from_str(&entry.text).unwrap_or(Value::String(entry.text)),
            ),
            Some((key, spec)) => {
                let Some(codec) = spec.relay else {
                    return Err(IngestError::NotRelayable { kind: entry.kind });
                };
                let input = (codec.decode)(&entry.text).map_err(|reason| IngestError::Decode {
                    kind: entry.kind.clone(),
                    reason,
                })?;
                let payload = match (&*input as &dyn Any).downcast_ref::<Fact>() {
                    Some(f) => f.payload.clone(),
                    None => serde_json::from_str(&entry.text).unwrap_or(Value::Null),
                };
                (key.id, payload)
            }
        };
        let mut ingested = shared
            .ingested
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let seq = shared
            .bus
            .republish_value(source, entry.cause, kind, payload);
        ingested.insert(seq);
        // Forget what the bus no longer keeps (`missed(0)`: every
        // number before its oldest).
        let first_kept = shared.bus.missed(0) + 1;
        ingested.retain(|n| *n >= first_kept);
        Ok(seq)
    }
}
