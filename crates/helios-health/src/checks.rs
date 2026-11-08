//! Helios's own checks: the free space where Helios writes, the vault's
//! database integrity, and the library opening with room to grow (the
//! backup checks are in [`crate::backup`]).

use std::collections::BTreeSet;
use std::path::Path;

use helios_core::Message;
use helios_storage::{Role, Volume, capacity_text, registry};
use serde_json::json;

use crate::{Check, Context, Finding, Fix, Level};

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

impl Check for FreeSpace {
    fn id(&self) -> &str {
        FREE_SPACE
    }

    fn title(&self) -> Message {
        Message::new("health-free-space", "Free space")
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let library = ctx
            .library
            .as_deref()
            .and_then(|l| registry::holding(&ctx.volumes, l))
            .map(|v| v.id.clone());
        let watched: Vec<&Volume> = ctx
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

pub const VAULT_INTEGRITY: &str = "vault.integrity";

/// SQLite's `PRAGMA quick_check` on the vault's database, read-only.
#[derive(Clone, Copy, Debug, Default)]
pub struct VaultIntegrity;

/// What `quick_check` says of the database at `path`: `ok`, else its
/// complaints.
pub fn quick_check(path: &Path) -> Result<Vec<String>, String> {
    let db = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| e.to_string())?;
    let mut stmt = db
        .prepare("PRAGMA quick_check")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

impl Check for VaultIntegrity {
    fn id(&self) -> &str {
        VAULT_INTEGRITY
    }

    fn title(&self) -> Message {
        Message::new("health-vault-integrity", "Catalogue integrity")
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let Some(path) = &ctx.vault else {
            return Vec::new();
        };
        let file = path.display().to_string();
        let finding = match quick_check(path) {
            Ok(lines) if lines.len() == 1 && lines[0] == "ok" => Finding::new(
                VAULT_INTEGRITY,
                "",
                Level::Ok,
                Message::new("health-vault-ok", "The catalogue is sound"),
                Message::new(
                    "health-vault-ok-detail",
                    "{ $file } passed SQLite's quick check",
                )
                .with("file", file),
            ),
            Ok(lines) => Finding::new(
                VAULT_INTEGRITY,
                "",
                Level::Problem,
                Message::new("health-vault-damaged", "The catalogue is damaged"),
                Message::new(
                    "health-vault-damaged-detail",
                    "{ $file }: { $first } ({ $count } complaints). Restore a catalogue snapshot.",
                )
                .with("file", file)
                .with("first", lines.first().cloned().unwrap_or_default())
                .with("count", lines.len()),
            ),
            Err(e) => Finding::new(
                VAULT_INTEGRITY,
                "",
                Level::Problem,
                Message::new("health-vault-unreadable", "The catalogue cannot be checked"),
                Message::new("health-vault-unreadable-detail", "{ $file }: { $error }")
                    .with("file", file)
                    .with("error", e),
            ),
        };
        vec![finding]
    }
}

pub const LIBRARY: &str = "library.default";

/// The library (the vault's file): it opens, and its disk has room to
/// grow (a warning under 5 % or 2 GB free).
#[derive(Clone, Debug)]
pub struct Library {
    pub warn_fraction: f64,
    pub warn_bytes: u64,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            warn_fraction: 0.05,
            warn_bytes: 2_000_000_000,
        }
    }
}

/// Whether the SQLite file at `path` opens and reads.
pub fn opens(path: &Path) -> Result<(), String> {
    let db = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| e.to_string())?;
    db.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    })
    .map(|_| ())
    .map_err(|e| e.to_string())
}

impl Check for Library {
    fn id(&self) -> &str {
        LIBRARY
    }

    fn title(&self) -> Message {
        Message::new("health-library", "The library")
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let Some(path) = &ctx.vault else {
            return Vec::new();
        };
        let file = path.display().to_string();
        let folder = path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if let Err(e) = opens(path) {
            return vec![
                Finding::new(
                    LIBRARY,
                    "",
                    Level::Problem,
                    Message::new("health-library-closed", "The library cannot be opened"),
                    Message::new("health-library-closed-detail", "{ $file }: { $error }")
                        .with("file", file)
                        .with("error", e),
                )
                .fix(Fix {
                    command: "library.openFolder".into(),
                    args: json!({ "path": folder }),
                    label: Message::new("health-fix-library-folder", "Open the library's folder"),
                }),
            ];
        }
        let holder = ctx
            .library
            .as_deref()
            .and_then(|l| registry::holding(&ctx.volumes, l));
        if let Some(v) = holder
            && let (Some(free), Some(capacity)) = (v.free, v.capacity)
            && (free < self.warn_bytes
                || (capacity > 0 && (free as f64) < self.warn_fraction * capacity as f64))
        {
            return vec![
                Finding::new(
                    LIBRARY,
                    "",
                    Level::Warning,
                    Message::new(
                        "health-library-no-room",
                        "The library's disk is almost full",
                    ),
                    Message::new(
                        "health-library-no-room-detail",
                        "{ $free } free of { $capacity } on { $volume }: the library needs room to grow.",
                    )
                    .with("free", capacity_text(free))
                    .with("capacity", capacity_text(capacity))
                    .with("volume", v.name.clone()),
                )
                .fix(Fix {
                    command: "library.storage".into(),
                    args: json!({ "volume": v.id.as_str() }),
                    label: Message::new(
                        "health-fix-show-volume",
                        "Show it on the Storage page",
                    ),
                }),
            ];
        }
        vec![Finding::new(
            LIBRARY,
            "",
            Level::Ok,
            Message::new("health-library-ok", "The library opens and has room"),
            Message::new("health-library-ok-detail", "{ $file }").with("file", file),
        )]
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use helios_storage::{Identity, VolumeId, VolumeKind};

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
        let ctx = Context {
            volumes: vec![
                volume("home", "/home", &[], 10 * GB, 1000 * GB),
                volume("backup", "/mnt/b", &[Role::Backup], 2 * GB, 1000 * GB),
                volume("card", "/media/c", &[Role::Ingest], 0, 64 * GB),
                volume("other", "/mnt/o", &[], 0, 64 * GB),
            ],
            library: Some(PathBuf::from("/home/me/Pictures")),
            ..Context::default()
        };
        let found = FreeSpace::default().run(&ctx);
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

    #[test]
    fn a_sound_vault_passes_and_garbage_is_a_problem() {
        let dir = std::env::temp_dir().join(format!("helios-health-vault-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.sqlite");
        let _ = std::fs::remove_file(&good);
        rusqlite::Connection::open(&good)
            .unwrap()
            .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);")
            .unwrap();
        let bad = dir.join("bad.sqlite");
        std::fs::write(&bad, vec![0x5a; 8192]).unwrap();
        let run = |p: &Path| {
            VaultIntegrity.run(&Context {
                vault: Some(p.to_path_buf()),
                ..Context::default()
            })
        };
        assert_eq!(run(&good)[0].level, Level::Ok);
        assert_eq!(run(&bad)[0].level, Level::Problem);
        assert!(VaultIntegrity.run(&Context::default()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
