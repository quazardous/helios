//! Both stores pass the vault contract.

use helios_core::Describe;
use helios_vault::sqlite::SqliteStore;
use helios_vault::{MemoryStore, Vault, contract};

#[test]
fn the_memory_store_passes_the_contract() {
    contract::check(MemoryStore::default);
}

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
    {
        let v = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap());
        assert_eq!(v.import(&dir.join("photos"), 1).unwrap().added, 1);
    }
    let v = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap());
    assert_eq!(v.import(&dir.join("photos"), 2).unwrap().unchanged, 1);
    assert_eq!(
        v.reads().full_hashes,
        0,
        "known from the database: nothing read"
    );
    assert_eq!(v.describe()["images"], 1);
    let _ = std::fs::remove_dir_all(&dir);
}
