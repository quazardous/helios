//! Import as a library operation: files from a folder or a mounted card
//! into the vault, referenced where they are or copied into the library's
//! folder, renamed by a pattern, their sidecars kept, run as a job.
//!
//! - **Sources.** A folder and its subfolders, or a mounted card (a folder
//!   holding `DCIM`, read from there). A phone over MTP is not a source
//!   yet: mount it as a folder first.
//! - **Placement.** Referenced in place (the vault records where the file
//!   is), or copied into the library folder as `YYYY/YYYY-MM-DD/<name>`,
//!   by capture date (or by month, by year, or all in one folder: a
//!   [`Layout`]). The source is read once: the copy is hashed as it
//!   is written (its identity, known before it takes a name), then read
//!   back to verify it, and measured from there. Nothing is ever overwritten: a
//!   name taken by another file gets `-1`, `-2`... Renaming by a
//!   [`Pattern`] needs a copy: a referenced file stays as it is.
//! - **Sidecars.** A copy takes its XMP sidecars along (darktable's base
//!   and `_NN` duplicates), renamed with it.
//! - **darktable edits.** With [`Options::darktable`], each sidecar of a
//!   file brought in becomes a version of its image, as
//!   `helios import-darktable` keeps them ([`crate::darktable`]): the base
//!   and each duplicate a head of its own, never merged; the line's
//!   `darktable` lists them with their import report (items not applied,
//!   modes substituted).
//! - **Exact duplicates.** A file's content hash is its identity: a file
//!   whose content the library already has, or that came earlier in the
//!   same import, is a duplicate. [`Duplicates`] says what to do: skip it
//!   (the default), keep it as one more copy of the image, or ask (left
//!   out and listed, for whoever started the import to answer by importing
//!   it again with skip or keep). Nothing is ever deleted.
//! - **Signals.** Each image's near-duplicate [`Signals`] are measured and
//!   kept with it.
//! - **Listed files.** An importer plugin lists the files to import with
//!   what it knows of each ([`Listed`]: rating, reject, labels, tags,
//!   title, description). They are imported as any file, and their values
//!   applied as field commands under the importer's name: the rating, the
//!   reject, the title and the description only where the image has none
//!   (what the library holds is never overwritten), labels and tags
//!   added. A duplicate left out gets none. They are applied before the
//!   file's darktable sidecars, so the importer's values come first.
//! - **A job.** It reports through [`helios_core::jobs`] (kind `import`):
//!   `job.started`, a `job.progress` per file (done, total, the file as
//!   its item), a log line per file that was not skipped, then
//!   `job.finished` (or `job.failed` when the source cannot be listed);
//!   one JSON line per file goes to `emit`.
//! - **Safe to format.** After a copy, each card read is checked by
//!   content hash: every photo on it in the library with a copy in the
//!   library folder ([`Card`] in the report, a log line "safe to format"
//!   or "N photos not in the Library"); when the library has a backup
//!   strategy, every photo must also have its verified backup copies
//!   ([`Card::short`]); without one, the card is "safe to format (1
//!   copy, in the Library: no backup set up)". [`card_safety`] computes
//!   it again from the card's files and their images, once a backup job
//!   ends. Helios never formats a card.
//! - **A lost source** (a card taken out, a folder moved away) pauses the
//!   job until it is back; it can still be cancelled.
//! - **History.** Each import, ended, cancelled or refused, leaves a
//!   record in the vault ([`crate::import_history`]): its sources,
//!   choices, counts, when and who asked ([`Options::origin`]).
//! - **Cancel.** [`Jobs::cancel`] with the job's id, or the caller's
//!   [`Options::cancel`] flag, stops the import before its next file; a
//!   copy under way stops too and its hidden part is removed. The files
//!   done stay imported; the job says `job.cancelled`.

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use helios_core::jobs::{Jobs, Progress, Start};
use helios_storage::strategy::Scope;
use helios_vault::fields::{self as vault_fields, Command, Op};
use helios_vault::{Change, Changes, ImageId, ImageRows, LibraryFlag, Location, Store, Vault};
use helios_xmp::files::Matched;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::darktable;
use crate::rename::{Naming, Pattern};
use crate::signals::{self, Signals};

/// The kind of an import's job.
pub const KIND: &str = "import";

/// The imports' job ids in this process: `import-1`, `import-2`...
pub(crate) static NEXT: AtomicU64 = AtomicU64::new(1);

/// Where imported files end up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Recorded where they are.
    Reference,
    /// Copied into the library folder: `into`, remembered in the vault for
    /// the next imports, or the one remembered when `None`.
    Copy { into: Option<PathBuf> },
}

/// What to do with a file whose content the library already has, or that
/// came earlier in the same import.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Duplicates {
    /// Leave it out.
    #[default]
    Skip,
    /// Keep it too: one more copy of the same image.
    Keep,
    /// Leave it out and list it, for a person to answer.
    Ask,
}

impl Duplicates {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "skip" => Ok(Duplicates::Skip),
            "keep" => Ok(Duplicates::Keep),
            "ask" => Ok(Duplicates::Ask),
            other => Err(format!("`{other}`: duplicates are skip, keep or ask")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Duplicates::Skip => "skip",
            Duplicates::Keep => "keep",
            Duplicates::Ask => "ask",
        }
    }
}

/// How copies are filed in the library folder, by capture date.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// `YYYY/YYYY-MM-DD/`.
    #[default]
    Day,
    /// `YYYY/YYYY-MM/`.
    Month,
    /// `YYYY/`.
    Year,
    /// All in the library folder itself.
    Flat,
}

impl Layout {
    pub const ALL: [Layout; 4] = [Layout::Day, Layout::Month, Layout::Year, Layout::Flat];

    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "day" => Ok(Layout::Day),
            "month" => Ok(Layout::Month),
            "year" => Ok(Layout::Year),
            "flat" => Ok(Layout::Flat),
            other => Err(format!("`{other}`: a layout is day, month, year or flat")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Layout::Day => "day",
            Layout::Month => "month",
            Layout::Year => "year",
            Layout::Flat => "flat",
        }
    }

    /// The folder, under the library folder, of a copy taken on `date`
    /// (`year, month, day`); `undated` without a date, unless flat.
    pub fn folder(self, date: Option<(i64, u32, u32)>) -> PathBuf {
        let Some((y, m, d)) = date else {
            return match self {
                Layout::Flat => PathBuf::new(),
                _ => PathBuf::from("undated"),
            };
        };
        let year = PathBuf::from(format!("{y:04}"));
        match self {
            Layout::Day => year.join(format!("{y:04}-{m:02}-{d:02}")),
            Layout::Month => year.join(format!("{y:04}-{m:02}")),
            Layout::Year => year,
            Layout::Flat => PathBuf::new(),
        }
    }
}

