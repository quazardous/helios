//! Verified snapshots of a vault database.
//!
//! A snapshot is a copy made through SQLite's backup API, so it is
//! consistent even while other connections write. It is written under a
//! temporary name, checked with `PRAGMA quick_check`, synced, and only
//! then renamed into place: a snapshot that exists is a good one. They
//! live next to the database, in `<file>.snapshots/`, named
//! `<sequence>-v<schema version>-<reason>.db` so that their order does not
//! depend on a clock. Taking one removes the oldest beyond
//! [`Options::keep_snapshots`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;
use rusqlite::backup::{Backup, StepResult};

use crate::{Options, Progress, err};

/// Where the snapshots of the database at `path` are kept.
pub fn dir(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".snapshots");
    path.with_file_name(name)
}

fn sequence(path: &Path) -> Option<u64> {
    if path.extension().and_then(|e| e.to_str()) != Some("db") {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    name.split('-').next()?.parse().ok()
}

/// The snapshots of the database at `path`, oldest first.
pub fn list(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir(path)) else {
        return Vec::new();
    };
    let mut found: Vec<(u64, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| sequence(&p).map(|s| (s, p)))
        .collect();
    found.sort();
    found.into_iter().map(|(_, p)| p).collect()
}

/// The problems `PRAGMA quick_check` finds; empty when the database is
/// sound.
pub(crate) fn quick_check(db: &Connection) -> Result<Vec<String>, String> {
    let mut statement = db.prepare("PRAGMA quick_check").map_err(err)?;
    let lines = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    Ok(if lines == ["ok"] { Vec::new() } else { lines })
}

/// Copies `db` (the database at `path`) into a new verified snapshot and
/// returns its path.
pub(crate) fn take(
    db: &Connection,
    path: &Path,
    reason: &str,
    options: &Options,
) -> Result<PathBuf, String> {
    let dir = dir(path);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let next = list(path)
        .last()
        .and_then(|p| sequence(p))
        .map_or(1, |s| s + 1);
    let version: u32 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(err)?;
    let reason: String = reason
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let target = dir.join(format!("{next:06}-v{version}-{reason}.db"));
    let partial = target.with_extension("db.partial");
    let _ = std::fs::remove_file(&partial);
    let copied = copy(db, &partial, options);
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("snapshot {}: {e}", target.display()));
    }
    std::fs::rename(&partial, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    // The rename itself must reach the disk. A directory cannot be opened
    // everywhere (Windows): there, the rename is as durable as it gets.
    if let Ok(d) = std::fs::File::open(&dir) {
        let _ = d.sync_all();
    }
    // Rotation, once the new one is safe: the oldest go first.
    let all = list(path);
    let excess = all.len().saturating_sub(options.keep_snapshots.max(1));
    for old in &all[..excess] {
        std::fs::remove_file(old).map_err(|e| format!("{}: {e}", old.display()))?;
    }
    Ok(target)
}

fn copy(db: &Connection, to: &Path, options: &Options) -> Result<(), String> {
    let mut dest = Connection::open(to).map_err(err)?;
    // The vault's own snapshots are written with every sync.
    dest.pragma_update(None, "synchronous", "FULL")
        .map_err(err)?;
    {
        let backup = Backup::new(db, &mut dest).map_err(err)?;
        let mut waits: u128 = 0;
        loop {
            let step = backup.step(1024).map_err(err)?;
            if matches!(step, StepResult::Done | StepResult::More)
                && let Some(report) = &options.on_progress
            {
                let p = backup.progress();
                report(&Progress {
                    stage: "snapshot".into(),
                    done: (p.pagecount - p.remaining) as u64,
                    total: p.pagecount as u64,
                    step: None,
                });
            }
            match step {
                StepResult::Done => break,
                StepResult::More => {}
                // Another connection holds a lock the copy needs: wait a
                // little, as long as the busy timeout allows.
                _ => {
                    waits += 1;
                    if waits * 10 > options.busy_timeout.as_millis() {
                        return Err("the database stayed busy".into());
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
    // One self-contained file: no write-ahead log next to it.
    let _mode: String = dest
        .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
        .map_err(err)?;
    let problems = quick_check(&dest)?;
    if !problems.is_empty() {
        return Err(format!("the copy fails its check: {}", problems.join("; ")));
    }
    dest.close().map_err(|(_, e)| err(e))?;
    std::fs::File::open(to)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("{}: {e}", to.display()))
}
