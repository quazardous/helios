//! A photo's lifecycle on the bus, and the warm-up that follows it.
//!
//! The surfaces say which photos they show and which are near
//! (`helios.photo.shown`, `helios.photo.near`, `helios.photo.hidden`, on
//! the node's fact bus, beside the router's `image.state.*`): the
//! darkroom says the photo it opens and its neighbours in the Library's
//! order. [`Lifecycle`] turns "this one shown, those near" into the facts
//! that changed, a photo leaving the window said hidden.
//!
//! A [`Warmer`] follows those facts: a photo near, once it stayed near for
//! [`SETTLE`] (a fast scroll warms nothing), has its states asked of
//! the router ([`crate::image_states`]) at [`Priority::Prefetch`], as the
//! application's [`Recipe`] names them; a photo hidden has them withdrawn
//! (a computation waiting or running stops unless another asker wants
//! it). It keeps only the asks in flight: a state ready stays the
//! router's, within its budget, never held outside it. The router starts
//! no background computation while an interactive one runs, so a warm-up
//! never competes with the shown photo. A photo warmed then hidden
//! without being shown is wasted work: counted ([`WarmCounters`]) and
//! said ([`facts::WARM_UNUSED`]).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helios_core::{Bus, Priority};
use serde::{Deserialize, Serialize};

use crate::image_states::{Compute, Handle, ImageStates, StateKey};

/// The source the lifecycle facts carry on the bus.
pub const SOURCE: &str = "photo_life";

/// A photo's lifecycle facts, each `{ photo }`.
pub mod facts {
    /// A surface shows the photo.
    pub const SHOWN: &str = "helios.photo.shown";
    /// The photo is likely to be shown next (a neighbour of the one
    /// shown): worth warming.
    pub const NEAR: &str = "helios.photo.near";
    /// The photo is neither shown nor near any more.
    pub const HIDDEN: &str = "helios.photo.hidden";
    /// A photo warmed was hidden without being shown: its warm-up was
    /// wasted work (the warmer's counters keep the total).
    pub const WARM_UNUSED: &str = "helios.photo.warm_unused";
    /// The lifecycle facts, which the warmer follows.
    pub const LIFECYCLE: [&str; 3] = [SHOWN, NEAR, HIDDEN];
    /// Every fact.
    pub const ALL: [&str; 4] = [SHOWN, NEAR, HIDDEN, WARM_UNUSED];
}

/// The payload of a lifecycle fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Photo {
    /// The photo, as the router's states name it (a vault image's id).
    pub photo: String,
}

/// Where a photo stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stand {
    Shown,
    Near,
}

/// What the surfaces show, to say only what changed.
#[derive(Debug, Default)]
pub struct Lifecycle {
    photos: BTreeMap<String, Stand>,
}

impl Lifecycle {
    /// The facts that move the lifecycle to `shown` (none: nothing shown)
    /// with `near` around it, in order: hidden first, then shown, then
    /// near. A photo already in its place says nothing.
    pub fn show(&mut self, shown: Option<&str>, near: &[String]) -> Vec<(&'static str, String)> {
        let mut next = BTreeMap::new();
        for photo in near {
            next.insert(photo.clone(), Stand::Near);
        }
        if let Some(shown) = shown {
            next.insert(shown.to_string(), Stand::Shown);
        }
        let mut said = Vec::new();
        for photo in self.photos.keys() {
            if !next.contains_key(photo) {
                said.push((facts::HIDDEN, photo.clone()));
            }
        }
        for (stand, kind) in [(Stand::Shown, facts::SHOWN), (Stand::Near, facts::NEAR)] {
            for (photo, now) in &next {
                if *now == stand && self.photos.get(photo) != Some(now) {
                    said.push((kind, photo.clone()));
                }
            }
        }
        self.photos = next;
        said
    }

    /// Moves to `shown` with `near` and says each change on `bus`.
    pub fn publish(&mut self, bus: &Bus, shown: Option<&str>, near: &[String]) {
        for (kind, photo) in self.show(shown, near) {
            let payload = serde_json::to_value(Photo { photo }).expect("a photo serializes");
            bus.publish_value(SOURCE, None, kind, payload);
        }
    }
}

