//! Helios's Lightroom Classic catalogue importer (`importer` interface
//! version 2; see Helios's `docs/extensions.md`, "Importers").
//!
//! It reads a `.lrcat` catalogue, an SQLite database, and lists each photo
//! file it holds with what Lightroom knows of it: stars, reject, colour
//! label, keywords (the whole `a|b|c` path) and the collections it is in
//! (as tags under `Lightroom collection|…`, until Helios has albums).
//! Virtual copies are folded into their master photo. Develop settings are
//! not read yet.
//!
//! The scan answers in pages (`importer/page` notifications) when Helios
//! asks for them (`pageSize`), as the darktable library importer does.
//!
//! The catalogue is never written: it is opened read-only (a `mode=ro`
//! URI), and while Lightroom has it open (a `<name>.lrcat.lock` beside it)
//! it is first copied through SQLite's backup API into a private temporary
//! folder, and the copy is read.
//!
//! # The schema read
//!
//! Adobe does not document the `.lrcat` schema. The tables and columns
//! below are those read by the open-source catalogue readers, chiefly
//! `lrcat-extractor` (Hubert Figuière, <https://github.com/hfiguiere/lrcat-extractor>,
//! its `images.rs`, `folders.rs`, `keywords.rs` and `collections.rs`
//! queries) and the `Lightroom-SQL-tools` scripts
//! (<https://github.com/fdenivac/Lightroom-SQL-tools>):
//!
//! - `AgLibraryRootFolder (id_local, absolutePath)`, `absolutePath` ending
//!   in `/`;
//! - `AgLibraryFolder (id_local, rootFolder, pathFromRoot)`,
//!   `pathFromRoot` relative to the root and ending in `/` (empty for the
//!   root itself);
//! - `AgLibraryFile (id_local, folder, baseName, extension)`;
//! - `Adobe_images (id_local, rootFile, masterImage, rating, colorLabels,
//!   pick)`: `masterImage` set on a virtual copy, `rating` 0 to 5 or NULL,
//!   `pick` 1 (picked), 0 or -1 (rejected), `colorLabels` the label's name
//!   in the catalogue's label set (`Red`, `Yellow`, `Green`, `Blue`,
//!   `Purple` in the default English set);
//! - `AgLibraryKeyword (id_local, name, parent)`, the root keyword's name
//!   NULL; `AgLibraryKeywordImage (image, tag)`;
//! - `AgLibraryCollection (id_local, name, parent, creationId,
//!   systemOnly)`, `creationId` telling collections
//!   (`com.adobe.ag.library.collection`), collection sets
//!   (`com.adobe.ag.library.group`) and smart collections
//!   (`com.adobe.ag.library.smart_collection`, whose rules are not
//!   membership rows); `AgLibraryCollectionImage (collection, image)`.
//!
//! These were not checked against a catalogue written by Lightroom
//! itself; the columns are checked before anything is read, so a
//! catalogue that differs is refused with what it lacks.
//!
//! - [`serve`]: the plugin's JSON-RPC loop, one message per line;
//! - [`scan`]: one `importer.scan`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use helios_plugin_darktable_library::read_only;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

/// The importer's id in the manifest.
pub const IMPORTER: &str = "lightroom-catalog";

pub const NOT_A_SOURCE: i64 = -32001;
pub const INVALID_PARAMS: i64 = -32602;
pub const CANCELLED: i64 = -32800;
pub const FAILED: i64 = -32000;

/// A progress notice, and a look for a cancellation, every so many files.
const BATCH: usize = 200;
/// Names quoted in a warning, at most.
const QUOTED: usize = 3;

/// Helios's colour labels, which Lightroom's default label set names the
/// same (capitalised).
const LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

/// The tag a collection is imported under, until Helios has albums.
pub const COLLECTION_TAG: &str = "Lightroom collection";

const SMART_COLLECTION: &str = "com.adobe.ag.library.smart_collection";

