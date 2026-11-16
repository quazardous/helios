//! The library module's health checks: the vault's database integrity,
//! the library opening with room to grow, a library folder chosen, its
//! photos changed outside Helios, and RAW+JPEG pairs shown as two photos.
//! They read the library's [`State`], gathered by [`read`] once per run.

use std::path::{Path, PathBuf};

use helios_core::Message;
use helios_health::{Declaration, Event, Finding, Fix, Gather, Level, Owned, Registry};
use helios_storage::{Volume, capacity_text, registry};
use helios_vault::{Store, Vault};
use serde_json::json;

/// The owner of the library checks.
pub const OWNER: &str = "library";

/// What the library checks read.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The volumes mounted now, with the roles the vault gives them.
    pub volumes: Vec<Volume>,
    /// The library's folder, when known: the volume holding it is the
    /// library's.
    pub library: Option<PathBuf>,
    /// The library folder chosen (where imports copy photos), read from
    /// the vault: `None` when not read (no vault: the check says
    /// nothing), `Some(None)` when none is chosen.
    pub library_folder: Option<Option<PathBuf>>,
    /// The vault's database file, when there is one.
    pub vault: Option<PathBuf>,
    /// How many Library photos are marked changed outside Helios (their
    /// file rewritten by another application); `None` without a vault.
    pub changed_outside: Option<usize>,
    /// How many RAW+JPEG pairs the Library shows as two photos (imported
    /// before pairs were one photo); `None` without a vault.
    pub raw_jpeg_pairs: Option<usize>,
    /// What the node read of the vault's database file (it opens, its
    /// integrity); `None` when not read (the library and integrity
    /// checks then say nothing).
    pub database: Option<Database>,
}

/// Reads the library's state: from `vault` (its folder chosen, its photos
/// changed outside, its pairs), the `volumes` mounted, the vault's `file`
/// (whose folder stands for the library's place) and what the database
/// driver read of it.
pub fn read<S: Store>(
    vault: Option<&Vault<S>>,
    volumes: Vec<Volume>,
    file: Option<&Path>,
    database: Option<Database>,
) -> State {
    State {
        volumes,
        library: file.and_then(Path::parent).map(Path::to_path_buf),
        vault: file.map(Path::to_path_buf),
        library_folder: vault.and_then(|v| {
            crate::import::library_root(v)
                .inspect_err(|e| log::warn!("health: library folder: {e}"))
                .ok()
        }),
        changed_outside: vault.and_then(|v| {
            v.changed_outside_images()
                .inspect_err(|e| log::warn!("health: changed outside: {e}"))
                .ok()
                .map(|l| l.len())
        }),
        raw_jpeg_pairs: vault.and_then(|v| {
            crate::pairs::count(v)
                .inspect_err(|e| log::warn!("health: RAW+JPEG pairs: {e}"))
                .ok()
        }),
        database,
    }
}