/// What an importer knows of a file it lists, as the library's fields
/// hold it (see [`Listed`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Metadata {
    /// 0 to 5 stars.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected: Option<bool>,
    /// Colour labels: `red`, `yellow`, `green`, `blue`, `purple`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The file's darktable edits as the importer found them (importer
    /// interface 2), in place of the sidecars beside the file: each with
    /// its duplicate `version`, `from` (`library` or `xmp`), `why`, and a
    /// `history` (see `helios_xmp::history`) or the `xmp` path.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<Value>,
}

impl Metadata {
    /// The field commands it makes for `image`: values set only where
    /// the image has none, labels and tags added.
    pub fn commands(&self, image: ImageId) -> Vec<Command> {
        let mut out = Vec::new();
        let mut absent = |field: &str, v: Value| {
            out.push(Command::with(image, field, Op::SetIfAbsent(v)));
        };
        if let Some(r) = self.rating {
            absent(vault_fields::RATING, json!(r));
        }
        if let Some(r) = self.rejected {
            absent(vault_fields::REJECTED, json!(r));
        }
        if let Some(t) = &self.title {
            absent("title", json!(t));
        }
        if let Some(d) = &self.description {
            absent("description", json!(d));
        }
        out.extend(
            self.labels
                .iter()
                .map(|l| Command::add(image, vault_fields::LABELS, l)),
        );
        out.extend(
            self.tags
                .iter()
                .map(|t| Command::add(image, vault_fields::TAGS, t)),
        );
        out
    }
}

/// Files an importer listed, each with what it knows of it, and who
/// writes those values (`importer:<plugin>/<importer>`, as the audit
/// trail says it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listed {
    pub by: String,
    /// By absolute path, as the import reads the file.
    pub files: BTreeMap<PathBuf, Metadata>,
}

/// How an import runs.
#[derive(Clone, Debug)]
pub struct Options {
    pub placement: Placement,
    /// New names for copies.
    pub rename: Option<Pattern>,
    /// How copies are filed.
    pub layout: Layout,
    pub duplicates: Duplicates,
    /// Measure and keep near-duplicate signals.
    pub signals: bool,
    /// Keep each file's darktable sidecars as versions of its image.
    pub darktable: bool,
    /// The values an importer gives the files it listed.
    pub listed: Option<Listed>,
    /// Set by the caller to stop the import (an interrupt): seen before
    /// each file and during a copy, as a cancel of its job is.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Who asked for it (`user`, `cli`, `agent:NAME`…), for its record in
    /// the import history ([`crate::import_history`]).
    pub origin: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            placement: Placement::Reference,
            rename: None,
            layout: Layout::Day,
            duplicates: Duplicates::Skip,
            signals: true,
            darktable: false,
            listed: None,
            cancel: None,
            origin: None,
        }
    }
}

/// What happened to one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// A new image.
    Added,
    /// An image the library had, at a new place.
    Located,
    /// Already recorded at that place.
    Unchanged,
    /// A duplicate left out.
    Skipped,
    /// A duplicate left out, waiting for an answer.
    Asked,
    Failed,
}

/// What an import did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Files gone through.
    pub files: u64,
    pub added: u64,
    pub located: u64,
    pub unchanged: u64,
    pub skipped: u64,
    pub asked: u64,
    pub failed: u64,
    /// Whether it was cancelled before its last file.
    pub cancelled: bool,
    /// Files copied into the library folder.
    pub copied: u64,
    /// Items of a connector whose bytes were fetched ([`crate::fetch`]).
    #[serde(skip_serializing_if = "is_zero")]
    pub fetched: u64,
    /// Items of a connector left out before any byte was fetched: known
    /// by their item id or their claimed hash, or probably in the Library.
    #[serde(skip_serializing_if = "is_zero")]
    pub unfetched: u64,
    /// Fetches tried again from zero after the bytes received were not
    /// the ones announced.
    #[serde(skip_serializing_if = "is_zero")]
    pub retried: u64,
    /// Sidecars copied with them.
    pub sidecars: u64,
    /// darktable edits kept as new versions ([`Options::darktable`]).
    pub edits: u64,
    /// darktable sidecars whose import reported items not applied or
    /// modes substituted.
    pub edits_reported: u64,
    /// darktable sidecars that could not be kept.
    pub edits_failed: u64,
    /// Images whose fields took values an importer listed.
    pub metadata: u64,
    /// Bytes read from the sources: a copied file is read once, hashed
    /// as it is copied.
    pub read: u64,
    /// For a copy, each card read: whether it is safe to format.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<Card>,
    /// What it changed in the library, image by image, for an undo to
    /// take back ([`Vault::revert`]) and a redo to put again
    /// ([`Vault::replay`]): the images it added with their locations,
    /// fields, signals and versions, and the rows it changed on images
    /// the library had. Files are not in it: an undo leaves every file
    /// where it is, copies included.
    #[serde(skip)]
    pub changes: Changes,
    /// The files it copied into the library folder, sidecars included:
    /// they stay on disk when the import is undone.
    #[serde(skip)]
    pub copies: Vec<PathBuf>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's signature
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Whether a card can be formatted after a copy: every photo on it is in
/// the library with a copy in the library folder, by content hash.
/// Helios never formats a card itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Card {
    /// Where it is mounted.
    pub path: PathBuf,
    /// Its name: the mount folder's (`EOS_DIGITAL`).
    pub label: String,
    /// Photos on it.
    pub photos: u64,
    /// Photos on it the library holds no copy of (failed, left out as
    /// duplicates of a referenced file, not reached).
    pub missing: u64,
    /// Backup copies each photo needs by the library's strategy (0
    /// without one).
    pub want: u8,
    /// Photos on it in the Library but short of their strategy's backup
    /// copies (a backup disk not plugged in yet).
    pub short: u64,
    pub safe_to_format: bool,
    /// The library folder its photos were copied into.
    pub root: PathBuf,
    /// Each file read on it, with its image (`None`: not imported): what
    /// [`card_safety`] recomputes it from once a backup job ends.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<(PathBuf, Option<ImageId>)>,
}

impl Card {
    /// What to tell a person: "`EOS_DIGITAL`: safe to format (library +
    /// 1 backup, verified)", "`EOS_DIGITAL`: safe to format (1 copy, in the
    /// Library: no backup set up)",
    /// "`EOS_DIGITAL`: 2 photos not in the Library" or "`EOS_DIGITAL`: in
    /// the Library, 3 photos short of their backups: keep the card".
    pub fn said(&self) -> String {
        match (self.missing, self.short) {
            (0, 0) if self.want == 0 => format!(
                "{}: safe to format (1 copy, in the Library: no backup set up)",
                self.label
            ),
            (0, 0) => format!(
                "{}: safe to format (library + {} backup{}, verified)",
                self.label,
                self.want,
                if self.want == 1 { "" } else { "s" }
            ),
            (0, 1) => format!(
                "{}: in the Library, 1 photo short of its backups: keep the card",
                self.label
            ),
            (0, n) => format!(
                "{}: in the Library, {n} photos short of their backups: keep the card",
                self.label
            ),
            (1, _) => format!("{}: 1 photo not in the Library", self.label),
            (n, _) => format!("{}: {n} photos not in the Library", self.label),
        }
    }
}

