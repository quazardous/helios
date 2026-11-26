//! Each image's history kept across switches: its line parked when
//! another image opens, found again when it opens again.
//!
//! The store holds one image's values at a time, so a switch takes the
//! image's line out ([`Store::park_image`]) before the next image's
//! values take the same paths. Back on the image, [`Store::resume_image`]
//! puts its line back once its values are loaded, so Ctrl+Z undoes its
//! last step as if it had never been left.
//!
//! A line comes back only onto the values it left: each path its steps
//! touched must hold what the line's head says (the last undo step's
//! value, the next redo step's value before it). An edit made elsewhere
//! meanwhile (an agent, a batch, another node) moves them, and the line
//! is dropped rather than undone into values it never made.
//!
//! Memory is bounded: the parked lines are a kind of the cache service
//! ([`helios_cache::IMAGE_HISTORY`]), each costed by its estimated bytes,
//! within a budget of their own ([`helios_cache::HISTORY_BUDGET`] by
//! default): past it, the least recently left go first. Each keeps at most
//! [`PARKED_STEPS`] undo steps, the oldest dropped first. Lines are kept
//! for the session only: after a restart, or once evicted, the image's
//! line is rebuilt from its saved versions and installed the same way
//! ([`Store::install_image_line`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::mem::size_of;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use helios_cache::{Counters, IMAGE_HISTORY, Lru, Scope, Service, Tier, Usage};
use helios_core::Value;

use crate::history_scope::{HistoryScope, Parked};
use crate::points::Branch;
use crate::{Place, Step, Store};

/// The undo steps a parked line keeps: its oldest go first.
pub const PARKED_STEPS: usize = 500;

/// The parked image lines, attached to the cache service while the store
/// lives.
pub(crate) struct Images(Arc<Parking>);

/// The lines and their eviction order, shared with the cache service.
struct Parking {
    lines: Mutex<Lines>,
    counters: Arc<Counters>,
}

struct Lines {
    by_image: HashMap<String, Parked>,
    /// Their bytes, the least recently left first (a counter clock).
    order: Lru<String>,
    clock: u64,
    budget: Option<u64>,
}

impl Default for Images {
    fn default() -> Self {
        let parking = Arc::new(Parking {
            lines: Mutex::new(Lines {
                by_image: HashMap::new(),
                order: Lru::new(),
                clock: 0,
                budget: Some(helios_cache::HISTORY_BUDGET),
            }),
            counters: Service::global().counters(&Scope::Shared, &IMAGE_HISTORY),
        });
        let weak: Weak<dyn helios_cache::Store> = Arc::downgrade(&parking) as _;
        Service::global().attach(weak);
        Images(parking)
    }
}

impl Parking {
    fn lock(&self) -> MutexGuard<'_, Lines> {
        self.lines.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl helios_cache::Store for Parking {
    fn usage(&self) -> Vec<Usage> {
        let lines = self.lock();
        vec![Usage {
            scope: Scope::Shared,
            kind: IMAGE_HISTORY,
            tier: Tier::Ram,
            entries: lines.order.len() as u64,
            bytes: lines.order.bytes(),
            pinned: 0,
            budget: lines.budget,
            shown: None,
        }]
    }

    fn clear(&self, kind: Option<&str>) -> u64 {
        if kind.is_some_and(|k| k != IMAGE_HISTORY.id) {
            return 0;
        }
        let mut lines = self.lock();
        let gone = lines.order.clear(|_| false);
        Counters::add(&self.counters.cleared, gone.len() as u64);
        lines.by_image.clear();
        gone.iter().map(|(_, b)| b).sum()
    }
}

impl Lines {
    /// Evicts the least recently left lines past the budget, down to
    /// [`helios_cache::eviction_target`]: how many went.
    fn settle(&mut self) -> u64 {
        let Some(budget) = self.budget else { return 0 };
        if self.order.bytes() <= budget {
            return 0;
        }
        let gone = self
            .order
            .evict(helios_cache::eviction_target(budget), |_| false);
        for (image, _) in &gone {
            self.by_image.remove(image);
        }
        gone.len() as u64
    }
}

impl Images {
    /// Parks `line` under `image` (replacing one there), then evicts past
    /// the budget.
    fn park(&self, image: &str, line: Parked) {
        let mut lines = self.0.lock();
        lines.order.remove(&image.to_owned());
        lines.by_image.remove(image);
        if line.is_empty() {
            return;
        }
        lines.clock += 1;
        let (bytes, now) = (line.bytes(image), lines.clock);
        lines.order.insert(image.to_owned(), bytes, now, 0);
        lines.by_image.insert(image.to_owned(), line);
        let evicted = lines.settle();
        Counters::add(&self.0.counters.evictions, evicted);
    }