/// The columns read, checked before anything is read.
const COLUMNS: &[(&str, &[&str])] = &[
    ("AgLibraryRootFolder", &["id_local", "absolutePath"]),
    (
        "AgLibraryFolder",
        &["id_local", "rootFolder", "pathFromRoot"],
    ),
    (
        "AgLibraryFile",
        &["id_local", "folder", "baseName", "extension"],
    ),
    (
        "Adobe_images",
        &[
            "id_local",
            "rootFile",
            "masterImage",
            "rating",
            "colorLabels",
            "pick",
        ],
    ),
    ("AgLibraryKeyword", &["id_local", "name", "parent"]),
    ("AgLibraryKeywordImage", &["image", "tag"]),
    (
        "AgLibraryCollection",
        &["id_local", "name", "parent", "creationId", "systemOnly"],
    ),
    ("AgLibraryCollectionImage", &["collection", "image"]),
];

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
            ScanError::Failed(m) => format!("the Lightroom catalogue could not be read: {m}"),
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

fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        read_only(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// A private temporary folder, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Scratch> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "helios-lightroom-{}-{}",
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

fn check_columns(con: &Connection) -> Result<(), ScanError> {
    for (table, columns) in COLUMNS {
        let mut stmt = con.prepare(&format!("PRAGMA main.table_info({table})"))?;
        let have: BTreeSet<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<_, _>>()?;
        if have.is_empty() {
            return Err(refused(format!(
                "this is not a Lightroom Classic catalogue: it has no table {table}"
            )));
        }
        let missing: Vec<&str> = columns
            .iter()
            .copied()
            .filter(|c| !have.contains(*c))
            .collect();
        if !missing.is_empty() {
            return Err(refused(format!(
                "this Lightroom catalogue is not one this importer reads: table {table} lacks {}",
                missing.join(", ")
            )));
        }
    }
    Ok(())
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

/// A number from a column Lightroom keeps as REAL, INTEGER or text.
fn number(v: ValueRef<'_>) -> Option<f64> {
    match v {
        ValueRef::Integer(i) => Some(i as f64),
        ValueRef::Real(f) => Some(f),
        ValueRef::Text(t) => String::from_utf8_lossy(t).trim().parse().ok(),
        _ => None,
    }
}

fn quoted(names: impl IntoIterator<Item = String>) -> String {
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort();
    names.dedup();
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

/// A photo's marks, from its `Adobe_images` row: (stars or none,
/// rejected, picked).
pub fn marks(rating: Option<f64>, pick: Option<f64>) -> (Option<i64>, bool, bool) {
    let stars = rating
        .filter(|r| r.is_finite())
        .map(|r| r.round() as i64)
        .filter(|r| (0..=5).contains(r));
    let pick = pick.unwrap_or_default();
    (stars, pick < 0.0, pick > 0.0)
}

/// Helios's label for a Lightroom label name, when it has one.
pub fn label(name: &str) -> Option<&'static str> {
    let name = name.trim().to_lowercase();
    LABELS.iter().copied().find(|l| *l == name)
}

/// The full path of a file: root, folder, then `base.ext`.
pub fn file_path(root: &str, folder: &str, base: &str, extension: &str) -> PathBuf {
    let mut path = String::from(root);
    if !path.is_empty() && !path.ends_with('/') && !path.ends_with('\\') {
        path.push('/');
    }
    path.push_str(folder);
    if !path.is_empty() && !path.ends_with('/') && !path.ends_with('\\') {
        path.push('/');
    }
    path.push_str(base);
    if !extension.is_empty() {
        path.push('.');
        path.push_str(extension);
    }
    PathBuf::from(path)
}

/// One `Adobe_images` row, with its file.
struct Row {
    id: i64,
    file: i64,
    copy: bool,
    path: PathBuf,
    rating: Option<f64>,
    label: Option<String>,
    pick: Option<f64>,
}

/// A node of a tree kept as `(id, name, parent)` rows: its path from the
/// top, `|` between levels, nameless nodes (Lightroom's root keyword)
/// left out. A cycle stops the walk.
fn tree_paths(nodes: &HashMap<i64, (Option<String>, Option<i64>)>) -> HashMap<i64, String> {
    let mut out = HashMap::new();
    for id in nodes.keys() {
        let mut parts = Vec::new();
        let mut seen = BTreeSet::new();
        let mut at = Some(*id);
        while let Some(n) = at {
            if !seen.insert(n) {
                break;
            }
            let Some((name, parent)) = nodes.get(&n) else {
                break;
            };
            if let Some(name) = name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                parts.push(name.replace('|', "/"));
            }
            at = *parent;
        }
        if !parts.is_empty() {
            parts.reverse();
            out.insert(*id, parts.join("|"));
        }
    }
    out
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
/// in place of `images`.
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
    if params["kind"].as_str() != Some("file") {
        return Err(refused("pick the Lightroom catalogue, a .lrcat file"));
    }
    let mut catalogue = params["source"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| ScanError::Invalid("no source".into()))?;
    if !catalogue.is_file() {
        return Err(refused(format!("{} is not a file", catalogue.display())));
    }
    let mut warnings = Vec::new();
    let mut lock = catalogue.as_os_str().to_owned();
    lock.push(".lock");
    // Held until the scan ends: the copy is deleted with it.
    let mut _scratch = None;
    if Path::new(&lock).exists() {
        let copy = || -> Result<Scratch, String> {
            let s = Scratch::new().map_err(|e| e.to_string())?;
            let src = open_read_only(&catalogue).map_err(|e| e.to_string())?;
            src.backup(
                rusqlite::MAIN_DB,
                s.0.join("catalogue.lrcat"),
                None::<fn(rusqlite::backup::Progress)>,
            )
            .map_err(|e| e.to_string())?;
            Ok(s)
        };
        let s = copy().map_err(|e| {
            refused(format!(
                "Lightroom has this catalogue open and it could not be copied ({e}): \
                 close Lightroom, then import again"
            ))
        })?;
        catalogue = s.0.join("catalogue.lrcat");
        _scratch = Some(s);
        warnings.push(
            "Lightroom had the catalogue open: it was read from a copy taken when the scan \
             started; what changed in Lightroom since is not in this import"
                .to_string(),
        );
    }
    let con = open_read_only(&catalogue)?;
    // Not an SQLite database at all: SQLite says so on the first read.
    con.query_row("SELECT count(*) FROM main.sqlite_master", [], |_| Ok(()))
        .map_err(|e| refused(format!("this is not a Lightroom Classic catalogue ({e})")))?;
    check_columns(&con)?;
    read(&con, cancelled, progress, warnings, pager)
}

/// The rows of a two-column `(image, node)` table, each image's node
/// paths.
fn memberships(
    con: &Connection,
    sql: &str,
    paths: &HashMap<i64, String>,
) -> Result<HashMap<i64, BTreeSet<String>>, ScanError> {
    let mut out: HashMap<i64, BTreeSet<String>> = HashMap::new();
    let mut stmt = con.prepare(sql)?;
    for r in stmt.query_map([], |r| {
        Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?))
    })? {
        if let (Some(image), Some(node)) = r?
            && let Some(path) = paths.get(&node)
        {
            out.entry(image).or_default().insert(path.clone());
        }
    }
    Ok(out)
}

