//! Helios's darktable library importer (`importer` interface version 2;
//! see Helios's `docs/extensions.md`, "Importers", and
//! `docs/darktable-library-import.md`).
//!
//! It reads a darktable configuration folder, `library.db` and `data.db`
//! beside it, and lists each photo file the library holds with what
//! darktable knows of it: stars, reject, colour labels, tags (the whole
//! `a|b|c` path, darktable's own `darktable|…` tags left out), title and
//! description, and each duplicate's edit: rebuilt from the library's
//! `history`, `masks_history` and `module_order`, or the `.xmp` beside
//! the photo when it is the newer one (see [`choose`]). Helios imports
//! the files itself and maps the edits as it maps a sidecar's.
//!
//! The scan answers in pages (`importer/page` notifications, then an
//! answer with their count) when Helios asks for them (`pageSize`), so a
//! library of 100 000 photos never travels as one message; a cancel is
//! seen between pages.
//!
//! The library is never written: both files are opened read-only (a
//! `mode=ro` URI, never `immutable`, which would ignore darktable's
//! locks), and while darktable runs (its pid in `library.db.lock` alive)
//! they are first copied through SQLite's backup API into a private
//! temporary folder, and the copy is read.
//!
//! - [`serve`]: the plugin's JSON-RPC loop, one message per line;
//! - [`scan`]: one `importer.scan`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::io::{BufRead, Write};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};

#[cfg(feature = "fixture")]
#[doc(hidden)]
pub mod fixture;

/// The importer's id in the manifest.
pub const IMPORTER: &str = "darktable-library";

/// The schema versions read (`db_info`, key `version`): darktable 4.0
/// (library 36, data 9) to 5.6 (library 57, data 13).
pub const LIBRARY: RangeInclusive<i64> = 36..=57;
pub const DATA: RangeInclusive<i64> = 9..=13;

pub const NOT_A_SOURCE: i64 = -32001;
pub const INVALID_PARAMS: i64 = -32602;
pub const CANCELLED: i64 = -32800;
pub const FAILED: i64 = -32000;

/// A progress notice, and a look for a cancellation, every so many files.
const BATCH: usize = 200;
/// Names quoted in a warning, at most.
const QUOTED: usize = 3;

/// For Helios's tests alone: set, a scan waits after its first progress
/// notice until it is cancelled (at most a minute).
pub const HOLD: &str = "HELIOS_DARKTABLE_LIBRARY_HOLD";

// `images.flags` (src/common/image.h, src/common/ratings.h).
const STARS: i64 = 0x7;
const REJECTED: i64 = 0x8;
/// A reject before library schema 29: the rating value 6.
const OLD_REJECT: i64 = 6;

/// `color_labels.color` (src/common/colorlabels.h).
const LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

const TITLE: &str = "Xmp.dc.title";
const DESCRIPTION: &str = "Xmp.dc.description";
/// `meta_data.key` up to data schema 12 (`dt_metadata_t`,
/// src/common/metadata.h); from 13 the rows of `data.meta_data`, which
/// keep these numbers for these keys.
const FIXED_KEYS: [&str; 9] = [
    "Xmp.dc.creator",
    "Xmp.dc.publisher",
    TITLE,
    DESCRIPTION,
    "Xmp.dc.rights",
    "Xmp.acdsee.notes",
    "Xmp.darktable.version_name",
    "Xmp.darktable.image_id",
    "Xmp.xmpMM.PreservedFileName",
];
/// darktable's own bookkeeping: neither imported nor counted as lost.
const OWN_KEYS: [&str; 3] = [
    "Xmp.darktable.version_name",
    "Xmp.darktable.image_id",
    "Xmp.xmpMM.PreservedFileName",
];

/// The columns read, checked before anything is read.
const LIBRARY_COLUMNS: &[(&str, &[&str])] = &[
    ("film_rolls", &["id", "folder"]),
    ("images", &["id", "film_id", "filename", "version", "flags"]),
    ("color_labels", &["imgid", "color"]),
    ("tagged_images", &["imgid", "tagid"]),
    ("meta_data", &["id", "key", "value"]),
    (
        "history",
        &[
            "imgid",
            "num",
            "module",
            "operation",
            "op_params",
            "enabled",
            "blendop_params",
            "blendop_version",
            "multi_priority",
            "multi_name",
        ],
    ),
    (
        "masks_history",
        &[
            "imgid",
            "num",
            "formid",
            "form",
            "name",
            "version",
            "points",
            "points_count",
            "source",
        ],
    ),
    ("history_hash", &["imgid", "current_hash"]),
    ("module_order", &["imgid", "version", "iop_list"]),
];
const DATA_COLUMNS: &[(&str, &[&str])] = &[("tags", &["id", "name"])];

