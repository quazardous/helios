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

use helios_vault::fields::{AuditEntry, LABELS, Order, PICKED, Query, RATING, REJECTED, TAGS};
use helios_vault::{Image, ImageId, Location, Store, Version, VersionId, parse_hash};

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

type LocationRow = (String, String, i64, i64, i64, bool);

fn location(row: &rusqlite::Row) -> rusqlite::Result<LocationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn to_location(
    (image, path, size, modified, verified, present): LocationRow,
) -> Result<(ImageId, Location), String> {
    Ok((
        ImageId::from_hex(&image)?,
        Location {
            path: PathBuf::from(path),
            size: size as u64,
            modified: modified as u64,
            last_verified: verified as u64,
            present,
        },
    ))
}

const LOCATION_COLUMNS: &str = "image, path, size, modified, last_verified, present";

impl Store for SqliteStore {
    fn add_image(&mut self, image: &Image) -> Result<bool, String> {
        self.writable()?;
        let added = self
            .db
            .execute(
                "INSERT OR IGNORE INTO images (id, size, partial) VALUES (?1, ?2, ?3)",
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
            .prepare("SELECT id, size, partial FROM images ORDER BY id")
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

    fn put_location(&mut self, id: &ImageId, location: &Location) -> Result<(), String> {
        self.writable()?;
        self.db
            .execute(
                "INSERT INTO locations (path, image, size, modified, last_verified, present)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (path) DO UPDATE SET image = ?2, size = ?3, modified = ?4,
                     last_verified = ?5, present = ?6",
                params![
                    location.path.to_string_lossy(),
                    id.hex(),
                    location.size as i64,
                    location.modified as i64,
                    location.last_verified as i64,
                    location.present
                ],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(_, _) if self.image(id).ok().flatten().is_none() => {
                    format!("no image {}", id.hex())
                }
                other => other.to_string(),
            })?;
        Ok(())
    }

    fn locations(&self, id: &ImageId) -> Result<Vec<Location>, String> {
        let sql =
            format!("SELECT {LOCATION_COLUMNS} FROM locations WHERE image = ?1 ORDER BY path");
        let mut statement = self.db.prepare(&sql).map_err(err)?;
        let rows = statement
            .query_map(params![id.hex()], location)
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        rows.into_iter()
            .map(|r| to_location(r).map(|(_, l)| l))
            .collect()
    }

    fn at_path(&self, path: &Path) -> Result<Option<(ImageId, Location)>, String> {
        let sql = format!("SELECT {LOCATION_COLUMNS} FROM locations WHERE path = ?1");
        let row = self
            .db
            .query_row(&sql, params![path.to_string_lossy()], location)
            .optional()
            .map_err(err)?;
        row.map(to_location).transpose()
    }

    fn all_locations(&self) -> Result<Vec<(ImageId, Location)>, String> {
        let sql = format!("SELECT {LOCATION_COLUMNS} FROM locations ORDER BY path");
        let mut statement = self.db.prepare(&sql).map_err(err)?;
        let rows = statement
            .query_map([], location)
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        rows.into_iter().map(to_location).collect()
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
             WHERE (?1 IS NULL OR COALESCE(r.number, 0) >= ?1)
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
