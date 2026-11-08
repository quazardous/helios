//! The SQLite driver of the vault: one database file, readable with
//! standard tools. It implements `helios_vault::Store` and passes the
//! vault's contract; nothing above the store knows SQLite is there.
//!
//! Durability: the database runs in WAL mode with `synchronous=NORMAL`, so
//! readers never block the writer and a crash loses at most the last
//! commits, never the file. Several connections (several processes) share
//! one vault through SQLite's own locking: a writer waits up to
//! [`Options::busy_timeout`] for another one, and every write that takes
//! more than one statement runs in one `BEGIN IMMEDIATE` transaction. There
//! is no lock file.
//!
//! Schema: numbered migrations and `PRAGMA user_version`, with a verified
//! snapshot before any change (see the `migrate` module and [`snapshot`]).
//!
//! Integrity: closing a vault leaves a clean-shutdown marker in it, and
//! opening one clears it. A vault opened without the marker was not closed
//! (a crash, a kill, a power cut): `PRAGMA quick_check` runs before it is
//! used, and a vault that fails it opens read-only, with the latest
//! snapshot named ([`OpenError::Damaged`]). With several processes on one
//! vault, the marker is a hint, not a proof: a process that opens while
//! another one runs checks for nothing, and a crash while another process
//! closes cleanly goes unseen until the next scheduled snapshot, whose copy
//! is checked too.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use helios_core::ContentHash;
use helios_vault::fields::{AuditEntry, LABELS, Order, PICKED, Query, RATING, REJECTED, TAGS};
use helios_vault::storage::VolumeId;
use helios_vault::{
    Folder, FolderState, Image, ImageId, LibraryFlag, Location, Presence, Sighting, Store, ToRead,
    Version, VersionId, parse_hash,
};

mod migrate;
pub mod snapshot;

pub use migrate::{Migrated, SCHEMA_VERSION};

/// How a vault database is opened.
#[derive(Clone)]
pub struct Options {
    /// How long a write waits for another connection's write before it
    /// fails as busy.
    pub busy_timeout: Duration,
    /// Told how a long operation (a snapshot, a migration) advances. The
    /// store has no bus of its own: [`Progress`] is a bus event, so the
    /// caller can publish it as it comes.
    pub on_progress: Option<OnProgress>,
    /// How many snapshots are kept: taking one removes the oldest beyond
    /// this (at least one is always kept).
    pub keep_snapshots: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            busy_timeout: Duration::from_secs(5),
            on_progress: None,
            keep_snapshots: 5,
        }
    }
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("busy_timeout", &self.busy_timeout)
            .field("on_progress", &self.on_progress.is_some())
            .field("keep_snapshots", &self.keep_snapshots)
            .finish()
    }
}

/// Receives [`Progress`] reports.
pub type OnProgress = Arc<dyn Fn(&Progress) + Send + Sync>;

/// How far a long operation is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    /// "snapshot" (pages copied) or "migration" (steps run).
    pub stage: String,
    pub done: u64,
    pub total: u64,
    /// The step starting, for a migration ("2 add an index").
    pub step: Option<String>,
}

impl helios_core::Event for Progress {
    const KIND: &'static str = "vault.migration_progress";
}

/// Why a vault database could not be opened.
#[derive(Debug)]
pub enum OpenError {
    Sqlite(String),
    /// Written by a newer Helios: this one does not know its schema and
    /// does not touch it (there is no downgrade).
    TooNew {
        found: u32,
        supported: u32,
    },
    /// The snapshot before a migration could not be made or verified;
    /// nothing was migrated.
    Snapshot(String),
    /// A migration step failed; the database stays at the step before.
    Migration {
        version: u32,
        name: &'static str,
        message: String,
        snapshot: Option<PathBuf>,
    },
    /// The database failed its integrity check after an unclean shutdown,
    /// or could not be read at all.
    Damaged(Damaged),
}

/// A vault that failed its integrity check.
#[derive(Debug)]
pub struct Damaged {
    pub path: PathBuf,
    /// What `PRAGMA quick_check` (or SQLite) reported.
    pub problems: Vec<String>,
    /// The latest snapshot, to restore the vault from.
    pub snapshot: Option<PathBuf>,
    /// The vault open read-only, when SQLite can still open it: what is
    /// readable can be read (and exported), every write is refused.
    pub read_only: Option<Box<SqliteStore>>,
}

fn restore_advice(path: &Path, snapshot: Option<&Path>) -> String {
    match snapshot {
        Some(s) => format!(
            "the latest snapshot is {}: close Helios, then replace {} with it",
            s.display(),
            path.display()
        ),
        None => "there is no snapshot to restore from".into(),
    }
}

impl std::fmt::Display for Damaged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown: Vec<&str> = self.problems.iter().take(3).map(String::as_str).collect();
        write!(
            f,
            "the vault {} is damaged ({}{}); {}",
            self.path.display(),
            shown.join("; "),
            if self.problems.len() > 3 {
                format!("; {} more", self.problems.len() - 3)
            } else {
                String::new()
            },
            restore_advice(&self.path, self.snapshot.as_deref())
        )
    }
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "{e}"),
            Self::TooNew { found, supported } => write!(
                f,
                "this vault has schema version {found}, written by a newer Helios; \
                 this one reads up to version {supported} and leaves it untouched"
            ),
            Self::Snapshot(e) => write!(
                f,
                "the vault was not upgraded: its snapshot failed first ({e})"
            ),
            Self::Migration {
                version,
                name,
                message,
                snapshot,
            } => {
                write!(
                    f,
                    "upgrading the vault to version {version} ({name}) failed: {message}"
                )?;
                if let Some(s) = snapshot {
                    write!(f, "; a copy from before the upgrade is in {}", s.display())?;
                }
                Ok(())
            }
            Self::Damaged(d) => d.fmt(f),
        }
    }
}

