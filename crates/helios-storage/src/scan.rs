//! The background scan (`docs/storage-backup-research.md`, sections 2.2
//! and 2.3): it records the volumes mounted now, links each location to
//! the volume it is on, and says whether its file is present, offline or
//! missing.
//!
//! - **Record**: [`registry::record`] with the volumes detected, the node
//!   and the library folder.
//! - **Relink**: a location on no volume yet is put on the volume holding
//!   its path, with its path inside it. An online volume takes it only
//!   when the file is there (a path under `/` whose disk is unplugged is
//!   not the system disk's); a known offline volume last mounted above it
//!   on this node takes it as is.
//! - **Presence**: a location is offline when its volume is not mounted,
//!   or is a fixed disk of another node; present when its file is there;
//!   missing when its volume is online (or it has none) and the file is
//!   not there.
//! - **Resolve**: on an online volume, the absolute path is the volume's
//!   mount point joined with the path inside it, so a changed mount point
//!   or drive letter loses nothing.
//!
//! [`Scanner`] runs it on a thread of its own: at start, when poked (a
//! volume appeared or went, see [`crate::Watch::on_volumes`]) and every
//! few minutes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use helios_vault::{Presence, Store, Vault};
use serde::Serialize;

use crate::{Volume, Volumes, registry};

/// What one scan did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ScanReport {
    /// Volumes known (online and offline).
    pub volumes: usize,
    /// Locations put on a volume in this scan.
    pub relinked: u64,
    /// Locations whose absolute path changed with their volume's mount
    /// point.
    pub moved: u64,
    pub present: u64,
    pub offline: u64,
    pub missing: u64,
}

/// Whether `volume`'s files can be read from `node`: it is mounted, and
/// is not a fixed disk belonging to another node.
fn at_hand(volume: &Volume, node: &str) -> bool {
    volume.online()
        && !(volume.kind.is_fixed() && volume.node.as_deref().is_some_and(|n| n != node))
}

/// The volume a location on none belongs to, among `known` (online ones
/// with their mount point, offline ones with their last one on `node`):
/// the deepest mount point above `path`, an online volume first on a tie.
fn owner<'a>(known: &'a [Volume], node: &str, path: &Path) -> Option<(&'a Volume, PathBuf)> {
    known
        .iter()
        .filter_map(|v| {
            let mount = match &v.mount {
                Some(m) => m.clone(),
                None => v
                    .last_seen
                    .as_ref()
                    .filter(|s| s.node == node)?
                    .mount
                    .clone()?,
            };
            path.starts_with(&mount).then_some((v, mount))
        })
        .max_by_key(|(v, m)| (m.components().count(), v.online()))
}

/// One scan made on `node` at `now` (seconds since the epoch) with the
/// volumes `scanned` mounted now; `library` is the library folder.
pub fn scan<S: Store>(
    vault: &Vault<S>,
    scanned: &[Volume],
    node: &str,
    now: u64,
    library: Option<&Path>,
) -> Result<ScanReport, String> {
    let known = registry::record(vault, scanned, node, now, library)?;
    let mut report = ScanReport {
        volumes: known.len(),
        ..ScanReport::default()
    };
    let mut updates = Vec::new();
    for (id, old) in vault.all_locations()? {
        let mut location = old.clone();
        if location.volume.is_none()
            && let Some((volume, mount)) = owner(&known, node, &location.path)
            && (!volume.online() || location.path.is_file())
            && let Ok(rel) = location.path.strip_prefix(&mount)
        {
            location.volume = Some(volume.id.clone());
            location.rel_path = Some(rel.to_path_buf());
            report.relinked += 1;
        }
        let volume = location
            .volume
            .as_ref()
            .map(|id| known.iter().find(|v| &v.id == id));
        location.presence = match volume {
            // Its volume is mounted here: the path resolves through its
            // mount point now.
            Some(Some(v)) if at_hand(v, node) => {
                if let (Some(mount), Some(rel)) = (&v.mount, &location.rel_path) {
                    location.path = mount.join(rel);
                }
                if location.path.is_file() {
                    Presence::Present
                } else {
                    Presence::Missing
                }
            }
            // Unplugged, another node's disk, or a volume the registry
            // no longer has: not at hand, not known to be gone.
            Some(_) => Presence::Offline,
            None if location.path.is_file() => Presence::Present,
            None => Presence::Missing,
        };
        if location.path != old.path {
            report.moved += 1;
        }
        match location.presence {
            Presence::Present => report.present += 1,
            Presence::Offline => report.offline += 1,
            Presence::Missing => report.missing += 1,
        }
        if location != old {
            if location.presence == Presence::Present && old.presence != Presence::Present {
                location.last_verified = now;
            }
            updates.push((id, old.path, location));
        }
    }
    vault.update_locations(&updates)?;
    Ok(report)
}

