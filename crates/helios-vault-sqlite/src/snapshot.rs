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
//!
//! The catalogue's own schedule ([`take_file`], from a connection of its
//! own, so any thread can take one while the library is in use), the
//! copies on backup disks ([`copy_verified`]) and restoring one
//! ([`request_restore`], applied by [`apply_pending`] before the vault is
//! opened) live here too: a snapshot that exists anywhere was read back
//! and found whole.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};

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
    list_in(&dir(path))
}

/// The snapshots in the folder `dir` (next to a database, or a copy on a
/// backup disk), oldest first.
pub fn list_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
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
    rename(&partial, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    // The rename itself must reach the disk. A directory cannot be opened
    // everywhere (Windows): there, the rename is as durable as it gets.
    if let Ok(d) = std::fs::File::open(&dir) {
        let _ = d.sync_all();
    }
    // Rotation, once the new one is safe: the oldest go first.
    prune(&dir, options.keep_snapshots)?;
    Ok(target)
}

/// Removes the oldest snapshots in `dir` beyond `keep` (at least one is
/// always kept, and every one there was verified before it got its name).
pub fn prune(dir: &Path, keep: usize) -> Result<Vec<PathBuf>, String> {
    let all = list_in(dir);
    let excess = all.len().saturating_sub(keep.max(1));
    for old in &all[..excess] {
        std::fs::remove_file(old).map_err(|e| format!("{}: {e}", old.display()))?;
    }
    Ok(all[..excess].to_vec())
}

/// Takes a verified snapshot of the database file at `path` through a
/// connection of its own: the copy is one read transaction, consistent
/// while other connections write.
pub fn take_file(path: &Path, reason: &str, options: &Options) -> Result<PathBuf, String> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    db.busy_timeout(options.busy_timeout).map_err(err)?;
    take(&db, path, reason, options)
}

/// Reads the snapshot (or any vault file) at `file` back: it opens
/// read-only, passes `PRAGMA quick_check` and has a schema version.
pub fn verify(file: &Path) -> Result<(), String> {
    let at = |e: rusqlite::Error| format!("{}: {e}", file.display());
    if !file.is_file() {
        return Err(format!("{}: no such file", file.display()));
    }
    let db = Connection::open_with_flags(
        file,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(at)?;
    let version: u32 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(at)?;
    if version == 0 {
        return Err(format!("{}: not a Helios catalogue", file.display()));
    }
    let problems = quick_check(&db).map_err(|e| format!("{}: {e}", file.display()))?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{}: fails its check: {}",
            file.display(),
            problems.join("; ")
        ))
    }
}

/// What a snapshot's file says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub file: PathBuf,
    pub sequence: u64,
    /// The schema version it was taken at.
    pub version: u32,
    /// Why it was taken: `daily`, `by-hand`, `before-v9`...
    pub reason: String,
    /// When it was taken (its file's time), in seconds since the epoch.
    pub taken: u64,
    pub bytes: u64,
}

/// [`Info`] of the snapshot at `file`; `None` for a file not named as one.
pub fn info(file: &Path) -> Option<Info> {
    let sequence = sequence(file)?;
    let stem = file.file_stem()?.to_str()?;
    let mut parts = stem.splitn(3, '-');
    let _ = parts.next();
    let version = parts.next()?.strip_prefix('v')?.parse().ok()?;
    let reason = parts.next().unwrap_or_default().to_string();
    let meta = std::fs::metadata(file).ok()?;
    let taken = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Some(Info {
        file: file.to_path_buf(),
        sequence,
        version,
        reason,
        taken,
        bytes: meta.len(),
    })
}

/// Whether a scheduled snapshot of the database at `path` is due: none
/// yet, or the newest older than `every` seconds at `now`.
pub fn due(path: &Path, every: u64, now: u64) -> bool {
    list(path)
        .last()
        .and_then(|f| info(f))
        .is_none_or(|i| now.saturating_sub(i.taken) >= every)
}