/// Why a scan answers an error.
#[derive(Debug, PartialEq, Eq)]
pub enum ScanError {
    /// Not a source this importer reads: shown to the user.
    Refused(String),
    Cancelled,
    Invalid(String),
    Failed(String),
}

impl ScanError {
    pub fn code(&self) -> i64 {
        match self {
            ScanError::Refused(_) => NOT_A_SOURCE,
            ScanError::Cancelled => CANCELLED,
            ScanError::Invalid(_) => INVALID_PARAMS,
            ScanError::Failed(_) => FAILED,
        }
    }

    pub fn message(&self) -> String {
        match self {
            ScanError::Refused(m) => m.clone(),
            ScanError::Cancelled => "cancelled".into(),
            ScanError::Invalid(m) => format!("invalid parameters: {m}"),
            ScanError::Failed(m) => format!("the darktable library could not be read: {m}"),
        }
    }
}

impl From<rusqlite::Error> for ScanError {
    fn from(e: rusqlite::Error) -> Self {
        ScanError::Failed(e.to_string())
    }
}

fn refused(m: impl Into<String>) -> ScanError {
    ScanError::Refused(m.into())
}

/// A URI opening `path` read-only. Never `immutable`: darktable may be
/// writing, and SQLite must keep honouring its locks.
pub fn read_only(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    file_uri(&absolute.to_string_lossy()) + "?mode=ro"
}

/// The `file:` URI of an absolute path, Unix (`/a/b`) or Windows
/// (`D:\a\b`, `\\server\share\b`), percent-encoded: always
/// `file:///…`, since SQLite rejects `file:/D:/…` on Windows.
fn file_uri(path: &str) -> String {
    let mut text = path.replace('\\', "/");
    // a verbatim Windows path: \\?\D:\… or \\?\UNC\server\…
    if let Some(rest) = text.strip_prefix("//?/UNC/") {
        text = format!("//{rest}");
    } else if let Some(rest) = text.strip_prefix("//?/") {
        text = rest.to_string();
    }
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        // a Windows drive: file:///D:/…
        uri.push('/');
    }
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            uri.push(char::from(b));
        } else {
            let _ = write!(uri, "%{b:02X}");
        }
    }
    uri
}

fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        read_only(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// Whether the lock darktable keeps beside a database (`<file>.lock`,
/// its pid) names a process alive. A lock that cannot be read, or on a
/// system where a process cannot be probed, counts as alive: a copy is
/// harmless.
pub fn darktable_running(lock: &Path) -> bool {
    let text = match std::fs::read(lock) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let digits: String = text
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| char::from(*b))
        .collect();
    let Ok(pid) = digits.trim().parse::<u32>() else {
        return true;
    };
    pid == 0 || alive(pid)
}

#[cfg(target_os = "linux")]
fn alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map_or(true, |s| s.success())
}

#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    true
}

/// A private temporary folder, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Scratch> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "helios-darktable-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir)?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A consistent copy of a live database, through SQLite's backup API
/// from a read-only connection.
fn snapshot(path: &Path, into: &Path) -> rusqlite::Result<()> {
    let src = open_read_only(path)?;
    src.backup(
        rusqlite::MAIN_DB,
        into,
        None::<fn(rusqlite::backup::Progress)>,
    )
}

fn schema_version(con: &Connection, schema: &str, name: &str) -> Result<i64, ScanError> {
    let row: Option<Value> = con
        .query_row(
            &format!("SELECT value FROM {schema}.db_info WHERE key = 'version'"),
            [],
            |r| Ok(sql_value(r.get_ref(0)?)),
        )
        .optional()
        .map_err(|e| refused(format!("{name} is not a darktable database ({e})")))?;
    row.and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
    .ok_or_else(|| {
        refused(format!(
            "{name} is not a darktable database (no schema version)"
        ))
    })
}

fn sql_value(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Integer(i) => json!(i),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        _ => Value::Null,
    }
}

fn check_version(kind: &str, found: i64, known: &RangeInclusive<i64>) -> Result<(), ScanError> {
    let (low, high) = (known.start(), known.end());
    if found > *high {
        return Err(refused(format!(
            "this {kind}.db was written by a darktable newer than this importer knows \
             ({kind} schema {found}; it reads {low} to {high})"
        )));
    }
    if found < *low {
        let what = if kind == "library" {
            format!("darktable library from an older darktable (schema {found})")
        } else {
            format!("darktable {kind}.db from an older darktable ({kind} schema {found})")
        };
        return Err(refused(format!(
            "{what}: open it once in darktable 4.0 or later to upgrade it, then import again"
        )));
    }
    Ok(())
}

