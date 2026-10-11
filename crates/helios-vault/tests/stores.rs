//! The memory store passes the vault contract (drivers pass it in their
//! own crates).

use helios_vault::{MemoryStore, contract};

#[test]
fn the_memory_store_passes_the_contract() {
    contract::check(MemoryStore::default);
}

#[test]
fn a_journal_in_the_memory_vault_passes_the_journal_contract() {
    use helios_vault::journal::VaultJournal;
    helios_journal::contract::check(|| {
        VaultJournal::new(
            std::sync::Arc::new(helios_vault::Vault::new(
                helios_vault::MemoryStore::default(),
            )),
            "test",
        )
    });
}
