//! The storage module's health check: the free space where Helios
//! writes. It reads the volumes mounted, with the roles the vault gives
//! them ([`Disks`], gathered by [`disks`] once per run). The backup checks
//! are in [`backup`].

pub mod backup;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use helios_core::Message;
use helios_health::{Declaration, Event, Finding, Fix, Gather, Level, Owned, Registry};
use helios_vault::{Store, Vault};
use serde_json::json;

use crate::{Role, Volume, capacity_text, registry};

/// The owner of the storage checks.
pub const OWNER: &str = "storage";

/// What the storage checks read.
#[derive(Clone, Debug, Default)]
pub struct Disks {
    /// The volumes mounted now, with the roles the vault gives them.
    pub volumes: Vec<Volume>,
    /// The library's folder, when known: the volume holding it is the
    /// library's.
    pub library: Option<PathBuf>,
}

/// The volumes `mounted` now, each named and given its roles as `vault`
/// knows it, and the `library`'s folder.
pub fn disks<S: Store>(
    mut mounted: Vec<Volume>,
    vault: Option<&Vault<S>>,
    library: Option<&Path>,
) -> Disks {
    if let Some(known) = vault.and_then(|v| registry::known(v).ok()) {
        for v in &mut mounted {
            if let Some(k) = known.iter().find(|k| k.id == v.id) {
                v.roles.clone_from(&k.roles);
                v.name.clone_from(&k.name);
            }
        }
    }
    Disks {
        volumes: mounted,
        library: library.map(Path::to_path_buf),
    }
}

/// Adds the storage module's checks to `registry`, owned by [`OWNER`],
/// over what `source` gathers.
pub fn register(registry: &mut Registry, source: &Gather<Disks>) {
    helios_health::add_owned(
        registry,
        OWNER,
        vec![helios_health::over(FreeSpace::default(), source)],
    );
}

/// Free space on the library's volume and on every volume with a role
/// Helios writes to (library, archive, backup, scratch).
#[derive(Clone, Debug)]
pub struct FreeSpace {
    /// Under this fraction or this many bytes free: a warning.
    pub warn_fraction: f64,
    pub warn_bytes: u64,
    /// Under this fraction or this many bytes free: a problem.
    pub problem_fraction: f64,
    pub problem_bytes: u64,
}

impl Default for FreeSpace {
    fn default() -> Self {
        Self {
            warn_fraction: 0.10,
            warn_bytes: 20_000_000_000,
            problem_fraction: 0.03,
            problem_bytes: 5_000_000_000,
        }
    }
}

pub const FREE_SPACE: &str = "storage.free-space";

impl FreeSpace {
    /// The level of a volume with `free` bytes of `capacity`.
    pub fn level(&self, free: u64, capacity: u64) -> Level {
        let fraction = if capacity == 0 {
            1.0
        } else {
            free as f64 / capacity as f64
        };
        if fraction < self.problem_fraction || free < self.problem_bytes {
            Level::Problem
        } else if fraction < self.warn_fraction || free < self.warn_bytes {
            Level::Warning
        } else {
            Level::Ok
        }
    }
}

/// Whether Helios writes to a volume with these roles.
fn writes_to(roles: &BTreeSet<Role>) -> bool {
    roles.iter().any(|r| {
        matches!(
            r,
            Role::Library | Role::Archive | Role::Backup | Role::Scratch
        )
    })
}

impl Owned for FreeSpace {
    type State = Disks;

    fn id(&self) -> &str {
        FREE_SPACE
    }

    fn title(&self) -> Message {
        Message::new("health-free-space", "Free space")
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("Free space on the library's volume and on every volume Helios writes to")
            .levels(&[Level::Ok, Level::Warning, Level::Problem])
            .every(60)
            .on(&[Event::Volumes])
            .fixes(&["library.storage"])
    }

