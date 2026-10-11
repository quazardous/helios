//! The memory store passes the vault contract (drivers pass it in their
//! own crates).

use helios_vault::{MemoryStore, contract};

#[test]
fn the_memory_store_passes_the_contract() {
    contract::check(MemoryStore::default);
}
