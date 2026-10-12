//! Schema versions.
//!
//! The schema is a list of numbered migrations; `PRAGMA user_version` says
//! which ones a database has. Opening a database brings it to the last
//! one:
//!
//! - a database newer than this binary is refused, never downgraded;
//! - before the first step, a verified snapshot of the database is taken
//!   (not for a new, empty one);
//! - each step runs in its own transaction, which ends by setting
//!   `user_version`: a crash or a failure leaves the database at the last
//!   step that completed;
//! - each step drops every view and trigger first and creates the current
//!   ones last ([`VIEWS`]), so a view left over from an older schema never
//!   blocks a change to a table it reads;
//! - rows whose reference points nowhere (orphans) do not stop a
//!   migration: foreign keys are off while it runs, and what
//!   `PRAGMA foreign_key_check` finds afterwards is reported.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, TransactionBehavior};

use crate::{OpenError, Options, Progress, err, snapshot};

/// One step of the schema.
#[derive(Clone, Copy)]
pub(crate) struct Migration {
    pub(crate) version: u32,
    pub(crate) name: &'static str,
    pub(crate) sql: &'static str,
}

/// The schema as it was before versions: written with `IF NOT EXISTS`, so
/// a database made before versions existed (version 0 with these tables)
/// takes it as is.
const V1: &str = "
CREATE TABLE IF NOT EXISTS images (
    id      TEXT PRIMARY KEY,
    size    INTEGER NOT NULL,
    partial TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS images_size ON images (size);
CREATE TABLE IF NOT EXISTS locations (
    path          TEXT PRIMARY KEY,
    image         TEXT NOT NULL REFERENCES images (id),
    size          INTEGER NOT NULL,
    modified      INTEGER NOT NULL,
    last_verified INTEGER NOT NULL,
    present       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS locations_image ON locations (image);
CREATE TABLE IF NOT EXISTS fields (
    image  TEXT NOT NULL REFERENCES images (id),
    name   TEXT NOT NULL,
    value  TEXT NOT NULL,   -- JSON
    number REAL,            -- the value when it is a number, for queries
    PRIMARY KEY (image, name)
);
CREATE INDEX IF NOT EXISTS fields_name_number ON fields (name, number);
CREATE TABLE IF NOT EXISTS audit (
    seq   INTEGER PRIMARY KEY AUTOINCREMENT,
    at    INTEGER NOT NULL,
    who   TEXT NOT NULL,
    job   INTEGER,
    image TEXT NOT NULL,
    field TEXT NOT NULL,
    old   TEXT,
    new   TEXT
);
CREATE INDEX IF NOT EXISTS audit_image ON audit (image);
CREATE TABLE IF NOT EXISTS versions (
    id      TEXT PRIMARY KEY,
    image   TEXT NOT NULL REFERENCES images (id),
    parents TEXT NOT NULL,  -- JSON array of version ids
    edit    TEXT NOT NULL,  -- JSON
    origin  TEXT NOT NULL,
    at      INTEGER NOT NULL,
    label   TEXT
);
CREATE INDEX IF NOT EXISTS versions_image ON versions (image);
CREATE TABLE IF NOT EXISTS documents (
    ns    TEXT NOT NULL,
    key   TEXT NOT NULL,
    value TEXT NOT NULL,  -- JSON
    PRIMARY KEY (ns, key)
);
";

/// Every step, in order. A step is never edited once released: a change
/// is a new step.
pub(crate) const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial schema",
    sql: V1,
}];

/// The views and triggers of the current schema (none yet), created again
/// at the end of every step.
pub(crate) const VIEWS: &str = "";

/// The schema version this binary writes.
pub const SCHEMA_VERSION: u32 = MIGRATIONS[MIGRATIONS.len() - 1].version;

/// What opening a database changed in its schema.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Migrated {
    pub from: u32,
    pub to: u32,
    /// The snapshot taken first (none for a new database).
    pub snapshot: Option<PathBuf>,
    /// Views and triggers the file had and the current schema does not
    /// define: dropped (as "view name" or "trigger name").
    pub dropped: Vec<String>,
    /// Rows that point to a row that does not exist, as found by
    /// `PRAGMA foreign_key_check` once migrated: kept, reported.
    pub orphans: Vec<String>,
}

