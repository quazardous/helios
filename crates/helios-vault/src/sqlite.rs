//! The SQLite store: one database file, readable with standard tools.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use std::collections::BTreeMap;

use serde_json::Value;

use crate::fields::{AuditEntry, LABELS, Order, Query, RATING};
use crate::{Image, ImageId, Location, Store, parse_hash};

const SCHEMA: &str = "
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
";

pub struct SqliteStore {
    db: Connection,
}

fn err(e: rusqlite::Error) -> String {
    e.to_string()
}

impl SqliteStore {
    /// Opens (or creates) a vault database file.
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::init(Connection::open(path).map_err(err)?)
    }

    pub fn in_memory() -> Result<Self, String> {
        Self::init(Connection::open_in_memory().map_err(err)?)
    }

    fn init(db: Connection) -> Result<Self, String> {
        db.execute_batch("PRAGMA foreign_keys = ON;").map_err(err)?;
        db.execute_batch(SCHEMA).map_err(err)?;
        Ok(Self { db })
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
        if self.image(id)?.is_none() {
            return Err(format!("no image {}", id.hex()));
        }
        match value {
            Some(v) => self
                .db
                .execute(
                    "INSERT INTO fields (image, name, value, number) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (image, name) DO UPDATE SET value = ?3, number = ?4",
                    params![id.hex(), name, v.to_string(), v.as_f64()],
                )
                .map_err(err)?,
            None => self
                .db
                .execute(
                    "DELETE FROM fields WHERE image = ?1 AND name = ?2",
                    params![id.hex(), name],
                )
                .map_err(err)?,
        };
        Ok(())
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
            .chain(names.iter().map(|n| n.to_string()))
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
        let order = match query.order {
            Order::Id => "i.id",
            Order::RatingDesc => "COALESCE(r.number, 0) DESC, i.id",
        };
        let sql = format!(
            "SELECT i.id FROM images i
             LEFT JOIN fields r ON r.image = i.id AND r.name = '{RATING}'
             LEFT JOIN fields l ON l.image = i.id AND l.name = '{LABELS}'
             WHERE (?1 IS NULL OR COALESCE(r.number, 0) >= ?1)
               AND (?2 IS NULL OR EXISTS (SELECT 1 FROM json_each(l.value) WHERE json_each.value = ?2))
             ORDER BY {order}"
        );
        let mut statement = self.db.prepare(&sql).map_err(err)?;
        let ids = statement
            .query_map(params![query.min_rating, query.label], |r| {
                r.get::<_, String>(0)
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        ids.iter().map(|id| ImageId::from_hex(id)).collect()
    }

    fn add_audit(&mut self, entry: &AuditEntry) -> Result<u64, String> {
        self.db
            .execute(
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
        Ok(self.db.last_insert_rowid() as u64)
    }

    fn audit(&self, id: &ImageId) -> Result<Vec<AuditEntry>, String> {
        let mut statement = self
            .db
            .prepare("SELECT seq, at, who, job, field, old, new FROM audit WHERE image = ?1 ORDER BY seq")
            .map_err(err)?;
        type Raw = (
            i64,
            i64,
            String,
            Option<i64>,
            String,
            Option<String>,
            Option<String>,
        );
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
}
