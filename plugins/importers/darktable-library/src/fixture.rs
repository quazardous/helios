//! Small darktable configuration folders (`library.db`, `data.db`) for
//! tests, built from darktable's own `CREATE TABLE` statements
//! (`fixtures/*.sql`, the tables the importer reads, at the release tags
//! `release-4.6.0`: library schema 47, data 10; and `release-5.2.0`:
//! library 57, data 13). No real user's library is ever used.
//!
//! Every file of the photos folder, sorted, becomes a photo of a first
//! film roll; a second film roll names a folder that does not exist (a
//! drive not plugged in). The marks, by file name:
//!
//! - `a.jpg`: 3 stars, red, tags `places|France|Lyon` and
//!   `darktable|format|jpg`, title "Low tide", creator "Ann"; a duplicate
//!   (version 1) with 5 stars, blue and `people|Ann`;
//! - `b.jpg`: rejected with its 2 stars kept, green, description "Rocks";
//! - `c.jpg`: no star (0), nothing else;
//! - any other file: 1 star;
//! - `gone.cr2`, in the missing folder: 4 stars.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

/// A darktable release's schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Schema {
    /// darktable 4.6: library 47, data 10.
    V4_6,
    /// darktable 5.2: library 57, data 13.
    V5_2,
}

impl Schema {
    fn sql(self) -> (&'static str, i64, &'static str, i64) {
        match self {
            Schema::V4_6 => (
                include_str!("../fixtures/library-4.6.sql"),
                47,
                include_str!("../fixtures/data-4.6.sql"),
                10,
            ),
            Schema::V5_2 => (
                include_str!("../fixtures/library-5.2.sql"),
                57,
                include_str!("../fixtures/data-5.2.sql"),
                13,
            ),
        }
    }
}

/// What to build beyond the schema's defaults.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The library's schema version written, in place of the schema's.
    pub library_version: Option<i64>,
    pub data_version: Option<i64>,
    /// Writes `library.db.lock` holding this pid, as darktable does while
    /// it runs.
    pub lock_pid: Option<u32>,
    /// No `data.db`.
    pub no_data: bool,
}

const RED: i64 = 0;
const GREEN: i64 = 2;
const BLUE: i64 = 3;
const REJECTED: i64 = 8;
const RAW: i64 = 64;
const LDR: i64 = 32;
const CREATOR: i64 = 0;
const TITLE: i64 = 2;
const DESCRIPTION: i64 = 3;

/// The folder a library's second film roll names, which does not exist.
pub fn missing_folder(photos: &Path) -> PathBuf {
    photos
        .parent()
        .unwrap_or(photos)
        .join("drive-not-plugged-in")
}

/// Builds a darktable folder at `out` over the files of `photos`.
pub fn build(out: &Path, schema: Schema, photos: &Path, options: &Options) -> rusqlite::Result<()> {
    let (library_sql, library_v, data_sql, data_v) = schema.sql();
    std::fs::create_dir_all(out).expect("the fixture's folder");
    for name in ["library.db", "data.db", "library.db.lock"] {
        let _ = std::fs::remove_file(out.join(name));
    }
    let con = Connection::open(out.join("library.db"))?;
    // darktable's tables name lookup tables these minimal fixtures leave
    // out (`makers`, `cameras`...); rusqlite's bundled SQLite enforces
    // foreign keys by default, darktable's connection does not.
    con.pragma_update(None, "foreign_keys", false)?;
    con.execute(
        "ATTACH DATABASE ?1 AS data",
        [out.join("data.db").to_string_lossy()],
    )?;
    con.execute_batch(library_sql)?;
    con.execute_batch(data_sql)?;
    con.execute(
        "INSERT INTO main.db_info VALUES ('version', ?1)",
        [options.library_version.unwrap_or(library_v).to_string()],
    )?;
    con.execute(
        "INSERT INTO data.db_info VALUES ('version', ?1)",
        [options.data_version.unwrap_or(data_v).to_string()],
    )?;

    let mut tags: Vec<String> = Vec::new();
    let tag = |imgid: i64, name: &str, tags: &mut Vec<String>| -> rusqlite::Result<()> {
        let id = if let Some(i) = tags.iter().position(|t| t == name) {
            i + 1
        } else {
            tags.push(name.into());
            con.execute(
                "INSERT INTO data.tags (id, name) VALUES (?1, ?2)",
                params![tags.len() as i64, name],
            )?;
            tags.len()
        };
        con.execute(
            "INSERT INTO main.tagged_images VALUES (?1, ?2, ?3)",
            params![imgid, id as i64, tags.len() as i64],
        )?;
        Ok(())
    };
    let image = |film: i64, filename: &str, flags: i64, version: i64| -> rusqlite::Result<i64> {
        con.execute(
            "INSERT INTO main.images (film_id, filename, flags, version, max_version) \
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![film, filename, flags, version],
        )?;
        let id = con.last_insert_rowid();
        con.execute("UPDATE main.images SET group_id = ?1 WHERE id = ?1", [id])?;
        Ok(id)
    };
    let label = |imgid: i64, color: i64| {
        con.execute(
            "INSERT INTO main.color_labels VALUES (?1, ?2)",
            [imgid, color],
        )
    };
    let text = |imgid: i64, key: i64, value: &str| {
        con.execute(
            "INSERT INTO main.meta_data VALUES (?1, ?2, ?3)",
            params![imgid, key, value],
        )
    };

    let photos = std::path::absolute(photos).expect("an absolute photos folder");
    con.execute(
        "INSERT INTO main.film_rolls (id, folder) VALUES (1, ?1)",
        [photos.to_string_lossy()],
    )?;
    con.execute(
        "INSERT INTO main.film_rolls (id, folder) VALUES (2, ?1)",
        [missing_folder(&photos).to_string_lossy()],
    )?;
    let mut names: Vec<String> = std::fs::read_dir(&photos)
        .expect("the photos folder")
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    for name in &names {
        match name.as_str() {
            "a.jpg" => {
                let a = image(1, name, 3 | LDR, 0)?;
                label(a, RED)?;
                tag(a, "places|France|Lyon", &mut tags)?;
                tag(a, "darktable|format|jpg", &mut tags)?;
                text(a, TITLE, "Low tide")?;
                text(a, CREATOR, "Ann")?;
                let dup = image(1, name, 5 | LDR, 1)?;
                label(dup, BLUE)?;
                tag(dup, "people|Ann", &mut tags)?;
            }
            "b.jpg" => {
                let b = image(1, name, 2 | REJECTED | LDR, 0)?;
                label(b, GREEN)?;
                text(b, DESCRIPTION, "Rocks")?;
            }
            "c.jpg" => {
                image(1, name, LDR, 0)?;
            }
            _ => {
                image(1, name, 1 | LDR, 0)?;
            }
        }
    }
    image(2, "gone.cr2", 4 | RAW, 0)?;
    drop(con);
    if options.no_data {
        std::fs::remove_file(out.join("data.db")).expect("remove data.db");
    }
    if let Some(pid) = options.lock_pid {
        std::fs::write(out.join("library.db.lock"), format!("{pid}\0")).expect("the lock");
    }
    Ok(())
}
