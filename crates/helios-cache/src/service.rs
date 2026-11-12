//! The service: kinds, budgets per tier, the stores attached, their usage
//! and statistics, and clearing.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::Duration;

use serde_json::{Value, json};

/// Where the bytes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tier {
    Ram,
    Vram,
    Disk,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Ram, Tier::Vram, Tier::Disk];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Ram => "ram",
            Tier::Vram => "vram",
            Tier::Disk => "disk",
        }
    }
}

/// How dear a miss is: what it takes to make the entry again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cost {
    /// A copy, a crop.
    Cheap,
    /// Reading and decoding a file.
    Decode,
    /// Rendering through modules.
    Render,
    /// Asking another machine or a service.
    Network,
}

impl Cost {
    pub fn as_str(self) -> &'static str {
        match self {
            Cost::Cheap => "cheap",
            Cost::Decode => "decode",
            Cost::Render => "render",
            Cost::Network => "network",
        }
    }

    /// How much later than an entry of [`Cost::Cheap`] used at the same
    /// time an entry goes, in nanoseconds of last use (for [`crate::Lru`]
    /// on a nanosecond clock): its class's head start, plus `produced`,
    /// the time it took to make, per MiB it holds. A render step that took
    /// 800 ms outlives a 2 ms crop of the same size.
    pub fn bonus(self, produced_ns: u64, bytes: u64) -> u64 {
        let class: u64 = match self {
            Cost::Cheap => 0,
            Cost::Decode => 1_000_000_000,
            Cost::Render => 2_000_000_000,
            Cost::Network => 10_000_000_000,
        };
        let mib = (bytes >> 20).max(1);
        class.saturating_add(produced_ns.saturating_mul(16) / mib)
    }
}

/// What an entry's key is made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    /// What defines the bytes: the rule.
    Content,
    /// A path and a time: allowed, shown as debt.
    Path,
}

/// A kind's place in its tier's budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Share {
    /// Competes with the others by cost.
    Default,
    /// Never evicted by other kinds below this many bytes.
    Reserved(u64),
}

/// Where a kind's entries live by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeRule {
    /// The process's shared pool.
    Shared,
    /// A silo per library (privacy: its data stays with its vault).
    PerLibrary,
    /// A silo per user.
    PerUser,
    /// A silo of its own (a third party, an unborrowable budget).
    Private,
}

/// A kind of entry, declared once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    /// Stable: settings, statistics and folders use it.
    pub id: &'static str,
    /// What the Caches page calls it.
    pub title: &'static str,
    pub tiers: &'static [Tier],
    pub identity: Identity,
    pub cost: Cost,
    pub share: Share,
    /// Its disk tier survives restarts.
    pub persist: bool,
    /// Bumped when the format changes: older entries are dropped.
    pub version: u32,
    /// Its disk entries not used for this long are pruned, on top of the
    /// size budget; `None`: only the budget evicts them.
    pub unused: Option<Duration>,
    pub scope: ScopeRule,
    /// Why it is not in the shared pool, for a silo.
    pub reason: &'static str,
}

/// The library's 256 px thumbnails.
pub const THUMB_SMALL: Kind = Kind {
    id: "thumb.256",
    title: "Thumbnails",
    tiers: &[Tier::Disk],
    identity: Identity::Content,
    cost: Cost::Decode,
    share: Share::Default,
    persist: true,
    version: 2,
    // A Library photo's thumbnail is all an offline photo shows: kept
    // while the budget allows, whatever its age.
    unused: None,
    scope: ScopeRule::PerLibrary,
    reason: "a library's thumbnails stay beside its catalogue and move with it",
};

/// The library's 1024 px thumbnails (Compare, the loupe's first frame).
pub const THUMB_LARGE: Kind = Kind {
    id: "thumb.1024",
    title: "Large thumbnails",
    ..THUMB_SMALL
};

/// A render step's outputs: the render cache.
pub const RENDER_STEP: Kind = Kind {
    id: "render.step",
    title: "Rendered images",
    tiers: &[Tier::Ram],
    identity: Identity::Content,
    cost: Cost::Render,
    share: Share::Default,
    persist: false,
    version: 1,
    unused: None,
    scope: ScopeRule::Shared,
    reason: "",
};

/// How long an import preview not seen again is kept on disk: 30 days.
pub const GLANCE_UNUSED: Duration = Duration::from_hours(30 * 24);

/// The import window's glances of files not yet in the Library: each
/// file's small preview, capture time and camera, kept in RAM by path,
/// size and modification time (no content hash is read before an import),
/// so the window opened again or scrolled back reads nothing; a card's
/// also on disk by the card's volume, the path on it, size and
/// modification time, so a card plugged in again, even after a restart or
/// at another mount point, shows its previews without a read.
pub const IMPORT_GLANCE: Kind = Kind {
    id: "import.glance",
    title: "Import previews",
    tiers: &[Tier::Ram, Tier::Disk],
    identity: Identity::Path,
    cost: Cost::Decode,
    share: Share::Default,
    persist: true,
    version: 2,
    unused: Some(GLANCE_UNUSED),
    scope: ScopeRule::Shared,
    reason: "",
};