    /// Takes `image`'s line out, if parked.
    fn take(&self, image: &str) -> Option<Parked> {
        let mut lines = self.0.lock();
        lines.order.remove(&image.to_owned());
        let line = lines.by_image.remove(image);
        let counter = if line.is_some() {
            &self.0.counters.hits
        } else {
            &self.0.counters.misses
        };
        Counters::add(counter, 1);
        line
    }

    /// The images parked, the least recently left first.
    fn keys(&self) -> Vec<String> {
        self.0.lock().order.by_priority().cloned().collect()
    }

    /// The groups of the steps parked.
    pub(crate) fn groups(&self) -> BTreeSet<u64> {
        let lines = self.0.lock();
        lines
            .by_image
            .values()
            .flat_map(|p| p.undo.iter().chain(&p.redo))
            .filter_map(|s| s.group)
            .collect()
    }
}

/// A value's estimated bytes beyond its own size.
fn value_heap(v: &Value) -> usize {
    match v {
        Value::Choice(s) | Value::Text(s) => s.len(),
        Value::List(l) => l.len() * size_of::<f64>(),
        Value::Ints(l) => l.len() * size_of::<i64>(),
        Value::Float(_) | Value::Int(_) | Value::Bool(_) => 0,
    }
}

/// A step's estimated bytes in memory: its own size, its texts, places
/// and changes.
pub fn step_bytes(step: &Step) -> usize {
    let place = |p: &Place| size_of::<Place>() + p.frame.len() + p.payload.to_string().len();
    let change = |(path, before, after): &(String, Value, Value)| {
        size_of::<(String, Value, Value)>() + path.len() + value_heap(before) + value_heap(after)
    };
    size_of::<Step>()
        + step.label.len()
        + step.aim.as_ref().map_or(0, String::len)
        + step.place.iter().map(place).sum::<usize>()
        + step.changes.iter().map(change).sum::<usize>()
}

impl Parked {
    /// Its estimated bytes parked under `image`: its steps, its replaced
    /// lines' and the key.
    fn bytes(&self, image: &str) -> u64 {
        let steps = self
            .undo
            .iter()
            .chain(&self.redo)
            .chain(self.replaced.iter().flat_map(|b: &Branch| &b.steps));
        (image.len() + size_of::<Parked>() + steps.map(step_bytes).sum::<usize>()) as u64
    }

    fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty() && self.replaced.is_empty()
    }

    /// Drops its oldest undo steps past [`PARKED_STEPS`], and the
    /// replaced lines left from a step dropped.
    fn cap(&mut self) {
        let over = self.undo.len().saturating_sub(PARKED_STEPS);
        if over == 0 {
            return;
        }
        self.undo.drain(..over);
        let kept: BTreeSet<u64> = self.undo.iter().map(|s| s.id).collect();
        self.replaced.retain(|b| kept.contains(&b.after));
    }

    /// The value each path it touched holds at its head: after the last
    /// undo step, before the next redo step.
    fn head(&self) -> Vec<(&str, &Value)> {
        let mut undone: BTreeMap<&str, &Value> = BTreeMap::new();
        for step in &self.undo {
            for (path, _, after) in &step.changes {
                undone.insert(path, after);
            }
        }
        let mut redone: BTreeMap<&str, &Value> = BTreeMap::new();
        // The next redo is the last: the first change of each path in it
        // wins.
        for step in &self.redo {
            for (path, before, _) in step.changes.iter().rev() {
                redone.insert(path, before);
            }
        }
        undone.into_iter().chain(redone).collect()
    }
}