/// Whether the card mounted at `path` (named `label`) is safe to format:
/// each of its `files` (with the image it holds, `None` for one not
/// imported) in the Library, held by a file in the library folder `root`
/// and, when the library has a backup strategy, its image with its
/// verified backup copies, from the images' locations in the vault. An import computes it
/// at its end; it is computed again when a backup job ends, for the cards
/// still inserted.
pub fn card_safety<S: Store>(
    vault: &Vault<S>,
    root: &Path,
    path: &Path,
    label: &str,
    files: Vec<(PathBuf, Option<ImageId>)>,
) -> Result<Card, String> {
    let strategy = helios_storage::strategy::load(vault, &Scope::Library)?;
    let (volumes, verified) = match &strategy {
        Some(_) => (
            helios_storage::registry::known(vault)?,
            helios_storage::strategy::verified(vault)?,
        ),
        None => Default::default(),
    };
    let (mut missing, mut short) = (0, 0);
    for (_, id) in &files {
        // A file whose image is not in the Library is not held by it.
        let id = match id {
            Some(id) if vault.in_library(id)? => *id,
            _ => {
                missing += 1;
                continue;
            }
        };
        let locations = vault.locations(&id)?;
        if !locations
            .iter()
            .any(|l| l.present() && l.path.starts_with(root) && l.path.is_file())
        {
            missing += 1;
            continue;
        }
        if let Some(s) = &strategy
            && !helios_storage::strategy::status(&locations, &volumes, verified.get(&id), Some(s))
                .met()
        {
            short += 1;
        }
    }
    Ok(Card {
        path: path.to_path_buf(),
        label: label.to_string(),
        photos: files.len() as u64,
        missing,
        want: strategy.as_ref().map_or(0, |s| s.copies),
        short,
        safe_to_format: missing == 0 && short == 0,
        root: root.to_path_buf(),
        files,
    })
}

impl Report {
    pub(crate) fn count(&mut self, status: Status) {
        self.files += 1;
        match status {
            Status::Added => self.added += 1,
            Status::Located => self.located += 1,
            Status::Unchanged => self.unchanged += 1,
            Status::Skipped => self.skipped += 1,
            Status::Asked => self.asked += 1,
            Status::Failed => self.failed += 1,
        }
    }
}

/// The library folder copies go to, as the vault remembers it.
pub fn library_root<S: Store>(vault: &Vault<S>) -> Result<Option<PathBuf>, String> {
    vault.library_folder()
}

/// Remembers the library folder copies go to.
pub fn set_library_root<S: Store>(vault: &Vault<S>, root: &Path) -> Result<(), String> {
    vault.set_library_folder(root)
}

/// The room a library folder needs at least to be offered: 1 GiB.
pub const LIBRARY_FOLDER_ROOM: u64 = 1 << 30;

/// What is wrong with a folder chosen as the library folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderProblem {
    /// Not a full path (`Photos`, `~/Photos`).
    NotAbsolute,
    /// It cannot be made, or written in (the system's reason).
    NotWritable(String),
    /// Less than [`LIBRARY_FOLDER_ROOM`] free on its disk.
    NoRoom { free: u64 },
}

impl FolderProblem {
    /// For people.
    pub fn message(&self) -> helios_core::Message {
        use helios_core::Message;
        match self {
            FolderProblem::NotAbsolute => Message::new(
                "library-folder-not-absolute",
                "Give the folder's full path.",
            ),
            FolderProblem::NotWritable(why) => Message::new(
                "library-folder-not-writable",
                "Helios cannot write in this folder: { $why }",
            )
            .with("why", why.clone()),
            FolderProblem::NoRoom { free } => Message::new(
                "library-folder-no-room",
                "This disk has { $free } MiB free: choose a folder with room for photos.",
            )
            .with("free", (free >> 20).to_string()),
        }
    }
}

/// Checks `dir` can be the library folder: a full path, made if missing,
/// written in (a file written and removed), with room left on its disk
/// (where the disk says how much).
///
/// # Errors
/// What is wrong with it.
pub fn check_library_folder(dir: &Path) -> Result<(), FolderProblem> {
    if !dir.is_absolute() {
        return Err(FolderProblem::NotAbsolute);
    }
    std::fs::create_dir_all(dir).map_err(|e| FolderProblem::NotWritable(e.to_string()))?;
    let probe = dir.join(format!(".helios-write-test-{}", std::process::id()));
    std::fs::write(&probe, b"helios").map_err(|e| FolderProblem::NotWritable(e.to_string()))?;
    let _ = std::fs::remove_file(&probe);
    match helios_storage::detect::free_space(dir) {
        Some(free) if free < LIBRARY_FOLDER_ROOM => Err(FolderProblem::NoRoom { free }),
        _ => Ok(()),
    }
}

/// What is wrong with `dir` as the library folder, read without writing
/// anything (as it is typed): a full path whose nearest folder that
/// exists is a folder not read-only, with room left on its disk.
/// [`check_library_folder`] makes it and writes in it.
pub fn library_folder_problem(dir: &Path) -> Option<FolderProblem> {
    if !dir.is_absolute() {
        return Some(FolderProblem::NotAbsolute);
    }
    let existing = dir.ancestors().find(|a| a.exists())?;
    match std::fs::metadata(existing) {
        Ok(m) if !m.is_dir() => {
            return Some(FolderProblem::NotWritable(format!(
                "{} is a file",
                existing.display()
            )));
        }
        Ok(m) if m.permissions().readonly() => {
            return Some(FolderProblem::NotWritable(format!(
                "{} is read-only",
                existing.display()
            )));
        }
        Ok(_) => {}
        Err(e) => return Some(FolderProblem::NotWritable(e.to_string())),
    }
    match helios_storage::detect::free_space(existing) {
        Some(free) if free < LIBRARY_FOLDER_ROOM => Some(FolderProblem::NoRoom { free }),
        _ => None,
    }
}

/// The library folder offered first: the platform's Pictures folder's
/// `Helios` (`~/Pictures/Helios`), `None` without a home folder.
pub fn suggested_library_folder() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Pictures").join("Helios"))
}

// What a source is and the image files it holds: the folder and card
// connectors' walk (`helios_connect::local`), so the import reads what the
// import window listed.
pub use helios_connect::local::{files_of, source_kind};

/// The BLAKE3 hash of a file, streamed.
pub fn hash_file(path: &Path) -> Result<ImageId, String> {
    hash_counted(path, &mut 0)
}