fn check_columns(
    con: &Connection,
    schema: &str,
    tables: &[(&str, &[&str])],
    name: &str,
) -> Result<(), ScanError> {
    for (table, columns) in tables {
        let mut stmt = con.prepare(&format!("PRAGMA {schema}.table_info({table})"))?;
        let have: BTreeSet<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<_, _>>()?;
        let missing: Vec<&str> = columns
            .iter()
            .copied()
            .filter(|c| !have.contains(*c))
            .collect();
        if !missing.is_empty() {
            return Err(refused(format!(
                "{name} is not a darktable library this importer reads: table {table} lacks {}",
                missing.join(", ")
            )));
        }
    }
    Ok(())
}

fn has_table(con: &Connection, schema: &str, table: &str) -> rusqlite::Result<bool> {
    con.query_row(
        &format!("SELECT 1 FROM {schema}.sqlite_master WHERE type = 'table' AND name = ?1"),
        [table],
        |_| Ok(()),
    )
    .optional()
    .map(|r| r.is_some())
}

fn quoted(names: impl IntoIterator<Item = String>) -> String {
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort();
    let more = names.len().saturating_sub(QUOTED);
    let text = names
        .iter()
        .take(QUOTED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if more > 0 {
        format!("{text} and {more} more")
    } else {
        text
    }
}

fn photos(n: usize) -> String {
    if n == 1 {
        "1 photo".into()
    } else {
        format!("{n} photos")
    }
}

/// (stars or none, rejected) from `images.flags`.
pub fn marks(flags: i64) -> (Option<i64>, bool) {
    let stars = flags & STARS;
    let rejected = flags & REJECTED != 0 || stars == OLD_REJECT;
    ((stars <= 5).then_some(stars), rejected)
}

/// Text from a column, whatever SQLite holds there; none for NULL.
fn text(v: ValueRef<'_>) -> Option<String> {
    match v {
        ValueRef::Text(t) | ValueRef::Blob(t) => Some(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Integer(i) => Some(i.to_string()),
        ValueRef::Real(f) => Some(f.to_string()),
        ValueRef::Null => None,
    }
}

/// One `images` row.
struct Row {
    id: i64,
    film: i64,
    filename: String,
    version: i64,
    flags: i64,
    folder: String,
    history_end: Option<i64>,
    write_timestamp: Option<i64>,
}

/// darktable's allowance between a sidecar's time and the library's
/// (`MAX_TIME_SKEW`, src/control/crawler.c), in seconds.
const TIME_SKEW: i64 = 2;

/// The name of a duplicate's sidecar, as darktable spells it: `a.cr2.xmp`
/// for the original, `a_01.cr2.xmp` for duplicate 1
/// (`dt_image_path_append_version_no_db`).
pub fn sidecar_name(filename: &str, version: i64) -> String {
    if version <= 0 {
        return format!("{filename}.xmp");
    }
    match filename.rfind('.') {
        Some(dot) => format!("{}_{version:02}{}.xmp", &filename[..dot], &filename[dot..]),
        None => format!("{filename}_{version:02}.xmp"),
    }
}

/// Lowercase hex, darktable's spelling of params in a sidecar.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn blob(v: ValueRef<'_>) -> Option<Vec<u8>> {
    match v {
        ValueRef::Blob(b) | ValueRef::Text(b) => Some(b.to_vec()),
        _ => None,
    }
}

/// The `darktable:history_current_hash` a sidecar's text holds, as
/// written (hex, or `gz…` when darktable compressed it).
pub fn xmp_hash(xml: &str) -> Option<String> {
    let key = "darktable:history_current_hash=\"";
    let at = xml.find(key)? + key.len();
    let end = xml[at..].find('"')?;
    Some(xml[at..at + end].to_string())
}

/// What is known of a duplicate's `.xmp`.
pub struct Xmp {
    pub path: PathBuf,
    /// Its history hash, hex; none when absent or compressed.
    pub hash: Option<String>,
    /// Its modification time, seconds since the epoch.
    pub modified: Option<i64>,
}

/// Where a duplicate's edit comes from, and why: the library's history
/// when there is no `.xmp`, or when the history hashes differ and the
/// `.xmp` did not change after darktable last wrote it (the library is
/// newer); the `.xmp` otherwise. None when neither holds an edit.
pub fn choose(
    history: bool,
    library_hash: Option<&str>,
    write_timestamp: Option<i64>,
    xmp: Option<&Xmp>,
) -> Option<(&'static str, String)> {
    match (history, xmp) {
        (false, None) => None,
        (true, None) => Some((
            "library",
            "no .xmp beside the photo (darktable's write_sidecar_files off?): the edit comes \
             from the library"
                .into(),
        )),
        (false, Some(_)) => Some(("xmp", "the library holds no history for it".into())),
        (true, Some(x)) => {
            if let (Some(l), Some(h)) = (library_hash, x.hash.as_deref())
                && l.eq_ignore_ascii_case(h)
            {
                return Some((
                    "xmp",
                    "the .xmp and the library hold the same edit (same history hash)".into(),
                ));
            }
            if let (Some(m), Some(w)) = (x.modified, write_timestamp)
                && m > w + TIME_SKEW
            {
                return Some((
                    "xmp",
                    "the .xmp changed after darktable last wrote it: it is newer than the library"
                        .into(),
                ));
            }
            Some((
                "library",
                "the library's history hash differs from the .xmp's and the .xmp is not newer: \
                 the library's history is"
                    .into(),
            ))
        }
    }
}

fn modified(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(secs).ok()
}

/// One duplicate's history, in the protocol's shape (see
/// `helios_xmp::history`).
fn history(con: &Connection, row: &Row) -> Result<Option<Value>, ScanError> {
    let mut stmt = con.prepare_cached(
        "SELECT num, module, operation, op_params, enabled, blendop_params, \
         blendop_version, multi_priority, multi_name \
         FROM main.history WHERE imgid = ?1 ORDER BY num",
    )?;
    let items: Vec<Value> = stmt
        .query_map([row.id], |r| {
            let mut item = json!({
                "num": r.get::<_, Option<i64>>(0)?.unwrap_or_default(),
                "modversion": r.get::<_, Option<i64>>(1)?.unwrap_or_default(),
                "operation": text(r.get_ref(2)?).unwrap_or_default(),
                "params": hex(&blob(r.get_ref(3)?).unwrap_or_default()),
                "enabled": r.get::<_, Option<i64>>(4)?.unwrap_or_default() != 0,
                "multi_priority": r.get::<_, Option<i64>>(7)?.unwrap_or_default(),
                "multi_name": text(r.get_ref(8)?).unwrap_or_default(),
            });
            if let Some(b) = blob(r.get_ref(5)?) {
                item["blendop_params"] = json!(hex(&b));
            }
            if let Some(v) = r.get::<_, Option<i64>>(6)? {
                item["blendop_version"] = json!(v);
            }
            Ok(item)
        })?
        .collect::<Result<_, _>>()?;
    if items.is_empty() {
        return Ok(None);
    }
    let mut stmt = con.prepare_cached(
        "SELECT num, formid, form, name, version, points, points_count, source \
         FROM main.masks_history WHERE imgid = ?1 ORDER BY num, formid",
    )?;
    let masks: Vec<Value> = stmt
        .query_map([row.id], |r| {
            let mut mask = json!({
                "num": r.get::<_, Option<i64>>(0)?.unwrap_or_default(),
                "id": r.get::<_, Option<i64>>(1)?.unwrap_or_default(),
                "type": r.get::<_, Option<i64>>(2)?.unwrap_or_default(),
                "name": text(r.get_ref(3)?).unwrap_or_default(),
                "version": r.get::<_, Option<i64>>(4)?.unwrap_or_default(),
                "points": hex(&blob(r.get_ref(5)?).unwrap_or_default()),
                "count": r.get::<_, Option<i64>>(6)?.unwrap_or_default(),
            });
            if let Some(b) = blob(r.get_ref(7)?) {
                mask["source"] = json!(hex(&b));
            }
            Ok(mask)
        })?
        .collect::<Result<_, _>>()?;
    let end = row.history_end.unwrap_or(items.len() as i64).max(0);
    let mut out = json!({ "end": end, "items": items, "masks": masks });
    let mut stmt =
        con.prepare_cached("SELECT version, iop_list FROM main.module_order WHERE imgid = ?1")?;
    let order = stmt
        .query_row([row.id], |r| {
            Ok((r.get::<_, Option<i64>>(0)?, text(r.get_ref(1)?)))
        })
        .optional()?;
    if let Some((version, list)) = order {
        if let Some(v) = version {
            out["iop_order_version"] = json!(v);
        }
        if let Some(l) = list.filter(|l| !l.is_empty()) {
            out["iop_order_list"] = json!(l);
        }
    }
    Ok(Some(out))
}

/// The edits of a photo's duplicates, each from the library or its
/// `.xmp` (see [`choose`]).
fn edits(con: &Connection, versions: &[Row]) -> Result<Vec<Value>, ScanError> {
    let mut out = Vec::new();
    for row in versions {
        let history = history(con, row)?;
        let mut stmt =
            con.prepare_cached("SELECT current_hash FROM main.history_hash WHERE imgid = ?1")?;
        let hash = stmt
            .query_row([row.id], |r| Ok(blob(r.get_ref(0)?)))
            .optional()?
            .flatten()
            .map(|b| hex(&b));
        let path = Path::new(&row.folder).join(sidecar_name(&row.filename, row.version));
        let xmp = path.is_file().then(|| Xmp {
            hash: std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| xmp_hash(&t))
                .filter(|h| !h.starts_with("gz")),
            modified: modified(&path),
            path,
        });
        let Some((from, why)) = choose(
            history.is_some(),
            hash.as_deref(),
            row.write_timestamp,
            xmp.as_ref(),
        ) else {
            continue;
        };
        let mut edit = json!({ "version": row.version, "from": from, "why": why });
        match (from, history, &xmp) {
            ("library", Some(h), _) => edit["history"] = h,
            (_, _, Some(x)) => edit["xmp"] = json!(x.path.to_string_lossy()),
            _ => continue,
        }
        out.push(edit);
    }
    Ok(out)
}

