//! Backup strategies and how copies count
//! (`docs/storage-backup-research.md`, sections 3.1 and 3.2).
//!
//! - A [`Strategy`] says how many verified backup copies a photo needs,
//!   where they go ([`Destination`]: any volume of a role, or one volume),
//!   whether one must be off site, and when they are made ([`Trigger`]).
//! - The library has one default strategy, or none (a new library's
//!   default). It is kept in the vault's documents (`storage/strategies`,
//!   key `library`), beside the volumes. v1 has only that one: a
//!   [`Scope`] names where a strategy applies, so per-shoot, per-import
//!   and per-card overrides can be added as more scopes.
//! - [`status`] counts an image's backup copies ([`BackupStatus`]): its
//!   locations on volumes with the backup or archive role, present or
//!   offline, verified by hash (the backup job records each copy it read
//!   back, see [`record_verified`]). One per physical drive (a RAID, or two
//!   partitions of one disk, count once); a volume known only by its label
//!   and size never counts; the library copy is not a backup copy.
//! - [`may_write`] is the write permission: only volumes with a backup,
//!   archive or scratch role, never a card or an ingest volume, and only
//!   inside `Helios Backup/` or `Helios Archive/` at their root.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use helios_vault::{ImageId, Location, Presence, Store, Vault};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Identity, Role, Volume, VolumeId, VolumeKind};

/// The folder backup copies go to, at a volume's root.
pub const BACKUP_FOLDER: &str = "Helios Backup";
/// The folder archived shoots go to, at a volume's root.
pub const ARCHIVE_FOLDER: &str = "Helios Archive";

/// Where a strategy's copies may go.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    /// Any volume with this role (a rotation of backup disks).
    Role(Role),
    /// This volume only.
    Volume(VolumeId),
}

impl Destination {
    /// Whether `volume` is one of this destination's.
    pub fn takes(&self, volume: &Volume) -> bool {
        match self {
            Self::Role(role) => volume.roles.contains(role),
            Self::Volume(id) => volume.id == *id,
        }
    }
}

/// When a strategy's copies are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// Right after an import, from the library copy.
    OnImport,
    /// When a destination volume is plugged in.
    OnConnect,
    /// "Back up now".
    ByHand,
}

/// A backup rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Strategy {
    pub name: String,
    /// Verified backup copies a photo needs, on distinct drives, besides
    /// its library copy.
    pub copies: u8,
    pub destinations: Vec<Destination>,
    /// One of the copies must be on a volume marked off site.
    #[serde(default)]
    pub off_site_required: bool,
    #[serde(default)]
    pub triggers: BTreeSet<Trigger>,
}

impl Default for Strategy {
    /// One copy on any backup disk, made after each import, when a backup
    /// disk is plugged in and by hand.
    fn default() -> Self {
        Self {
            name: "Backup".into(),
            copies: 1,
            destinations: vec![Destination::Role(Role::Backup)],
            off_site_required: false,
            triggers: BTreeSet::from([Trigger::OnImport, Trigger::OnConnect, Trigger::ByHand]),
        }
    }
}

impl Strategy {
    /// Whether it runs on `trigger` ("by hand" always does).
    pub fn runs_on(&self, trigger: Trigger) -> bool {
        trigger == Trigger::ByHand || self.triggers.contains(&trigger)
    }

    /// Whether a photo with `status` needs a copy on `volume`: short of
    /// copies, or of its off-site one when `volume` is off site.
    pub fn needs(&self, status: &BackupStatus, volume: &Volume) -> bool {
        status.have < status.want
            || (self.off_site_required && !status.off_site_ok && volume.off_site)
    }
}

/// Where a strategy applies. v1 knows the library default only; overrides
/// (a shoot, an import session, a card, a selection) are scopes to come,
/// the first that applies winning, the strongest among several.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Scope {
    Library,
}

impl Scope {
    /// Its document's key.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Library => "library",
        }
    }
}

/// The strategy kept for `scope`; `None` when there is none (a new
/// library's default).
pub fn load<S: Store>(vault: &Vault<S>, scope: &Scope) -> Result<Option<Strategy>, String> {
    let doc = vault.storage_transaction(|tx| tx.strategy(scope.key()))?;
    doc.map(|d| serde_json::from_value(d).map_err(|e| format!("strategy {}: {e}", scope.key())))
        .transpose()
}