/// The BLAKE3 hash of a file, streamed, the bytes read added to `read`.
fn hash_counted(path: &Path, read: &mut u64) -> Result<ImageId, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut from = Counting { inner: file, read };
    std::io::copy(&mut from, &mut hasher).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(ImageId(helios_core::ContentHash(
        *hasher.finalize().as_bytes(),
    )))
}

/// Whether the file at `path` holds the image `id`.
fn holds(path: &Path, id: &ImageId) -> Result<bool, String> {
    Ok(hash_file(path)? == *id)
}

/// A reader that counts the bytes it reads: what an import read of its
/// sources.
struct Counting<'a, R> {
    inner: R,
    read: &'a mut u64,
}

impl<R: Read> Read for Counting<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        *self.read += n as u64;
        Ok(n)
    }
}

/// Copies `from` to `part`, hashing what it reads: the hash of the
/// content written. Stopped (an `Interrupted` error) between two chunks
/// once `stopped` says so.
fn write_hashed(
    from: &mut impl Read,
    part: &Path,
    stopped: &dyn Fn() -> bool,
) -> std::io::Result<ImageId> {
    let mut to = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(part)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        if stopped() {
            return Err(std::io::Error::new(ErrorKind::Interrupted, "cancelled"));
        }
        let n = from.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        to.write_all(&buffer[..n])?;
    }
    to.sync_all()?;
    Ok(ImageId(helios_core::ContentHash(
        *hasher.finalize().as_bytes(),
    )))
}

/// Copies `src` to `part`, reading the source once and hashing it as it
/// goes, then reads the copy back to verify it: the source's content
/// hash. The bytes read from the source are added to `read`. On an error
/// (or stopped by `stopped`) the part is removed.
pub(crate) fn stage(
    src: &Path,
    part: &Path,
    read: &mut u64,
    stopped: &dyn Fn() -> bool,
) -> Result<ImageId, String> {
    let fail = |e: String| {
        let _ = std::fs::remove_file(part);
        Err(e)
    };
    let mut file = match File::open(src) {
        Ok(f) => f,
        Err(e) => return Err(format!("{}: {e}", src.display())),
    };
    let modified = file.metadata().and_then(|m| m.modified()).ok();
    let mut from = Counting {
        inner: &mut file,
        read,
    };
    let id = match write_hashed(&mut from, part, stopped) {
        Ok(id) => id,
        Err(e) => return fail(format!("{} to {}: {e}", src.display(), part.display())),
    };
    if let Some(modified) = modified {
        // The copy keeps the original's date; a file system that cannot
        // set it still gets the copy.
        let _ = File::options()
            .write(true)
            .open(part)
            .and_then(|f| f.set_modified(modified));
    }
    match holds(part, &id) {
        Ok(true) => Ok(id),
        Ok(false) => fail(format!(
            "{}: the copy differs from the source",
            src.display()
        )),
        Err(e) => fail(e),
    }
}

/// Gives the verified copy `part` the name `dest`, never replacing a
/// file: false when `dest` was taken meanwhile (the part is kept).
fn settle(part: &Path, dest: &Path) -> Result<bool, String> {
    // A hard link never replaces a file; a file system without them gets
    // a rename, after a last look.
    match std::fs::hard_link(part, dest) {
        Ok(()) => {
            let _ = std::fs::remove_file(part);
            Ok(true)
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(_) if dest.exists() => Ok(false),
        Err(_) => std::fs::rename(part, dest)
            .map(|()| true)
            .map_err(|e| format!("{}: {e}", dest.display())),
    }
}

/// Places the verified copy `part` of the image `id` in `dir` as
/// `name.ext`, or `name-1.ext`... when the name is taken by another file.
/// The path, and whether the copy took it (false when a file there
/// already holds the image: the part is then removed).
pub(crate) fn place(
    part: &Path,
    dir: &Path,
    name: &str,
    ext: &str,
    id: &ImageId,
) -> Result<(PathBuf, bool), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dot = if ext.is_empty() { "" } else { "." };
    let held = |dest: PathBuf| {
        let _ = std::fs::remove_file(part);
        Ok((dest, false))
    };
    for k in 0..10_000u32 {
        let file = if k == 0 {
            format!("{name}{dot}{ext}")
        } else {
            format!("{name}-{k}{dot}{ext}")
        };
        let dest = dir.join(file);
        if dest.exists() {
            if holds(&dest, id)? {
                return held(dest);
            }
            continue;
        }
        if settle(part, &dest)? {
            return Ok((dest, true));
        }
        if holds(&dest, id)? {
            return held(dest);
        }
    }
    Err(format!("{}: no free name for {name}", dir.display()))
}

/// Copies a file's sidecars beside its copy, renamed with it as darktable
/// names them (`NAME.EXT.xmp`, `NAME_01.EXT.xmp`...); a sidecar whose name
/// is taken is left out, never written over. The sidecars copied.
fn copy_sidecars(
    sidecars: &[helios_xmp::files::SidecarFile],
    dest: &Path,
) -> Result<Vec<Value>, String> {
    let dir = dest.parent().ok_or("a copy needs a folder")?;
    let file = dest
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("a copy needs a name")?;
    let (stem, ext) = file.rsplit_once('.').unwrap_or((file, ""));
    let mut copied = Vec::new();
    for sidecar in sidecars {
        let name = if sidecar.version == 0 {
            format!("{file}.xmp")
        } else {
            format!("{stem}_{:02}.{ext}.xmp", sidecar.version)
        };
        let target = dir.join(name);
        let out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target);
        let mut out = match out {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                copied.push(json!({ "from": sidecar.path, "left": target, "why": "taken" }));
                continue;
            }
            Err(e) => return Err(format!("{}: {e}", target.display())),
        };
        let mut from =
            File::open(&sidecar.path).map_err(|e| format!("{}: {e}", sidecar.path.display()))?;
        std::io::copy(&mut from, &mut out).map_err(|e| format!("{}: {e}", target.display()))?;
        copied.push(json!({ "from": sidecar.path, "to": target, "version": sidecar.version }));
    }
    Ok(copied)
}

/// The size and modification time (seconds since the epoch) of the file
/// at `path`, when it can be read.
fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Some((meta.len(), modified))
}

/// A copy before it takes its name: removed when dropped, unless it
/// took one.
pub(crate) struct Part(pub(crate) PathBuf);

impl Drop for Part {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The state of one import.
struct Run<'a, S: Store> {
    vault: &'a Vault<S>,
    options: &'a Options,
    root: Option<PathBuf>,
    /// Where copies are written before they take their name: a hidden
    /// folder of the library folder, this import's own.
    staging: Option<PathBuf>,
    now: u64,
    /// The images seen so far in this import, with the first file of each.
    seen: HashMap<ImageId, PathBuf>,
    /// Folder listings, for sidecars.
    folders: BTreeMap<PathBuf, Matched>,
    /// Each file's content hash, once read.
    hashes: HashMap<PathBuf, ImageId>,
    /// Copies made so far, for `{seq}`.
    placed: u64,
    report: Report,
    /// Asked to stop: by the job's cancel or the caller's flag.
    stop: Arc<AtomicBool>,
    /// Each image this import touched, in order, with its rows before.
    touched: Vec<(ImageId, Option<ImageRows>)>,
    /// Its job's id: what sets the Library flag (`library_by`).
    by: String,
}