/// Where a paged scan sends its pages: `(images, done, total)`.
pub type Pager<'a> = (usize, &'a mut dyn FnMut(Vec<Value>, usize, usize));

/// One `importer.scan`: `progress(done, total, message)` as it goes;
/// `cancelled` seen between batches.
pub fn scan(
    params: &Value,
    cancelled: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize, &str),
) -> Result<Value, ScanError> {
    scan_paged(params, cancelled, progress, None)
}

/// [`scan`], its images sent in pages of `pager.0` to `pager.1` when
/// given, the answer then holding their count (`pages`) and the `total`
/// in place of `images`; `cancelled` is seen before each page.
pub fn scan_paged(
    params: &Value,
    cancelled: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize, &str),
    pager: Option<Pager<'_>>,
) -> Result<Value, ScanError> {
    let importer = params["importer"].as_str().unwrap_or_default();
    if importer != IMPORTER {
        return Err(ScanError::Invalid(format!("no importer `{importer}`")));
    }
    if params["kind"].as_str() != Some("folder") {
        return Err(refused(
            "pick darktable's configuration folder, the one holding library.db",
        ));
    }
    let folder = params["source"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| ScanError::Invalid("no source".into()))?;
    let mut library = folder.join("library.db");
    let mut data = folder.join("data.db");
    if !library.is_file() {
        return Err(refused(format!(
            "{} holds no library.db: pick darktable's configuration folder",
            folder.display()
        )));
    }
    let has_data = data.is_file();
    let mut warnings = Vec::new();
    let lock = |p: &Path| {
        let mut name = p.as_os_str().to_owned();
        name.push(".lock");
        PathBuf::from(name)
    };
    // Held until the scan ends: the copy is deleted with it.
    let mut _scratch = None;
    if darktable_running(&lock(&library)) || (has_data && darktable_running(&lock(&data))) {
        let copy = || -> Result<Scratch, String> {
            let s = Scratch::new().map_err(|e| e.to_string())?;
            snapshot(&library, &s.0.join("library.db")).map_err(|e| e.to_string())?;
            if has_data {
                snapshot(&data, &s.0.join("data.db")).map_err(|e| e.to_string())?;
            }
            Ok(s)
        };
        let s = copy().map_err(|e| {
            refused(format!(
                "darktable is running and its library could not be copied ({e}): \
                 close darktable, then import again"
            ))
        })?;
        library = s.0.join("library.db");
        data = s.0.join("data.db");
        _scratch = Some(s);
        warnings.push(
            "darktable was running: its library was read from a copy taken when the scan \
             started; what changed in darktable since is not in this import"
                .to_string(),
        );
    }
    let con = open_read_only(&library)?;
    let found = schema_version(&con, "main", "library.db")?;
    check_version("library", found, &LIBRARY)?;
    check_columns(&con, "main", LIBRARY_COLUMNS, "library.db")?;
    let mut data_version = None;
    if has_data {
        con.execute("ATTACH DATABASE ?1 AS data", [read_only(&data)])?;
        let v = schema_version(&con, "data", "data.db")?;
        check_version("data", v, &DATA)?;
        check_columns(&con, "data", DATA_COLUMNS, "data.db")?;
        data_version = Some(v);
    } else {
        warnings.push("there is no data.db beside library.db: the tags were skipped".into());
    }
    read(&con, data_version, cancelled, progress, warnings, pager)
}