/// The states worth warming for a photo, each with how it is computed:
/// the application knows where the photo's file is and through which
/// network it is developed. Empty for a photo it cannot warm.
pub type Recipe = Arc<dyn Fn(&str) -> Vec<(StateKey, Compute)> + Send + Sync>;

/// How often the warmer looks at its asks and whether it should stop.
const TICK: Duration = Duration::from_millis(100);

/// How long a photo stays near before its warm-up starts: a fast scroll
/// or a run of arrows passes photos near and hidden again within it, and
/// none of them is decoded.
pub const SETTLE: Duration = Duration::from_millis(200);

/// What the warmer did, since it started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct WarmCounters {
    /// Photos whose warm-up was asked of the router.
    pub started: u64,
    /// Photos near that were hidden again before they settled: never
    /// asked, no work done.
    pub skipped: u64,
    /// Warmed photos then shown: the warm-up served an open.
    pub used: u64,
    /// Warmed photos hidden without being shown: wasted work, also said
    /// as [`facts::WARM_UNUSED`].
    pub unused: u64,
}

/// The warmer's state, shared with its thread.
#[derive(Default)]
struct Warming {
    /// Photos near, waiting to settle, since when.
    pending: BTreeMap<String, Instant>,
    /// Asks in flight, by photo.
    asks: BTreeMap<String, Vec<Handle>>,
    /// Photos warmed (asked) and not shown since.
    warmed: BTreeSet<String>,
    counters: WarmCounters,
}

impl Warming {
    /// Follows one lifecycle fact.
    fn heard(&mut self, bus: &Bus, kind: &str, photo: String) {
        match kind {
            facts::NEAR if !self.asks.contains_key(&photo) && !self.warmed.contains(&photo) => {
                self.pending.entry(photo).or_insert_with(Instant::now);
            }
            facts::SHOWN => {
                // The open asks for its own states (joining a warm-up in
                // flight); one still settling is not started for it.
                self.pending.remove(&photo);
                if self.warmed.remove(&photo) {
                    self.counters.used += 1;
                }
            }
            facts::HIDDEN => {
                if self.pending.remove(&photo).is_some() {
                    self.counters.skipped += 1;
                }
                // Withdrawn: dropping an ask cancels it.
                self.asks.remove(&photo);
                if self.warmed.remove(&photo) {
                    self.counters.unused += 1;
                    let payload =
                        serde_json::to_value(Photo { photo }).expect("a photo serializes");
                    bus.publish_value(SOURCE, None, facts::WARM_UNUSED, payload);
                }
            }
            _ => {}
        }
    }

    /// Starts the warm-ups of the photos settled near.
    fn start_settled(&mut self, states: &ImageStates, recipe: &Recipe) {
        let now = Instant::now();
        let settled: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, since)| now.duration_since(**since) >= SETTLE)
            .map(|(photo, _)| photo.clone())
            .collect();
        for photo in settled {
            self.pending.remove(&photo);
            let asks: Vec<Handle> = recipe(&photo)
                .into_iter()
                .map(|(key, compute)| states.ask(key, Priority::Prefetch, compute))
                .collect();
            if !asks.is_empty() {
                self.counters.started += 1;
                self.warmed.insert(photo.clone());
                self.asks.insert(photo, asks);
            }
        }
    }

    /// How long the thread may wait for a fact: until the next photo
    /// settles, at most a tick.
    fn wait(&self) -> Duration {
        let now = Instant::now();
        self.pending
            .values()
            .map(|since| (*since + SETTLE).saturating_duration_since(now))
            .min()
            .map_or(TICK, |d| d.clamp(Duration::from_millis(1), TICK))
    }
}

/// Follows the lifecycle facts on the router's bus and warms the photos
/// near once they settle ([`SETTLE`]); stops with it.
pub struct Warmer {
    stop: Arc<AtomicBool>,
    warming: Arc<Mutex<Warming>>,
}