impl<S: Store> Run<'_, S> {
    /// Notes the rows of image `id` before this import first writes them.
    fn touch(&mut self, id: ImageId) -> Result<(), String> {
        if !self.touched.iter().any(|(t, _)| *t == id) {
            let before = self.vault.rows(&id)?;
            self.touched.push((id, before));
        }
        Ok(())
    }

    /// What this import changed, image by image: what it saw is kept by
    /// an undo ([`facts_kept`]).
    fn changes(&self) -> Changes {
        let images = self
            .touched
            .iter()
            .filter_map(|(id, before)| {
                let after = self.vault.rows(id).ok()?;
                let change = Change {
                    image: *id,
                    before: facts_kept(before.clone(), after.as_ref()),
                    after,
                };
                change.changed().then_some(change)
            })
            .collect();
        Changes {
            images,
            ..Changes::default()
        }
    }

    /// The image of the file at `path` when it was seen before, unchanged
    /// since (the same size and modification time, its content hashed
    /// then): known without a read.
    fn seen_before(&self, path: &Path) -> Result<Option<ImageId>, String> {
        let Some((size, modified)) = stamp(path) else {
            return Ok(None);
        };
        let absolute = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(self.vault.at_path(&absolute)?.and_then(|(id, l)| {
            (l.present() && l.size == size && l.modified == modified).then_some(id)
        }))
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
            || self
                .options
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::SeqCst))
    }

    /// What says this import was asked to stop, owned.
    fn stopper(&self) -> impl Fn() -> bool + use<S> {
        let stop = self.stop.clone();
        let cancel = self.options.cancel.clone();
        move || {
            stop.load(Ordering::SeqCst) || cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst))
        }
    }

    fn sidecars(&mut self, path: &Path) -> Result<Vec<helios_xmp::files::SidecarFile>, String> {
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("{}: not a file name", path.display()))?
            .to_string();
        if !self.folders.contains_key(&dir) {
            let matched = helios_xmp::files::scan(&dir)?;
            self.folders.insert(dir.clone(), matched);
        }
        Ok(self.folders[&dir]
            .images
            .get(&name)
            .cloned()
            .unwrap_or_default())
    }

    /// Imports one file: its line, and what happened. A file seen before,
    /// unchanged since, is not read again: the map knows its content. A
    /// copy notes its source in the map once it has read it, so a copy
    /// import run again (resumed, say) reads none of what it copied.
    fn file(&mut self, path: &Path) -> Result<(Value, Status), String> {
        let copying = matches!(self.options.placement, Placement::Copy { .. });
        // Its size and time before any read: a file changed while it is
        // read is read again next time.
        let before = if copying { stamp(path) } else { None };
        let seen = self.seen_before(path)?;
        let outcome = self.file_known(path, copying, seen)?;
        if let Some((size, modified)) = before
            && seen.is_none()
            && let Some(id) = self.hashes.get(path).copied()
            && self.vault.image(&id)?.is_some()
        {
            self.vault.record_read(path, id, size, modified, self.now)?;
        }
        Ok(outcome)
    }

    /// [`Run::file`], `seen` the image the map knows the file holds.
    fn file_known(
        &mut self,
        path: &Path,
        copying: bool,
        seen: Option<ImageId>,
    ) -> Result<(Value, Status), String> {
        // A copy of content already held (in the Library, or met earlier
        // in this import) and not to be copied again is not read: the
        // duplicate check below leaves it out. An image taken out of the
        // Library is copied again, read as a new file is.
        let unread = copying
            && !matches!(self.options.duplicates, Duplicates::Keep)
            && match seen {
                Some(id) => self.seen.contains_key(&id) || self.vault.in_library(&id)?,
                None => false,
            };
        // A copy reads the source once, hashing it as it is copied; its
        // identity is known before the copy takes a name.
        let staged = match &self.staging {
            Some(staging) if !unread => {
                let name = path
                    .file_name()
                    .ok_or_else(|| format!("{}: not a file name", path.display()))?;
                let part = Part(staging.join(name));
                let stopped = self.stopper();
                let id = stage(path, &part.0, &mut self.report.read, &stopped)?;
                Some((part, id))
            }
            _ => None,
        };
        let id = match (&staged, seen) {
            (Some((_, id)), _) => *id,
            (None, Some(id)) => id,
            (None, None) => hash_counted(path, &mut self.report.read)?,
        };
        self.hashes.insert(path.to_path_buf(), id);
        self.touch(id)?;
        let mut line = json!({ "path": path, "id": id.hex() });
        let in_library = self.vault.in_library(&id)?;
        if !copying
            && in_library
            && self
                .vault
                .at_path(path)?
                .is_some_and(|(known, l)| known == id && l.present())
        {
            self.vault
                .import_known(path, id, self.now, Some(&self.by))?;
            self.seen.entry(id).or_insert_with(|| path.to_path_buf());
            self.keep_capture(path, id, None, &mut line)?;
            self.keep_metadata(path, id, &mut line)?;
            self.keep_edits(path, id, &mut line)?;
            return Ok((line, Status::Unchanged));
        }
        let duplicate = if let Some(first) = self.seen.get(&id) {
            Some(json!({ "of": "source", "first": first }))
        } else if in_library {
            // A lookup: the image is in the Library (an image only seen,
            // or whose import was undone, is not a duplicate).
            let places: Vec<PathBuf> = self
                .vault
                .locations(&id)?
                .into_iter()
                .filter(Location::present)
                .map(|l| l.path)
                .collect();
            Some(json!({ "of": "library", "at": places }))
        } else {
            None
        };
        self.seen.entry(id).or_insert_with(|| path.to_path_buf());
        if let Some(d) = duplicate {
            line["duplicate"] = d;
            match self.options.duplicates {
                Duplicates::Skip => return Ok((line, Status::Skipped)),
                Duplicates::Ask => return Ok((line, Status::Asked)),
                Duplicates::Keep => {}
            }
        }
        // Signals measured before are kept: the same content measures the
        // same.
        let measure = copying || (self.options.signals && self.vault.signals(&id)?.is_none());
        let measured = if measure {
            // A copy is measured, not the source: the source is read once.
            let at = staged.as_ref().map_or(path, |(part, _)| part.0.as_path());
            match signals::measure(at) {
                Ok(s) => Some(s),
                Err(e) => {
                    line["signals_error"] = json!(e);
                    None
                }
            }
        } else {
            None
        };
        let new = if let Some(root) = &self.root {
            let none = Signals::default();
            let signals = measured.as_ref().unwrap_or(&none);
            let folder = root.join(self.options.layout.folder(signals.date()));
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let name = match &self.options.rename {
                Some(pattern) => pattern.name(&Naming {
                    stem,
                    signals,
                    seq: self.placed + 1,
                }),
                None => stem.to_string(),
            };
            let part = staged.as_ref().map(|(part, _)| &part.0).ok_or("no copy")?;
            let (dest, copied) = place(part, &folder, &name, ext, &id)?;
            self.placed += 1;
            if copied {
                self.report.copied += 1;
            }
            let sidecars = self.sidecars(path)?;
            let kept = copy_sidecars(&sidecars, &dest)?;
            self.report.sidecars += kept.iter().filter(|s| s["to"].is_string()).count() as u64;
            if copied {
                self.report.copies.push(dest.clone());
            }
            self.report.copies.extend(
                kept.iter()
                    .filter_map(|s| s["to"].as_str().map(PathBuf::from)),
            );
            line["to"] = json!(dest);
            line["copied"] = json!(copied);
            line["sidecars"] = json!(kept);
            // The copy is a sighting at its destination; the image is
            // flagged in the Library.
            self.vault
                .import_known(&dest, id, self.now, Some(&self.by))?
        } else {
            let sidecars = self.sidecars(path)?;
            line["sidecars"] = json!(sidecars.iter().map(|s| &s.path).collect::<Vec<_>>());
            // Added where it is: no copy, its sighting there and the flag.
            self.vault
                .import_known(path, id, self.now, Some(&self.by))?
        };
        if self.options.signals
            && let Some(s) = &measured
        {
            let doc = serde_json::to_value(s).expect("signals serialize");
            self.vault.set_signals(&id, Some(&doc))?;
            line["signals"] = doc;
        }
        // The copy when there is one: the source is read once.
        let placed = line["to"].as_str().map(PathBuf::from);
        self.keep_capture(
            placed.as_deref().unwrap_or(path),
            id,
            measured.as_ref(),
            &mut line,
        )?;
        // An importer's values first: a catalogue is its application's
        // truth, a sidecar may lag behind it.
        self.keep_metadata(path, id, &mut line)?;
        self.keep_edits(path, id, &mut line)?;
        Ok((line, if new { Status::Added } else { Status::Located }))
    }

    /// Whether the card `source` is safe to format ([`card_safety`] over
    /// the images this import found its files hold).
    fn card(&self, source: &Source) -> Result<Card, String> {
        let Some(root) = &self.root else {
            return Err("only a copy makes a card safe to format".into());
        };
        let files: Vec<(PathBuf, Option<ImageId>)> = source
            .files
            .iter()
            .map(|f| (f.clone(), self.hashes.get(f).copied()))
            .collect();
        card_safety(self.vault, root, &source.path, &source.label(), files)
    }

    /// Records how the file at `path` was taken as its image's capture
    /// fields, unless the image has them already: its metadata read (no
    /// pixel decoded), else what `signals` measured.
    fn keep_capture(
        &mut self,
        path: &Path,
        id: ImageId,
        signals: Option<&Signals>,
        line: &mut Value,
    ) -> Result<(), String> {
        if self.vault.captures_of(&[id])?[0].is_some() {
            return Ok(());
        }
        let capture = match crate::capture::read(path) {
            Ok(c) => Some(c),
            Err(e) => {
                line["capture_error"] = json!(e);
                let doc = signals.map(|s| serde_json::to_value(s).expect("signals serialize"));
                let seen = self.vault.sightings(&id)?;
                crate::capture::of_kept(doc.as_ref(), &seen)
            }
        };
        if let Some(c) = capture {
            self.vault.set_captures(&[(id, c)], None)?;
        }
        Ok(())
    }

    /// Applies what an importer listed for the file at `path` to its image
    /// `id`: the fields changed under the line's `metadata`.
    fn keep_metadata(&mut self, path: &Path, id: ImageId, line: &mut Value) -> Result<(), String> {
        let Some(listed) = &self.options.listed else {
            return Ok(());
        };
        let Some(metadata) = listed.files.get(path) else {
            return Ok(());
        };
        let commands = metadata.commands(id);
        if commands.is_empty() {
            return Ok(());
        }
        let applied = self.vault.apply(&commands, &listed.by, None, self.now)?;
        line["metadata"] = json!({ "changed": applied.changed, "kept": applied.unchanged });
        if applied.changed > 0 {
            self.report.metadata += 1;
        }
        Ok(())
    }

    /// Keeps the darktable sidecars of the file at `path` (read where the
    /// file came from) as versions of its image `id`, when asked: each
    /// sidecar's outcome under the line's `darktable`.
    fn keep_edits(&mut self, path: &Path, id: ImageId, line: &mut Value) -> Result<(), String> {
        if !self.options.darktable {
            return Ok(());
        }
        if let Some(edits) = self
            .options
            .listed
            .as_ref()
            .and_then(|l| l.files.get(path))
            .map(|m| m.edits.clone())
            .filter(|e| !e.is_empty())
        {
            self.keep_listed_edits(path, id, &edits, line);
            return Ok(());
        }
        let files = self.sidecars(path)?;
        if files.is_empty() {
            return Ok(());
        }
        let mut kept = Vec::with_capacity(files.len());
        for (i, file) in files.iter().enumerate() {
            let base = i == 0 && file.version == 0;
            let out =
                darktable::keep_file_of(self.vault, id, path, file, base, self.now, &mut |_, _| {});
            if out["error"].is_string() {
                self.report.edits_failed += 1;
            } else if out["vault"]["added"] == true {
                self.report.edits += 1;
            }
            if out["warning"].is_string() {
                self.report.edits_reported += 1;
            }
            kept.push(out);
        }
        line["darktable"] = json!(kept);
        Ok(())
    }

    /// Keeps the edits an importer listed for the file at `path` (see
    /// [`Metadata::edits`]) as versions of its image `id`: a history
    /// goes through the same darktable mapping a sidecar does
    /// ([`helios_xmp::Sidecar::from_history`]), an `xmp` path is read as
    /// the sidecar it is. Each edit's outcome, with where it came from
    /// and why, under the line's `darktable`.
    fn keep_listed_edits(&mut self, path: &Path, id: ImageId, edits: &[Value], line: &mut Value) {
        let mut kept = Vec::with_capacity(edits.len());
        for edit in edits {
            let version = edit["version"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0);
            let base = version == 0;
            let mut out = if let Some(xmp) = edit["xmp"].as_str() {
                let file = helios_xmp::files::SidecarFile {
                    path: PathBuf::from(xmp),
                    version,
                    exact: true,
                };
                darktable::keep_file_of(self.vault, id, path, &file, base, self.now, &mut |_, _| {})
            } else {
                let mut out = json!({
                    "path": format!("{} (darktable library, version {version})", path.display()),
                    "version": version,
                });
                let kept =
                    helios_xmp::Sidecar::from_history(&edit["history"]).and_then(|sidecar| {
                        let imported = darktable::import_for(&sidecar, path);
                        if let Some(warning) = imported.report.summary() {
                            out["warning"] = json!(warning.english);
                            out["warning_message"] = json!(warning);
                        }
                        darktable::keep_sidecar_of(
                            self.vault, id, version, base, &sidecar, &imported, self.now,
                        )
                    });
                match kept {
                    Ok(vault) => out["vault"] = vault,
                    Err(e) => out["error"] = json!(e),
                }
                out
            };
            out["from"] = edit["from"].clone();
            out["why"] = edit["why"].clone();
            if out["error"].is_string() {
                self.report.edits_failed += 1;
            } else if out["vault"]["added"] == true {
                self.report.edits += 1;
            }
            if out["warning"].is_string() {
                self.report.edits_reported += 1;
            }
            kept.push(out);
        }
        line["darktable"] = json!(kept);
    }
}