#[allow(clippy::too_many_lines)]
fn read(
    con: &Connection,
    data_version: Option<i64>,
    cancelled: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize, &str),
    mut warnings: Vec<String>,
    mut pager: Option<Pager<'_>>,
) -> Result<Value, ScanError> {
    let mut stmt = con.prepare(
        "SELECT i.id, i.film_id, i.filename, i.flags, f.folder, i.version, i.history_end, \
         i.write_timestamp \
         FROM main.images AS i JOIN main.film_rolls AS f ON f.id = i.film_id \
         WHERE i.filename IS NOT NULL AND f.folder IS NOT NULL \
         ORDER BY i.film_id, i.filename, i.version, i.id",
    )?;
    let rows: Vec<Row> = stmt
        .query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                film: r.get::<_, Option<i64>>(1)?.unwrap_or_default(),
                filename: text(r.get_ref(2)?).unwrap_or_default(),
                flags: r.get::<_, Option<i64>>(3)?.unwrap_or_default(),
                folder: text(r.get_ref(4)?).unwrap_or_default(),
                version: r.get::<_, Option<i64>>(5)?.unwrap_or_default(),
                history_end: r.get(6)?,
                write_timestamp: r.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    let mut labels: HashMap<i64, BTreeSet<usize>> = HashMap::new();
    let mut stmt = con.prepare("SELECT imgid, color FROM main.color_labels")?;
    for r in stmt.query_map([], |r| {
        Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?))
    })? {
        if let (Some(id), Some(color)) = r?
            && let Ok(c) = usize::try_from(color)
            && c < LABELS.len()
        {
            labels.entry(id).or_default().insert(c);
        }
    }

    let mut tags: HashMap<i64, BTreeSet<String>> = HashMap::new();
    if data_version.is_some() {
        let mut stmt = con.prepare(
            "SELECT t.imgid, g.name FROM main.tagged_images AS t \
             JOIN data.tags AS g ON g.id = t.tagid \
             WHERE g.name NOT LIKE 'darktable|%'",
        )?;
        for r in stmt.query_map([], |r| {
            Ok((r.get::<_, Option<i64>>(0)?, text(r.get_ref(1)?)))
        })? {
            if let (Some(id), Some(name)) = r? {
                let name = name.trim();
                if !name.is_empty() {
                    tags.entry(id).or_default().insert(name.to_string());
                }
            }
        }
    }

    let mut keys: HashMap<i64, String> = FIXED_KEYS
        .iter()
        .enumerate()
        .map(|(k, n)| (k as i64, (*n).to_string()))
        .collect();
    if data_version.is_some_and(|v| v >= 13) && has_table(con, "data", "meta_data")? {
        let mut stmt = con.prepare("SELECT key, tagname FROM data.meta_data")?;
        keys = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, text(r.get_ref(1)?))))?
            .filter_map(|r| match r {
                Ok((k, Some(n))) => Some(Ok((k, n))),
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            })
            .collect::<Result<_, _>>()?;
    }
    let mut texts: HashMap<i64, BTreeMap<&'static str, String>> = HashMap::new();
    let mut dropped: BTreeMap<String, usize> = BTreeMap::new();
    let mut stmt = con.prepare("SELECT id, key, value FROM main.meta_data")?;
    for r in stmt.query_map([], |r| {
        Ok((
            r.get::<_, Option<i64>>(0)?,
            r.get::<_, Option<i64>>(1)?,
            text(r.get_ref(2)?),
        ))
    })? {
        let (Some(id), Some(key), Some(value)) = r? else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let name = keys
            .get(&key)
            .cloned()
            .unwrap_or_else(|| format!("key {key}"));
        if name == TITLE || name == DESCRIPTION {
            let field = if name == TITLE { TITLE } else { DESCRIPTION };
            texts
                .entry(id)
                .or_default()
                .insert(field, value.to_string());
        } else if !OWN_KEYS.contains(&name.as_str()) {
            *dropped.entry(name).or_default() += 1;
        }
    }

    // One file per (film roll, file name): its rows, the original first.
    let mut files: Vec<&[Row]> = Vec::new();
    let mut start = 0;
    for i in 1..=rows.len() {
        if i == rows.len()
            || (rows[i].film, &rows[i].filename) != (rows[start].film, &rows[start].filename)
        {
            files.push(&rows[start..i]);
            start = i;
        }
    }
    let total = files.len();
    let mut images = Vec::new();
    let (mut missing, mut missing_folders, mut differing) = (0, BTreeSet::new(), Vec::new());
    progress(0, total, "");
    if std::env::var_os(HOLD).is_some() {
        let until = Instant::now() + Duration::from_mins(1);
        while !cancelled.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let mut pages = 0usize;
    for (done, versions) in files.iter().enumerate() {
        if let Some((size, send)) = &mut pager
            && images.len() >= (*size).max(1)
        {
            if cancelled.load(Ordering::SeqCst) {
                return Err(ScanError::Cancelled);
            }
            send(std::mem::take(&mut images), done, total);
            pages += 1;
        }
        if done % BATCH == 0 {
            if cancelled.load(Ordering::SeqCst) {
                return Err(ScanError::Cancelled);
            }
            if done > 0 {
                progress(done, total, &versions[0].filename);
            }
        }
        let base = &versions[0];
        let path = Path::new(&base.folder).join(&base.filename);
        if !path.is_file() {
            missing += 1;
            missing_folders.insert(base.folder.clone());
            continue;
        }
        let (stars, rejected) = marks(base.flags);
        let mut entry = json!({ "path": path.to_string_lossy(), "rejected": rejected });
        if let Some(s) = stars {
            entry["rating"] = json!(s);
        }
        if versions[1..]
            .iter()
            .any(|v| marks(v.flags) != (stars, rejected))
        {
            differing.push(base.filename.clone());
        }
        let found_labels: BTreeSet<usize> = versions
            .iter()
            .filter_map(|v| labels.get(&v.id))
            .flatten()
            .copied()
            .collect();
        if !found_labels.is_empty() {
            entry["labels"] = json!(found_labels.iter().map(|c| LABELS[*c]).collect::<Vec<_>>());
        }
        let found_tags: BTreeSet<&String> = versions
            .iter()
            .filter_map(|v| tags.get(&v.id))
            .flatten()
            .collect();
        if !found_tags.is_empty() {
            entry["tags"] = json!(found_tags);
        }
        for (name, field) in [(TITLE, "title"), (DESCRIPTION, "description")] {
            // the original's, else the first duplicate's that has one
            if let Some(value) = versions
                .iter()
                .find_map(|v| texts.get(&v.id).and_then(|t| t.get(name)))
            {
                entry[field] = json!(value);
            }
        }
        let found = edits(con, versions)?;
        if !found.is_empty() {
            entry["edits"] = json!(found);
        }
        images.push(entry);
    }
    if cancelled.load(Ordering::SeqCst) {
        return Err(ScanError::Cancelled);
    }
    progress(total, total, "");

    if rows.is_empty() {
        warnings.push("the library holds no photos".into());
    }
    if missing > 0 {
        warnings.push(format!(
            "{} of the library left out, not on disk (a drive not plugged in?); \
             their folders: {}",
            photos(missing),
            quoted(missing_folders)
        ));
    }
    if !differing.is_empty() {
        warnings.push(format!(
            "{} with darktable duplicates whose stars or reject differ from the \
             original's, which Helios keeps: {}",
            photos(differing.len()),
            quoted(differing)
        ));
    }
    if !dropped.is_empty() {
        let counts: Vec<String> = dropped.iter().map(|(n, c)| format!("{n} ({c})")).collect();
        warnings.push(format!(
            "texts with no place in Helios yet were not imported: {}",
            counts.join(", ")
        ));
    }
    if let Some((_, send)) = &mut pager {
        if !images.is_empty() {
            send(images, total, total);
            pages += 1;
        }
        return Ok(json!({ "pages": pages, "total": total, "warnings": warnings }));
    }
    Ok(json!({ "images": images, "warnings": warnings }))
}