impl Warmer {
    /// Starts following `states`' bus, warming with `recipe`.
    pub fn start(states: ImageStates, recipe: Recipe) -> Warmer {
        let stop = Arc::new(AtomicBool::new(false));
        let warming: Arc<Mutex<Warming>> = Arc::default();
        let rx = states.bus().subscribe_kinds(&facts::LIFECYCLE, None);
        let (s, w) = (stop.clone(), warming.clone());
        let spawned = std::thread::Builder::new()
            .name("helios-warm".into())
            .spawn(move || {
                while !s.load(Ordering::SeqCst) {
                    let wait = w.lock().unwrap_or_else(PoisonError::into_inner).wait();
                    let fact = match rx.recv_timeout(wait) {
                        Ok(fact) => Some(fact),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    };
                    let mut warming = w.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(fact) = fact
                        && let Ok(Photo { photo }) = serde_json::from_value(fact.payload)
                    {
                        warming.heard(states.bus(), fact.kind, photo);
                    }
                    warming.start_settled(&states, &recipe);
                    // A state ready (or failed) is the router's: its ask
                    // is let go, so nothing holds it past the budget.
                    for asks in warming.asks.values_mut() {
                        asks.retain_mut(|ask| ask.try_get().is_none());
                    }
                    warming.asks.retain(|_, asks| !asks.is_empty());
                }
            });
        if let Err(e) = spawned {
            log::warn!("no thread to warm the photos near: {e}");
        }
        Warmer { stop, warming }
    }

    /// The photos with a warm-up still in flight.
    pub fn warming(&self) -> BTreeSet<String> {
        self.warming
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .asks
            .keys()
            .cloned()
            .collect()
    }

    /// What it did so far.
    pub fn counters(&self) -> WarmCounters {
        self.warming
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .counters
    }
}

