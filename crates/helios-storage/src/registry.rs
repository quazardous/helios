//! The volumes Helios knows, kept in the vault (namespace
//! `storage/volumes`, see `helios_vault::storage`).
//!
//! Each scan is recorded with [`record`]: a volume seen now gets its
//! detected facts (kind, size, free space, drive, mount point) and
//! `last_seen` (when, on which node, where); a new one starts with its
//! label as name and its kind's roles, plus the library role when it holds
//! the library folder. What the user set (name, roles, off site, RAID) is
//! kept, and [`save`] writes it (the Storage page). A volume not seen
//! stays as it was: offline, never dropped.

use std::collections::BTreeMap;
use std::path::Path;

use helios_vault::storage::StorageTx;
use helios_vault::{Store, Vault};

use crate::{LastSeen, Role, Volume, VolumeId, marker};

fn decode(id: &str, doc: serde_json::Value) -> Result<Volume, String> {
    serde_json::from_value(doc).map_err(|e| format!("volume {id}: {e}"))
}

fn encode(volume: &Volume) -> Result<serde_json::Value, String> {
    serde_json::to_value(volume).map_err(|e| format!("volume {}: {e}", volume.id))
}

fn load<S: Store>(tx: &StorageTx<'_, S>) -> Result<BTreeMap<VolumeId, Volume>, String> {
    tx.volumes()?
        .into_iter()
        .map(|(id, doc)| decode(&id, doc).map(|v| (v.id.clone(), v)))
        .collect()
}

/// Every volume the vault knows, offline (no mount point), by name.
pub fn known<S: Store>(vault: &Vault<S>) -> Result<Vec<Volume>, String> {
    let mut all: Vec<Volume> = vault
        .storage_transaction(|tx| load(tx))?
        .into_values()
        .collect();
    all.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(all)
}

/// The volumes the vault knows, with what is mounted now: the mount point
/// and fresh free space of those plugged in (online first, by name).
pub fn merged<S: Store>(vault: &Vault<S>, mounted: &[Volume]) -> Result<Vec<Volume>, String> {
    let (mut online, offline): (Vec<Volume>, Vec<Volume>) = known(vault)?
        .into_iter()
        .map(|mut v| {
            if let Some(m) = mounted.iter().find(|m| m.id == v.id) {
                v.mount.clone_from(&m.mount);
                v.free = m.free.or(v.free);
                v.capacity = m.capacity.or(v.capacity);
            }
            v
        })
        .partition(Volume::online);
    online.extend(offline);
    Ok(online)
}

/// The volume among `volumes` holding `path`: the deepest mount point
/// above it.
pub fn holding<'a>(volumes: &'a [Volume], path: &Path) -> Option<&'a Volume> {
    volumes
        .iter()
        .filter(|v| v.mount.as_deref().is_some_and(|m| path.starts_with(m)))
        .max_by_key(|v| v.mount.as_ref().map_or(0, |m| m.components().count()))
}

/// Records a scan made on `node` at `now` (seconds since the epoch):
/// returns every known volume, those online first (with their mount
/// point), then the offline ones, each by name. `library` is the library
/// folder: the volume holding it starts with the library role.
pub fn record<S: Store>(
    vault: &Vault<S>,
    scanned: &[Volume],
    node: &str,
    now: u64,
    library: Option<&Path>,
) -> Result<Vec<Volume>, String> {
    let holder = library
        .and_then(|l| holding(scanned, l))
        .map(|v| v.id.clone());
    vault.storage_transaction(|tx| {
        let mut known = load(tx)?;
        let mut online = Vec::with_capacity(scanned.len());
        for seen in scanned {
            let mut volume = seen.clone();
            volume.last_seen = Some(LastSeen {
                at: now,
                node: node.to_string(),
                mount: seen.mount.clone(),
            });
            if let Some(old) = known.remove(&seen.id) {
                // The user's: its name, roles and flags.
                volume.name = old.name;
                volume.roles = old.roles;
                volume.off_site = old.off_site;
                volume.raid = old.raid;
            } else {
                volume.roles = seen.default_roles();
                if holder.as_ref() == Some(&seen.id) {
                    volume.roles.insert(Role::Library);
                }
            }
            tx.set_volume(volume.id.as_str(), Some(&encode(&volume)?))?;
            online.push(volume);
        }
        let mut offline: Vec<Volume> = known.into_values().collect();
        online.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        offline.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        online.extend(offline);
        Ok(online)
    })
}