/// The scan on a thread of its own; dropping it stops the thread after
/// the scan running, if any.
pub struct Scanner {
    poke: Sender<()>,
}

impl Scanner {
    /// Starts scanning `volumes` into `vault` as `node`: now, when
    /// [poked](Self::poke), and every `every`. `library` gives the library
    /// folder at each scan; `done` hears each scan's outcome.
    pub fn start<S: Store + 'static>(
        vault: Arc<Vault<S>>,
        volumes: Arc<dyn Volumes>,
        node: String,
        library: impl Fn() -> Option<PathBuf> + Send + 'static,
        every: Duration,
        done: impl Fn(Result<ScanReport, String>) + Send + 'static,
    ) -> Self {
        let (poke, poked) = mpsc::channel::<()>();
        let spawned = std::thread::Builder::new()
            .name("helios-volume-scan".into())
            .spawn(move || {
                loop {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs());
                    let folder = library();
                    done(scan(
                        &vault,
                        &volumes.volumes(),
                        &node,
                        now,
                        folder.as_deref(),
                    ));
                    match poked.recv_timeout(every) {
                        // Several pokes while scanning make one scan.
                        Ok(()) => while poked.try_recv().is_ok() {},
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
        if let Err(e) = spawned {
            log::warn!("volumes: no thread to scan them: {e}");
        }
        Self { poke }
    }

    /// Asks for a scan soon (a volume appeared or went).
    pub fn poke(&self) {
        let _ = self.poke.send(());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use helios_vault::{ImageId, Location, MemoryStore};

    use super::*;
    use crate::{Identity, VolumeId, VolumeKind, detect};

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "helios-storage-scan-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn disk(id: &str, mount: &Path, kind: VolumeKind, node: &str) -> Volume {
        Volume {
            id: VolumeId(id.into()),
            identified_by: Identity::Uuid,
            kind,
            mount: Some(mount.to_path_buf()),
            node: kind.is_fixed().then(|| node.into()),
            ..detect::weak(id, Some(1_000))
        }
    }

    fn write(path: &Path, seed: u8) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![seed; 2_000]).unwrap();
    }

    fn only(vault: &Vault<MemoryStore>) -> (ImageId, Location) {
        let mut all = vault.all_locations().unwrap();
        assert_eq!(all.len(), 1, "{all:?}");
        all.remove(0)
    }

    /// An external disk: relinked, then unplugged (offline, and repair
    /// leaves it alone), then plugged in on another mount point (present
    /// at its new path), then its file deleted (missing).
    #[test]
    fn an_external_disk_goes_offline_moves_and_loses_a_file() {
        let dir = scratch("external");
        let first = dir.join("run/media/u/Backup");
        write(&first.join("2024/a.raf"), 1);
        let vault = Vault::new(MemoryStore::default());
        vault.import(&first, 10).unwrap();
        let (_, before) = only(&vault);
        assert_eq!(
            (before.volume.as_ref(), before.rel_path.as_ref()),
            (None, None)
        );

        // Relinked to the deepest volume: the disk, not the root above it.
        let root = disk("uuid:root", &dir, VolumeKind::Internal, "studio");
        let mut backup = disk("uuid:backup", &first, VolumeKind::External, "studio");
        let report = scan(&vault, &[root.clone(), backup.clone()], "studio", 20, None).unwrap();
        assert_eq!((report.relinked, report.present), (1, 1));
        let (id, linked) = only(&vault);
        assert_eq!(linked.volume, Some(VolumeId("uuid:backup".into())));
        assert_eq!(linked.rel_path.as_deref(), Some(Path::new("2024/a.raf")));
        assert_eq!(linked.presence, Presence::Present);

        // Unplugged: offline, and repair neither marks it missing nor
        // removes it.
        let report = scan(&vault, std::slice::from_ref(&root), "studio", 30, None).unwrap();
        assert_eq!((report.offline, report.missing), (1, 0));
        assert_eq!(only(&vault).1.presence, Presence::Offline);
        std::fs::rename(&first, dir.join("unplugged")).unwrap();
        let repaired = vault.repair(&[dir.as_path()], 35).unwrap();
        assert_eq!((repaired.lost, repaired.missing), (0, 0));
        assert_eq!(only(&vault).1.presence, Presence::Offline);

        // Plugged in again, mounted elsewhere (a new drive letter): the
        // path resolves through the new mount point.
        let second = dir.join("media/Backup");
        std::fs::create_dir_all(second.parent().unwrap()).unwrap();
        std::fs::rename(dir.join("unplugged"), &second).unwrap();
        backup.mount = Some(second.clone());
        let report = scan(&vault, &[root.clone(), backup.clone()], "studio", 40, None).unwrap();
        assert_eq!((report.moved, report.present), (1, 1));
        let (again, moved) = only(&vault);
        assert_eq!(again, id);
        assert_eq!(moved.path, second.join("2024/a.raf"));
        assert_eq!(moved.presence, Presence::Present);
        assert_eq!(moved.rel_path, linked.rel_path);

        // Its volume online, the file deleted: missing.
        std::fs::remove_file(second.join("2024/a.raf")).unwrap();
        scan(&vault, &[root, backup], "studio", 50, None).unwrap();
        assert_eq!(only(&vault).1.presence, Presence::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The studio's internal disk, seen from the laptop sharing the
    /// vault: offline, whether the laptop does not see it or a scan
    /// reports it with its own node.
    #[test]
    fn a_fixed_disk_of_another_node_is_offline() {
        let dir = scratch("fixed");
        let photos = dir.join("mnt/photos");
        write(&photos.join("b.nef"), 2);
        let vault = Vault::new(MemoryStore::default());
        vault.import(&photos, 10).unwrap();
        let internal = disk("uuid:photos", &photos, VolumeKind::Internal, "studio");
        scan(&vault, std::slice::from_ref(&internal), "studio", 20, None).unwrap();
        assert_eq!(only(&vault).1.presence, Presence::Present);

        let laptop = scan(&vault, &[], "laptop", 30, None).unwrap();
        assert_eq!((laptop.offline, laptop.missing), (1, 0));
        assert_eq!(only(&vault).1.presence, Presence::Offline);
        // Even listed with the studio as its node, the file there (a
        // shared folder of the same path, say) is not read from the
        // laptop.
        scan(&vault, std::slice::from_ref(&internal), "laptop", 40, None).unwrap();
        assert_eq!(only(&vault).1.presence, Presence::Offline);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A location left from before volumes, on a disk unplugged now but
    /// known with its last mount point: linked to that disk, offline,
    /// never the root's missing file.
    #[test]
    fn an_old_location_on_an_unplugged_known_disk_is_offline() {
        let dir = scratch("legacy");
        let card = dir.join("media/Card");
        write(&card.join("c.raf"), 3);
        let vault = Vault::new(MemoryStore::default());
        vault.import(&card, 10).unwrap();
        let root = disk("uuid:root", &dir, VolumeKind::Internal, "studio");
        let removable = disk("uuid:card", &card, VolumeKind::Removable, "studio");
        // The registry sees the card once (a scan of phase 1, say),
        // before any location is linked.
        registry::record(&vault, &[root.clone(), removable], "studio", 15, None).unwrap();
        std::fs::remove_dir_all(&card).unwrap();
        let report = scan(&vault, &[root], "studio", 20, None).unwrap();
        assert_eq!((report.relinked, report.offline), (1, 1));
        let (_, location) = only(&vault);
        assert_eq!(location.volume, Some(VolumeId("uuid:card".into())));
        assert_eq!(location.presence, Presence::Offline);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_scanner_scans_at_start_and_when_poked() {
        let dir = scratch("thread");
        write(&dir.join("d.raf"), 4);
        let vault = Arc::new(Vault::new(MemoryStore::default()));
        vault.import(&dir, 10).unwrap();
        let volumes: Arc<dyn Volumes> =
            Arc::new(vec![disk("uuid:d", &dir, VolumeKind::External, "studio")]);
        let (tx, rx) = mpsc::channel();
        let scanner = Scanner::start(
            vault.clone(),
            volumes,
            "studio".into(),
            || None,
            Duration::from_hours(1),
            move |r| {
                let _ = tx.send(r);
            },
        );
        let first = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!((first.relinked, first.present), (1, 1));
        scanner.poke();
        let second = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!((second.relinked, second.present), (0, 1));
        drop(scanner);
        assert_eq!(only(&vault).1.volume, Some(VolumeId("uuid:d".into())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