/// Copies the snapshot `file` into the folder `to` (a backup disk's),
/// verified: written under a part name, synced, read back and checked,
/// then given its name; then the oldest beyond `keep` there are removed.
/// A copy already there and whole is kept as it is.
pub fn copy_verified(file: &Path, to: &Path, keep: usize) -> Result<PathBuf, String> {
    let at = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let name = file
        .file_name()
        .filter(|_| sequence(file).is_some())
        .ok_or_else(|| format!("{}: not a snapshot", file.display()))?;
    let target = to.join(name);
    if target.is_file() && verify(&target).is_ok() {
        prune(to, keep)?;
        return Ok(target);
    }
    std::fs::create_dir_all(to).map_err(|e| at(to, e))?;
    let partial = target.with_extension("db.partial");
    let written = (|| {
        std::fs::copy(file, &partial).map_err(|e| at(&partial, e))?;
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(&partial)
            .map_err(|e| at(&partial, e))?;
        // Its age is the snapshot's, not the copy's.
        if let Ok(t) = std::fs::metadata(file).and_then(|m| m.modified()) {
            let _ = f.set_modified(t);
        }
        f.sync_all().map_err(|e| at(&partial, e))?;
        drop(f);
        verify(&partial)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    rename(&partial, &target).map_err(|e| at(&target, e))?;
    if let Ok(d) = std::fs::File::open(to) {
        let _ = d.sync_all();
    }
    prune(to, keep)?;
    Ok(target)
}

/// The file naming the snapshot to restore at the next opening of the
/// database at `path`.
pub fn pending_file(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".restore");
    path.with_file_name(name)
}

/// Asks for the database at `path` to be replaced by `snapshot` the next
/// time it is opened ([`apply_pending`]): the snapshot is verified first,
/// so a broken one is refused now.
pub fn request_restore(path: &Path, snapshot: &Path) -> Result<(), String> {
    verify(snapshot)?;
    let marker = pending_file(path);
    std::fs::write(&marker, snapshot.to_string_lossy().as_bytes())
        .map_err(|e| format!("{}: {e}", marker.display()))
}

/// The snapshot a restore was asked for, if any.
pub fn pending(path: &Path) -> Option<PathBuf> {
    std::fs::read_to_string(pending_file(path))
        .ok()
        .map(|s| PathBuf::from(s.trim_end()))
        .filter(|p| !p.as_os_str().is_empty())
}

/// A restore done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    /// The snapshot put in place.
    pub from: PathBuf,
    /// Where the catalogue it replaced was kept.
    pub aside: PathBuf,
}

/// Applies the restore asked for at `path` ([`request_restore`]), before
/// the database is opened: see [`restore`]. The request is dropped
/// either way, so a failing one does not come back at every start.
pub fn apply_pending(path: &Path, now: u64) -> Result<Option<Restored>, String> {
    let Some(from) = pending(path) else {
        return Ok(None);
    };
    let _ = std::fs::remove_file(pending_file(path));
    restore(path, &from, now).map(Some)
}

/// Replaces the database at `path` (closed: no connection on it) by the
/// snapshot `from`. The snapshot is verified, copied next to the database
/// and verified again; the current database (with its write-ahead log)
/// is kept aside as `<file>.before-restore-<now>`; then the copy takes
/// its name. A step that fails leaves the current database in place.
pub fn restore(path: &Path, from: &Path, now: u64) -> Result<Restored, String> {
    let at = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let with = |p: &Path, suffix: &str| {
        let mut name = p.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        p.with_file_name(name)
    };
    verify(from)?;
    let incoming = with(path, ".restoring");
    let _ = std::fs::remove_file(&incoming);
    let copied = (|| {
        std::fs::copy(from, &incoming).map_err(|e| at(&incoming, e))?;
        sync_file(&incoming).map_err(|e| at(&incoming, e))?;
        verify(&incoming)
    })();
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&incoming);
        return Err(e);
    }
    let aside = with(path, &format!(".before-restore-{now}"));
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let (src, dst) = (with(path, suffix), with(&aside, suffix));
        if !src.exists() {
            continue;
        }
        if let Err(e) = std::fs::rename(&src, &dst) {
            for (s, d) in moved.iter().rev() {
                let _ = std::fs::rename(d, s);
            }
            let _ = std::fs::remove_file(&incoming);
            return Err(at(&src, e));
        }
        moved.push((src, dst));
    }
    if let Err(e) = rename(&incoming, path) {
        for (s, d) in moved.iter().rev() {
            let _ = std::fs::rename(d, s);
        }
        return Err(at(path, e));
    }
    if let Some(d) = path.parent().and_then(|d| std::fs::File::open(d).ok()) {
        let _ = d.sync_all();
    }
    Ok(Restored {
        from: from.to_path_buf(),
        aside,
    })
}

fn copy(db: &Connection, to: &Path, options: &Options) -> Result<(), String> {
    let mut dest = Connection::open(to).map_err(err)?;
    // The vault's own snapshots are written with every sync.
    dest.pragma_update(None, "synchronous", "FULL")
        .map_err(err)?;
    {
        let backup = Backup::new(db, &mut dest).map_err(err)?;
        let mut waits: u128 = 0;
        // Without anyone to tell, the copy is one step: one read
        // transaction, never restarted by a write from elsewhere.
        let pages = if options.on_progress.is_some() {
            1024
        } else {
            -1
        };
        loop {
            let step = backup.step(pages).map_err(err)?;
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
    sync_file(to).map_err(|e| format!("{}: {e}", to.display()))
}

/// Renames `from` to `to`. On Windows a file just closed can stay held
/// for a moment (an indexer or a virus scanner opens new files), and the
/// rename fails with a sharing violation: it is tried again for up to a
/// second.
fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
    let held = |e: &std::io::Error| cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33));
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if held(&e) && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            done => return done,
        }
    }
}

/// Flushes the file at `path` to the disk. It is opened for writing:
/// Windows refuses to flush a handle opened read-only ("Access is
/// denied").
fn sync_file(path: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .sync_all()
}
