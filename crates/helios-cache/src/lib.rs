//! Helios's cache service (see `docs/design/cache.md`): one per process,
//! owning the accounting of every cache, a budget per tier, one eviction
//! policy, pins, request merging and statistics.
//!
//! Features do not own caches: they declare a [`Kind`] and keep their
//! entries in a [`Store`] attached to the [`Service`], which can then say
//! what is held (per scope, kind and tier) and clear it.
//!
//! - [`Lru`]: the eviction index, a cost-aware least-recently-used order
//!   kept sorted, so evicting is a walk from its start, never a full scan
//!   per victim;
//! - [`Pins`]: entries that must not be evicted, each held by a guard
//!   ([`Pin`]) that unpins when dropped;
//! - [`Flight`]: single flight, concurrent requests for one key merged
//!   into one computation, every wait bounded;
//! - [`Service`]: kinds, budgets per tier, the stores attached, their
//!   usage and statistics, and clearing.

mod flight;
mod lru;
mod service;

pub use flight::{Flight, FlightError};
pub use lru::{Lru, Pin, Pins};
pub use service::{
    Cost, Counters, IMPORT_GLANCE, Identity, Kind, RENDER_STEP, Scope, ScopeRule, Service, Share,
    Store, THUMB_LARGE, THUMB_SMALL, Tier, Usage, memory_share, physical_memory, size_text,
};

/// The share of physical memory the RAM tier may use by default, in
/// percent.
pub const DEFAULT_RAM_SHARE: f64 = 25.0;

/// The RAM tier's budget where physical memory is not known: 2 GiB.
pub const UNKNOWN_MEMORY_BUDGET: usize = 2 << 30;

/// The fraction of a budget eviction brings a tier back under, so a full
/// cache does not evict at every write: 90 %.
pub fn eviction_target(budget: u64) -> u64 {
    budget - budget / 10
}