impl Store {
    /// The image `image` is left: its line (undo, redo and replaced) is
    /// parked under it, capped, the least recently left images' dropped
    /// past the budget ([`Store::set_parked_budget`]); then the image's
    /// history leaves as [`Store::leave_image`] says. An empty line parks
    /// nothing.
    pub fn park_image(&mut self, image: &str) {
        let mut line = self.in_history(HistoryScope::Image, |s| Parked {
            undo: std::mem::take(&mut s.undo),
            redo: std::mem::take(&mut s.redo),
            replaced: std::mem::take(&mut s.replaced),
        });
        line.cap();
        self.images.park(image, line);
        self.leave_image();
    }

    /// The image `image` opened again, its values loaded: its parked
    /// line comes back as the image's, when the values are still those
    /// it left and the image has no step of its own yet. Whether it came
    /// back; a line that does not fit is dropped.
    pub fn resume_image(&mut self, image: &str) -> bool {
        let Some(line) = self.images.take(image) else {
            return false;
        };
        self.put_image_line(line)
    }

    /// The image's line rebuilt from its saved versions (a restart, a
    /// crash: no line was parked), installed as its edit's history once
    /// its values are loaded: `undo` oldest first, `redo` the next redo
    /// first. Each step gets a new id; past [`PARKED_STEPS`] the oldest
    /// undo steps are dropped. Installed only onto the values the line
    /// reached and an image with no step of its own yet, as
    /// [`Store::resume_image`] does; whether it was.
    pub fn install_image_line(&mut self, undo: Vec<Step>, redo: Vec<Step>) -> bool {
        let mut line = Parked {
            undo,
            redo: redo.into_iter().rev().collect(),
            replaced: Vec::new(),
        };
        for step in line.undo.iter_mut().chain(line.redo.iter_mut()) {
            self.next_step += 1;
            step.id = self.next_step;
        }
        line.cap();
        if line.is_empty() {
            return false;
        }
        self.put_image_line(line)
    }

    /// `line` as the image's history, when it fits the values and the
    /// image has no step of its own yet; whether it did.
    fn put_image_line(&mut self, line: Parked) -> bool {
        let fits = line
            .head()
            .into_iter()
            .all(|(path, v)| self.get(path).is_some_and(|e| &e.value == v));
        let fresh = self.in_history(HistoryScope::Image, |s| {
            s.undo.is_empty() && s.redo.is_empty()
        });
        if !(fits && fresh) {
            return false;
        }
        self.in_history(HistoryScope::Image, |s| {
            s.undo = line.undo;
            s.redo = line.redo;
            s.replaced = line.replaced;
        });
        self.history_rev += 1;
        self.history_moved(None);
        self.announce();
        true
    }

    /// The images whose line is parked, the least recently left first.
    pub fn parked_images(&self) -> Vec<String> {
        self.images.keys()
    }

    /// The undo steps parked for `image`, oldest first.
    pub fn parked_undo_steps(&self, image: &str) -> Vec<Step> {
        let lines = self.images.0.lock();
        lines
            .by_image
            .get(image)
            .map_or_else(Vec::new, |p| p.undo.clone())
    }

    /// The bytes the parked lines are estimated to hold.
    pub fn parked_bytes(&self) -> u64 {
        self.images.0.lock().order.bytes()
    }