/// Adds the library module's checks to `registry`, owned by [`OWNER`],
/// over what `source` gathers.
pub fn register(registry: &mut Registry, source: &Gather<State>) {
    helios_health::add_owned(
        registry,
        OWNER,
        vec![
            helios_health::over(VaultIntegrity, source),
            helios_health::over(Library::default(), source),
            helios_health::over(FolderChosen, source),
            helios_health::over(ChangedOutside, source),
            helios_health::over(RawJpegPairs, source),
        ],
    );
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

impl Owned for VaultIntegrity {
    type State = State;

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

    fn findings(&self, state: &State) -> Vec<Finding> {
        let (Some(path), Some(database)) = (&state.vault, &state.database) else {
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

impl Owned for Library {
    type State = State;

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

    fn findings(&self, state: &State) -> Vec<Finding> {
        let (Some(path), Some(database)) = (&state.vault, &state.database) else {
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
        let holder = state
            .library
            .as_deref()
            .and_then(|l| registry::holding(&state.volumes, l));
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
/// first-run setup's library folder step.
pub const CHOOSE_LIBRARY_FOLDER: &str = "init.libraryFolder";

/// A library folder is chosen: the folder imports copy photos into. None
/// chosen (unset, or an empty path) is a warning; so is a folder chosen
/// but gone, or read-only, with the same fix.
#[derive(Clone, Copy, Debug, Default)]
pub struct FolderChosen;

impl Owned for FolderChosen {
    type State = State;

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

    fn findings(&self, state: &State) -> Vec<Finding> {
        let Some(folder) = &state.library_folder else {
            return Vec::new();
        };
        let fix = || Fix {
            command: CHOOSE_LIBRARY_FOLDER.into(),
            args: json!({}),
            label: Message::new("health-fix-choose-library-folder", "Choose a folder…"),
        };
        match folder.as_deref().filter(|p| !p.as_os_str().is_empty()) {
            Some(path) if !writable_dir(path) => vec![
                Finding::new(
                    FOLDER_CHOSEN,
                    "",
                    Level::Warning,
                    Message::new(
                        "health-library-folder-unwritable",
                        "The library folder cannot be written",
                    ),
                    Message::new(
                        "health-library-folder-unwritable-detail",
                        "{ $folder } is missing or read-only: imports cannot copy photos into it.",
                    )
                    .with("folder", path.display().to_string()),
                )
                .fix(fix()),
            ],
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
                .fix(fix()),
            ],
        }
    }
}

/// `path` is a folder, not marked read-only.
fn writable_dir(path: &std::path::Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_dir() && !m.permissions().readonly())
}

pub const CHANGED_OUTSIDE: &str = "library.changed-outside";

/// The command that clears the mark "changed outside Helios" (every
/// photo's, without `images`).
pub const ACKNOWLEDGE_CHANGED: &str = "library.changedOutside.acknowledge";

/// Library photos whose file another application rewrote: still linked,
/// their edits kept, until the user acknowledges them.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChangedOutside;

impl Owned for ChangedOutside {
    type State = State;

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

    fn findings(&self, state: &State) -> Vec<Finding> {
        let Some(count) = state.changed_outside.filter(|c| *c > 0) else {
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

pub const RAW_JPEG_PAIRS: &str = "library.raw-jpeg-pairs";

/// The desktop command that pairs RAW+JPEG files ("Pair RAW+JPEG
/// Files"); with `all`, the whole Library's pairs.
pub const PAIR_FILES: &str = "library.files.pair";

/// RAW+JPEG pairs the Library shows as two photos (imported before a
/// pair was one photo, `docs/design/raw-jpeg-pairs.md`): offered to pair,
/// never merged without the user's word, since the merge chooses between
/// their marks.
#[derive(Clone, Copy, Debug, Default)]
pub struct RawJpegPairs;

impl Owned for RawJpegPairs {
    type State = State;

    fn id(&self) -> &str {
        RAW_JPEG_PAIRS
    }

    fn title(&self) -> Message {
        Message::new("health-raw-jpeg-pairs", "RAW+JPEG pairs")
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("RAW+JPEG pairs the Library shows as two photos")
            .levels(&[Level::Warning])
            .fixes(&[PAIR_FILES])
    }

    fn findings(&self, state: &State) -> Vec<Finding> {
        let Some(count) = state.raw_jpeg_pairs.filter(|c| *c > 0) else {
            return Vec::new();
        };
        vec![
            Finding::new(
                RAW_JPEG_PAIRS,
                "",
                Level::Warning,
                Message::new(
                    "health-raw-jpeg-pairs-some",
                    "{ $count } RAW+JPEG pairs are shown as two photos",
                )
                .with("count", count),
                Message::new(
                    "health-raw-jpeg-pairs-detail",
                    "Pairing makes each one photo with two files: the RAW keeps its marks where both have one, labels, tags and albums are merged. One undo step.",
                ),
            )
            .fix(Fix {
                command: PAIR_FILES.into(),
                args: json!({ "all": true }),
                label: Message::new("health-fix-pair-files", "Pair them"),
            }),
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn photos_changed_outside_are_a_warning_until_acknowledged() {
        let quiet = |n| {
            ChangedOutside.findings(&State {
                changed_outside: n,
                ..State::default()
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
    fn raw_jpeg_pairs_shown_as_two_are_offered_to_pair() {
        let run = |n| {
            RawJpegPairs.findings(&State {
                raw_jpeg_pairs: n,
                ..State::default()
            })
        };
        assert!(run(None).is_empty(), "no vault: nothing to say");
        assert!(run(Some(0)).is_empty(), "no pair: nothing to say");
        let found = run(Some(4));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].level, Level::Warning);
        assert_eq!(found[0].id, RAW_JPEG_PAIRS);
        let fix = found[0].fix.as_ref().unwrap();
        assert_eq!(fix.command, PAIR_FILES);
        assert_eq!(fix.args, json!({ "all": true }));
    }

    #[test]
    fn no_library_folder_is_a_warning_whose_fix_chooses_one() {
        let run = |folder: Option<Option<PathBuf>>| {
            FolderChosen.findings(&State {
                library_folder: folder,
                ..State::default()
            })
        };
        assert!(run(None).is_empty(), "no vault read: nothing to say");
        let gone = PathBuf::from("/nowhere/helios-test/Pictures/Helios");
        for unset in [None, Some(PathBuf::new()), Some(gone)] {
            let found = run(Some(unset));
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].level, Level::Warning);
            assert_eq!(found[0].id, FOLDER_CHOSEN);
            let fix = found[0].fix.as_ref().unwrap();
            assert_eq!(fix.command, CHOOSE_LIBRARY_FOLDER);
        }
        let found = run(Some(Some(std::env::temp_dir())));
        assert_eq!(found[0].level, Level::Ok);
        assert!(found[0].fix.is_none());
    }

    #[test]
    fn a_sound_vault_passes_and_garbage_is_a_problem() {
        let run = |database: Database| {
            VaultIntegrity.findings(&State {
                vault: Some(PathBuf::from("/lib/vault.sqlite")),
                database: Some(database),
                ..State::default()
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
        assert!(VaultIntegrity.findings(&State::default()).is_empty());
    }
}
