//! Import as a library operation: files from a folder or a mounted card
//! into the vault, referenced where they are or copied into the library's
//! folder, renamed by a pattern, their sidecars kept, run as a job.
//!
//! - **Sources.** A folder and its subfolders, or a mounted card (a folder
//!   holding `DCIM`, read from there). A phone over MTP is not a source
//!   yet: mount it as a folder first.
//! - **Placement.** Referenced in place (the vault records where the file
//!   is), or copied into the library folder as `YYYY/YYYY-MM-DD/<name>`,
//!   by capture date, the copy hashed as it is written and checked against
//!   the source before it takes its name. Nothing is ever overwritten: a
//!   name taken by another file gets `-1`, `-2`... Renaming by a
//!   [`Pattern`] needs a copy: a referenced file stays as it is.
//! - **Sidecars.** A copy takes its XMP sidecars along (darktable's base
//!   and `_NN` duplicates), renamed with it.
//! - **Exact duplicates.** A file's content hash is its identity: a file
//!   whose content the library already has, or that came earlier in the
//!   same import, is a duplicate. [`Duplicates`] says what to do: skip it
//!   (the default), keep it as one more copy of the image, or ask (left
//!   out and listed, for whoever started the import to answer by importing
//!   it again with skip or keep). Nothing is ever deleted.
//! - **Signals.** Each image's near-duplicate [`Signals`] are measured and
//!   kept with it.
//! - **A job.** `job.started`, a `job.progress` per file (done, total, the
//!   file), then `job.finished` with the [`Report`] (or `job.failed` when
//!   the source cannot be listed); one JSON line per file goes to `emit`.

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::Bus;
use helios_core::job::Job;
use helios_vault::{IMAGE_EXTENSIONS, ImageId, Store, Vault};
use helios_xmp::files::Matched;
use serde::Serialize;
use serde_json::{Value, json};

use crate::rename::{Naming, Pattern};
use crate::signals::{self, Signals};

/// The kind of an import's job.
pub const KIND: &str = "library.import";

/// Where the library folder is kept (`library/settings`, key `root`).
const SETTINGS: &str = "settings";
const ROOT: &str = "root";

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
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
}

/// How an import runs.
#[derive(Clone, Debug)]
pub struct Options {
    pub placement: Placement,
    /// New names for copies.
    pub rename: Option<Pattern>,
    pub duplicates: Duplicates,
    /// Measure and keep near-duplicate signals.
    pub signals: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            placement: Placement::Reference,
            rename: None,
            duplicates: Duplicates::Skip,
            signals: true,
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
    /// Files copied into the library folder.
    pub copied: u64,
    /// Sidecars copied with them.
    pub sidecars: u64,
}