    /// The parked lines' budget in bytes (`None`: no limit); the least
    /// recently left go at once when over it.
    pub fn set_parked_budget(&mut self, budget: Option<u64>) {
        let mut lines = self.images.0.lock();
        lines.budget = budget;
        let evicted = lines.settle();
        Counters::add(&self.images.0.counters.evictions, evicted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Change, Descriptor, Set};
    use helios_shell::commands::Origin;

    const EV: &str = "node/exposure/exposure";
    const STATE: &str = "workspace/state";

    fn store() -> Store {
        let mut s = Store::new();
        s.declare(EV, Descriptor::float("Exposure", -100.0, 2000.0, 0.0));
        s.declare(STATE, Descriptor::text("workspace", "a"));
        s
    }

    fn set(s: &mut Store, v: f64) {
        s.set(Set::new(EV, Change::To(Value::Float(v)), Origin::User))
            .unwrap();
    }

    fn ev(s: &Store) -> Value {
        s.get(EV).unwrap().value.clone()
    }

    /// What a switch does to the values: the next image's loaded.
    fn load(s: &mut Store, v: f64) {
        s.declare_with(
            EV,
            Descriptor::float("Exposure", -100.0, 2000.0, 0.0),
            Value::Float(v),
        );
    }

    fn step(label: &str, before: f64, after: f64) -> Step {
        Step {
            id: 0,
            label: label.into(),
            origin: Origin::User,
            changes: vec![(EV.into(), Value::Float(before), Value::Float(after))],
            place: Vec::new(),
            group: None,
            aim: None,
        }
    }

    #[test]
    fn a_rebuilt_line_installed_is_undone_and_redone() {
        let mut s = store();
        load(&mut s, 2.0);
        let undo = vec![step("a", 0.0, 1.0), step("b", 1.0, 2.0)];
        let redo = vec![step("c", 2.0, 3.0), step("d", 3.0, 4.0)];
        assert!(s.install_image_line(undo, redo));
        let ids: Vec<u64> = s.undo_steps().iter().map(|s| s.id).collect();
        assert!(ids.iter().all(|&i| i != 0) && ids[0] != ids[1]);
        s.undo(&Origin::User).unwrap();
        assert_eq!(ev(&s), Value::Float(1.0));
        s.redo(&Origin::User).unwrap();
        s.redo(&Origin::User).unwrap();
        assert_eq!(ev(&s), Value::Float(3.0), "the next redo is c");
        s.redo(&Origin::User).unwrap();
        assert_eq!(ev(&s), Value::Float(4.0));
    }

    #[test]
    fn a_rebuilt_line_off_the_values_or_over_steps_is_not_installed() {
        let mut s = store();
        load(&mut s, 7.0);
        assert!(!s.install_image_line(vec![step("a", 0.0, 1.0)], Vec::new()));
        assert!(s.undo_steps().is_empty());
        set(&mut s, 1.0);
        assert!(
            !s.install_image_line(vec![step("a", 0.0, 1.0)], Vec::new()),
            "the image has a step of its own"
        );
        let mut s = store();
        load(&mut s, PARKED_STEPS as f64 + 10.0);
        let long: Vec<Step> = (0..PARKED_STEPS + 10)
            .map(|i| step("x", i as f64, i as f64 + 1.0))
            .collect();
        assert!(s.install_image_line(long, Vec::new()));
        assert_eq!(s.undo_steps().len(), PARKED_STEPS);
    }

    #[test]
    fn a_to_b_to_a_undoes_a_s_last_step() {
        let mut s = store();
        set(&mut s, 1.0);
        set(&mut s, 2.0);
        s.park_image("a");
        assert!(s.undo_steps().is_empty(), "B starts with no step");
        load(&mut s, 0.0);
        set(&mut s, 5.0);
        s.park_image("b");
        load(&mut s, 2.0);
        assert!(s.resume_image("a"));
        assert_eq!(s.undo_steps().len(), 2);
        s.undo(&Origin::User).unwrap();
        assert_eq!(ev(&s), Value::Float(1.0), "A's last step undone");
        // B's line is still parked, A's left again with its redo.
        assert_eq!(s.parked_undo_steps("b").len(), 1);
        s.park_image("a");
        load(&mut s, 5.0);
        assert!(s.resume_image("b"));
        s.undo(&Origin::User).unwrap();
        assert_eq!(ev(&s), Value::Float(0.0));
        load(&mut s, 1.0);
        s.park_image("b");
        assert!(s.resume_image("a"));
        assert_eq!(s.redo_steps().len(), 1, "A's redo kept");
        s.redo(&Origin::User).unwrap();
        assert_eq!(ev(&s), Value::Float(2.0));
    }

    #[test]
    fn a_line_does_not_come_back_onto_values_changed_meanwhile() {
        let mut s = store();
        set(&mut s, 1.0);
        s.park_image("a");
        // An agent edited A elsewhere: its values are not those left.
        load(&mut s, 3.0);
        assert!(!s.resume_image("a"));
        assert!(s.undo_steps().is_empty());
        assert_eq!(s.parked_images().len(), 0, "dropped");
    }

    #[test]
    fn the_workspace_s_steps_stay_their_own() {
        let mut s = store();
        set(&mut s, 1.0);
        s.set(Set::new(
            STATE,
            Change::To(Value::Text("b".into())),
            Origin::User,
        ))
        .unwrap();
        s.park_image("a");
        assert_eq!(s.parked_undo_steps("a").len(), 1, "the exposure only");
        assert_eq!(s.undo_steps_of(HistoryScope::Workspace).len(), 1);
        assert!(s.resume_image("a"));
        assert_eq!(s.undo_steps().len(), 1);
    }

    #[test]
    fn a_parked_line_keeps_its_newest_steps() {
        let mut s = store();
        for i in 1..=1000 {
            set(&mut s, f64::from(i));
        }
        s.park_image("first");
        assert_eq!(s.parked_undo_steps("first").len(), PARKED_STEPS);
        // A capped line still comes back and undoes its newest steps.
        assert!(s.resume_image("first"));
        s.undo(&Origin::User).unwrap();
        assert_eq!(ev(&s), Value::Float(999.0));
    }

    #[test]
    fn parked_lines_are_counted_by_their_bytes() {
        let mut s = store();
        assert_eq!(s.parked_bytes(), 0);
        set(&mut s, 1.0);
        s.park_image("a");
        let one = s.parked_bytes();
        let step = step_bytes(&s.parked_undo_steps("a")[0]) as u64;
        assert!(one >= step && step > EV.len() as u64, "{one} {step}");
        load(&mut s, 1.0);
        set(&mut s, 2.0);
        set(&mut s, 3.0);
        s.park_image("b");
        assert!(s.parked_bytes() > 2 * one, "b's two steps added");
        load(&mut s, 1.0);
        assert!(s.resume_image("a"));
        assert!(s.parked_bytes() < 2 * one + step, "a's taken out");
    }

    #[test]
    fn past_the_budget_the_least_recently_left_go_first() {
        let line = {
            let mut probe = store();
            set(&mut probe, 1.0);
            probe.park_image("probe");
            probe.parked_bytes()
        };
        let mut s = store();
        // Room for about four lines of one step.
        s.set_parked_budget(Some(line * 4 + line / 2));
        for i in 0..50 {
            load(&mut s, 0.0);
            set(&mut s, f64::from(i) + 1.0);
            s.park_image(&format!("img{i}"));
            assert!(s.parked_bytes() <= line * 4 + line / 2, "under the budget");
        }
        let kept = s.parked_images();
        assert!((2..=4).contains(&kept.len()), "{kept:?}");
        assert_eq!(kept.last().map(String::as_str), Some("img49"));
        assert!(kept.iter().all(|k| k.as_str() >= "img46"), "{kept:?}");
        // An evicted image's line is gone: it is rebuilt from its versions
        // (`Store::install_image_line`) instead.
        load(&mut s, 1.0);
        assert!(!s.resume_image("img0"));
        assert!(s.install_image_line(vec![step("a", 0.0, 1.0)], Vec::new()));
        assert_eq!(s.undo_steps().len(), 1);
    }

    #[test]
    fn no_budget_keeps_every_line_and_a_lower_one_evicts_at_once() {
        let mut s = store();
        s.set_parked_budget(None);
        for i in 0..30 {
            load(&mut s, 0.0);
            set(&mut s, f64::from(i) + 1.0);
            s.park_image(&format!("img{i:02}"));
        }
        assert_eq!(s.parked_images().len(), 30, "no fixed count");
        let all = s.parked_bytes();
        s.set_parked_budget(Some(all / 2));
        assert!(s.parked_bytes() <= all / 2);
        assert!(s.parked_images().iter().all(|k| k.as_str() > "img10"));
    }

    #[test]
    fn the_cache_service_sees_and_clears_the_parked_lines() {
        use helios_cache::Store as _;
        let mut s = store();
        set(&mut s, 1.0);
        s.park_image("a");
        let usage = s.images.0.usage();
        assert_eq!(usage[0].kind.id, helios_cache::IMAGE_HISTORY.id);
        assert_eq!((usage[0].entries, usage[0].bytes), (1, s.parked_bytes()));
        assert_eq!(usage[0].budget, Some(helios_cache::HISTORY_BUDGET));
        assert_eq!(s.images.0.clear(Some("render.step")), 0);
        let freed = s.images.0.clear(Some(helios_cache::IMAGE_HISTORY.id));
        assert!(freed > 0 && s.parked_images().is_empty());
        assert!(!s.resume_image("a"));
    }

    #[test]
    fn an_empty_line_parks_nothing() {
        let mut s = store();
        s.park_image("a");
        assert_eq!(s.parked_images().len(), 0);
        assert!(!s.resume_image("a"));
    }
}
