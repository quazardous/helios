//! The health check's reads of a vault database file: whether it opens,
//! and what SQLite's `PRAGMA quick_check` says of it. Each opens its own
//! read-only connection, so the check's thread never shares the vault's.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::err;

/// A read-only connection to the database at `path`, waiting up to 10 s
/// on a writer.
fn read_only(path: &Path) -> Result<Connection, String> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(err)?;
    db.busy_timeout(Duration::from_secs(10)).map_err(err)?;
    Ok(db)
}

/// Whether the database at `path` opens and reads its schema.
pub fn opens(path: &Path) -> Result<(), String> {
    read_only(path)?
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })
        .map(|_| ())
        .map_err(err)
}

/// The problems `PRAGMA quick_check` finds in the database at `path`;
/// empty when it is sound.
pub fn quick_check(path: &Path) -> Result<Vec<String>, String> {
    crate::snapshot::quick_check(&read_only(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_database_opens_and_passes_and_garbage_does_not() {
        let dir = std::env::temp_dir().join(format!("helios-vault-health-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.sqlite");
        Connection::open(&good)
            .unwrap()
            .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);")
            .unwrap();
        assert_eq!(opens(&good), Ok(()));
        assert_eq!(quick_check(&good), Ok(Vec::new()));

        let bad = dir.join("bad.sqlite");
        std::fs::write(&bad, vec![0x5a; 8192]).unwrap();
        assert!(opens(&bad).is_err());
        assert!(quick_check(&bad).is_err());

        let missing = dir.join("missing.sqlite");
        assert!(opens(&missing).is_err());
        assert!(quick_check(&missing).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