pub(crate) fn user_version(db: &Connection) -> Result<u32, rusqlite::Error> {
    db.query_row("PRAGMA user_version", [], |r| r.get(0))
}

fn has_tables(db: &Connection) -> Result<bool, rusqlite::Error> {
    db.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%')",
        [],
        |r| r.get(0),
    )
}

/// The rows `PRAGMA foreign_key_check` reports, described.
pub(crate) fn orphans(db: &Connection) -> Result<Vec<String>, String> {
    let mut statement = db.prepare("PRAGMA foreign_key_check").map_err(err)?;
    let mut rows = statement
        .query_map([], |r| {
            Ok(format!(
                "{} row {} points to a missing {} row",
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?
                    .map_or_else(|| "?".to_string(), |i| i.to_string()),
                r.get::<_, String>(2)?
            ))
        })
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    rows.sort();
    Ok(rows)
}

fn views_and_triggers(db: &Connection) -> Result<Vec<(String, String)>, rusqlite::Error> {
    let mut statement = db.prepare(
        "SELECT type, name FROM sqlite_schema WHERE type IN ('view', 'trigger') ORDER BY type, name",
    )?;
    statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect()
}

/// Brings the database to the last of `migrations`.
pub(crate) fn migrate(
    db: &mut Connection,
    path: Option<&Path>,
    options: &Options,
    migrations: &[Migration],
    views: &str,
) -> Result<Option<Migrated>, OpenError> {
    let latest = migrations.last().map_or(0, |m| m.version);
    let found = user_version(db).map_err(|e| OpenError::Sqlite(err(e)))?;
    if found > latest {
        return Err(OpenError::TooNew {
            found,
            supported: latest,
        });
    }
    if found == latest {
        return Ok(None);
    }
    let fresh = found == 0 && !has_tables(db).map_err(|e| OpenError::Sqlite(err(e)))?;
    let snapshot = match path {
        Some(path) if !fresh => Some(
            snapshot::take(db, path, &format!("before-v{latest}"), options)
                .map_err(OpenError::Snapshot)?,
        ),
        _ => None,
    };
    // Foreign keys can only be switched outside a transaction; orphans must
    // not stop a step that rebuilds a table.
    db.pragma_update(None, "foreign_keys", "OFF")
        .map_err(|e| OpenError::Sqlite(err(e)))?;
    let steps = run(db, found, options, migrations, views);
    db.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| OpenError::Sqlite(err(e)))?;
    let dropped = steps.map_err(|(version, name, message)| OpenError::Migration {
        version,
        name,
        message,
        snapshot: snapshot.clone(),
    })?;
    Ok(Some(Migrated {
        from: found,
        to: latest,
        snapshot,
        dropped,
        orphans: orphans(db).map_err(OpenError::Sqlite)?,
    }))
}