/// Writes one message, a line of JSON.
fn send<W: Write>(out: &Mutex<W>, message: &Value) {
    let mut out = out.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = writeln!(out, "{message}");
    let _ = out.flush();
}

/// The plugin: JSON-RPC 2.0 over `input` and `output`, one message per
/// line. Each scan runs in a thread of its own, so a `$/cancelRequest`
/// is read while it runs; at the end of the input the scans under way
/// still answer.
pub fn serve<R: BufRead, W: Write + Send + 'static>(input: R, output: W) {
    let out = Arc::new(Mutex::new(output));
    let scans: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>> = Arc::default();
    let mut threads = Vec::new();
    for line in input.lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = message["method"].as_str().unwrap_or_default().to_string();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = message.get("id").cloned() else {
            if method == "exit" {
                return;
            }
            if method == "$/cancelRequest"
                && let Some(flag) = scans
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&params["id"].to_string())
            {
                flag.store(true, Ordering::SeqCst);
            }
            continue;
        };
        let reply = match method.as_str() {
            "initialize" => json!({"jsonrpc": "2.0", "id": id, "result": {"name": IMPORTER}}),
            "shutdown" => json!({"jsonrpc": "2.0", "id": id, "result": null}),
            "importer.scan" => {
                let flag = Arc::new(AtomicBool::new(false));
                scans
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(id.to_string(), flag.clone());
                let (out, scans) = (out.clone(), scans.clone());
                threads.push(std::thread::spawn(move || {
                    let token = params["progress"].as_str().map(String::from);
                    let mut progress = |done: usize, total: usize, message: &str| {
                        if let Some(token) = &token {
                            send(
                                &out,
                                &json!({"jsonrpc": "2.0", "method": "$/progress", "params": {
                                    "token": token, "done": done, "total": total, "message": message,
                                }}),
                            );
                        }
                    };
                    let size = params["pageSize"]
                        .as_u64()
                        .and_then(|s| usize::try_from(s).ok())
                        .filter(|s| *s > 0);
                    let page_token = token.clone();
                    let page_out = out.clone();
                    let mut page = |images: Vec<Value>, done: usize, total: usize| {
                        send(
                            &page_out,
                            &json!({"jsonrpc": "2.0", "method": "importer/page", "params": {
                                "token": page_token, "images": images, "done": done, "total": total,
                            }}),
                        );
                    };
                    let pager = size.map(|s| (s, &mut page as &mut dyn FnMut(Vec<Value>, usize, usize)));
                    let answer = scan_paged(&params, &flag, &mut progress, pager);
                    scans
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(&id.to_string());
                    let reply = match answer {
                        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": {
                            "code": e.code(), "message": e.message(),
                        }}),
                    };
                    send(&out, &reply);
                }));
                continue;
            }
            _ => json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": -32601, "message": format!("no method {method}"),
            }}),
        };
        send(&out, &reply);
    }
    for t in threads {
        let _ = t.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_give_stars_and_reject() {
        assert_eq!(marks(3 | 32), (Some(3), false));
        assert_eq!(marks(2 | REJECTED), (Some(2), true));
        assert_eq!(marks(0), (Some(0), false));
        // before library schema 29
        assert_eq!(marks(6), (None, true));
        assert_eq!(marks(7), (None, false));
    }

    #[test]
    fn a_read_only_uri_escapes_the_path() {
        assert_eq!(
            read_only(Path::new("/a b/Été?#%.db")),
            "file:///a%20b/%C3%89t%C3%A9%3F%23%25.db?mode=ro"
        );
    }

    #[test]
    fn a_file_uri_has_three_slashes_on_unix_and_windows() {
        assert_eq!(
            file_uri("/home/ann/.config/darktable/library.db"),
            "file:///home/ann/.config/darktable/library.db"
        );
        assert_eq!(
            file_uri(r"D:\Users\Ann Été\darktable\library.db"),
            "file:///D:/Users/Ann%20%C3%89t%C3%A9/darktable/library.db"
        );
        assert_eq!(
            file_uri(r"\\?\D:\darktable\data.db"),
            "file:///D:/darktable/data.db"
        );
        assert_eq!(
            file_uri(r"\\nas\photos\library.db"),
            "file:////nas/photos/library.db"
        );
    }

    #[test]
    fn a_library_older_than_darktable_4_0_is_refused() {
        // a minimal library at schema 34 (darktable 3.6 and 3.8)
        let dir = std::env::temp_dir().join(format!("helios-dt-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let con = Connection::open(dir.join("library.db")).unwrap();
        con.execute_batch(
            "CREATE TABLE db_info (key VARCHAR PRIMARY KEY, value VARCHAR); \
             INSERT INTO db_info VALUES ('version', '34');",
        )
        .unwrap();
        drop(con);
        let params = json!({"importer": IMPORTER, "kind": "folder", "source": dir});
        let e = scan(&params, &AtomicBool::new(false), &mut |_, _, _| {}).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(e.code(), NOT_A_SOURCE);
        assert_eq!(
            e.message(),
            "darktable library from an older darktable (schema 34): open it once in \
             darktable 4.0 or later to upgrade it, then import again"
        );
        let e = check_version("data", 8, &DATA).unwrap_err();
        assert!(
            e.message()
                .contains("(data schema 8): open it once in darktable 4.0")
        );
    }

    #[test]
    fn schemas_outside_the_range_are_refused_with_their_number() {
        let e = check_version("library", 58, &LIBRARY).unwrap_err();
        assert!(e.message().contains("library schema 58; it reads 36 to 57"));
        assert_eq!(e.code(), NOT_A_SOURCE);
        assert!(check_version("data", 8, &DATA).is_err());
        assert!(check_version("library", 47, &LIBRARY).is_ok());
    }

    #[test]
    fn a_lock_names_a_live_process_or_not() {
        let dir = std::env::temp_dir().join(format!("helios-dt-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("library.db.lock");
        assert!(!darktable_running(&lock), "no lock");
        std::fs::write(&lock, format!("{}\0", std::process::id())).unwrap();
        assert!(darktable_running(&lock), "this process");
        std::fs::write(&lock, "garbage").unwrap();
        assert!(darktable_running(&lock), "unreadable: a copy is harmless");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