impl Drop for Warmer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Its asks in flight are withdrawn.
        self.warming
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .asks
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_states::{Ready, StateError, StateKind};
    use helios_core::{ContentHash, cancel};
    use std::sync::atomic::AtomicUsize;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn the_lifecycle_says_only_what_changed() {
        let mut life = Lifecycle::default();
        assert_eq!(
            life.show(Some("b"), &ids(&["a", "c"])),
            [
                (facts::SHOWN, "b".into()),
                (facts::NEAR, "a".into()),
                (facts::NEAR, "c".into())
            ]
        );
        // The next photo: "a" leaves, "c" is shown, "b" turns near, "d"
        // comes near.
        assert_eq!(
            life.show(Some("c"), &ids(&["b", "d"])),
            [
                (facts::HIDDEN, "a".into()),
                (facts::SHOWN, "c".into()),
                (facts::NEAR, "b".into()),
                (facts::NEAR, "d".into())
            ]
        );
        assert!(life.show(Some("c"), &ids(&["b", "d"])).is_empty());
        let mut gone: Vec<_> = life.show(None, &[]);
        gone.sort();
        assert_eq!(
            gone,
            [
                (facts::HIDDEN, "b".into()),
                (facts::HIDDEN, "c".into()),
                (facts::HIDDEN, "d".into())
            ]
        );
    }

    fn decoded(photo: &str) -> StateKey {
        StateKey {
            photo: photo.into(),
            development: ContentHash::of(b"network"),
            state: StateKind::Decoded,
            size: 0,
        }
    }

    const LONG: Duration = Duration::from_secs(20);

    fn until(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + LONG;
        while !done() {
            assert!(Instant::now() < deadline, "never: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A recipe whose one state runs until `go` (or a cancel), counting
    /// its runs and the priorities they were asked at.
    fn recipe(runs: &Arc<AtomicUsize>, go: &Arc<AtomicBool>) -> Recipe {
        let (runs, go) = (runs.clone(), go.clone());
        Arc::new(move |photo: &str| {
            let (runs, go) = (runs.clone(), go.clone());
            let compute: Compute = Arc::new(move || {
                runs.fetch_add(1, Ordering::SeqCst);
                let token = cancel::current();
                let deadline = Instant::now() + LONG;
                while !go.load(Ordering::SeqCst) {
                    if token.is_cancelled() {
                        return Err(StateError::Cancelled);
                    }
                    assert!(Instant::now() < deadline, "never released");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(Ready::new(7u32, 100))
            });
            vec![(decoded(photo), compute)]
        })
    }

    #[test]
    fn a_photo_near_is_warmed_and_an_open_joins_its_warm_up() {
        let bus = Arc::new(Bus::new());
        let states = ImageStates::new(bus.clone(), 2);
        let (runs, go) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        let warmer = Warmer::start(states.clone(), recipe(&runs, &go));
        Lifecycle::default().publish(&bus, Some("a"), &ids(&["b"]));
        until("b warming", || runs.load(Ordering::SeqCst) == 1);
        assert_eq!(warmer.warming(), ids(&["b"]).into_iter().collect());
        // The warm-up was asked at Prefetch.
        let computing = bus
            .since(0)
            .into_iter()
            .find(|e| e.kind == crate::image_states::facts::COMPUTING)
            .expect("computing");
        assert_eq!(computing.payload["priority"], "prefetch");
        assert_eq!(computing.payload["state"], "decoded");
        // The user opens "b" while it warms: the open joins it.
        let never: Compute = Arc::new(|| Ok(Ready::new(0u32, 0)));
        let g = go.clone();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            g.store(true, Ordering::SeqCst);
        });
        let value = states
            .get_here(decoded("b"), Priority::Interactive, never, LONG)
            .expect("decoded");
        release.join().unwrap();
        assert_eq!(*value.downcast_ref::<u32>().unwrap(), 7);
        assert_eq!(runs.load(Ordering::SeqCst), 1, "one decode for both");
        assert_eq!(states.counters().merged, 1);
        // Ready, the warm-up's ask is let go: the state is the router's.
        until("warm-up let go", || warmer.warming().is_empty());
        assert!(states.list().0.contains(&decoded("b")));
    }

    #[test]
    fn a_photo_hidden_has_its_warm_up_withdrawn() {
        let bus = Arc::new(Bus::new());
        let states = ImageStates::new(bus.clone(), 1);
        let (runs, go) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        let warmer = Warmer::start(states.clone(), recipe(&runs, &go));
        let mut life = Lifecycle::default();
        life.publish(&bus, Some("a"), &ids(&["b"]));
        until("b warming", || runs.load(Ordering::SeqCst) == 1);
        // Away from "a": "b" is neither shown nor near any more.
        life.publish(&bus, Some("x"), &[]);
        until("b cancelled", || states.counters().cancelled == 1);
        until("nothing warming", || warmer.warming().is_empty());
        assert!(states.list().0.is_empty(), "nothing kept");
    }

    #[test]
    fn warm_ups_stay_within_the_budget() {
        let bus = Arc::new(Bus::new());
        let states = ImageStates::new(bus.clone(), 2);
        states.set_budget(Some(250));
        let (runs, go) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(true)),
        );
        let warmer = Warmer::start(states.clone(), recipe(&runs, &go));
        let mut life = Lifecycle::default();
        let photos: Vec<String> = (0..10).map(|i| format!("p{i}")).collect();
        for w in photos.windows(3) {
            life.publish(&bus, Some(&w[1]), &[w[0].clone(), w[2].clone()]);
            assert!(states.usage().bytes <= 250, "{:?}", states.usage());
        }
        until("every warm-up done", || warmer.warming().is_empty());
        let usage = states.usage();
        assert!(usage.bytes <= 250, "{usage:?}");
        assert!(states.counters().evicted > 0, "the budget let some go");
    }

    #[test]
    fn an_open_with_nothing_in_flight_computes_on_its_own_thread_and_keeps_it() {
        let states = ImageStates::new(Arc::new(Bus::new()), 1);
        let here = std::thread::current().id();
        let compute: Compute = Arc::new(move || {
            assert_eq!(std::thread::current().id(), here, "not queued on the pool");
            Ok(Ready::new(5u32, 10))
        });
        let value = states
            .get_here(decoded("a"), Priority::Interactive, compute, LONG)
            .expect("decoded");
        assert_eq!(*value.downcast_ref::<u32>().unwrap(), 5);
        // Kept: a second open is a ready hit.
        let never: Compute = Arc::new(|| unreachable!("computed again"));
        states
            .get_here(decoded("a"), Priority::Interactive, never, LONG)
            .expect("ready");
        let c = states.counters();
        assert_eq!((c.computed, c.ready_hits), (1, 1), "{c:?}");
    }

    #[test]
    fn a_background_state_waits_while_an_interactive_one_runs() {
        let states = ImageStates::new(Arc::new(Bus::new()), 2);
        let (runs, go) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        let shown = recipe(&runs, &go)("shown").remove(0).1;
        let mut open = states.ask(decoded("shown"), Priority::Interactive, shown);
        until("shown running", || runs.load(Ordering::SeqCst) == 1);
        let near_ran = Arc::new(AtomicBool::new(false));
        let n = near_ran.clone();
        let near: Compute = Arc::new(move || {
            n.store(true, Ordering::SeqCst);
            Ok(Ready::new(0u32, 0))
        });
        let mut warm = states.ask(decoded("near"), Priority::Prefetch, near);
        // A thread is free, but the interactive state runs.
        std::thread::sleep(Duration::from_millis(100));
        assert!(!near_ran.load(Ordering::SeqCst), "the warm-up waited");
        go.store(true, Ordering::SeqCst);
        open.wait(LONG).expect("shown");
        warm.wait(LONG).expect("near");
        assert!(near_ran.load(Ordering::SeqCst));
    }

    #[test]
    fn a_fast_scroll_over_a_thousand_photos_decodes_none() {
        let bus = Arc::new(Bus::new());
        let states = ImageStates::new(bus.clone(), 2);
        let (runs, go) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(true)),
        );
        let warmer = Warmer::start(states.clone(), recipe(&runs, &go));
        let mut life = Lifecycle::default();
        let photos: Vec<String> = (0..1000).map(|i| format!("p{i}")).collect();
        // A cell a millisecond: each photo near, then shown, then hidden,
        // well within the settle time.
        for w in photos.windows(3) {
            life.publish(&bus, Some(&w[1]), &[w[0].clone(), w[2].clone()]);
            std::thread::sleep(Duration::from_millis(1));
            assert_eq!(
                runs.load(Ordering::SeqCst),
                0,
                "a full decode while scrolling"
            );
        }
        let scrolled = warmer.counters();
        assert_eq!(scrolled.started, 0, "{scrolled:?}");
        assert!(scrolled.skipped > 900, "{scrolled:?}");
        assert_eq!(states.counters().computed, 0);
        // The scroll stops: the last photo's neighbour settles and warms.
        until("the neighbour warmed", || runs.load(Ordering::SeqCst) == 1);
        assert_eq!(warmer.counters().started, 1);
    }

    #[test]
    fn a_warm_up_never_shown_is_counted_unused_and_said() {
        let bus = Arc::new(Bus::new());
        let states = ImageStates::new(bus.clone(), 2);
        let (runs, go) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(true)),
        );
        let warmer = Warmer::start(states.clone(), recipe(&runs, &go));
        let mut life = Lifecycle::default();
        life.publish(&bus, Some("b"), &ids(&["a", "c"]));
        until("both warmed", || warmer.counters().started == 2);
        until("both ready", || warmer.warming().is_empty());
        // The user moves to "c" (used), "a" leaves unseen (unused).
        life.publish(&bus, Some("c"), &ids(&["b", "d"]));
        until("counted", || {
            let c = warmer.counters();
            c.used == 1 && c.unused == 1
        });
        let said: Vec<_> = bus
            .since(0)
            .into_iter()
            .filter(|e| e.kind == facts::WARM_UNUSED)
            .map(|e| e.payload["photo"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(said, ["a"]);
    }
}