/// Imports the image files of `sources` (folders, cards or files) into
/// `vault`, as a job reported to `jobs`; one JSON line per file goes to
/// `emit` (`path`, `id`, `status`, and `duplicate`, `to`, `sidecars`,
/// `signals` or `error` when they apply). A file that fails is reported
/// and the others go on; the import fails only when it cannot start.
/// Cancelled, it stops before its next file (a copy under way is
/// dropped, its part removed) and its report says `cancelled`.
pub fn import<S: Store>(
    vault: &Vault<S>,
    jobs: &Jobs,
    sources: &[PathBuf],
    options: &Options,
    now: u64,
    emit: &mut impl FnMut(&Value),
) -> Result<Report, String> {
    let ended = run_import(vault, jobs, sources, options, now, emit);
    // Each import leaves its record, whoever ran it; a record that cannot
    // be written does not undo the import, it is said.
    let outcome = ended.as_ref().map_err(String::as_str);
    if let Err(e) = crate::import_history::record(vault, sources, options, now, outcome) {
        log::warn!("import history: {e}");
    }
    ended
}

/// [`import`], before its record.
fn run_import<S: Store>(
    vault: &Vault<S>,
    jobs: &Jobs,
    sources: &[PathBuf],
    options: &Options,
    now: u64,
    emit: &mut impl FnMut(&Value),
) -> Result<Report, String> {
    let title = format!(
        "Import {}",
        sources
            .iter()
            .map(|s| s.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let id = format!("import-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let prepared = prepare(vault, sources, options);
    let (root, sources) = match prepared {
        Ok(p) => p,
        Err(e) => {
            jobs.start(Start::new(&id, &title, KIND)).failed(&e);
            return Err(e);
        }
    };
    let files = Files(sources);
    let total = files.len() as u64;
    let job = jobs.start(Start::new(&id, &title, KIND).total(total));
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        job.on_cancel(move || stop.store(true, Ordering::SeqCst));
    }
    let staging = match &root {
        Some(root) => {
            let dir = root
                .join(".helios-part")
                .join(format!("{}-{id}", std::process::id()));
            if let Err(e) = std::fs::create_dir_all(&dir) {
                let e = format!("{}: {e}", dir.display());
                job.failed(&e);
                return Err(e);
            }
            Some(dir)
        }
        None => None,
    };
    let mut run = Run {
        vault,
        options,
        root,
        staging,
        now,
        seen: HashMap::new(),
        folders: BTreeMap::new(),
        hashes: HashMap::new(),
        placed: 0,
        report: Report::default(),
        stop,
        touched: Vec::new(),
        by: id.clone(),
    };
    for (done, (source, path)) in files.iter().enumerate() {
        if run.stopped() {
            break;
        }
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into(),
        );
        job.progress(
            Progress::new()
                .counts(done as u64, total)
                .item(&name)
                .failed(run.report.failed)
                .skipped(run.report.skipped + run.report.asked),
        );
        let mut outcome = Err(String::new());
        while !run.stopped() {
            if !source.present() {
                // The card or folder went away: wait for it rather than
                // fail every file left.
                job.paused(&source.lost());
                while !source.present() && !run.stopped() {
                    std::thread::sleep(POLL);
                }
                if run.stopped() {
                    break;
                }
                job.resumed();
                job.log(format!("{} is back", source.label()));
            }
            outcome = run.file(path);
            // A file that failed as its source went away is tried again.
            if outcome.is_ok() || source.present() {
                break;
            }
        }
        if run.stopped() && outcome.is_err() {
            // Stopped during this file: it is not imported, its copy
            // removed.
            emit(&json!({ "path": path, "status": "cancelled" }));
            break;
        }
        let (mut line, status) =
            outcome.unwrap_or_else(|e| (json!({ "path": path, "error": e }), Status::Failed));
        line["status"] = json!(status);
        run.report.count(status);
        match status {
            Status::Failed => job.item_failed(&name, line["error"].as_str().unwrap_or("failed")),
            Status::Added | Status::Located | Status::Unchanged => {
                let said = line["status"].as_str().unwrap_or_default();
                job.log(format!("{}: {said}", path.display()));
                for edit in line["darktable"].as_array().into_iter().flatten() {
                    let sidecar = edit["path"].as_str().unwrap_or_default();
                    if let (Some(from), Some(why)) = (edit["from"].as_str(), edit["why"].as_str()) {
                        job.log(format!("{sidecar}: edit from the {from}: {why}"));
                    }
                    if let Some(e) = edit["error"].as_str() {
                        job.log(format!("{sidecar}: {e}"));
                    } else if let Some(w) = edit["warning"].as_str() {
                        job.log(format!("{sidecar}: {w}"));
                    }
                }
            }
            Status::Skipped | Status::Asked => {}
        }
        emit(&line);
    }
    if run.root.is_some() {
        for source in files.0.iter().filter(|s| s.kind == "card") {
            match run.card(source) {
                Ok(card) => {
                    job.log(card.said());
                    run.report.cards.push(card);
                }
                Err(e) => job.log(format!("{}: {e}", source.label())),
            }
        }
    }
    if let Some(staging) = &run.staging {
        // This import's own folder, then the shared one when empty.
        let _ = std::fs::remove_dir_all(staging);
        if let Some(shared) = staging.parent() {
            let _ = std::fs::remove_dir(shared);
        }
    }
    // What a cancelled import added is recorded too: its undo takes it
    // back.
    run.report.changes = run.changes();
    if run.stopped() && run.report.files < total {
        run.report.cancelled = true;
        job.log(format!(
            "cancelled after {} of {total} files",
            run.report.files
        ));
        job.cancelled();
        return Ok(run.report);
    }
    job.progress(
        Progress::new()
            .counts(total, total)
            .failed(run.report.failed)
            .skipped(run.report.skipped + run.report.asked),
    );
    job.log(format!(
        "{} added, {} located, {} unchanged, {} skipped, {} asked, {} failed",
        run.report.added,
        run.report.located,
        run.report.unchanged,
        run.report.skipped,
        run.report.asked,
        run.report.failed
    ));
    if options.darktable {
        job.log(format!(
            "{} darktable edits kept, {} with a report, {} failed",
            run.report.edits, run.report.edits_reported, run.report.edits_failed
        ));
    }
    if let Some(listed) = &options.listed {
        job.log(format!(
            "{} images took values from {}",
            run.report.metadata, listed.by
        ));
    }
    job.finished();
    Ok(run.report)
}

/// The rows an undo of an import goes back to: `before`, with what the
/// import saw kept, as facts are: the image's locations (its sightings)
/// and, for an image the import recorded, the image itself, off the
/// Library, with its measured signals. So an undo clears the Library flag
/// and takes back fields and versions; a redo sets them again.
pub(crate) fn facts_kept(
    before: Option<ImageRows>,
    after: Option<&ImageRows>,
) -> Option<ImageRows> {
    let Some(after) = after else {
        return before;
    };
    Some(match before {
        Some(mut before) => {
            before.locations.clone_from(&after.locations);
            before
        }
        None => ImageRows {
            image: after.image.clone(),
            library: LibraryFlag::default(),
            locations: after.locations.clone(),
            fields: BTreeMap::new(),
            signals: after.signals.clone(),
            versions: Vec::new(),
        },
    })
}

/// How often a paused import looks for its source again.
pub(crate) const POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// A source of an import and its files.
struct Source {
    /// As given, absolute.
    path: PathBuf,
    /// `card`, `folder` or `file` ([`source_kind`]).
    kind: &'static str,
    /// What is read: `DCIM` for a card. Gone, the source is lost.
    root: PathBuf,
    files: Vec<PathBuf>,
}

impl Source {
    /// Whether it is there to be read: a card unmounted may leave its
    /// mount point behind, never its `DCIM`.
    fn present(&self) -> bool {
        if self.kind == "file" {
            self.root.is_file()
        } else {
            self.root.is_dir()
        }
    }

    /// Its name for people: the card's label, the folder's name.
    fn label(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into(),
        )
    }

    /// Why an import waits for it.
    fn lost(&self) -> String {
        match self.kind {
            "card" => format!("Reinsert {} to go on", self.label()),
            _ => format!("{} is gone: bring it back to go on", self.path.display()),
        }
    }
}