/// Saves what the user set on a known volume: its name (its label when
/// left blank), roles, off site and RAID flags. What a scan detects
/// (kind, size, last seen) stays as recorded. Returns the volume as
/// stored.
pub fn save<S: Store>(vault: &Vault<S>, volume: &Volume) -> Result<Volume, String> {
    vault.storage_transaction(|tx| {
        let mut record = match tx.volume(volume.id.as_str())? {
            Some(doc) => decode(volume.id.as_str(), doc)?,
            None => return Err(format!("no volume {}", volume.id)),
        };
        let name = volume.name.trim();
        record.name = if name.is_empty() {
            record.label.clone().unwrap_or_else(|| record.name.clone())
        } else {
            name.to_string()
        };
        record.roles.clone_from(&volume.roles);
        record.off_site = volume.off_site;
        record.raid = volume.raid;
        tx.set_volume(record.id.as_str(), Some(&encode(&record)?))?;
        Ok(record)
    })
}

/// Marks a mounted volume with an identity file (see [`marker::mark`])
/// and moves its record to the marker's id, keeping what the user set.
/// Only ever called on the user's say.
pub fn mark<S: Store>(
    vault: &Vault<S>,
    volume: &Volume,
    library: &str,
    now: u64,
) -> Result<Volume, String> {
    let marked = marker::mark(volume, library, now)?;
    if marked.id == volume.id {
        return Ok(marked);
    }
    vault.storage_transaction(|tx| {
        let mut record = match tx.volume(volume.id.as_str())? {
            Some(doc) => decode(volume.id.as_str(), doc)?,
            None => volume.clone(),
        };
        record.id = marked.id.clone();
        record.identified_by = marked.identified_by;
        tx.set_volume(volume.id.as_str(), None)?;
        tx.set_volume(record.id.as_str(), Some(&encode(&record)?))?;
        Ok(Volume {
            mount: volume.mount.clone(),
            ..record
        })
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use helios_vault::MemoryStore;

    use super::*;
    use crate::{Identity, VolumeKind, detect};

    fn disk(id: &str, mount: &str, kind: VolumeKind) -> Volume {
        Volume {
            id: VolumeId(id.into()),
            identified_by: Identity::Uuid,
            kind,
            mount: Some(mount.into()),
            node: kind.is_fixed().then(|| "studio".into()),
            ..detect::weak(id, Some(1_000))
        }
    }

    #[test]
    fn the_registry_keeps_offline_volumes_and_updates_last_seen() {
        let vault = Vault::new(MemoryStore::default());
        let system = disk("uuid:sys", "/", VolumeKind::Internal);
        let photos = disk("uuid:photos", "/mnt/photos", VolumeKind::Internal);
        let mut backup = disk("uuid:backup", "/run/media/u/Backup", VolumeKind::External);
        let mut card = disk("uuid:card", "/run/media/u/EOS", VolumeKind::Removable);
        card.has_dcim = true;

        let first = [system.clone(), photos.clone(), backup.clone(), card.clone()];
        let all = record(
            &vault,
            &first,
            "studio",
            100,
            Some(Path::new("/mnt/photos/Library")),
        )
        .unwrap();
        assert_eq!(all.len(), 4);
        let roles = |all: &[Volume], id: &str| {
            all.iter()
                .find(|v| v.id.as_str() == id)
                .unwrap()
                .roles
                .clone()
        };
        // The library folder's disk, not the system disk above it.
        assert_eq!(roles(&all, "uuid:photos"), BTreeSet::from([Role::Library]));
        assert_eq!(roles(&all, "uuid:sys"), BTreeSet::new());
        assert_eq!(roles(&all, "uuid:card"), BTreeSet::from([Role::Ingest]));

        // The user names the backup disk and gives it a role (phase 4's
        // page; here, straight in the record).
        vault
            .storage_transaction(|tx| {
                let mut v = decode("uuid:backup", tx.volume("uuid:backup")?.unwrap())?;
                v.name = "Backup-2".into();
                v.roles = BTreeSet::from([Role::Backup]);
                tx.set_volume("uuid:backup", Some(&encode(&v)?))
            })
            .unwrap();

        // Later, on the laptop: the card is gone, the backup disk moved
        // to another mount point and has less free space.
        backup.mount = Some("/media/Backup".into());
        backup.free = Some(10);
        let all = record(&vault, &[backup.clone()], "laptop", 200, None).unwrap();
        assert_eq!(all.len(), 4, "offline volumes are kept");
        assert_eq!(all[0].id.as_str(), "uuid:backup", "online first");
        assert_eq!(all[0].name, "Backup-2");
        assert_eq!(all[0].roles, BTreeSet::from([Role::Backup]));
        assert_eq!(all[0].free, Some(10));
        assert_eq!(
            all[0].last_seen,
            Some(LastSeen {
                at: 200,
                node: "laptop".into(),
                mount: Some(PathBuf::from("/media/Backup")),
            })
        );
        let card_now = all.iter().find(|v| v.id.as_str() == "uuid:card").unwrap();
        assert!(!card_now.online());
        assert_eq!(
            card_now.last_seen.as_ref().map(|s| (s.at, s.node.as_str())),
            Some((100, "studio"))
        );
        let photos_now = all.iter().find(|v| v.id.as_str() == "uuid:photos").unwrap();
        assert_eq!(photos_now.node.as_deref(), Some("studio"));
        assert_eq!(known(&vault).unwrap().len(), 4);
    }

    #[test]
    fn saving_keeps_what_the_user_set_across_scans() {
        let vault = Vault::new(MemoryStore::default());
        let backup = disk("uuid:backup", "/media/Backup", VolumeKind::External);
        record(&vault, std::slice::from_ref(&backup), "studio", 1, None).unwrap();
        let mut edited = backup.clone();
        edited.name = "  Backup-2 ".into();
        edited.roles = BTreeSet::from([Role::Backup]);
        edited.off_site = true;
        edited.raid = true;
        // A save does not take the detected facts it is given.
        edited.free = Some(1);
        let saved = save(&vault, &edited).unwrap();
        assert_eq!(saved.name, "Backup-2");
        assert_eq!(saved.free, backup.free);
        let all = record(&vault, std::slice::from_ref(&backup), "studio", 2, None).unwrap();
        assert_eq!(all[0].name, "Backup-2");
        assert_eq!(all[0].roles, BTreeSet::from([Role::Backup]));
        assert!(all[0].off_site && all[0].raid);
        // A blank name is its label again.
        edited.name = " ".into();
        assert_eq!(save(&vault, &edited).unwrap().name, "uuid:backup");
        let unknown = disk("uuid:other", "/media/Other", VolumeKind::External);
        assert!(save(&vault, &unknown).is_err());
    }

    #[test]
    fn marking_moves_the_record_to_the_marker_id() {
        let dir =
            std::env::temp_dir().join(format!("helios-storage-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let vault = Vault::new(MemoryStore::default());
        let mut fat = detect::weak("BACKUP", Some(5));
        fat.kind = VolumeKind::External;
        fat.mount = Some(dir.clone());
        record(&vault, &[fat.clone()], "studio", 1, None).unwrap();
        let marked = mark(&vault, &fat, "lib", 2).unwrap();
        assert_eq!(marked.identified_by, Identity::Marker);
        let ids: Vec<VolumeId> = known(&vault).unwrap().into_iter().map(|v| v.id).collect();
        assert_eq!(ids, std::slice::from_ref(&marked.id));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
