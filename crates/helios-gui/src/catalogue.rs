//! Catalogue snapshots on the desktop: verified copies of the library's
//! vault database (`helios_vault_sqlite::snapshot`), taken on a schedule
//! (`catalogue.snapshotEvery`, in hours, daily by default), before a
//! migration (the vault does it) and by hand (`library.catalogue.snapshot`),
//! kept next to the catalogue (`catalogue.snapshotsKept`, five by
//! default) and copied, verified, to the backup disks of the library's
//! strategy plugged in, under `Helios Backup/catalogue/<library>/`.
//!
//! Restoring one (`library.catalogue.restore`) asks the vault to replace
//! the catalogue at its next opening: Helios closes and starts again,
//! the current catalogue kept aside, the snapshot checked before and
//! after it is put in place.

use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use egui_pop::core::binding::SharedStore;
use egui_pop::model::ids::SettingId;
use helios_inspect::client::Client;
use helios_storage::strategy::{self, Strategy};
use helios_storage::{Volume, registry};
use helios_vault_sqlite::snapshot;
use serde::Serialize;

/// Hours between scheduled snapshots, at `settings/catalogue.snapshotEvery`.
pub const SETTING_EVERY: SettingId = SettingId::from_static("catalogue.snapshotEvery");

/// Snapshots kept next to the catalogue and on each backup disk, at
/// `settings/catalogue.snapshotsKept`.
pub const SETTING_KEPT: SettingId = SettingId::from_static("catalogue.snapshotsKept");

/// Hours between scheduled snapshots by default: daily.
pub const EVERY_HOURS: u64 = 24;

/// Snapshots kept by default.
pub const KEPT: usize = 5;

/// Takes a snapshot now.
pub const SNAPSHOT_COMMAND: &str = "library.catalogue.snapshot";

/// Lists the snapshots, here and on the backup disks plugged in.
pub const LIST_COMMAND: &str = "library.catalogue.snapshots";

/// Restores one (`file`), Helios starting again.
pub const RESTORE_COMMAND: &str = "library.catalogue.restore";

fn setting(store: &SharedStore, id: SettingId) -> Option<u64> {
    store
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&format!("settings/{id}"))
        .and_then(|e| e.value.as_i64())
        .and_then(|d| u64::try_from(d).ok())
}

/// Seconds between scheduled snapshots, from the settings' `store`.
pub fn every(store: &SharedStore) -> u64 {
    setting(store, SETTING_EVERY).unwrap_or(EVERY_HOURS).max(1) * 3600
}

/// Snapshots kept, from the settings' `store`.
pub fn kept(store: &SharedStore) -> usize {
    setting(store, SETTING_KEPT)
        .and_then(|k| usize::try_from(k).ok())
        .unwrap_or(KEPT)
        .max(1)
}

/// The catalogue's name on backup disks: its file's stem.
pub fn library_name(file: &Path) -> String {
    file.file_stem()
        .map_or_else(|| "library".into(), |s| s.to_string_lossy().into_owned())
}

/// One snapshot, where it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Listed {
    pub file: PathBuf,
    /// When it was taken, in seconds since the epoch.
    pub taken: u64,
    pub reason: String,
    /// The schema version it was taken at.
    pub version: u32,
    pub bytes: u64,
    /// The backup disk holding it; `None`: next to the catalogue.
    pub disk: Option<String>,
}

fn listed(dir: &Path, disk: Option<&str>) -> Vec<Listed> {
    snapshot::list_in(dir)
        .iter()
        .filter_map(|f| snapshot::info(f))
        .map(|i| Listed {
            file: i.file,
            taken: i.taken,
            reason: i.reason,
            version: i.version,
            bytes: i.bytes,
            disk: disk.map(String::from),
        })
        .collect()
}

/// The snapshots of the catalogue at `file`: those next to it, then those
/// on the mounted `volumes` (any disk holding a `Helios Backup/catalogue/`
/// folder of this library), newest first in each place.
pub fn list(file: &Path, volumes: &[Volume]) -> Vec<Listed> {
    let mut out: Vec<Listed> = listed(&snapshot::dir(file), None)
        .into_iter()
        .rev()
        .collect();
    let name = library_name(file);
    for v in volumes {
        let Some(mount) = &v.mount else { continue };
        let mut on = listed(&strategy::catalogue_path(mount, &name), Some(&v.name));
        on.reverse();
        out.extend(on);
    }
    out
}

/// Copies the snapshot `taken` to every backup disk of the library's
/// strategy plugged in, verified, keeping `keep` there; the copies made,
/// and the errors met (one disk failing does not stop the others). The
/// strategy and the disks the library knows are read through its node
/// (`library.storage.get`).
pub fn copy_to_backups(
    node: &Client,
    mounted: &[Volume],
    file: &Path,
    taken: &Path,
    keep: usize,
) -> (Vec<PathBuf>, Vec<String>) {
    let stored = match node.request("library.storage.get", serde_json::json!({})) {
        Ok(stored) => stored,
        Err(e) => return (Vec::new(), vec![e.message]),
    };
    let Ok(Some(s)) = serde_json::from_value::<Option<Strategy>>(stored["strategy"].clone()) else {
        return (Vec::new(), Vec::new());
    };
    let known: Vec<Volume> = match serde_json::from_value(stored["volumes"].clone()) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), vec![e.to_string()]),
    };
    let volumes = registry::merge(known, mounted);
    let (mut copies, mut errors) = (Vec::new(), Vec::new());
    for (v, folder) in strategy::catalogue_destinations(&s, &volumes, &library_name(file)) {
        match snapshot::copy_verified(taken, &folder, keep) {
            Ok(c) => copies.push(c),
            Err(e) => errors.push(format!("{}: {e}", v.name)),
        }
    }
    (copies, errors)
}

/// Takes a snapshot of the catalogue at `file` now (`reason`: `daily`,
/// `by-hand`), keeping `keep`, and copies it to the backup disks
/// ([`copy_to_backups`]); a failing copy is logged, the snapshot stands.
pub fn take(
    node: &Client,
    mounted: &[Volume],
    file: &Path,
    reason: &str,
    keep: usize,
) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let options = helios_vault_sqlite::Options {
        keep_snapshots: keep,
        ..helios_vault_sqlite::Options::default()
    };
    let taken = snapshot::take_file(file, reason, &options)?;
    let (copies, errors) = copy_to_backups(node, mounted, file, &taken, keep);
    for e in errors {
        log::warn!("catalogue snapshot copy: {e}");
    }
    Ok((taken, copies))
}

/// The health check's view of the snapshots of the catalogue at `file`
/// (next to it and on the mounted `volumes`): each one there was verified
/// when it was written; the newest is read back again now, to say
/// whether it can be restored.
pub fn health(file: &Path, volumes: &[Volume], now: u64) -> helios_health::snapshots::Snapshots {
    let all = list(file, volumes);
    let newest = all.iter().max_by_key(|l| l.taken).map(|l| l.file.clone());
    let list = all
        .into_iter()
        .map(|l| helios_health::snapshots::Snapshot {
            restorable: newest.as_ref() != Some(&l.file) || snapshot::verify(&l.file).is_ok(),
            verified: Some(l.taken),
            taken: l.taken,
            file: l.file,
        })
        .collect();
    helios_health::snapshots::Snapshots {
        list,
        now,
        max_age_days: helios_health::snapshots::MAX_AGE_DAYS,
    }
}