/// The files of an import, by source.
struct Files(Vec<Source>);

impl Files {
    fn len(&self) -> usize {
        self.0.iter().map(|s| s.files.len()).sum()
    }

    /// Each file with the source it was listed from.
    fn iter(&self) -> impl Iterator<Item = (&Source, &PathBuf)> {
        self.0
            .iter()
            .flat_map(|s| s.files.iter().map(move |f| (s, f)))
    }
}

/// The library folder for copies (remembered when given) and the sources
/// with their files to import.
fn prepare<S: Store>(
    vault: &Vault<S>,
    sources: &[PathBuf],
    options: &Options,
) -> Result<(Option<PathBuf>, Vec<Source>), String> {
    if sources.is_empty() {
        return Err("nothing to import".into());
    }
    let root = match &options.placement {
        Placement::Reference => {
            if options.rename.is_some() {
                return Err("renaming needs a copy: a referenced file stays as it is".into());
            }
            None
        }
        Placement::Copy { into: Some(dir) } => {
            let dir = std::path::absolute(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            set_library_root(vault, &dir)?;
            Some(dir)
        }
        Placement::Copy { into: None } => Some(library_root(vault)?.ok_or(
            "copying needs the library folder: give it once (`--into DIR`) and it is remembered",
        )?),
    };
    let mut listed = Vec::new();
    for source in sources {
        let source =
            std::path::absolute(source).map_err(|e| format!("{}: {e}", source.display()))?;
        if !source.exists() {
            return Err(format!("{}: no such file or folder", source.display()));
        }
        let files = files_of(&source)?;
        let (kind, root) = source_kind(&source);
        listed.push(Source {
            path: source,
            kind,
            root,
            files,
        });
    }
    Ok((root, listed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("helios-import-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_copy_stopped_midway_leaves_no_part() {
        let dir = scratch("stop");
        let src = dir.join("big.raw");
        // Three chunks: stopped after the first.
        std::fs::write(&src, vec![7u8; 3 << 20]).unwrap();
        let id = hash_file(&src).unwrap();
        let part = dir.join(".big.raw.helios-part");
        let asked = std::cell::Cell::new(0);
        let stopped = || {
            asked.set(asked.get() + 1);
            asked.get() > 1
        };
        assert!(stage(&src, &part, &mut 0, &stopped).is_err());
        assert!(asked.get() > 1, "stopped while copying");
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            left,
            [std::ffi::OsString::from("big.raw")],
            "no part, no copy"
        );
        // Not stopped, the copy is whole and takes its name.
        assert_eq!(stage(&src, &part, &mut 0, &|| false).unwrap(), id);
        let dest = dir.join("copy.raw");
        assert!(settle(&part, &dest).unwrap());
        assert!(!part.exists());
        assert_eq!(hash_file(&dest).unwrap(), id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The source is read once: hashed while it is copied, the copy read
    /// back to verify it, and measured from the copy.
    #[test]
    fn a_copy_reads_its_source_once() {
        let dir = scratch("once");
        let card = dir.join("card");
        let dcim = card.join("DCIM/100CANON");
        std::fs::create_dir_all(&dcim).unwrap();
        let sizes = [(3 << 20) + 17, 1234];
        for (n, size) in sizes.iter().enumerate() {
            std::fs::write(dcim.join(format!("IMG_{n}.raw")), vec![n as u8 + 1; *size]).unwrap();
        }
        let vault = Vault::new(helios_vault::MemoryStore::default());
        let jobs = Jobs::new(Arc::default());
        let options = Options {
            placement: Placement::Copy {
                into: Some(dir.join("library")),
            },
            ..Options::default()
        };
        let report = import(&vault, &jobs, &[card], &options, 1000, &mut |_| {}).unwrap();
        assert_eq!((report.added, report.copied), (2, 2), "{report:?}");
        assert_eq!(report.read, sizes.iter().sum::<usize>() as u64, "read once");
        // The same counter sees a second read: a source hashed apart.
        let mut read = 0;
        hash_counted(&dir.join("card/DCIM/100CANON/IMG_1.raw"), &mut read).unwrap();
        assert_eq!(read, 1234);
        assert!(
            !dir.join("library/.helios-part").exists(),
            "no staging left"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
