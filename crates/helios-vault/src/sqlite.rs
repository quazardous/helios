//! The SQLite store: one database file, readable with standard tools.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

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
}