/// Where entries live: the shared pool, or a silo.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    Shared,
    Silo(String),
}

impl Scope {
    pub fn silo(name: impl Into<String>) -> Scope {
        Scope::Silo(name.into())
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::Shared => f.write_str("shared"),
            Scope::Silo(name) => write!(f, "silo:{name}"),
        }
    }
}

/// What a store holds of one kind in one scope and tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    pub scope: Scope,
    pub kind: Kind,
    pub tier: Tier,
    pub entries: u64,
    pub bytes: u64,
    pub pinned: u64,
    /// The scope's own budget in this tier, for a silo with one.
    pub budget: Option<u64>,
}

/// A cache attached to the service: it says what it holds and clears it.
pub trait Store: Send + Sync {
    /// What it holds, per scope, kind and tier.
    fn usage(&self) -> Vec<Usage>;

    /// Drops its entries of `kind` (every kind: `None`), never pinned
    /// ones or ones being computed: the bytes freed.
    fn clear(&self, kind: Option<&str>) -> u64;
}

/// One scope's and kind's statistics, counted atomically.
#[derive(Debug, Default)]
pub struct Counters {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    /// Requests merged into another's computation.
    pub merged: AtomicU64,
    /// Entries evicted to stay within a budget.
    pub evictions: AtomicU64,
    /// Entries dropped by a clear.
    pub cleared: AtomicU64,
    pub bytes_written: AtomicU64,
    pub bytes_read: AtomicU64,
    /// Time spent producing misses.
    pub produce_ns: AtomicU64,
}

impl Counters {
    pub fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    fn json(&self) -> Value {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        json!({
            "hits": get(&self.hits),
            "misses": get(&self.misses),
            "merged": get(&self.merged),
            "evictions": get(&self.evictions),
            "cleared": get(&self.cleared),
            "bytes_written": get(&self.bytes_written),
            "bytes_read": get(&self.bytes_read),
            "produce_ms": get(&self.produce_ns) / 1_000_000,
        })
    }
}

/// This machine's physical memory in bytes, where it is known (Linux:
/// `MemTotal` of `/proc/meminfo`).
pub fn physical_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// A budget of `share` percent of `memory` bytes (2 GiB when memory is
/// not known); `None`, no limit, for a share of 0.
pub fn memory_share(share: f64, memory: Option<u64>) -> Option<usize> {
    if share <= 0.0 {
        return None;
    }
    Some(memory.map_or(crate::UNKNOWN_MEMORY_BUDGET, |m| {
        (m as f64 * share.min(100.0) / 100.0) as usize
    }))
}

/// A size for people: `512 KiB`, `12.5 MiB`, `2.0 GiB`.
pub fn size_text(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KIB * KIB {
        format!("{:.0} KiB", (b / KIB).ceil())
    } else if b < KIB * KIB * KIB {
        format!("{:.1} MiB", b / KIB / KIB)
    } else {
        format!("{:.1} GiB", b / KIB / KIB / KIB)
    }
}