type StepError = (u32, &'static str, String);

fn run(
    db: &mut Connection,
    found: u32,
    options: &Options,
    migrations: &[Migration],
    views: &str,
) -> Result<Vec<String>, StepError> {
    let steps: Vec<&Migration> = migrations.iter().filter(|m| m.version > found).collect();
    let mut dropped = BTreeSet::new();
    for (i, m) in steps.iter().enumerate() {
        if let Some(report) = &options.on_progress {
            report(&Progress {
                stage: "migration".into(),
                done: i as u64,
                total: steps.len() as u64,
                step: Some(format!("{} {}", m.version, m.name)),
            });
        }
        let fail = |e: rusqlite::Error| (m.version, m.name, err(e));
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(fail)?;
        // Another process may have migrated while this one waited.
        if user_version(&tx).map_err(fail)? >= m.version {
            continue;
        }
        for (kind, name) in views_and_triggers(&tx).map_err(fail)? {
            let quoted = name.replace('"', "\"\"");
            tx.execute_batch(&format!("DROP {kind} IF EXISTS \"{quoted}\""))
                .map_err(fail)?;
            dropped.insert((kind, name));
        }
        tx.execute_batch(m.sql).map_err(fail)?;
        tx.execute_batch(views).map_err(fail)?;
        tx.pragma_update(None, "user_version", m.version)
            .map_err(fail)?;
        for kept in views_and_triggers(&tx).map_err(fail)? {
            dropped.remove(&kept);
        }
        tx.commit().map_err(fail)?;
    }
    if let Some(report) = &options.on_progress {
        report(&Progress {
            stage: "migration".into(),
            done: steps.len() as u64,
            total: steps.len() as u64,
            step: None,
        });
    }
    Ok(dropped
        .into_iter()
        .map(|(kind, name)| format!("{kind} {name}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "helios-vault-migrate-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The real steps, then one that drops a column.
    const WITH_DROP: &[Migration] = &[
        MIGRATIONS[0],
        Migration {
            version: 2,
            name: "images lose partial",
            sql: "ALTER TABLE images DROP COLUMN partial;",
        },
    ];

    /// darktable's v53-to-v54 failure: a view left by an older schema reads
    /// a column a later step drops. Dropping every view first lets the
    /// step run, and the stale view is reported with the orphan rows.
    #[test]
    fn a_stale_view_and_orphan_rows_do_not_stop_a_migration() {
        let dir = scratch("stale");
        let path = dir.join("vault.db");
        let mut db = Connection::open(&path).unwrap();
        let options = Options::default();
        migrate(&mut db, Some(&path), &options, MIGRATIONS, VIEWS).unwrap();
        db.execute_batch(
            "CREATE VIEW v_partial AS SELECT id, partial FROM images;
             PRAGMA foreign_keys = OFF;
             INSERT INTO locations VALUES ('/gone.cr3', 'no-such-image', 1, 1, 1, 1);",
        )
        .unwrap();
        // Without dropping the view first, the step fails.
        let direct = db.execute_batch("ALTER TABLE images DROP COLUMN partial");
        assert!(direct.unwrap_err().to_string().contains("v_partial"));
        let migrated = migrate(&mut db, Some(&path), &options, WITH_DROP, VIEWS)
            .unwrap()
            .unwrap();
        assert_eq!((migrated.from, migrated.to), (1, 2));
        assert_eq!(migrated.dropped, ["view v_partial"]);
        assert_eq!(
            migrated.orphans,
            ["locations row 1 points to a missing images row"]
        );
        assert!(migrated.snapshot.unwrap().exists());
        assert_eq!(user_version(&db).unwrap(), 2);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failing_step_leaves_the_last_version_that_completed() {
        let dir = scratch("failing");
        let path = dir.join("vault.db");
        let mut db = Connection::open(&path).unwrap();
        let options = Options::default();
        migrate(&mut db, Some(&path), &options, MIGRATIONS, VIEWS).unwrap();
        let broken = [
            MIGRATIONS[0],
            Migration {
                version: 2,
                name: "adds a table",
                sql: "CREATE TABLE extra (x);",
            },
            Migration {
                version: 3,
                name: "breaks",
                sql: "CREATE TABLE half (x); SELECT * FROM nowhere;",
            },
        ];
        let failed = migrate(&mut db, Some(&path), &options, &broken, VIEWS).unwrap_err();
        let OpenError::Migration {
            version, snapshot, ..
        } = &failed
        else {
            panic!("{failed}");
        };
        assert_eq!(*version, 3);
        assert!(snapshot.as_ref().unwrap().exists());
        assert!(failed.to_string().contains("nowhere"), "{failed}");
        assert_eq!(user_version(&db).unwrap(), 2, "step 2 is kept");
        assert!(
            !has_table(&db, "half"),
            "nothing of the failed step is kept"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn has_table(db: &Connection, name: &str) -> bool {
        db.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name = ?1)",
            [name],
            |r| r.get(0),
        )
        .unwrap()
    }
}
