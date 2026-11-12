//! Helpers shared by the vault node's test targets (`vault_node`,
//! `stability`): a deadline wait, the disks a backup strategy sees, and
//! the photos with a copy on the backup disk. Each target uses a part.

#![allow(dead_code)]

use std::path::Path;
use std::time::{Duration, Instant};

use helios_storage::{Identity, Role, Volume, VolumeId, VolumeKind, detect};
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;

/// Waits until `done`, at most `deadline`; whether it came.
pub fn until(deadline: Duration, done: impl Fn() -> bool) -> bool {
    let end = Instant::now() + deadline;
    while !done() {
        if Instant::now() > end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

/// An external disk mounted at `mount` (made), with `roles`.
pub fn disk(id: &str, mount: &Path, roles: &[Role]) -> Volume {
    std::fs::create_dir_all(mount).unwrap();
    Volume {
        id: VolumeId(id.into()),
        identified_by: Identity::Uuid,
        name: id.into(),
        kind: VolumeKind::External,
        capacity: Some(1_000_000_000),
        free: Some(400_000_000),
        mount: Some(mount.to_path_buf()),
        roles: roles.iter().copied().collect(),
        ..detect::weak(id, None)
    }
}

/// The backup disk's id.
pub const BACKUP_DISK: &str = "uuid:backup";

/// Photos with a copy on the backup disk.
pub fn backed_up(vault: &Vault<SqliteStore>) -> usize {
    vault
        .images()
        .unwrap()
        .iter()
        .filter(|i| {
            vault
                .locations(&i.id)
                .unwrap()
                .iter()
                .any(|l| l.volume.as_ref().is_some_and(|v| v.as_str() == BACKUP_DISK))
        })
        .count()
}