type CounterKey = (Scope, &'static str);

/// The cache service: one per process ([`Service::global`]); tests make
/// their own ([`Service::new`]).
pub struct Service {
    budgets: Mutex<HashMap<Tier, Option<u64>>>,
    kinds: Mutex<BTreeMap<&'static str, Kind>>,
    stores: Mutex<Vec<Weak<dyn Store>>>,
    counters: Mutex<BTreeMap<CounterKey, Arc<Counters>>>,
}

impl Default for Service {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Service {
    /// A service whose RAM tier has its default budget
    /// ([`crate::DEFAULT_RAM_SHARE`] of physical memory), the other tiers
    /// none of their own (their stores keep theirs).
    pub fn new() -> Service {
        let ram = memory_share(crate::DEFAULT_RAM_SHARE, physical_memory()).map(|b| b as u64);
        Service {
            budgets: Mutex::new(HashMap::from([(Tier::Ram, ram)])),
            kinds: Mutex::default(),
            stores: Mutex::default(),
            counters: Mutex::default(),
        }
    }

    /// The process's service.
    pub fn global() -> &'static Service {
        static GLOBAL: OnceLock<Service> = OnceLock::new();
        GLOBAL.get_or_init(Service::new)
    }

    /// A tier's budget in bytes; `None` for no limit.
    pub fn budget(&self, tier: Tier) -> Option<u64> {
        lock(&self.budgets).get(&tier).copied().flatten()
    }

    /// Sets a tier's budget (`None`: no limit). Stores read it when they
    /// settle, so it applies from their next eviction.
    pub fn set_budget(&self, tier: Tier, budget: Option<u64>) {
        lock(&self.budgets).insert(tier, budget);
    }

    /// Declares a kind (again: replaced).
    pub fn register(&self, kind: Kind) {
        lock(&self.kinds).insert(kind.id, kind);
    }

    /// The kinds declared, by id.
    pub fn kinds(&self) -> Vec<Kind> {
        lock(&self.kinds).values().copied().collect()
    }

    /// Attaches a store: its usage is counted and it is cleared with the
    /// rest until it is dropped.
    pub fn attach(&self, store: Weak<dyn Store>) {
        let mut stores = lock(&self.stores);
        stores.retain(|s| s.strong_count() > 0);
        stores.push(store);
    }

    fn stores(&self) -> Vec<Arc<dyn Store>> {
        let mut stores = lock(&self.stores);
        stores.retain(|s| s.strong_count() > 0);
        stores.iter().filter_map(Weak::upgrade).collect()
    }

    /// The statistics of `kind` in `scope`, shared by whoever counts them.
    pub fn counters(&self, scope: &Scope, kind: &Kind) -> Arc<Counters> {
        self.register(*kind);
        lock(&self.counters)
            .entry((scope.clone(), kind.id))
            .or_default()
            .clone()
    }

    /// What every store holds, merged per scope, kind and tier.
    pub fn usage(&self) -> Vec<Usage> {
        let mut merged: BTreeMap<(Scope, &'static str, Tier), Usage> = BTreeMap::new();
        for usage in self.stores().iter().flat_map(|s| s.usage()) {
            match merged.entry((usage.scope.clone(), usage.kind.id, usage.tier)) {
                std::collections::btree_map::Entry::Vacant(v) => {
                    v.insert(usage);
                }
                std::collections::btree_map::Entry::Occupied(mut o) => {
                    let u = o.get_mut();
                    u.entries += usage.entries;
                    u.bytes += usage.bytes;
                    u.pinned += usage.pinned;
                    u.budget = u.budget.or(usage.budget);
                }
            }
        }
        merged.into_values().collect()
    }

    /// The bytes held in a tier, every scope.
    pub fn used(&self, tier: Tier) -> u64 {
        self.usage()
            .iter()
            .filter(|u| u.tier == tier)
            .map(|u| u.bytes)
            .sum()
    }

    /// Clears the entries of `kind` (every kind: `None`) in every store,
    /// never pinned ones: the bytes freed.
    pub fn clear(&self, kind: Option<&str>) -> u64 {
        self.stores().iter().map(|s| s.clear(kind)).sum()
    }

    /// Everything as JSON, for the `helios.cache` collector and the
    /// `cache.stats` command: each tier's budget and use, and per scope,
    /// kind and tier the entries, bytes, pins and statistics.
    pub fn snapshot(&self) -> Value {
        let usage = self.usage();
        let counters = lock(&self.counters).clone();
        let tiers: Vec<Value> = Tier::ALL
            .iter()
            .map(|t| {
                let used: u64 = usage.iter().filter(|u| u.tier == *t).map(|u| u.bytes).sum();
                json!({ "tier": t.as_str(), "budget": self.budget(*t), "used": used })
            })
            .collect();
        let mut kinds: Vec<Value> = usage
            .iter()
            .map(|u| {
                let stats = counters
                    .get(&(u.scope.clone(), u.kind.id))
                    .map_or_else(|| Counters::default().json(), |c| c.json());
                json!({
                    "kind": u.kind.id,
                    "title": u.kind.title,
                    "scope": u.scope.to_string(),
                    "tier": u.tier.as_str(),
                    "entries": u.entries,
                    "bytes": u.bytes,
                    "pinned": u.pinned,
                    "budget": u.budget,
                    "identity": match u.kind.identity {
                        Identity::Content => "content",
                        Identity::Path => "path",
                    },
                    "cost": u.kind.cost.as_str(),
                    "stats": stats,
                })
            })
            .collect();
        // Counted but holding nothing now (cleared, not opened yet).
        for ((scope, id), c) in &counters {
            let scope = scope.to_string();
            if !kinds
                .iter()
                .any(|k| k["kind"] == *id && k["scope"] == scope.as_str())
            {
                let kind = lock(&self.kinds).get(id).copied();
                kinds.push(json!({
                    "kind": id,
                    "title": kind.map_or(*id, |k| k.title),
                    "scope": scope,
                    "tier": kind.and_then(|k| k.tiers.first()).map_or("ram", |t| t.as_str()),
                    "entries": 0, "bytes": 0, "pinned": 0, "budget": null,
                    "stats": c.json(),
                }));
            }
        }
        json!({ "tiers": tiers, "kinds": kinds })
    }
}