impl Report {
    fn count(&mut self, status: Status) {
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
    Ok(vault
        .library_document(SETTINGS, ROOT)?
        .and_then(|v| v["path"].as_str().map(PathBuf::from)))
}

/// Remembers the library folder copies go to.
pub fn set_library_root<S: Store>(vault: &Vault<S>, root: &Path) -> Result<(), String> {
    vault.set_library_document(SETTINGS, ROOT, Some(&json!({ "path": root })))
}

/// What a source is: `card` (a folder holding `DCIM`, read from there),
/// `folder` or `file`; and where to read.
pub fn source_kind(source: &Path) -> (&'static str, PathBuf) {
    if source.is_file() {
        return ("file", source.to_path_buf());
    }
    let dcim = source.join("DCIM");
    if dcim.is_dir() {
        ("card", dcim)
    } else {
        ("folder", source.to_path_buf())
    }
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The image files of a source, in a stable order.
pub fn files_of(source: &Path) -> Result<Vec<PathBuf>, String> {
    let (_, root) = source_kind(source);
    if root.is_file() {
        return if is_image(&root) {
            Ok(vec![root])
        } else {
            Err(format!("{}: not an image file", root.display()))
        };
    }
    let mut found = Vec::new();
    let mut dirs = vec![root];
    while let Some(dir) = dirs.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file() && is_image(&path) {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The BLAKE3 hash of a file, streamed.
pub fn hash_file(path: &Path) -> Result<ImageId, String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(ImageId(helios_core::ContentHash(
        *hasher.finalize().as_bytes(),
    )))
}

/// Whether the file at `path` holds the image `id`.
fn holds(path: &Path, id: &ImageId) -> Result<bool, String> {
    Ok(hash_file(path)? == *id)
}

/// Writes `src` to `part`, hashing what it writes: an error message when
/// what was written is not the image `id`.
fn write_hashed(src: &Path, part: &Path, id: &ImageId) -> std::io::Result<Option<String>> {
    let mut from = File::open(src)?;
    let mut to = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(part)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = from.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        to.write_all(&buffer[..n])?;
    }
    if let Ok(modified) = from.metadata().and_then(|m| m.modified()) {
        // The copy keeps the original's date; a file system that cannot
        // set it still gets the copy.
        let _ = to.set_modified(modified);
    }
    to.sync_all()?;
    Ok((*hasher.finalize().as_bytes() != id.0.0)
        .then(|| format!("{} changed while it was copied", src.display())))
}

/// Copies `src` to `dest`, hashing what it writes: the copy takes its
/// name only when it holds `id`, and never replaces a file. False when
/// `dest` was taken meanwhile (the copy is dropped).
fn copy_verified(src: &Path, dest: &Path, id: &ImageId) -> Result<bool, String> {
    let dir = dest.parent().ok_or("a copy needs a folder")?;
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("a copy needs a name")?;
    let part = dir.join(format!(".{name}.helios-part"));
    let written = write_hashed(src, &part, id);
    let fail = |e: String| {
        let _ = std::fs::remove_file(&part);
        Err(e)
    };
    match written {
        Err(e) => return fail(format!("{} to {}: {e}", src.display(), dest.display())),
        Ok(Some(e)) => return fail(e),
        Ok(None) => {}
    }
    // A hard link never replaces a file; a file system without them gets
    // a rename, after a last look.
    let placed = match std::fs::hard_link(&part, dest) {
        Ok(()) => true,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => false,
        Err(_) if dest.exists() => false,
        Err(_) => {
            return std::fs::rename(&part, dest)
                .map(|()| true)
                .map_err(|e| format!("{}: {e}", dest.display()));
        }
    };
    let _ = std::fs::remove_file(&part);
    Ok(placed)
}

/// Copies `src` into `dir` as `name.ext`, or `name-1.ext`... when the
/// name is taken by another file. The path, and whether a copy was made
/// (false when a file there already holds the image).
fn place(
    src: &Path,
    dir: &Path,
    name: &str,
    ext: &str,
    id: &ImageId,
) -> Result<(PathBuf, bool), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dot = if ext.is_empty() { "" } else { "." };
    for k in 0..10_000u32 {
        let file = if k == 0 {
            format!("{name}{dot}{ext}")
        } else {
            format!("{name}-{k}{dot}{ext}")
        };
        let dest = dir.join(file);
        if dest.exists() {
            if holds(&dest, id)? {
                return Ok((dest, false));
            }
            continue;
        }
        if copy_verified(src, &dest, id)? {
            return Ok((dest, true));
        }
        if holds(&dest, id)? {
            return Ok((dest, false));
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

/// The state of one import.
struct Run<'a, S: Store> {
    vault: &'a Vault<S>,
    options: &'a Options,
    root: Option<PathBuf>,
    now: u64,
    /// The images seen so far in this import, with the first file of each.
    seen: HashMap<ImageId, PathBuf>,
    /// Folder listings, for sidecars.
    folders: BTreeMap<PathBuf, Matched>,
    /// Copies made so far, for `{seq}`.
    placed: u64,
    report: Report,
}

impl<S: Store> Run<'_, S> {
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

    /// Imports one file: its line, and what happened.
    fn file(&mut self, path: &Path) -> Result<(Value, Status), String> {
        let id = hash_file(path)?;
        let mut line = json!({ "path": path, "id": id.hex() });
        let copying = matches!(self.options.placement, Placement::Copy { .. });
        if !copying
            && self
                .vault
                .at_path(path)?
                .is_some_and(|(known, l)| known == id && l.present)
        {
            self.vault.import_hashed(path, id, self.now)?;
            self.seen.entry(id).or_insert_with(|| path.to_path_buf());
            return Ok((line, Status::Unchanged));
        }
        let duplicate = if let Some(first) = self.seen.get(&id) {
            Some(json!({ "of": "source", "first": first }))
        } else if self.vault.image(&id)?.is_some() {
            let places: Vec<PathBuf> = self
                .vault
                .locations(&id)?
                .into_iter()
                .filter(|l| l.present)
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
        let measured = if self.options.signals || copying {
            match signals::measure(path) {
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
            let folder = match signals.date() {
                Some((y, m, d)) => root
                    .join(format!("{y:04}"))
                    .join(format!("{y:04}-{m:02}-{d:02}")),
                None => root.join("undated"),
            };
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
            let (dest, copied) = place(path, &folder, &name, ext, &id)?;
            self.placed += 1;
            if copied {
                self.report.copied += 1;
            }
            let sidecars = self.sidecars(path)?;
            let kept = copy_sidecars(&sidecars, &dest)?;
            self.report.sidecars += kept.iter().filter(|s| s["to"].is_string()).count() as u64;
            line["to"] = json!(dest);
            line["copied"] = json!(copied);
            line["sidecars"] = json!(kept);
            self.vault.import_hashed(&dest, id, self.now)?
        } else {
            let sidecars = self.sidecars(path)?;
            line["sidecars"] = json!(sidecars.iter().map(|s| &s.path).collect::<Vec<_>>());
            self.vault.import_hashed(path, id, self.now)?
        };
        if self.options.signals
            && let Some(s) = &measured
        {
            let doc = serde_json::to_value(s).expect("signals serialize");
            self.vault.set_signals(&id, Some(&doc))?;
            line["signals"] = doc;
        }
        Ok((line, if new { Status::Added } else { Status::Located }))
    }
}

/// Imports the image files of `sources` (folders, cards or files) into
/// `vault`, as a job announced on `bus`; one JSON line per file goes to
/// `emit` (`path`, `id`, `status`, and `duplicate`, `to`, `sidecars`,
/// `signals` or `error` when they apply). A file that fails is reported
/// and the others go on; the import fails only when it cannot start.
pub fn import<S: Store>(
    vault: &Vault<S>,
    bus: Option<Arc<Bus>>,
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
    let prepared = prepare(vault, sources, options);
    let (root, files) = match prepared {
        Ok(p) => p,
        Err(e) => {
            Job::start(bus, KIND, &title, None).fail(&e);
            return Err(e);
        }
    };
    let job = Job::start(bus, KIND, &title, Some(files.len() as u64));
    let mut run = Run {
        vault,
        options,
        root,
        now,
        seen: HashMap::new(),
        folders: BTreeMap::new(),
        placed: 0,
        report: Report::default(),
    };
    for (done, path) in files.iter().enumerate() {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        job.progress(done as u64, name.as_deref());
        let (mut line, status) = run
            .file(path)
            .unwrap_or_else(|e| (json!({ "path": path, "error": e }), Status::Failed));
        line["status"] = json!(status);
        run.report.count(status);
        emit(&line);
    }
    job.progress(files.len() as u64, None);
    job.finish(json!(run.report));
    Ok(run.report)
}

/// The library folder for copies (remembered when given) and the files
/// to import.
fn prepare<S: Store>(
    vault: &Vault<S>,
    sources: &[PathBuf],
    options: &Options,
) -> Result<(Option<PathBuf>, Vec<PathBuf>), String> {
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
    let mut files = Vec::new();
    for source in sources {
        let source =
            std::path::absolute(source).map_err(|e| format!("{}: {e}", source.display()))?;
        if !source.exists() {
            return Err(format!("{}: no such file or folder", source.display()));
        }
        files.extend(files_of(&source)?);
    }
    Ok((root, files))
}
