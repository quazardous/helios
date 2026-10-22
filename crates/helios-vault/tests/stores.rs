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

#[test]
fn a_journal_in_the_memory_vault_passes_the_lasting_contract() {
    use helios_vault::journal::VaultJournal;
    helios_journal::contract::check_lasting(
        || {
            std::sync::Arc::new(helios_vault::Vault::new(
                helios_vault::MemoryStore::default(),
            ))
        },
        |vault| VaultJournal::new(std::sync::Arc::clone(vault), "test"),
    );
}

#[test]
fn configuration_layers_round_trip_as_text() {
    let vault = helios_vault::Vault::new(helios_vault::MemoryStore::default());
    assert_eq!(vault.config_layer("user").unwrap(), None);
    vault
        .set_config_layer("user", Some("[interface.graph]\nzoom = \"overview\"\n"))
        .unwrap();
    vault
        .set_config_layer("project//photos/wedding", Some(""))
        .unwrap();
    assert_eq!(
        vault.config_layer("user").unwrap().as_deref(),
        Some("[interface.graph]\nzoom = \"overview\"\n")
    );
    assert_eq!(
        vault
            .config_layer("project//photos/wedding")
            .unwrap()
            .as_deref(),
        Some("")
    );
    vault.set_config_layer("user", None).unwrap();
    assert_eq!(vault.config_layer("user").unwrap(), None);
}
