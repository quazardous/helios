//! The eviction index and the pins.

use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;
use std::sync::{Arc, Mutex, PoisonError};

/// One entry's place in the order.
#[derive(Clone, Copy)]
struct Slot {
    /// When it goes: its last use plus its bonus.
    priority: u64,
    /// Breaks ties: the entry put or used first goes first.
    seq: u64,
    bytes: u64,
    /// How much later it goes than an entry used at the same time: the
    /// cost of a miss (see [`crate::Cost::bonus`]).
    bonus: u64,
}

/// A cost-aware least-recently-used index (GreedyDual-Size style): each
/// entry's priority is its last use plus a bonus for what a miss would
/// cost, and the lowest priority goes first. Times are the caller's clock
/// (a counter, nanoseconds): the index only compares them.
///
/// The order is kept sorted, so [`Lru::evict`] walks from the lowest
/// priority and stops once under its target: its cost is the victims',
/// not the entries'.
pub struct Lru<K> {
    order: BTreeSet<(u64, u64, K)>,
    slots: HashMap<K, Slot>,
    seq: u64,
    bytes: u64,
}

impl<K> Default for Lru<K> {
    fn default() -> Self {
        Lru {
            order: BTreeSet::new(),
            slots: HashMap::new(),
            seq: 0,
            bytes: 0,
        }
    }
}

impl<K: Clone + Ord + Hash> Lru<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many entries it holds.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The bytes its entries hold.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn contains(&self, key: &K) -> bool {
        self.slots.contains_key(key)
    }

    /// The bytes an entry holds, when it is there.
    pub fn size(&self, key: &K) -> Option<u64> {
        self.slots.get(key).map(|s| s.bytes)
    }

    /// The keys, lowest priority (next to go) first.
    pub fn by_priority(&self) -> impl Iterator<Item = &K> {
        self.order.iter().map(|(_, _, k)| k)
    }

    /// The entries and their bytes, in no order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, u64)> {
        self.slots.iter().map(|(k, s)| (k, s.bytes))
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// Puts `key`, holding `bytes`, used at `now`, going `bonus` later
    /// than an entry used at the same time; replaces it when there. The
    /// bytes it held before, if it was there.
    pub fn insert(&mut self, key: K, bytes: u64, now: u64, bonus: u64) -> Option<u64> {
        let old = self.remove(&key);
        let seq = self.next_seq();
        let priority = now.saturating_add(bonus);
        self.order.insert((priority, seq, key.clone()));
        self.slots.insert(
            key,
            Slot {
                priority,
                seq,
                bytes,
                bonus,
            },
        );
        self.bytes += bytes;
        old
    }

    /// Marks `key` used at `now` (its bonus kept); false when it is not
    /// there.
    pub fn touch(&mut self, key: &K, now: u64) -> bool {
        let seq = self.next_seq();
        let Some(slot) = self.slots.get_mut(key) else {
            return false;
        };
        self.order.remove(&(slot.priority, slot.seq, key.clone()));
        slot.priority = now.saturating_add(slot.bonus);
        slot.seq = seq;
        self.order.insert((slot.priority, seq, key.clone()));
        true
    }

    /// Counts `bytes` for `key` from now on (its content grew or was
    /// measured); false when it is not there.
    pub fn resize(&mut self, key: &K, bytes: u64) -> bool {
        let Some(slot) = self.slots.get_mut(key) else {
            return false;
        };
        self.bytes = self.bytes - slot.bytes + bytes;
        slot.bytes = bytes;
        true
    }

    /// Takes `key` out: the bytes it held.
    pub fn remove(&mut self, key: &K) -> Option<u64> {
        let slot = self.slots.remove(key)?;
        self.order.remove(&(slot.priority, slot.seq, key.clone()));
        self.bytes -= slot.bytes;
        Some(slot.bytes)
    }

    /// Takes out the entries of lowest priority until the index holds at
    /// most `target` bytes, never one `keep` says to keep (pinned, being
    /// written): the entries taken and their bytes, first taken first.
    pub fn evict(&mut self, target: u64, mut keep: impl FnMut(&K) -> bool) -> Vec<(K, u64)> {
        let mut gone = Vec::new();
        if self.bytes <= target {
            return gone;
        }
        let mut freed = 0;
        for (_, _, key) in &self.order {
            if self.bytes - freed <= target {
                break;
            }
            if keep(key) {
                continue;
            }
            let bytes = self.slots[key].bytes;
            freed += bytes;
            gone.push((key.clone(), bytes));
        }
        for (key, _) in &gone {
            self.remove(key);
        }
        gone
    }

    /// Takes out every entry `keep` does not keep: those taken and their
    /// bytes.
    pub fn clear(&mut self, mut keep: impl FnMut(&K) -> bool) -> Vec<(K, u64)> {
        let gone: Vec<(K, u64)> = self
            .slots
            .iter()
            .filter(|(k, _)| !keep(k))
            .map(|(k, s)| (k.clone(), s.bytes))
            .collect();
        for (key, _) in &gone {
            self.remove(key);
        }
        gone
    }
}

/// Keys that must not be evicted, each pinned by as many [`Pin`]s as hold
/// it. Clones share the same set.
pub struct Pins<K>(Arc<Mutex<HashMap<K, usize>>>);

impl<K> Clone for Pins<K> {
    fn clone(&self) -> Self {
        Pins(self.0.clone())
    }
}

impl<K> Default for Pins<K> {
    fn default() -> Self {
        Pins(Arc::default())
    }
}

impl<K: Clone + Eq + Hash> Pins<K> {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<K, usize>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Pins `key` until the guard is dropped: a crashed holder cannot
    /// leak a pin.
    #[must_use = "the key is unpinned when the guard drops"]
    pub fn pin(&self, key: K) -> Pin<K> {
        *self.lock().entry(key.clone()).or_default() += 1;
        Pin {
            pins: self.clone(),
            key,
        }
    }

    pub fn is_pinned(&self, key: &K) -> bool {
        self.lock().contains_key(key)
    }

    /// How many keys are pinned.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn unpin(&self, key: &K) {
        let mut pins = self.lock();
        if let Some(n) = pins.get_mut(key) {
            *n -= 1;
            if *n == 0 {
                pins.remove(key);
            }
        }
    }
}

/// A pin on one key: dropped, it unpins.
pub struct Pin<K: Clone + Eq + Hash> {
    pins: Pins<K>,
    key: K,
}

impl<K: Clone + Eq + Hash> Pin<K> {
    pub fn key(&self) -> &K {
        &self.key
    }
}

impl<K: Clone + Eq + Hash> Drop for Pin<K> {
    fn drop(&mut self) {
        self.pins.unpin(&self.key);
    }
}
