//! The SQLite driver passes the vault contract, and a vault on it keeps
//! what it knows across reopening.

use helios_core::Describe;
use helios_vault::{Vault, Version, contract};
use helios_vault_sqlite::SqliteStore;

#[test]
fn the_sqlite_store_passes_the_contract() {
    contract::check(|| SqliteStore::in_memory().unwrap());
}

#[test]
fn a_sqlite_vault_survives_reopening() {
    let dir = std::env::temp_dir().join(format!("helios-vault-reopen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("photos")).unwrap();
    std::fs::write(dir.join("photos/a.cr3"), b"raw bytes").unwrap();
    let edit = serde_json::json!({ "modules": [{ "name": "exp", "module": "exposure" }] });
    let first = {
        let v = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap());
        assert_eq!(v.import(&dir.join("photos"), 1).unwrap().added, 1);
        let image = v.images().unwrap()[0].id;
        let first = Version::new(image, vec![], edit.clone(), "user", 1).labelled("first");
        v.add_version(&first).unwrap();
        first
    };
    let v = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap());
    assert_eq!(v.import(&dir.join("photos"), 2).unwrap().unchanged, 1);
    assert_eq!(
        v.reads().full_hashes,
        0,
        "known from the database: nothing read"
    );
    assert_eq!(v.describe()["images"], 1);
    // The edit history came back with the images, unchanged.
    assert_eq!(v.heads(&first.image).unwrap(), std::slice::from_ref(&first));
    assert!(
        !v.add_version(&Version::new(first.image, vec![], edit, "agent", 9))
            .unwrap()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_journal_in_a_sqlite_vault_passes_the_journal_contract() {
    use helios_vault::journal::VaultJournal;
    helios_journal::contract::check(|| {
        VaultJournal::new(
            std::sync::Arc::new(Vault::new(SqliteStore::in_memory().unwrap())),
            "test",
        )
    });
}

#[test]
fn a_journal_in_a_sqlite_vault_passes_the_lasting_contract() {
    use helios_vault::journal::VaultJournal;
    helios_journal::contract::check_lasting(
        || std::sync::Arc::new(Vault::new(SqliteStore::in_memory().unwrap())),
        |vault| VaultJournal::new(std::sync::Arc::clone(vault), "test"),
    );
}