    fn findings(&self, state: &Disks) -> Vec<Finding> {
        let library = state
            .library
            .as_deref()
            .and_then(|l| registry::holding(&state.volumes, l))
            .map(|v| v.id.clone());
        let watched: Vec<&Volume> = state
            .volumes
            .iter()
            .filter(|v| v.online() && (Some(&v.id) == library.as_ref() || writes_to(&v.roles)))
            .collect();
        watched
            .into_iter()
            .filter_map(|v| {
                let (free, capacity) = (v.free?, v.capacity?);
                let level = self.level(free, capacity);
                let percent = if capacity == 0 {
                    100
                } else {
                    (free as f64 * 100.0 / capacity as f64).round() as u64
                };
                let message = match level {
                    Level::Ok => Message::new("health-free-space-ok", "{ $volume } has room"),
                    Level::Warning => Message::new(
                        "health-free-space-low",
                        "{ $volume } is running out of space",
                    ),
                    Level::Problem => {
                        Message::new("health-free-space-full", "{ $volume } is almost full")
                    }
                }
                .with("volume", v.name.clone());
                let detail = Message::new(
                    "health-free-space-detail",
                    "{ $free } free of { $capacity } ({ $percent } %)",
                )
                .with("free", capacity_text(free))
                .with("capacity", capacity_text(capacity))
                .with("percent", percent);
                let mut finding = Finding::new(FREE_SPACE, v.id.as_str(), level, message, detail);
                if level > Level::Ok {
                    // The Storage page, on this volume.
                    finding = finding.fix(Fix {
                        command: "library.storage".into(),
                        args: json!({ "volume": v.id.as_str() }),
                        label: Message::new(
                            "health-fix-show-volume",
                            "Show it on the Storage page",
                        ),
                    });
                }
                Some(finding)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::{Identity, VolumeId, VolumeKind};

    use super::*;

    const GB: u64 = 1_000_000_000;

    #[test]
    fn free_space_levels_follow_the_fraction_and_the_bytes() {
        let c = FreeSpace::default();
        // A big disk: the fraction decides.
        assert_eq!(c.level(400 * GB, 2000 * GB), Level::Ok);
        assert_eq!(c.level(150 * GB, 2000 * GB), Level::Warning);
        assert_eq!(c.level(50 * GB, 2000 * GB), Level::Problem);
        // A small disk: the bytes decide.
        assert_eq!(c.level(15 * GB, 100 * GB), Level::Warning);
        assert_eq!(c.level(4 * GB, 100 * GB), Level::Problem);
        assert_eq!(c.level(30 * GB, 100 * GB), Level::Ok);
    }

    fn volume(id: &str, mount: &str, roles: &[Role], free: u64, capacity: u64) -> Volume {
        Volume {
            id: VolumeId(id.into()),
            identified_by: Identity::Uuid,
            name: id.into(),
            label: None,
            kind: VolumeKind::External,
            roles: roles.iter().copied().collect(),
            drive: None,
            capacity: Some(capacity),
            free: Some(free),
            mount: Some(mount.into()),
            node: None,
            last_seen: None,
            has_dcim: false,
            off_site: false,
            raid: false,
        }
    }

    #[test]
    fn free_space_watches_the_library_and_the_writing_roles_only() {
        let ctx = Disks {
            volumes: vec![
                volume("home", "/home", &[], 10 * GB, 1000 * GB),
                volume("backup", "/mnt/b", &[Role::Backup], 2 * GB, 1000 * GB),
                volume("card", "/media/c", &[Role::Ingest], 0, 64 * GB),
                volume("other", "/mnt/o", &[], 0, 64 * GB),
            ],
            library: Some(PathBuf::from("/home/me/Pictures")),
        };
        let found = FreeSpace::default().findings(&ctx);
        let ids: Vec<(&str, Level)> = found.iter().map(|f| (f.id.as_str(), f.level)).collect();
        assert_eq!(
            ids,
            [
                ("storage.free-space/home", Level::Problem),
                ("storage.free-space/backup", Level::Problem),
            ]
        );
        let fix = found[1].fix.as_ref().unwrap();
        assert_eq!(fix.command, "library.storage");
        assert_eq!(fix.args["volume"], "backup");
    }
}