impl std::error::Error for OpenError {}

impl From<OpenError> for String {
    fn from(e: OpenError) -> Self {
        e.to_string()
    }
}

/// What happened while a vault was opened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenReport {
    /// The schema change, when there was one.
    pub migrated: Option<Migrated>,
    /// The vault had not been closed: it was checked, and passed.
    pub unclean_shutdown: bool,
}

/// The settings a connection runs with, as SQLite reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pragmas {
    pub journal_mode: String,
    /// 0 OFF, 1 NORMAL, 2 FULL, 3 EXTRA.
    pub synchronous: i64,
    pub busy_timeout_ms: i64,
    pub foreign_keys: bool,
}

pub struct SqliteStore {
    db: Connection,
    /// The database file; none in memory.
    path: Option<PathBuf>,
    options: Options,
    report: OpenReport,
    /// Why writes are refused, for a vault open read-only after a failed
    /// check.
    read_only: Option<String>,
    /// The clean-shutdown marker is already written.
    closed: bool,
}

/// The key of the clean-shutdown marker in `vault_state`.
const CLEAN_SHUTDOWN: &str = "clean_shutdown";

fn closed_cleanly(db: &Connection) -> bool {
    db.query_row(
        "SELECT 1 FROM vault_state WHERE key = ?1",
        [CLEAN_SHUTDOWN],
        |_| Ok(()),
    )
    .is_ok()
}

/// SQLite errors that mean the file itself is broken.
fn corrupt(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
    )
}