/// Keeps (or with `None` removes) the strategy of `scope`.
pub fn save<S: Store>(
    vault: &Vault<S>,
    scope: &Scope,
    strategy: Option<&Strategy>,
) -> Result<(), String> {
    let doc = strategy
        .map(|s| serde_json::to_value(s).map_err(|e| format!("strategy: {e}")))
        .transpose()?;
    vault.storage_transaction(|tx| tx.set_strategy(scope.key(), doc.as_ref()))
}

/// The strategy an image follows. v1: the library default.
pub fn of_image<S: Store>(vault: &Vault<S>, _image: &ImageId) -> Result<Option<Strategy>, String> {
    load(vault, &Scope::Library)
}

/// An image's backup copies against its strategy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct BackupStatus {
    /// Verified copies, one per physical drive.
    pub have: u8,
    /// Copies the strategy asks for (0 without one).
    pub want: u8,
    /// Whether the off-site copy is there, or none is asked for.
    pub off_site_ok: bool,
    /// Copies on backup or archive volumes not verified by hash.
    pub unverified: u8,
}

impl BackupStatus {
    /// Whether it meets its strategy (always, without one).
    pub fn met(&self) -> bool {
        self.have >= self.want && self.off_site_ok
    }
}

/// The key a verified copy is recorded under.
pub fn copy_key(volume: &VolumeId, rel: &Path) -> String {
    format!("{volume}\t{}", rel.display())
}

/// The copies verified by hash, by image: each `(volume, path inside it)`
/// with when it was verified.
pub type Verified = BTreeMap<ImageId, BTreeMap<String, u64>>;

fn verified_of(doc: &Value) -> BTreeMap<String, u64> {
    doc.as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_u64()?)))
                .collect()
        })
        .unwrap_or_default()
}

/// Every image's copies verified by hash.
pub fn verified<S: Store>(vault: &Vault<S>) -> Result<Verified, String> {
    let all = vault.storage_transaction(|tx| tx.all_verified())?;
    Ok(all
        .into_iter()
        .filter_map(|(k, d)| Some((ImageId::from_hex(&k).ok()?, verified_of(&d))))
        .collect())
}

/// Records that the copy of `image` at `rel` on `volume` was read back and
/// matched its hash at `now`.
pub fn record_verified<S: Store>(
    vault: &Vault<S>,
    image: &ImageId,
    volume: &VolumeId,
    rel: &Path,
    now: u64,
) -> Result<(), String> {
    let key = image.hex();
    vault.storage_transaction(|tx| {
        let mut doc = tx.verified(&key)?.unwrap_or_else(|| json!({}));
        if let Some(o) = doc.as_object_mut() {
            o.insert(copy_key(volume, rel), now.into());
        }
        tx.set_verified(&key, Some(&doc))
    })
}

/// A volume's physical drive, as copies count: two partitions of one disk
/// (or two volumes of one RAID) are one.
pub fn drive_of(volume: &Volume) -> String {
    volume
        .drive
        .clone()
        .unwrap_or_else(|| volume.id.as_str().to_string())
}

/// Whether a copy on `volume` is a backup copy: a backup or archive
/// volume, strongly identified.
fn counts_on(volume: &Volume) -> bool {
    (volume.roles.contains(&Role::Backup) || volume.roles.contains(&Role::Archive))
        && volume.identified_by != Identity::Weak
        && volume.kind != VolumeKind::Remote
}

/// An image's backup status from its `locations`, the volumes known and
/// the copies verified (`verified`, its own entry), against `strategy`.
pub fn status(
    locations: &[Location],
    volumes: &[Volume],
    verified: Option<&BTreeMap<String, u64>>,
    strategy: Option<&Strategy>,
) -> BackupStatus {
    let mut drives = BTreeSet::new();
    let mut unverified = BTreeSet::new();
    let mut off_site = false;
    for location in locations {
        if location.presence == Presence::Missing {
            continue;
        }
        let (Some(id), Some(rel)) = (&location.volume, &location.rel_path) else {
            continue;
        };
        let Some(volume) = volumes.iter().find(|v| &v.id == id) else {
            continue;
        };
        if !counts_on(volume) {
            continue;
        }
        let drive = drive_of(volume);
        if verified.is_some_and(|v| v.contains_key(&copy_key(id, rel))) {
            off_site |= volume.off_site;
            drives.insert(drive);
        } else {
            unverified.insert(drive);
        }
    }
    let want = strategy.map_or(0, |s| s.copies);
    BackupStatus {
        have: u8::try_from(drives.len()).unwrap_or(u8::MAX),
        want,
        off_site_ok: off_site || !strategy.is_some_and(|s| s.off_site_required),
        unverified: u8::try_from(unverified.difference(&drives).count()).unwrap_or(u8::MAX),
    }
}

