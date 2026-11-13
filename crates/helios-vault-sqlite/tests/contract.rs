//! The SQLite driver passes the vault contract, and a vault on it keeps
//! what it knows across reopening.

use helios_core::Describe;
use helios_vault::{Vault, Version, contract};
use helios_vault_sqlite::SqliteStore;

#[test]
fn the_sqlite_store_passes_the_contract() {
    contract::check(|| SqliteStore::in_memory().unwrap());
}

/// On a file, the vault reads through connections of their own beside
/// its writer: every rule of the contract still holds.
#[test]
fn a_sqlite_vault_on_a_file_passes_the_contract_through_its_read_connections() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let dir =
        std::env::temp_dir().join(format!("helios-vault-file-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let next = AtomicU64::new(0);
    contract::check(|| {
        let n = next.fetch_add(1, Ordering::Relaxed);
        SqliteStore::open(&dir.join(format!("vault-{n}.db"))).unwrap()
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// A read on a vault file never waits for a write, however long: with the
/// writer held in an open transaction, reads (an image, its edit head, its
/// fields, the stacks) answer from what is committed.
#[test]
fn reads_do_not_wait_for_a_write_held_open() {
    let dir = std::env::temp_dir().join(format!("helios-vault-held-write-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = SqliteStore::open(&dir.join("vault.db")).unwrap();
    contract::reads_do_not_wait_for_a_write(store, std::time::Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&dir);
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
        // Metadata comes back too, and leaves the id alone.
        let meta = Some(serde_json::json!({ "workshop_session": "s-1" }));
        let tagged = Version::new(image, vec![first.id], edit.clone(), "user", 2).with_meta(meta);
        assert_eq!(
            tagged.id,
            Version::new(image, vec![first.id], edit.clone(), "user", 2).id
        );
        v.add_version(&tagged).unwrap();
        v.set_workshop_session("s-1", Some(&serde_json::json!({ "kept": true })))
            .unwrap();
        tagged
    };
    let v = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap());
    assert_eq!(v.import(&dir.join("photos"), 2).unwrap().unchanged, 1);
    assert_eq!(
        v.reads().full_hashes,
        0,
        "known from the database: nothing read"
    );
    assert_eq!(v.describe()["images"], 1);
    // The edit history came back with the images, unchanged, and the
    // session records with it.
    assert_eq!(v.heads(&first.image).unwrap(), std::slice::from_ref(&first));
    assert_eq!(
        first.meta,
        Some(serde_json::json!({ "workshop_session": "s-1" }))
    );
    assert_eq!(
        v.workshop_session("s-1").unwrap(),
        Some(serde_json::json!({ "kept": true }))
    );
    assert!(
        !v.add_version(&Version::new(
            first.image,
            first.parents.clone(),
            edit,
            "agent",
            9
        ))
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

/// An image whose edit diverged many times along a long history (clients
/// saving the same image at once each leave a head) opens at its newest
/// head, the one a walk of each head's history picks, without that walk:
/// it read the store once per version of every head, holding the store,
/// and so every other call on the vault, all along. Timed against the
/// walk on the same vault, so a loaded machine slows both alike.
#[test]
fn an_image_with_many_diverged_heads_opens_without_a_walk_per_head() {
    use std::time::{Duration, Instant};

    use helios_core::ContentHash;
    use helios_vault::{EditSession, Image, ImageId, Store, VersionId, versions};

    const LINE: u64 = 400;
    const BRANCHES: u64 = 60;
    let id = ImageId(ContentHash([7; 32]));
    let mut store = SqliteStore::in_memory().unwrap();
    store
        .add_image(&Image {
            id,
            size: 100,
            partial: ContentHash([7; 32]),
        })
        .unwrap();
    let v = Vault::new(store);
    let mut line: Vec<VersionId> = Vec::new();
    for step in 0..LINE {
        let parents = line.last().copied().into_iter().collect();
        let version = Version::new(
            id,
            parents,
            serde_json::json!({ "step": step }),
            "user",
            step,
        );
        v.add_version(&version).unwrap();
        line.push(version.id);
    }
    // A save that lost the race at each of the last steps, all at the
    // same second; two of them on the tip, the longest lines.
    let mut tips = Vec::new();
    for k in 0..BRANCHES {
        let parent = line[(LINE - 1 - k) as usize];
        let edit = serde_json::json!({ "branch": k });
        let version = Version::new(id, vec![parent], edit, "agent", LINE + 10);
        v.add_version(&version).unwrap();
        if k == 0 {
            tips.push(version.id);
        }
    }
    let other = Version::new(
        id,
        vec![line[(LINE - 1) as usize]],
        serde_json::json!({ "other": 1 }),
        "agent",
        LINE + 10,
    );
    v.add_version(&other).unwrap();
    tips.push(other.id);
    assert_eq!(v.heads(&id).unwrap().len() as u64, BRANCHES + 1);

    // The walk: each head's history read from the store.
    let walk = || {
        let all = v.versions(&id).unwrap();
        versions::heads(&all)
            .into_iter()
            .map(|h| (h.at, v.history(&h.id).unwrap().len(), h.id))
            .max()
            .map(|(_, _, id)| id)
    };
    let open = || EditSession::open(&v, id).unwrap().1.map(|h| h.id);
    let best = |f: &dyn Fn() -> Option<VersionId>| {
        (0..3)
            .map(|_| {
                let at = Instant::now();
                let head = f();
                (at.elapsed(), head)
            })
            .min_by_key(|(took, _)| *took)
            .unwrap()
    };
    let (walked, by_walk) = best(&walk);
    let (opened, by_open) = best(&open);
    let newest = tips.iter().max().copied();
    assert_eq!(
        by_walk, newest,
        "the latest time, the longest line, the greatest id"
    );
    assert_eq!(by_open, newest, "open picks the head the walk picks");
    assert!(
        opened * 5 < walked.max(Duration::from_millis(1)),
        "opening took {opened:?}, the walk of every head {walked:?}: open walks the store again"
    );
}