fn read(
    con: &Connection,
    cancelled: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize, &str),
    mut warnings: Vec<String>,
    mut pager: Option<Pager<'_>>,
) -> Result<Value, ScanError> {
    let mut stmt = con.prepare(
        "SELECT i.id_local, i.rootFile, i.masterImage, i.rating, i.colorLabels, i.pick, \
         r.absolutePath, d.pathFromRoot, f.baseName, f.extension \
         FROM main.Adobe_images AS i \
         JOIN main.AgLibraryFile AS f ON f.id_local = i.rootFile \
         JOIN main.AgLibraryFolder AS d ON d.id_local = f.folder \
         JOIN main.AgLibraryRootFolder AS r ON r.id_local = d.rootFolder \
         WHERE f.baseName IS NOT NULL AND r.absolutePath IS NOT NULL \
         ORDER BY i.rootFile, i.masterImage IS NOT NULL, i.id_local",
    )?;
    let rows: Vec<Row> = stmt
        .query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                file: r.get(1)?,
                copy: r.get::<_, Option<i64>>(2)?.is_some(),
                rating: number(r.get_ref(3)?),
                label: text(r.get_ref(4)?).filter(|l| !l.trim().is_empty()),
                pick: number(r.get_ref(5)?),
                path: file_path(
                    &text(r.get_ref(6)?).unwrap_or_default(),
                    &text(r.get_ref(7)?).unwrap_or_default(),
                    &text(r.get_ref(8)?).unwrap_or_default(),
                    &text(r.get_ref(9)?).unwrap_or_default(),
                ),
            })
        })?
        .collect::<Result<_, _>>()?;

    let mut keywords = HashMap::new();
    let mut stmt = con.prepare("SELECT id_local, name, parent FROM main.AgLibraryKeyword")?;
    for r in stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            text(r.get_ref(1)?),
            r.get::<_, Option<i64>>(2)?,
        ))
    })? {
        let (id, name, parent) = r?;
        keywords.insert(id, (name, parent));
    }
    let keywords = tree_paths(&keywords);
    let keywords = memberships(
        con,
        "SELECT image, tag FROM main.AgLibraryKeywordImage",
        &keywords,
    )?;

    let mut collections = HashMap::new();
    let mut smart = 0usize;
    let mut leaves = BTreeSet::new();
    let mut stmt = con.prepare(
        "SELECT id_local, name, parent, creationId, systemOnly FROM main.AgLibraryCollection",
    )?;
    for r in stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            text(r.get_ref(1)?),
            r.get::<_, Option<i64>>(2)?,
            text(r.get_ref(3)?),
            number(r.get_ref(4)?),
        ))
    })? {
        let (id, name, parent, creation, system) = r?;
        // Lightroom's own collections (system only) are left out.
        if system.is_some_and(|s| s != 0.0) {
            continue;
        }
        if creation.as_deref() == Some(SMART_COLLECTION) {
            smart += 1;
        } else {
            leaves.insert(id);
        }
        collections.insert(id, (name, parent));
    }
    let collections: HashMap<i64, String> = tree_paths(&collections)
        .into_iter()
        .filter(|(id, _)| leaves.contains(id))
        .map(|(id, p)| (id, format!("{COLLECTION_TAG}|{p}")))
        .collect();
    let collections = memberships(
        con,
        "SELECT image, collection FROM main.AgLibraryCollectionImage",
        &collections,
    )?;

    // One file per `rootFile`: its rows, the master first.
    let mut files: Vec<&[Row]> = Vec::new();
    let mut start = 0;
    for i in 1..=rows.len() {
        if i == rows.len() || rows[i].file != rows[start].file {
            files.push(&rows[start..i]);
            start = i;
        }
    }
    let total = files.len();
    let mut images = Vec::new();
    let (mut missing, mut missing_folders) = (0, BTreeSet::new());
    let (mut differing, mut picked, mut copies) = (Vec::new(), 0usize, 0usize);
    let mut unknown_labels: BTreeMap<String, usize> = BTreeMap::new();
    progress(0, total, "");
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
        let base = &versions[0];
        let name = base
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if done % BATCH == 0 {
            if cancelled.load(Ordering::SeqCst) {
                return Err(ScanError::Cancelled);
            }
            if done > 0 {
                progress(done, total, &name);
            }
        }
        copies += versions.iter().filter(|v| v.copy).count();
        if !base.path.is_file() {
            missing += 1;
            missing_folders.insert(
                base.path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            );
            continue;
        }
        let (stars, rejected, pick) = marks(base.rating, base.pick);
        let mut entry = json!({ "path": base.path.to_string_lossy(), "rejected": rejected });
        if let Some(s) = stars {
            entry["rating"] = json!(s);
        }
        if pick {
            picked += 1;
        }
        if versions[1..]
            .iter()
            .any(|v| marks(v.rating, v.pick) != (stars, rejected, pick) || v.label != base.label)
        {
            differing.push(name);
        }
        if let Some(l) = &base.label {
            match label(l) {
                Some(l) => entry["labels"] = json!([l]),
                None => *unknown_labels.entry(l.trim().to_string()).or_default() += 1,
            }
        }
        let tags: BTreeSet<&String> = versions
            .iter()
            .flat_map(|v| [keywords.get(&v.id), collections.get(&v.id)])
            .flatten()
            .flatten()
            .collect();
        if !tags.is_empty() {
            entry["tags"] = json!(tags);
        }
        images.push(entry);
    }
    if cancelled.load(Ordering::SeqCst) {
        return Err(ScanError::Cancelled);
    }
    progress(total, total, "");

    if rows.is_empty() {
        warnings.push("the catalogue holds no photos".into());
    }
    if missing > 0 {
        warnings.push(format!(
            "{} of the catalogue left out, not on disk (a drive not plugged in?); \
             their folders: {}",
            photos(missing),
            quoted(missing_folders)
        ));
    }
    if copies > 0 {
        warnings.push(format!(
            "{copies} virtual {} folded into {} master photo: their keywords and collections \
             were kept, their develop settings are not imported yet",
            if copies == 1 { "copy" } else { "copies" },
            if copies == 1 { "its" } else { "their" },
        ));
    }
    if !differing.is_empty() {
        warnings.push(format!(
            "{} with virtual copies whose stars, pick or label differ from the master's, \
             which Helios keeps: {}",
            photos(differing.len()),
            quoted(differing)
        ));
    }
    if picked > 0 {
        warnings.push(format!(
            "{} flagged as picks: Helios has no pick flag, so the flag was not imported",
            photos(picked)
        ));
    }
    if !unknown_labels.is_empty() {
        let counts: Vec<String> = unknown_labels
            .iter()
            .map(|(n, c)| format!("{n} ({c})"))
            .collect();
        warnings.push(format!(
            "colour labels with no Helios colour were not imported: {}",
            counts.join(", ")
        ));
    }
    if smart > 0 {
        warnings.push(format!(
            "{smart} smart {} not imported: {} rules, not lists of photos",
            if smart == 1 {
                "collection was"
            } else {
                "collections were"
            },
            if smart == 1 { "it holds" } else { "they hold" },
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
    fn rating_and_pick_give_stars_reject_and_pick() {
        assert_eq!(marks(Some(3.0), Some(0.0)), (Some(3), false, false));
        assert_eq!(marks(Some(0.0), Some(-1.0)), (Some(0), true, false));
        assert_eq!(marks(None, Some(1.0)), (None, false, true));
        assert_eq!(marks(Some(9.0), None), (None, false, false));
    }

    #[test]
    fn default_label_names_map_to_helios_colours() {
        assert_eq!(label("Red"), Some("red"));
        assert_eq!(label(" purple "), Some("purple"));
        assert_eq!(label("Rouge"), None);
    }

    #[test]
    fn a_file_path_joins_root_folder_and_name() {
        assert_eq!(
            file_path("/photos/", "2024/trip/", "IMG_1", "CR3"),
            PathBuf::from("/photos/2024/trip/IMG_1.CR3")
        );
        assert_eq!(
            file_path("/photos", "", "a", "jpg"),
            PathBuf::from("/photos/a.jpg")
        );
        assert_eq!(file_path("/p/", "x", "b", ""), PathBuf::from("/p/x/b"));
    }

    #[test]
    fn tree_paths_skip_the_nameless_root_and_survive_a_cycle() {
        let nodes = HashMap::from([
            (1, (None, None)),
            (2, (Some("Places".to_string()), Some(1))),
            (3, (Some("Lyon".to_string()), Some(2))),
            (4, (Some("loop".to_string()), Some(5))),
            (5, (Some("back".to_string()), Some(4))),
        ]);
        let paths = tree_paths(&nodes);
        assert_eq!(paths.get(&3).map(String::as_str), Some("Places|Lyon"));
        assert!(!paths.contains_key(&1));
        assert!(paths.contains_key(&4));
    }
}
