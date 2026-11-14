//! Helios's own checks: the free space where Helios writes, the vault's
//! database integrity, the library opening with room to grow, a library
//! folder chosen, and its photos changed outside Helios (the backup checks are in
//! [`crate::backup`]).

use std::collections::BTreeSet;
use std::sync::Arc;

use helios_core::Message;
use helios_storage::{Role, Volume, capacity_text, registry};
use serde_json::json;

use crate::{Check, Context, Declaration, Event, Finding, Fix, Level};

/// The owner of the storage checks ([`storage`]).
pub const STORAGE_OWNER: &str = "storage";

/// The owner of the library checks ([`library`]).
pub const LIBRARY_OWNER: &str = "library";

/// The storage module's checks: the free space where Helios writes.
pub fn storage() -> Vec<Arc<dyn Check>> {
    vec![Arc::new(FreeSpace::default())]
}

/// The library module's checks: the vault's integrity, the library
/// opening with room, a library folder chosen, its photos changed
/// outside Helios.
pub fn library() -> Vec<Arc<dyn Check>> {
    vec![
        Arc::new(VaultIntegrity),
        Arc::new(Library::default()),
        Arc::new(FolderChosen),
        Arc::new(ChangedOutside),
    ]
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

impl Check for FreeSpace {
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

/// What the node read of the vault's database file (through the SQLite
/// driver), for [`VaultIntegrity`] and [`Library`].
#[derive(Clone, Debug)]
pub struct Database {
    /// Whether the file opens and reads its schema.
    pub opens: Result<(), String>,
    /// The problems SQLite's `quick_check` finds; empty when sound.
    pub problems: Result<Vec<String>, String>,
}

impl Check for VaultIntegrity {
    fn id(&self) -> &str {
        VAULT_INTEGRITY
    }

    fn title(&self) -> Message {
        Message::new("health-vault-integrity", "Catalogue integrity")
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("The vault's database passes SQLite's quick check")
            .levels(&[Level::Ok, Level::Problem])
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let (Some(path), Some(database)) = (&ctx.vault, &ctx.database) else {
            return Vec::new();
        };
        let file = path.display().to_string();
        let finding = match &database.problems {
            Ok(lines) if lines.is_empty() => Finding::new(
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
                    .with("error", e.clone()),
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

impl Check for Library {
    fn id(&self) -> &str {
        LIBRARY
    }

    fn title(&self) -> Message {
        Message::new("health-library", "The library")
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("The library's database opens, and its disk has room to grow")
            .levels(&[Level::Ok, Level::Warning, Level::Problem])
            .on(&[Event::Volumes])
            .fixes(&["library.openFolder", "library.storage"])
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let (Some(path), Some(database)) = (&ctx.vault, &ctx.database) else {
            return Vec::new();
        };
        let file = path.display().to_string();
        let folder = path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if let Err(e) = &database.opens {
            return vec![
                Finding::new(
                    LIBRARY,
                    "",
                    Level::Problem,
                    Message::new("health-library-closed", "The library cannot be opened"),
                    Message::new("health-library-closed-detail", "{ $file }: { $error }")
                        .with("file", file)
                        .with("error", e.clone()),
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

pub const FOLDER_CHOSEN: &str = "library.folder";

/// Where "Choose a folder…" leads when no library folder is chosen: the
/// one place to change when the first-run setup gets its folder step.
pub const CHOOSE_LIBRARY_FOLDER: &str = "library.chooseFolder";

/// A library folder is chosen: the folder imports copy photos into. None
/// chosen (unset, or an empty path) is a warning; a folder chosen but
/// gone is not this check's (the scan and the backups see it).
#[derive(Clone, Copy, Debug, Default)]
pub struct FolderChosen;

impl Check for FolderChosen {
    fn id(&self) -> &str {
        FOLDER_CHOSEN
    }

    fn title(&self) -> Message {
        Message::new("health-library-folder", "Library folder")
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("A library folder is chosen, for imports to copy photos into")
            .levels(&[Level::Ok, Level::Warning])
            .fixes(&[CHOOSE_LIBRARY_FOLDER])
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let Some(folder) = &ctx.library_folder else {
            return Vec::new();
        };
        match folder.as_deref().filter(|p| !p.as_os_str().is_empty()) {
            Some(path) => vec![Finding::new(
                FOLDER_CHOSEN,
                "",
                Level::Ok,
                Message::new("health-library-folder-ok", "A library folder is chosen"),
                Message::new("health-library-folder-ok-detail", "{ $folder }")
                    .with("folder", path.display().to_string()),
            )],
            None => vec![
                Finding::new(
                    FOLDER_CHOSEN,
                    "",
                    Level::Warning,
                    Message::new("health-library-folder-none", "No library folder chosen"),
                    Message::new(
                        "health-library-folder-none-detail",
                        "Imports that copy photos and picked photos have nowhere to go: they ask for a folder first.",
                    ),
                )
                .fix(Fix {
                    command: CHOOSE_LIBRARY_FOLDER.into(),
                    args: json!({}),
                    label: Message::new("health-fix-choose-library-folder", "Choose a folder…"),
                }),
            ],
        }
    }
}

pub const CHANGED_OUTSIDE: &str = "library.changed-outside";

/// The command that clears the mark "changed outside Helios" (every
/// photo's, without `images`).
pub const ACKNOWLEDGE_CHANGED: &str = "library.changedOutside.acknowledge";

/// Library photos whose file another application rewrote: still linked,
/// their edits kept, until the user acknowledges them.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChangedOutside;

impl Check for ChangedOutside {
    fn id(&self) -> &str {
        CHANGED_OUTSIDE
    }

    fn title(&self) -> Message {
        Message::new("health-changed-outside", "Photos changed outside Helios")
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("Library photos whose file another application rewrote")
            .levels(&[Level::Warning])
            .fixes(&[ACKNOWLEDGE_CHANGED])
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let Some(count) = ctx.changed_outside.filter(|c| *c > 0) else {
            return Vec::new();
        };
        vec![
            Finding::new(
                CHANGED_OUTSIDE,
                "",
                Level::Warning,
                Message::new(
                    "health-changed-outside-some",
                    "{ $count } photos changed outside Helios",
                )
                .with("count", count),
                Message::new(
                    "health-changed-outside-detail",
                    "Another application rewrote their files. They stay in your Library with their edits; their thumbnails were made again.",
                ),
            )
            .fix(Fix {
                command: ACKNOWLEDGE_CHANGED.into(),
                args: json!({}),
                label: Message::new("health-fix-acknowledge-changed", "Acknowledge"),
            }),
        ]
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
    fn photos_changed_outside_are_a_warning_until_acknowledged() {
        let quiet = |n| {
            ChangedOutside.run(&Context {
                changed_outside: n,
                ..Context::default()
            })
        };
        assert!(quiet(None).is_empty());
        assert!(quiet(Some(0)).is_empty(), "acknowledged: nothing to say");
        let found = quiet(Some(3));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].level, Level::Warning);
        assert_eq!(found[0].id, CHANGED_OUTSIDE);
        assert_eq!(found[0].fix.as_ref().unwrap().command, ACKNOWLEDGE_CHANGED);
    }

    #[test]
    fn no_library_folder_is_a_warning_whose_fix_chooses_one() {
        let run = |folder: Option<Option<PathBuf>>| {
            FolderChosen.run(&Context {
                library_folder: folder,
                ..Context::default()
            })
        };
        assert!(run(None).is_empty(), "no vault read: nothing to say");
        for unset in [None, Some(PathBuf::new())] {
            let found = run(Some(unset));
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].level, Level::Warning);
            assert_eq!(found[0].id, FOLDER_CHOSEN);
            let fix = found[0].fix.as_ref().unwrap();
            assert_eq!(fix.command, CHOOSE_LIBRARY_FOLDER);
        }
        let found = run(Some(Some(PathBuf::from("/home/me/Pictures/Helios"))));
        assert_eq!(found[0].level, Level::Ok);
        assert!(found[0].fix.is_none());
    }

    #[test]
    fn a_sound_vault_passes_and_garbage_is_a_problem() {
        let run = |database: Database| {
            VaultIntegrity.run(&Context {
                vault: Some(PathBuf::from("/lib/vault.sqlite")),
                database: Some(database),
                ..Context::default()
            })
        };
        let good = Database {
            opens: Ok(()),
            problems: Ok(Vec::new()),
        };
        let damaged = Database {
            opens: Ok(()),
            problems: Ok(vec!["row 3 missing from index".into()]),
        };
        let bad = Database {
            opens: Err("file is not a database".into()),
            problems: Err("file is not a database".into()),
        };
        assert_eq!(run(good)[0].level, Level::Ok);
        assert_eq!(run(damaged)[0].level, Level::Problem);
        assert_eq!(run(bad)[0].level, Level::Problem);
        assert!(VaultIntegrity.run(&Context::default()).is_empty());
    }
}