/// What [`status`] measures against, as a vault query takes it
/// (`helios_vault::Backups`): the volumes whose copies count, each with
/// its drive, and the strategy's demands (none without a strategy). A
/// photo's [`helios_vault::BackupState`] in the vault is then the
/// Library's mark of its [`status`].
pub fn rule(volumes: &[Volume], strategy: Option<&Strategy>) -> helios_vault::Backups {
    helios_vault::Backups {
        volumes: volumes
            .iter()
            .filter(|v| counts_on(v))
            .map(|v| helios_vault::BackupVolume {
                id: v.id.clone(),
                drive: drive_of(v),
                off_site: v.off_site,
            })
            .collect(),
        copies: strategy.map_or(0, |s| s.copies),
        off_site_required: strategy.is_some_and(|s| s.off_site_required),
    }
}

/// The vault's backup state of a [`status`]: `none` without any copy,
/// else `met`, else `unverified` while a copy waits for its check, else
/// `short`.
pub fn state_of(status: &BackupStatus) -> helios_vault::BackupState {
    use helios_vault::BackupState;
    if status.have == 0 && status.unverified == 0 {
        BackupState::None
    } else if status.met() {
        BackupState::Met
    } else if status.unverified > 0 {
        BackupState::Unverified
    } else {
        BackupState::Short
    }
}

/// Every image's status against the library strategy, from the vault (one
/// read of its locations and verified copies).
pub fn statuses<S: Store>(
    vault: &Vault<S>,
    volumes: &[Volume],
    strategy: Option<&Strategy>,
) -> Result<BTreeMap<ImageId, (BackupStatus, Vec<Location>)>, String> {
    let verified = verified(vault)?;
    let mut by_image: BTreeMap<ImageId, Vec<Location>> = BTreeMap::new();
    for image in vault.images()? {
        by_image.entry(image.id).or_default();
    }
    for (id, location) in vault.all_locations()? {
        by_image.entry(id).or_default().push(location);
    }
    Ok(by_image
        .into_iter()
        .map(|(id, locations)| {
            let s = status(&locations, volumes, verified.get(&id), strategy);
            (id, (s, locations))
        })
        .collect())
}

/// Whether Helios may write backup copies to `volume`: a backup, archive
/// or scratch role, never a card or an ingest volume.
pub fn may_back_up_to(volume: &Volume) -> Result<(), String> {
    if volume.kind == VolumeKind::Remote {
        // Helios cannot verify later what a remote holds, and a
        // re-encoding service never holds the original.
        return Err(format!(
            "{}: a remote source is never a backup or archive",
            volume.name
        ));
    }
    if volume.is_card() || volume.has_dcim || volume.roles.contains(&Role::Ingest) {
        return Err(format!("{}: Helios never writes to a card", volume.name));
    }
    if ![Role::Backup, Role::Archive, Role::Scratch]
        .iter()
        .any(|r| volume.roles.contains(r))
    {
        return Err(format!(
            "{}: not a backup, archive or scratch disk (give it a role on the Storage page)",
            volume.name
        ));
    }
    Ok(())
}

/// Whether Helios may write `path` on `volume`: [`may_back_up_to`], and
/// only inside its `Helios Backup/` or `Helios Archive/` folder.
pub fn may_write(volume: &Volume, path: &Path) -> Result<(), String> {
    may_back_up_to(volume)?;
    let mount = volume
        .mount
        .as_ref()
        .ok_or_else(|| format!("{}: not plugged in", volume.name))?;
    let inside = [BACKUP_FOLDER, ARCHIVE_FOLDER].iter().any(|f| {
        path.strip_prefix(mount.join(f)).is_ok_and(|rest| {
            rest.components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        })
    });
    if inside {
        Ok(())
    } else {
        Err(format!(
            "{}: Helios writes only in {BACKUP_FOLDER}/ or {ARCHIVE_FOLDER}/ there",
            path.display()
        ))
    }
}