fn damaged(path: &Path, problems: Vec<String>) -> OpenError {
    OpenError::Damaged(Damaged {
        path: path.to_path_buf(),
        problems,
        snapshot: None,
        read_only: None,
    })
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore")
            .field("path", &self.path)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl SqliteStore {
    fn versions_where(&self, condition: &str, key: &str) -> Result<Vec<Version>, String> {
        type Raw = (String, String, String, String, String, i64, Option<String>);
        let mut statement = self
            .db
            .prepare(&format!(
                "SELECT id, image, parents, edit, origin, at, label FROM versions WHERE {condition}"
            ))
            .map_err(err)?;
        let rows: Vec<Raw> = statement
            .query_map(params![key], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)?;
        rows.into_iter()
            .map(|(id, image, parents, edit, origin, at, label)| {
                let parents: Vec<String> =
                    serde_json::from_str(&parents).map_err(|e| format!("version {id}: {e}"))?;
                Ok(Version {
                    id: VersionId::from_hex(&id)?,
                    image: ImageId::from_hex(&image)?,
                    parents: parents
                        .iter()
                        .map(|p| VersionId::from_hex(p))
                        .collect::<Result<_, _>>()?,
                    edit: serde_json::from_str(&edit).map_err(|e| format!("version {id}: {e}"))?,
                    origin,
                    at: at as u64,
                    label,
                })
            })
            .collect()
    }
}

fn err(e: rusqlite::Error) -> String {
    e.to_string()
}

impl SqliteStore {
    /// Opens (or creates) a vault database file with the default options.
    pub fn open(path: &Path) -> Result<Self, String> {
        Ok(Self::open_with(path, &Options::default())?)
    }

    /// Opens (or creates) a vault database file, bringing its schema to
    /// [`SCHEMA_VERSION`].
    ///
    /// A vault that was not closed is checked first; one that fails the
    /// check is returned read-only inside [`OpenError::Damaged`].
    pub fn open_with(path: &Path, options: &Options) -> Result<Self, OpenError> {
        let db = Connection::open(path).map_err(|e| OpenError::Sqlite(err(e)))?;
        match Self::init(db, Some(path), options) {
            // The read-write connection is closed by now.
            Err(OpenError::Damaged(d)) => Err(OpenError::Damaged(Self::read_only(
                path, options, d.problems,
            ))),
            other => other,
        }
    }

    fn read_only(path: &Path, options: &Options, problems: Vec<String>) -> Damaged {
        let snapshot = snapshot::list(path).pop();
        let refusal = format!(
            "the vault {} failed its integrity check and is open read-only; {}",
            path.display(),
            restore_advice(path, snapshot.as_deref())
        );
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let read_only = Connection::open_with_flags(path, flags).ok().map(|db| {
            let _ = db.busy_timeout(options.busy_timeout);
            Box::new(Self {
                db,
                path: Some(path.to_path_buf()),
                options: options.clone(),
                report: OpenReport::default(),
                read_only: Some(refusal),
                closed: false,
            })
        });
        Damaged {
            path: path.to_path_buf(),
            problems,
            snapshot,
            read_only,
        }
    }

    pub fn in_memory() -> Result<Self, String> {
        let db = Connection::open_in_memory().map_err(err)?;
        Ok(Self::init(db, None, &Options::default())?)
    }

    fn init(mut db: Connection, path: Option<&Path>, options: &Options) -> Result<Self, OpenError> {
        let sqlite = |e: rusqlite::Error| match path {
            Some(path) if corrupt(&e) => damaged(path, vec![err(e)]),
            _ => OpenError::Sqlite(err(e)),
        };
        db.busy_timeout(options.busy_timeout).map_err(sqlite)?;
        db.pragma_update(None, "foreign_keys", "ON")
            .map_err(sqlite)?;
        // WAL is kept in the file; an in-memory database answers "memory".
        let _mode: String = db
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .map_err(sqlite)?;
        db.pragma_update(None, "synchronous", "NORMAL")
            .map_err(sqlite)?;
        let mut unclean_shutdown = false;
        if let Some(path) = path {
            let fresh = !migrate::has_tables(&db).map_err(sqlite)?;
            if !fresh && !closed_cleanly(&db) {
                unclean_shutdown = true;
                let problems = snapshot::quick_check(&db).unwrap_or_else(|e| vec![e]);
                if !problems.is_empty() {
                    return Err(damaged(path, problems));
                }
            }
        }
        let migrated =
            migrate::migrate(&mut db, path, options, migrate::MIGRATIONS, migrate::VIEWS)?;
        if path.is_some() {
            db.execute("DELETE FROM vault_state WHERE key = ?1", [CLEAN_SHUTDOWN])
                .map_err(sqlite)?;
        }
        Ok(Self {
            db,
            path: path.map(Path::to_path_buf),
            options: options.clone(),
            report: OpenReport {
                migrated,
                unclean_shutdown,
            },
            read_only: None,
            closed: false,
        })
    }

    /// Closes the vault, leaving the clean-shutdown marker; dropping it
    /// does the same, without the error.
    pub fn close(mut self) -> Result<(), String> {
        self.mark_clean()
    }

    fn mark_clean(&mut self) -> Result<(), String> {
        if self.path.is_none() || self.read_only.is_some() || self.closed {
            return Ok(());
        }
        self.closed = true;
        if !self.db.is_autocommit() {
            // A transaction left open is not kept.
            self.db.execute_batch("ROLLBACK").map_err(err)?;
        }
        self.db
            .execute(
                "INSERT INTO vault_state (key, value) VALUES (?1, '1') ON CONFLICT (key) DO NOTHING",
                [CLEAN_SHUTDOWN],
            )
            .map(|_| ())
            .map_err(err)
    }

    fn writable(&self) -> Result<(), String> {
        self.read_only
            .as_ref()
            .map_or(Ok(()), |why| Err(why.clone()))
    }

    /// What happened while the vault was opened.
    pub fn report(&self) -> &OpenReport {
        &self.report
    }

    /// Takes a verified snapshot of the database now (none in memory). Call
    /// it on a schedule: a snapshot is cheap next to losing a catalog.
    pub fn snapshot(&self, reason: &str) -> Result<Option<PathBuf>, String> {
        self.path
            .as_deref()
            .map(|path| snapshot::take(&self.db, path, reason, &self.options))
            .transpose()
    }

    pub fn pragmas(&self) -> Result<Pragmas, String> {
        let get = |name: &str| -> Result<rusqlite::types::Value, String> {
            self.db
                .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
                .map_err(err)
        };
        let int = |name: &str| match get(name)? {
            rusqlite::types::Value::Integer(i) => Ok(i),
            other => Err(format!("PRAGMA {name}: {other:?}")),
        };
        let journal_mode = match get("journal_mode")? {
            rusqlite::types::Value::Text(t) => t,
            other => return Err(format!("PRAGMA journal_mode: {other:?}")),
        };
        Ok(Pragmas {
            journal_mode,
            synchronous: int("synchronous")?,
            busy_timeout_ms: int("busy_timeout")?,
            foreign_keys: int("foreign_keys")? == 1,
        })
    }

    /// Runs `f` in one write transaction: the caller's when one is open,
    /// otherwise its own, taken at once (`BEGIN IMMEDIATE`) so that what
    /// it reads cannot change before it writes.
    fn write<T>(&mut self, f: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
        self.writable()?;
        if !self.db.is_autocommit() {
            return f(&self.db);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        // Dropping `tx` on an error rolls it back.
        let value = f(&tx)?;
        tx.commit().map_err(err)?;
        Ok(value)
    }

    fn row_image(id: String, size: i64, partial: String) -> Result<Image, String> {
        Ok(Image {
            id: ImageId::from_hex(&id)?,
            size: size as u64,
            partial: parse_hash(&partial)?,
        })
    }
}

/// The columns a [`Sighting`] is read from, in [`sighting`]'s order.
const SIGHTING_COLUMNS: &str = "volume, rel_path, path, size, modified, partial, image, taken, \
     camera, lens, first_seen, last_seen, last_verified, presence, seen_by";

type SightingRow = (
    (Option<String>, Option<String>, String, i64, i64),
    (Option<String>, Option<String>, Option<i64>, Option<String>),
    (Option<String>, i64, i64, Option<i64>, String, String),
);

fn sighting(r: &rusqlite::Row) -> rusqlite::Result<SightingRow> {
    Ok((
        (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?),
        (r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?),
        (
            r.get(9)?,
            r.get(10)?,
            r.get(11)?,
            r.get(12)?,
            r.get(13)?,
            r.get(14)?,
        ),
    ))
}

fn to_sighting(
    (
        (volume, rel_path, path, size, modified),
        (partial, image, taken, camera),
        (lens, first_seen, last_seen, last_verified, presence, seen_by),
    ): SightingRow,
) -> Result<Sighting, String> {
    Ok(Sighting {
        volume: volume.map(VolumeId),
        rel_path: rel_path.map(PathBuf::from),
        path: PathBuf::from(path),
        size: size as u64,
        modified: modified as u64,
        partial: partial.map(|p| parse_hash(&p)).transpose()?,
        image: image.map(|i| ImageId::from_hex(&i)).transpose()?,
        taken,
        camera,
        lens,
        first_seen: first_seen as u64,
        last_seen: last_seen as u64,
        last_verified: last_verified.map(|v| v as u64),
        presence: Presence::parse(&presence),
        seen_by,
    })
}

impl SqliteStore {
    /// The sightings a condition selects (its arguments in `args`), by
    /// path.
    fn sightings_where(
        &self,
        condition: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<Sighting>, String> {
        let sql = format!(
            "SELECT {SIGHTING_COLUMNS} FROM sightings s WHERE {condition} ORDER BY s.path, s.id"
        );
        let mut statement = self.db.prepare(&sql).map_err(err)?;
        let rows = statement
            .query_map(args, sighting)
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        rows.into_iter().map(to_sighting).collect()
    }

    fn locations_where(
        &self,
        condition: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<(ImageId, Location)>, String> {
        Ok(self
            .sightings_where(&format!("s.image IS NOT NULL AND ({condition})"), args)?
            .iter()
            .filter_map(Sighting::location)
            .collect())
    }
}

/// A sighting not in the Library: no image yet, or an image out of it.
const OUTSIDE: &str =
    "(image IS NULL OR image NOT IN (SELECT id FROM images WHERE in_library = 1))";

/// The condition (on `s`, with `?1` the volume and `?2` the folder) for
/// sightings in a folder and below: on a volume its path there, else the
/// absolute path. The empty folder is the whole volume.
fn under(volume: Option<&VolumeId>, rel_dir: &Path) -> (String, String) {
    let sep = std::path::MAIN_SEPARATOR;
    let dir = rel_dir.to_string_lossy();
    let dir = dir.trim_end_matches(sep);
    let (place, column) = if volume.is_some() {
        ("s.volume = ?1", "s.rel_path")
    } else {
        ("?1 IS NULL AND s.volume IS NULL", "s.path")
    };
    let condition = format!(
        "{place} AND (?2 = '' OR {column} = ?2 OR substr({column}, 1, length(?2) + 1) = ?2 || '{sep}')"
    );
    (condition, dir.to_string())
}

/// Writes a sighting as a new row.
fn insert_sighting(db: &Connection, s: &Sighting) -> Result<(), String> {
    db.execute(
        &format!(
            "INSERT INTO sightings ({SIGHTING_COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
        ),
        params![
            s.volume.as_ref().map(VolumeId::as_str),
            s.rel_path.as_ref().map(|p| p.to_string_lossy()),
            s.path.to_string_lossy(),
            s.size as i64,
            s.modified as i64,
            s.partial.as_ref().map(ToString::to_string),
            s.image.as_ref().map(ImageId::hex),
            s.taken,
            s.camera,
            s.lens,
            s.first_seen as i64,
            s.last_seen as i64,
            s.last_verified.map(|v| v as i64),
            s.presence.as_str(),
            s.seen_by
        ],
    )
    .map(|_| ())
    .map_err(err)
}

fn image_known(db: &Connection, id: &ImageId) -> Result<Option<String>, String> {
    db.query_row(
        "SELECT partial FROM images WHERE id = ?1",
        params![id.hex()],
        |r| r.get(0),
    )
    .optional()
    .map_err(err)
}

impl Store for SqliteStore {
    fn add_image(&mut self, image: &Image) -> Result<bool, String> {
        self.writable()?;
        let added = self
            .db
            .execute(
                "INSERT OR IGNORE INTO images (id, size, partial, in_library) VALUES (?1, ?2, ?3, 1)",
                params![image.id.hex(), image.size as i64, image.partial.to_string()],
            )
            .map_err(err)?;
        Ok(added == 1)
    }

    fn image(&self, id: &ImageId) -> Result<Option<Image>, String> {
        let row = self
            .db
            .query_row(
                "SELECT id, size, partial FROM images WHERE id = ?1",
                params![id.hex()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(err)?;
        row.map(|(i, s, p)| Self::row_image(i, s, p)).transpose()
    }

    fn images(&self) -> Result<Vec<Image>, String> {
        let mut statement = self
            .db
            .prepare("SELECT id, size, partial FROM images WHERE in_library = 1 ORDER BY id")
            .map_err(err)?;
        let rows = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(err)?
            .collect::<Result<Vec<(String, i64, String)>, _>>()
            .map_err(err)?;
        rows.into_iter()
            .map(|(i, s, p)| Self::row_image(i, s, p))
            .collect()
    }

    fn library_flag(&self, id: &ImageId) -> Result<Option<LibraryFlag>, String> {
        self.db
            .query_row(
                "SELECT in_library, library_at, library_by FROM images WHERE id = ?1",
                params![id.hex()],
                |r| {
                    Ok(LibraryFlag {
                        in_library: r.get::<_, i64>(0)? != 0,
                        at: r.get::<_, Option<i64>>(1)?.map(|a| a as u64),
                        by: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(err)
    }

    fn set_library_flag(&mut self, id: &ImageId, flag: &LibraryFlag) -> Result<(), String> {
        self.writable()?;
        let set = self
            .db
            .execute(
                "UPDATE images SET in_library = ?2, library_at = ?3, library_by = ?4 WHERE id = ?1",
                params![
                    id.hex(),
                    i64::from(flag.in_library),
                    flag.at.map(|a| a as i64),
                    flag.by
                ],
            )
            .map_err(err)?;
        if set == 0 {
            return Err(format!("no image {}", id.hex()));
        }
        Ok(())
    }

    fn put_location(&mut self, id: &ImageId, location: &Location) -> Result<(), String> {
        self.write(|db| {
            let partial = image_known(db, id)?.ok_or_else(|| format!("no image {}", id.hex()))?;
            let partial = parse_hash(&partial)?;
            let path = location.path.to_string_lossy();
            let volume = location.volume.as_ref().map(VolumeId::as_str);
            let rel = location.rel_path.as_ref().map(|p| p.to_string_lossy());
            // The row at this path, else the one at its place on its
            // volume; both go, the new row replaces them.
            let place = "path = ?1 OR (?2 IS NOT NULL AND ?3 IS NOT NULL AND volume = ?2 AND rel_path = ?3)";
            let old = db
                .query_row(
                    &format!(
                        "SELECT {SIGHTING_COLUMNS} FROM sightings WHERE {place}
                         ORDER BY path = ?1 DESC, id LIMIT 1"
                    ),
                    params![path, volume, rel],
                    sighting,
                )
                .optional()
                .map_err(err)?
                .map(to_sighting)
                .transpose()?;
            db.execute(
                &format!("DELETE FROM sightings WHERE {place}"),
                params![path, volume, rel],
            )
            .map_err(err)?;
            insert_sighting(
                db,
                &Sighting::of_location(id, location, partial, old.as_ref()),
            )
        })
    }

    fn locations(&self, id: &ImageId) -> Result<Vec<Location>, String> {
        Ok(self
            .locations_where("s.image = ?1", params![id.hex()])?
            .into_iter()
            .map(|(_, l)| l)
            .collect())
    }

    fn at_path(&self, path: &Path) -> Result<Option<(ImageId, Location)>, String> {
        Ok(self
            .locations_where("s.path = ?1", params![path.to_string_lossy()])?
            .into_iter()
            .next())
    }

    fn all_locations(&self) -> Result<Vec<(ImageId, Location)>, String> {
        self.locations_where(
            "s.image IN (SELECT id FROM images WHERE in_library = 1)",
            [],
        )
    }

    fn remove_location(&mut self, path: &Path) -> Result<bool, String> {
        self.writable()?;
        let removed = self
            .db
            .execute(
                "DELETE FROM sightings WHERE path = ?1 AND image IS NOT NULL",
                params![path.to_string_lossy()],
            )
            .map_err(err)?;
        Ok(removed > 0)
    }

    fn put_sighting(&mut self, s: &Sighting) -> Result<(), String> {
        s.check()?;
        self.write(|db| {
            if let Some(id) = &s.image
                && image_known(db, id)?.is_none()
            {
                return Err(format!("no image {}", id.hex()));
            }
            let path = s.path.to_string_lossy();
            let volume = s.volume.as_ref().map(VolumeId::as_str);
            let rel = s.rel_path.as_ref().map(|p| p.to_string_lossy());
            let place = "(?1 IS NULL AND volume IS NULL AND path = ?3)
                 OR (?1 IS NOT NULL AND volume = ?1 AND rel_path = ?2)";
            let first: Option<i64> = db
                .query_row(
                    &format!("SELECT MIN(first_seen) FROM sightings WHERE {place}"),
                    params![volume, rel, path],
                    |r| r.get(0),
                )
                .map_err(err)?;
            db.execute(
                &format!("DELETE FROM sightings WHERE {place}"),
                params![volume, rel, path],
            )
            .map_err(err)?;
            let mut new = s.clone();
            if let Some(first) = first {
                new.first_seen = new.first_seen.min(first as u64);
            }
            insert_sighting(db, &new)
        })
    }

    fn sightings_of(&self, id: &ImageId) -> Result<Vec<Sighting>, String> {
        self.sightings_where("s.image = ?1", params![id.hex()])
    }

    fn sightings_like(&self, size: u64, partial: &ContentHash) -> Result<Vec<Sighting>, String> {
        self.sightings_where(
            "s.size = ?1 AND s.partial = ?2",
            params![size as i64, partial.to_string()],
        )
    }

    fn sighting_at(
        &self,
        volume: Option<&VolumeId>,
        rel_path: Option<&Path>,
        path: &Path,
    ) -> Result<Option<Sighting>, String> {
        let volume = volume.map(VolumeId::as_str);
        let rel = rel_path.map(|p| p.to_string_lossy());
        Ok(self
            .sightings_where(
                "(?1 IS NULL AND s.volume IS NULL AND s.path = ?3)
                 OR (?1 IS NOT NULL AND s.volume = ?1 AND s.rel_path = ?2)",
                params![volume, rel, path.to_string_lossy()],
            )?
            .into_iter()
            .next())
    }

    fn sightings_under(
        &self,
        volume: Option<&VolumeId>,
        rel_dir: &Path,
    ) -> Result<Vec<Sighting>, String> {
        let (condition, dir) = under(volume, rel_dir);
        self.sightings_where(&condition, params![volume.map(VolumeId::as_str), dir])
    }

    fn sightings_to_read(
        &self,
        read: ToRead,
        volume: Option<&VolumeId>,
        limit: usize,
    ) -> Result<Vec<Sighting>, String> {
        let (what, order) = match read {
            ToRead::Partial => ("s.partial IS NULL", "s.last_seen DESC, s.path"),
            ToRead::Library => (
                "s.image IN (SELECT id FROM images WHERE in_library = 1)",
                "COALESCE(s.last_verified, 0), s.path",
            ),
            ToRead::Confirm => (
                "s.image IS NULL AND s.partial IS NOT NULL AND EXISTS
                 (SELECT 1 FROM images i WHERE i.size = s.size AND i.partial = s.partial)",
                "s.last_seen DESC, s.path",
            ),
        };
        let sql = format!(
            "SELECT {SIGHTING_COLUMNS} FROM sightings s
             WHERE s.presence = 'present' AND (?1 IS NULL OR s.volume = ?1) AND {what}
             ORDER BY {order} LIMIT ?2"
        );
        let mut statement = self.db.prepare(&sql).map_err(err)?;
        let rows = statement
            .query_map(
                params![volume.map(VolumeId::as_str), limit as i64],
                sighting,
            )
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        rows.into_iter().map(to_sighting).collect()
    }

    fn forget_sightings(
        &mut self,
        volume: Option<&VolumeId>,
        rel_dir: Option<&Path>,
    ) -> Result<usize, String> {
        let (condition, dir) = under(volume, rel_dir.unwrap_or(Path::new("")));
        let condition = condition.replace("s.", "");
        self.write(|db| {
            db.execute(
                &format!("DELETE FROM sightings WHERE ({condition}) AND {OUTSIDE}"),
                params![volume.map(VolumeId::as_str), dir],
            )
            .map_err(err)
        })
    }

    fn prune_sightings(&mut self, limit: usize, missing_before: u64) -> Result<usize, String> {
        self.write(|db| {
            let missing = db
                .execute(
                    &format!(
                        "DELETE FROM sightings WHERE presence = 'missing' AND last_seen < ?1
                         AND {OUTSIDE}"
                    ),
                    params![missing_before as i64],
                )
                .map_err(err)?;
            let count: i64 = db
                .query_row(
                    &format!("SELECT COUNT(*) FROM sightings WHERE {OUTSIDE}"),
                    [],
                    |r| r.get(0),
                )
                .map_err(err)?;
            let over = (count - limit as i64).max(0);
            let capped = db
                .execute(
                    &format!(
                        "DELETE FROM sightings WHERE id IN (SELECT id FROM sightings
                         WHERE {OUTSIDE} ORDER BY last_seen, path LIMIT ?1)"
                    ),
                    params![over],
                )
                .map_err(err)?;
            Ok(missing + capped)
        })
    }

    fn put_folder(&mut self, f: &Folder) -> Result<(), String> {
        self.write(|db| {
            let volume = f.volume.as_ref().map(VolumeId::as_str);
            let dir = f.rel_dir.to_string_lossy();
            db.execute(
                "DELETE FROM folders WHERE volume IS ?1 AND rel_dir = ?2",
                params![volume, dir],
            )
            .map_err(err)?;
            db.execute(
                "INSERT INTO folders (volume, rel_dir, last_listed, files, state)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    volume,
                    dir,
                    f.last_listed.map(|v| v as i64),
                    f.files.map(|v| v as i64),
                    f.state.as_str()
                ],
            )
            .map(|_| ())
            .map_err(err)
        })
    }

    fn folder(&self, volume: Option<&VolumeId>, rel_dir: &Path) -> Result<Option<Folder>, String> {
        self.db
            .query_row(
                "SELECT last_listed, files, state FROM folders WHERE volume IS ?1 AND rel_dir = ?2",
                params![volume.map(VolumeId::as_str), rel_dir.to_string_lossy()],
                |r| {
                    Ok((
                        r.get::<_, Option<i64>>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(err)
            .map(|row| {
                row.map(|(last_listed, files, state)| Folder {
                    volume: volume.cloned(),
                    rel_dir: rel_dir.to_path_buf(),
                    last_listed: last_listed.map(|v| v as u64),
                    files: files.map(|v| v as u64),
                    state: FolderState::parse(&state),
                })
            })
    }

    fn remove_image(&mut self, id: &ImageId) -> Result<bool, String> {
        self.write(|db| {
            let named = |table: &str| {
                db.query_row(
                    &format!("SELECT 1 FROM {table} WHERE image = ?1 LIMIT 1"),
                    params![id.hex()],
                    |_| Ok(()),
                )
                .optional()
                .map_err(err)
            };
            if named("sightings")?.is_some() || named("versions")?.is_some() {
                return Err(format!(
                    "image {} still has locations or versions",
                    id.hex()
                ));
            }
            db.execute("DELETE FROM fields WHERE image = ?1", params![id.hex()])
                .map_err(err)?;
            let removed = db
                .execute("DELETE FROM images WHERE id = ?1", params![id.hex()])
                .map_err(err)?;
            Ok(removed == 1)
        })
    }

    fn field(&self, id: &ImageId, name: &str) -> Result<Option<Value>, String> {
        let text: Option<String> = self
            .db
            .query_row(
                "SELECT value FROM fields WHERE image = ?1 AND name = ?2",
                params![id.hex(), name],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        text.map(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
            .transpose()
    }

    fn set_field(&mut self, id: &ImageId, name: &str, value: Option<&Value>) -> Result<(), String> {
        self.write(|db| {
            let known = db
                .query_row(
                    "SELECT 1 FROM images WHERE id = ?1",
                    params![id.hex()],
                    |_| Ok(()),
                )
                .optional()
                .map_err(err)?;
            if known.is_none() {
                return Err(format!("no image {}", id.hex()));
            }
            match value {
                Some(v) => db.execute(
                    "INSERT INTO fields (image, name, value, number) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (image, name) DO UPDATE SET value = ?3, number = ?4",
                    params![id.hex(), name, v.to_string(), v.as_f64()],
                ),
                None => db.execute(
                    "DELETE FROM fields WHERE image = ?1 AND name = ?2",
                    params![id.hex(), name],
                ),
            }
            .map_err(err)?;
            Ok(())
        })
    }

    fn fields_of(
        &self,
        ids: &[ImageId],
        names: &[&str],
    ) -> Result<Vec<BTreeMap<String, Value>>, String> {
        if ids.is_empty() || names.is_empty() {
            return Ok(vec![BTreeMap::new(); ids.len()]);
        }
        let image_marks = vec!["?"; ids.len()].join(", ");
        let name_marks = vec!["?"; names.len()].join(", ");
        let sql = format!(
            "SELECT image, name, value FROM fields WHERE image IN ({image_marks}) AND name IN ({name_marks})"
        );
        let mut statement = self.db.prepare(&sql).map_err(err)?;
        let args: Vec<String> = ids
            .iter()
            .map(ImageId::hex)
            .chain(names.iter().map(std::string::ToString::to_string))
            .collect();
        let rows = statement
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        let mut by_image: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
        for (image, name, value) in rows {
            let value = serde_json::from_str(&value).map_err(|e| e.to_string())?;
            by_image.entry(image).or_default().insert(name, value);
        }
        Ok(ids
            .iter()
            .map(|id| by_image.remove(&id.hex()).unwrap_or_default())
            .collect())
    }

    fn select(&self, query: &Query) -> Result<Vec<ImageId>, String> {
        use rusqlite::types::Value as Sql;
        query.check()?;
        let order = match query.order {
            Order::Id => "i.id",
            Order::RatingDesc => "COALESCE(r.number, 0) DESC, i.id",
        };
        let mut args: Vec<Sql> = vec![
            query.min_rating.map_or(Sql::Null, Sql::Integer),
            query.label.clone().map_or(Sql::Null, Sql::Text),
        ];
        let mut sql = format!(
            "SELECT i.id FROM images i
             LEFT JOIN fields r ON r.image = i.id AND r.name = '{RATING}'
             LEFT JOIN fields l ON l.image = i.id AND l.name = '{LABELS}'
             LEFT JOIN fields p ON p.image = i.id AND p.name = '{PICKED}'
             LEFT JOIN fields x ON x.image = i.id AND x.name = '{REJECTED}'
             LEFT JOIN fields t ON t.image = i.id AND t.name = '{TAGS}'
             WHERE i.in_library = 1
               AND (?1 IS NULL OR COALESCE(r.number, 0) >= ?1)
               AND (?2 IS NULL OR EXISTS (SELECT 1 FROM json_each(l.value) WHERE json_each.value = ?2))"
        );
        if !query.labels.is_empty() {
            let marks = vec!["?"; query.labels.len()].join(", ");
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM json_each(l.value) WHERE json_each.value IN (",
            );
            sql.push_str(&marks);
            sql.push_str("))");
            args.extend(query.labels.iter().cloned().map(Sql::Text));
        }
        for tag in &query.tags {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM json_each(t.value) WHERE json_each.value = ?)",
            );
            args.push(Sql::Text(tag.clone()));
        }
        if !query.flags.is_empty() {
            let picked = "COALESCE(p.value, '') = 'true'";
            let rejected = "COALESCE(x.value, '') = 'true'";
            let ways: Vec<String> = query
                .flags
                .iter()
                .map(|f| match f.as_str() {
                    "pick" => format!("({picked} AND NOT {rejected})"),
                    "reject" => rejected.to_string(),
                    _ => format!("(NOT {picked} AND NOT {rejected})"),
                })
                .collect();
            sql.push_str(" AND (");
            sql.push_str(&ways.join(" OR "));
            sql.push(')');
        }
        sql.push_str(" ORDER BY ");
        sql.push_str(order);
        let mut statement = self.db.prepare(&sql).map_err(err)?;
        let ids = statement
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                r.get::<_, String>(0)
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        ids.iter().map(|id| ImageId::from_hex(id)).collect()
    }

    fn add_audit(&mut self, entry: &AuditEntry) -> Result<u64, String> {
        self.write(|db| {
            db.execute(
                "INSERT INTO audit (at, who, job, image, field, old, new) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    entry.at as i64,
                    entry.who,
                    entry.job.map(|j| j as i64),
                    entry.image.hex(),
                    entry.field,
                    entry.old.as_ref().map(Value::to_string),
                    entry.new.as_ref().map(Value::to_string)
                ],
            )
            .map_err(err)?;
            Ok(db.last_insert_rowid() as u64)
        })
    }

    fn audit(&self, id: &ImageId) -> Result<Vec<AuditEntry>, String> {
        type Raw = (
            i64,
            i64,
            String,
            Option<i64>,
            String,
            Option<String>,
            Option<String>,
        );
        let mut statement = self
            .db
            .prepare("SELECT seq, at, who, job, field, old, new FROM audit WHERE image = ?1 ORDER BY seq")
            .map_err(err)?;
        let rows = statement
            .query_map(params![id.hex()], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .map_err(err)?
            .collect::<Result<Vec<Raw>, _>>()
            .map_err(err)?;
        let json = |t: Option<String>| {
            t.map(|t| serde_json::from_str(&t).map_err(|e: serde_json::Error| e.to_string()))
                .transpose()
        };
        rows.into_iter()
            .map(|(seq, at, who, job, field, old, new)| {
                Ok(AuditEntry {
                    seq: seq as u64,
                    at: at as u64,
                    who,
                    job: job.map(|j| j as u64),
                    image: *id,
                    field,
                    old: json(old)?,
                    new: json(new)?,
                })
            })
            .collect()
    }

    fn put_version(&mut self, v: &Version) -> Result<bool, String> {
        self.writable()?;
        let parents: Vec<String> = v.parents.iter().map(VersionId::hex).collect();
        let added = self
            .db
            .execute(
                "INSERT OR IGNORE INTO versions (id, image, parents, edit, origin, at, label) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    v.id.hex(),
                    v.image.hex(),
                    serde_json::to_string(&parents).expect("ids serialize"),
                    v.edit.to_string(),
                    v.origin,
                    v.at as i64,
                    v.label
                ],
            )
            .map_err(err)?;
        Ok(added == 1)
    }

    fn version(&self, id: &VersionId) -> Result<Option<Version>, String> {
        self.versions_where("id = ?1", &id.hex())
            .map(|mut v| v.pop())
    }

    fn versions(&self, image: &ImageId) -> Result<Vec<Version>, String> {
        self.versions_where("image = ?1", &image.hex())
    }

    fn remove_version(&mut self, id: &VersionId) -> Result<bool, String> {
        self.write(|db| {
            // Parents are a JSON array of ids: a child names this one.
            let child = db
                .query_row(
                    "SELECT 1 FROM versions WHERE parents LIKE ?1 LIMIT 1",
                    params![format!("%\"{}\"%", id.hex())],
                    |_| Ok(()),
                )
                .optional()
                .map_err(err)?;
            if child.is_some() {
                return Err(format!("version {} is a parent", id.hex()));
            }
            let removed = db
                .execute("DELETE FROM versions WHERE id = ?1", params![id.hex()])
                .map_err(err)?;
            Ok(removed == 1)
        })
    }

    fn document(&self, ns: &str, key: &str) -> Result<Option<Value>, String> {
        let text: Option<String> = self
            .db
            .query_row(
                "SELECT value FROM documents WHERE ns = ?1 AND key = ?2",
                params![ns, key],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        text.map(|t| serde_json::from_str(&t).map_err(|e| format!("document {ns}/{key}: {e}")))
            .transpose()
    }

    fn set_document(&mut self, ns: &str, key: &str, value: Option<&Value>) -> Result<(), String> {
        self.writable()?;
        match value {
            Some(v) => self.db.execute(
                "INSERT INTO documents (ns, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT (ns, key) DO UPDATE SET value = excluded.value",
                params![ns, key, v.to_string()],
            ),
            None => self.db.execute(
                "DELETE FROM documents WHERE ns = ?1 AND key = ?2",
                params![ns, key],
            ),
        }
        .map(|_| ())
        .map_err(err)
    }

    fn documents(&self, ns: &str) -> Result<Vec<(String, Value)>, String> {
        let mut statement = self
            .db
            .prepare("SELECT key, value FROM documents WHERE ns = ?1 ORDER BY key")
            .map_err(err)?;
        let rows: Vec<(String, String)> = statement
            .query_map(params![ns], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)?;
        rows.into_iter()
            .map(|(k, t)| {
                serde_json::from_str(&t)
                    .map(|v| (k.clone(), v))
                    .map_err(|e| format!("document {ns}/{k}: {e}"))
            })
            .collect()
    }

    fn begin(&mut self) -> Result<(), String> {
        self.writable()?;
        if !self.db.is_autocommit() {
            return Err("a transaction is already open".into());
        }
        self.db.execute_batch("BEGIN IMMEDIATE").map_err(err)
    }

    fn commit(&mut self) -> Result<(), String> {
        if self.db.is_autocommit() {
            return Err("no transaction to commit".into());
        }
        self.db.execute_batch("COMMIT").map_err(err)
    }

    fn rollback(&mut self) -> Result<(), String> {
        if self.db.is_autocommit() {
            return Err("no transaction to roll back".into());
        }
        self.db.execute_batch("ROLLBACK").map_err(err)
    }
}

impl Drop for SqliteStore {
    fn drop(&mut self) {
        // Nothing to tell anyone at this point: a vault whose marker could
        // not be written is checked at its next opening.
        let _ = self.mark_clean();
    }
}