/// The volumes `strategy` copies to that are plugged in now, writable and
/// strongly identified (a weak copy would never count), off-site ones
/// first when an off-site copy is asked for.
pub fn destinations_online(strategy: &Strategy, volumes: &[Volume]) -> Vec<Volume> {
    let mut found: Vec<Volume> = volumes
        .iter()
        .filter(|v| v.online() && v.identified_by != Identity::Weak)
        .filter(|v| strategy.destinations.iter().any(|d| d.takes(v)))
        .filter(|v| may_back_up_to(v).is_ok())
        .cloned()
        .collect();
    if strategy.off_site_required {
        found.sort_by_key(|v| !v.off_site);
    }
    found
}

/// Where a backup copy of a file goes on a volume mounted at `mount`:
/// `Helios Backup/` plus its path in the library.
pub fn backup_path(mount: &Path, in_library: &Path) -> PathBuf {
    mount.join(BACKUP_FOLDER).join(in_library)
}

/// The folder catalogue snapshots go to, inside [`BACKUP_FOLDER`].
pub const CATALOGUE_FOLDER: &str = "catalogue";

/// Where the snapshots of the catalogue named `library` (its database
/// file's stem: several libraries can share a disk) go on a volume
/// mounted at `mount`: `Helios Backup/catalogue/<library>/`.
pub fn catalogue_path(mount: &Path, library: &str) -> PathBuf {
    mount
        .join(BACKUP_FOLDER)
        .join(CATALOGUE_FOLDER)
        .join(library)
}

/// The folders the catalogue's snapshots are copied to: on each
/// destination of `strategy` plugged in ([`destinations_online`]), the
/// [`catalogue_path`], when Helios may write there ([`may_write`]).
pub fn catalogue_destinations(
    strategy: &Strategy,
    volumes: &[Volume],
    library: &str,
) -> Vec<(Volume, PathBuf)> {
    destinations_online(strategy, volumes)
        .into_iter()
        .filter_map(|v| {
            let folder = catalogue_path(v.mount.as_deref()?, library);
            may_write(&v, &folder).ok()?;
            Some((v, folder))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use helios_vault::MemoryStore;

    use super::*;
    use crate::{VolumeKind, detect};

    fn disk(id: &str, roles: &[Role]) -> Volume {
        Volume {
            id: VolumeId(id.into()),
            identified_by: Identity::Uuid,
            kind: VolumeKind::External,
            roles: roles.iter().copied().collect(),
            ..detect::weak(id, Some(1_000))
        }
    }

    fn at(volume: &str, rel: &str, presence: Presence) -> Location {
        Location {
            volume: Some(VolumeId(volume.into())),
            rel_path: Some(rel.into()),
            presence,
            ..Location::seen(PathBuf::from("/x").join(rel), 1, 1, 1)
        }
    }

    fn verified(entries: &[(&str, &str)]) -> BTreeMap<String, u64> {
        entries
            .iter()
            .map(|(v, r)| (copy_key(&VolumeId((*v).into()), Path::new(r)), 5))
            .collect()
    }

    /// The vault's backup filter (`docs/design/library.md`, L4) takes the
    /// photos whose [`status`] marks them so, for every state: what the
    /// Library's `backup:` words matched in the window, now in the query.
    #[test]
    fn the_vault_filter_takes_what_each_status_marks() {
        use helios_vault::{BackupFilter, BackupState, Image, Query, parse_hash};
        let library = disk("uuid:lib", &[Role::Library]);
        let a = disk("uuid:a", &[Role::Backup]);
        let b1 = Volume {
            drive: Some("sdb".into()),
            ..disk("uuid:b1", &[Role::Backup])
        };
        let b2 = Volume {
            drive: Some("sdb".into()),
            ..disk("uuid:b2", &[Role::Archive])
        };
        let far = Volume {
            off_site: true,
            ..disk("uuid:far", &[Role::Backup])
        };
        let weak = Volume {
            identified_by: Identity::Weak,
            ..disk("weak:w", &[Role::Backup])
        };
        let volumes = [library, a, b1, b2, far, weak];
        let mut store = MemoryStore::default();
        let ids: Vec<ImageId> = (1..=6u8)
            .map(|n| {
                let hash = parse_hash(&format!("{n:02x}").repeat(32)).unwrap();
                let image = Image {
                    id: ImageId(hash),
                    size: 10,
                    partial: hash,
                };
                store.add_to_library(&image).unwrap();
                image.id
            })
            .collect();
        // (image, volume, verified, presence)
        let copies = [
            (0, "uuid:a", true, Presence::Present),
            (1, "uuid:b1", true, Presence::Present),
            (1, "uuid:b2", false, Presence::Present),
            (2, "uuid:a", false, Presence::Offline),
            (4, "uuid:a", true, Presence::Present),
            (4, "uuid:far", true, Presence::Offline),
            (5, "weak:w", true, Presence::Present),
            (5, "uuid:a", true, Presence::Missing),
        ];
        for (n, id) in ids.iter().enumerate() {
            let rel = format!("2024/{n}.raf");
            store
                .put_location(id, &at("uuid:lib", &rel, Presence::Present))
                .unwrap();
        }
        for (n, volume, _, presence) in copies {
            let rel = format!("Helios Backup/{volume}/{n}.raf");
            store
                .put_location(&ids[n], &at(volume, &rel, presence))
                .unwrap();
        }
        let vault = Vault::new(store);
        for (n, volume, checked, _) in copies {
            let rel = format!("Helios Backup/{volume}/{n}.raf");
            if checked {
                record_verified(
                    &vault,
                    &ids[n],
                    &VolumeId(volume.into()),
                    Path::new(&rel),
                    5,
                )
                .unwrap();
            }
        }
        for strategy in [
            None,
            Some(Strategy::default()),
            Some(Strategy {
                copies: 2,
                off_site_required: true,
                ..Strategy::default()
            }),
        ] {
            let marks: BTreeMap<ImageId, BackupState> =
                statuses(&vault, &volumes, strategy.as_ref())
                    .unwrap()
                    .into_iter()
                    .map(|(id, (s, _))| (id, state_of(&s)))
                    .collect();
            let rule = rule(&volumes, strategy.as_ref());
            for state in BackupState::ALL {
                let query = Query {
                    backup: Some(BackupFilter {
                        rule: rule.clone(),
                        states: vec![state],
                    }),
                    ..Query::default()
                };
                let cursor = vault.open_cursor(query).unwrap();
                let got = vault.cursor_ids(cursor.id, 0, 99).unwrap();
                let want: Vec<ImageId> = marks
                    .iter()
                    .filter(|(_, s)| **s == state)
                    .map(|(id, _)| *id)
                    .collect();
                assert_eq!(got, want, "{strategy:?}: {state:?}");
                let facets = vault.facets(&Query::default(), Some(&rule)).unwrap();
                assert_eq!(facets.count("backup", state.name()), want.len() as u64);
            }
        }
    }

    #[test]
    fn copies_count_per_drive_raid_once_weak_never_library_excluded() {
        let library = disk("uuid:lib", &[Role::Library]);
        let a = disk("uuid:a", &[Role::Backup]);
        // Two partitions of one disk.
        let b1 = Volume {
            drive: Some("sdb".into()),
            ..disk("uuid:b1", &[Role::Backup])
        };
        let b2 = Volume {
            drive: Some("sdb".into()),
            ..disk("uuid:b2", &[Role::Archive])
        };
        // Two volumes of one RAID array.
        let r1 = Volume {
            raid: true,
            drive: Some("md0".into()),
            off_site: true,
            ..disk("uuid:r1", &[Role::Backup])
        };
        let r2 = Volume {
            raid: true,
            drive: Some("md0".into()),
            ..disk("uuid:r2", &[Role::Backup])
        };
        let weak = Volume {
            identified_by: Identity::Weak,
            ..disk("weak:w", &[Role::Backup])
        };
        let scratch = disk("uuid:s", &[Role::Scratch]);
        let volumes = [library, a, b1, b2, r1, r2, weak, scratch];
        let locations = [
            at("uuid:lib", "2024/x.raf", Presence::Present),
            at("uuid:a", "Helios Backup/2024/x.raf", Presence::Offline),
            at("uuid:b1", "Helios Backup/2024/x.raf", Presence::Present),
            at("uuid:b2", "Helios Archive/2024/x.raf", Presence::Present),
            at("uuid:r1", "Helios Backup/2024/x.raf", Presence::Present),
            at("uuid:r2", "Helios Backup/2024/x.raf", Presence::Present),
            at("weak:w", "Helios Backup/2024/x.raf", Presence::Present),
            at("uuid:s", "Helios Backup/2024/x.raf", Presence::Present),
        ];
        let all: Vec<(&str, &str)> = locations
            .iter()
            .map(|l| {
                (
                    l.volume.as_ref().unwrap().as_str(),
                    l.rel_path.as_ref().unwrap().to_str().unwrap(),
                )
            })
            .collect();
        let strategy = Strategy {
            copies: 3,
            off_site_required: true,
            ..Strategy::default()
        };
        let s = status(&locations, &volumes, Some(&verified(&all)), Some(&strategy));
        // a (offline still counts), sdb once, md0 once; never the library,
        // the weak or the scratch volume.
        assert_eq!(s.have, 3, "{s:?}");
        assert_eq!(s.want, 3);
        assert!(s.off_site_ok && s.met());

        // Unverified copies do not count, and are told apart.
        let s = status(
            &locations,
            &volumes,
            Some(&verified(&[("uuid:a", "Helios Backup/2024/x.raf")])),
            Some(&strategy),
        );
        assert_eq!((s.have, s.unverified), (1, 2), "{s:?}");
        assert!(!s.off_site_ok && !s.met());

        // A missing copy never counts; no strategy wants nothing.
        let gone = [at("uuid:a", "Helios Backup/2024/x.raf", Presence::Missing)];
        let s = status(&gone, &volumes, Some(&verified(&all)), None);
        assert_eq!((s.have, s.want, s.off_site_ok), (0, 0, true));
        assert!(s.met());
    }

    #[test]
    fn writes_go_only_to_backup_folders_of_writable_volumes() {
        let mut backup = disk("uuid:a", &[Role::Backup]);
        backup.mount = Some("/media/A".into());
        assert!(may_write(&backup, Path::new("/media/A/Helios Backup/2024/x.raf")).is_ok());
        assert!(may_write(&backup, Path::new("/media/A/Helios Archive/x.raf")).is_ok());
        assert!(may_write(&backup, Path::new("/media/A/photos/x.raf")).is_err());
        assert!(may_write(&backup, Path::new("/media/A/Helios Backup/../x.raf")).is_err());
        let mut none = disk("uuid:n", &[]);
        none.mount = Some("/media/N".into());
        assert!(may_write(&none, Path::new("/media/N/Helios Backup/x.raf")).is_err());
        let mut card = disk("uuid:c", &[Role::Backup]);
        card.kind = VolumeKind::Removable;
        card.has_dcim = true;
        card.mount = Some("/media/C".into());
        assert!(may_write(&card, Path::new("/media/C/Helios Backup/x.raf")).is_err());
        let ingest = disk("uuid:i", &[Role::Backup, Role::Ingest]);
        assert!(may_back_up_to(&ingest).is_err());
    }

    #[test]
    fn catalogue_snapshots_go_to_the_destinations_plugged_in() {
        let mut a = disk("uuid:a", &[Role::Backup]);
        a.mount = Some("/media/A".into());
        let b = disk("uuid:b", &[Role::Backup]);
        let mut none = disk("uuid:n", &[]);
        none.mount = Some("/media/N".into());
        let s = Strategy::default();
        let found = catalogue_destinations(&s, &[a, b, none], "library");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.id, VolumeId("uuid:a".into()));
        assert_eq!(
            found[0].1,
            Path::new("/media/A/Helios Backup/catalogue/library")
        );
        assert!(may_write(&found[0].0, &found[0].1).is_ok());
    }

    #[test]
    fn the_library_strategy_is_kept_in_the_vault() {
        let vault = Vault::new(MemoryStore::default());
        assert_eq!(
            load(&vault, &Scope::Library).unwrap(),
            None,
            "none at first"
        );
        let strategy = Strategy {
            copies: 2,
            destinations: vec![
                Destination::Role(Role::Backup),
                Destination::Volume(VolumeId("uuid:nas".into())),
            ],
            off_site_required: true,
            ..Strategy::default()
        };
        save(&vault, &Scope::Library, Some(&strategy)).unwrap();
        assert_eq!(load(&vault, &Scope::Library).unwrap(), Some(strategy));
        save(&vault, &Scope::Library, None).unwrap();
        assert_eq!(load(&vault, &Scope::Library).unwrap(), None);
    }
}
