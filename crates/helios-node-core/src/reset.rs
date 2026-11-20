//! Resetting Helios: its own folders put aside or removed, never a photo.
//!
//! The reset works on a fixed list of what Helios keeps outside the
//! photos ([`What`]): its configuration folder (settings, workspaces,
//! machine settings, AI settings), its data folder (the default library,
//! its snapshots, thumbnails and image map), its state folder (logs,
//! crash reports), its cache folder, and the library file in use with the
//! files Helios keeps beside it (`-wal`, `-shm`, `.snapshots`, `.restore`
//! and the `.helios` folder). Where each one is comes from its owner's
//! function, handed in as [`Places`]; nothing else is ever a target.
//!
//! A [`Scope`] picks what goes: everything (the next launch is a first
//! launch), the caches only, or the settings only. A [`plan`] lists every
//! [`Target`] and its [`Fate`] before anything moves, so a dialog shows
//! it and a confirmation runs exactly it ([`run`]):
//!
//! - everything but the caches is **moved** into a dated backup folder
//!   beside the data folder (`helios-reset-<stamp>`), so a reset can be
//!   undone by hand; only the caches are **removed**;
//! - the plan is **refused**, before anything is touched, when a photo
//!   folder the caller names (the library's folders, the import
//!   destinations) is one of the targets or inside one, when a Helios
//!   folder is not named `helios`, when a path is relative, or when a
//!   removal would take a folder the scope keeps.
//!
//! The photo files and their sidecars are never read, moved or removed:
//! the tests compare their hashes before and after.
//!
//! An "everything" reset records the photo folders it was checked against
//! in its backup folder ([`REIMPORT_FILE`]): the first-run setup that
//! follows reads them back ([`pending_reimport`]) and offers to add the
//! photos again where they are, nothing copied or moved; once offered,
//! the record is set aside ([`reimport_offered`]) so it is offered once.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// The name every Helios folder ends with (`~/.config/helios`, ...).
pub const FOLDER_NAME: &str = "helios";

/// The prefix of the backup folder a reset moves Helios's data into.
pub const BACKUP_PREFIX: &str = "helios-reset-";

/// The file, in an "everything" reset's backup folder, that lists the
/// photo folders to offer for a re-import in place.
pub const REIMPORT_FILE: &str = "reimport.json";

/// [`REIMPORT_FILE`] once the re-import was offered.
pub const REIMPORT_OFFERED: &str = "reimport-offered.json";

/// What a reset takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Everything Helios keeps: the next launch is a first launch.
    Everything,
    /// The caches only (thumbnails, extractor and preview caches).
    Caches,
    /// The settings only (the configuration folder); the library stays.
    Settings,
}

impl Scope {
    /// The scope named `name` (`everything`, `caches`, `settings`).
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "everything" => Some(Scope::Everything),
            "caches" => Some(Scope::Caches),
            "settings" => Some(Scope::Settings),
            _ => None,
        }
    }
}

/// One kind of thing Helios keeps outside the photos.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum What {
    /// The configuration folder: settings, workspaces, machine settings.
    Config,
    /// The data folder: the default library and what is kept beside it.
    Data,
    /// The state folder: logs and crash reports.
    State,
    /// The user cache folder.
    Cache,
    /// The library file in use, or a file Helios keeps beside it.
    Library,
    /// The thumbnails kept beside the library file (`.helios/thumbs`).
    Thumbnails,
}

/// What happens to a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fate {
    /// Moved into the backup folder.
    Backup,
    /// Removed (caches only).
    Remove,
}

/// Where Helios keeps each thing, from each one's owner (the desktop's
/// configuration, data, state and cache folders, the library opened).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Places {
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
    /// The library file in use, when one is open (`--vault` or the
    /// default one, `library.db` in the data folder).
    pub library: Option<PathBuf>,
}

/// One path a reset takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Target {
    pub what: What,
    pub path: PathBuf,
    pub fate: Fate,
}

/// What a reset will do, listed before it runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub scope: Scope,
    /// The paths taken, the removals first: a cache inside a folder that
    /// is moved goes before the folder.
    pub targets: Vec<Target>,
    /// Where the moved paths go; none when nothing is moved.
    pub backup: Option<PathBuf>,
    /// The photo folders the plan was checked against: an "everything"
    /// reset records them for the re-import ([`REIMPORT_FILE`]).
    pub photos: Vec<PathBuf>,
}

/// Why a reset is refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Places {
    /// Where Helios keeps its things on this machine, read from `env`
    /// (`HOME`, the `XDG_*` folders, `LOCALAPPDATA`), with `library` the
    /// library file in use: the same folders as the desktop's owners of
    /// each (the workspaces' configuration folder, the data folder, the
    /// state folder, the user cache folder). `None` when one cannot be
    /// told. The desktop's Danger zone and `helios --reset` both read it.
    pub fn from_env(
        env: impl Fn(&str) -> Option<String>,
        library: Option<PathBuf>,
    ) -> Option<Self> {
        let absolute = |name: &str| env(name).map(PathBuf::from).filter(|p| p.is_absolute());
        let any = |name: &str| env(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = absolute("HOME");
        // `helios_template::workspace::config_dir`.
        let config = any("XDG_CONFIG_HOME")
            .or_else(|| any("HOME").map(|h| h.join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join(FOLDER_NAME);
        // The desktop's data folder and egui-pop's state folder.
        let (data, state) = if cfg!(windows) {
            let local = absolute("LOCALAPPDATA")?;
            (local.clone(), local)
        } else if cfg!(target_os = "macos") {
            let support = home.clone()?.join("Library/Application Support");
            (support.clone(), support)
        } else {
            (
                absolute("XDG_DATA_HOME")
                    .or_else(|| home.clone().map(|h| h.join(".local/share")))?,
                absolute("XDG_STATE_HOME")
                    .or_else(|| home.clone().map(|h| h.join(".local/state")))?,
            )
        };
        // `helios_cache::user_cache_dir`.
        let cache = any("XDG_CACHE_HOME")
            .or_else(|| any("HOME").map(|h| h.join(".cache")))
            .or_else(|| any("LOCALAPPDATA"))?
            .join(FOLDER_NAME);
        Some(Places {
            config,
            data: data.join(FOLDER_NAME),
            state: state.join(FOLDER_NAME),
            cache,
            library,
        })
    }
}

/// The photos a reset keeps: the folders they are in, and how many.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Photos {
    /// The library's folders and its library folder.
    pub roots: Vec<PathBuf>,
    /// The photos of the library.
    pub count: u64,
}

/// The photos of `library.folders`'s answer (`{"folders": [{"volume",
/// "folders": {path: count}}]}`): the top folders of each volume (none
/// above them counted) and the photos of every volume; `library_folder`
/// (where imports copy to) is a root too.
pub fn photos_of(answer: &serde_json::Value, library_folder: Option<PathBuf>) -> Photos {
    let mut photos = Photos::default();
    for volume in answer["folders"].as_array().into_iter().flatten() {
        let Some(counts) = volume["folders"].as_object() else {
            continue;
        };
        // A volume's top folder counts every photo on it.
        photos.count += counts
            .values()
            .filter_map(serde_json::Value::as_u64)
            .max()
            .unwrap_or(0);
        for path in counts.keys() {
            let counted_above = Path::new(path)
                .ancestors()
                .skip(1)
                .any(|a| counts.contains_key(a.to_string_lossy().as_ref()));
            if !counted_above {
                photos.roots.push(PathBuf::from(path));
            }
        }
    }
    photos.roots.extend(library_folder);
    photos.roots.sort();
    photos.roots.dedup();
    photos
}

/// The photos of the library file at `file`, read from it directly (no
/// node: Helios is not running): its folders and its library folder,
/// as [`photos_of`] reads them. No file, no photos.
///
/// # Errors
/// The library cannot be opened or read.
pub fn library_photos(file: &Path) -> Result<Photos, String> {
    if !file.exists() {
        return Ok(Photos::default());
    }
    let store = helios_vault_sqlite::SqliteStore::open(file)
        .map_err(|e| format!("{}: {e}", file.display()))?;
    let vault = helios_vault::Vault::new(store);
    let folders = vault.folders(&helios_vault::Query::default())?;
    let answer = serde_json::json!({
        "folders": folders
            .into_iter()
            .map(|(volume, folders)| serde_json::json!({ "volume": volume.map(|v| v.0), "folders": folders }))
            .collect::<Vec<_>>(),
    });
    Ok(photos_of(&answer, vault.library_folder()?))
}

/// The files Helios keeps beside a library file, by suffix of its name.
const LIBRARY_SUFFIXES: &[&str] = &["", "-wal", "-shm", "-journal", ".snapshots", ".restore"];

/// Lists what a reset of `scope` takes from `places`, refusing it when a
/// path of `photo_roots` (the folders photos are in) is a target or
/// inside one, or when a place is not Helios's own. `stamp` names the
/// backup folder (`helios-reset-<stamp>`, a free name beside the data
/// folder).
pub fn plan(
    places: &Places,
    scope: Scope,
    photo_roots: &[PathBuf],
    stamp: &str,
) -> Result<Plan, Refused> {
    let folders = [
        (What::Config, &places.config),
        (What::Data, &places.data),
        (What::State, &places.state),
        (What::Cache, &places.cache),
    ];
    for (what, path) in folders {
        if !path.is_absolute() {
            return Err(Refused(format!(
                "the {what:?} folder is not an absolute path: {}",
                path.display()
            )));
        }
        if path.file_name().and_then(|n| n.to_str()) != Some(FOLDER_NAME) {
            return Err(Refused(format!(
                "the {what:?} folder is not Helios's own (not named `{FOLDER_NAME}`): {}",
                path.display()
            )));
        }
    }
    let library = places
        .library
        .as_ref()
        .map(|l| std::path::absolute(l).unwrap_or_else(|_| l.clone()));
    let mut targets = Vec::new();
    match scope {
        Scope::Everything => {
            targets.push(target(What::Cache, &places.cache, Fate::Remove));
            if let Some(library) = &library {
                targets.push(target(What::Thumbnails, &thumbnails(library), Fate::Remove));
            }
            targets.push(target(What::Config, &places.config, Fate::Backup));
            targets.push(target(What::Data, &places.data, Fate::Backup));
            targets.push(target(What::State, &places.state, Fate::Backup));
            if let Some(library) = &library {
                for path in beside(library) {
                    targets.push(target(What::Library, &path, Fate::Backup));
                }
            }
        }
        Scope::Caches => {
            targets.push(target(What::Cache, &places.cache, Fate::Remove));
            if let Some(library) = &library {
                targets.push(target(What::Thumbnails, &thumbnails(library), Fate::Remove));
            }
        }
        Scope::Settings => targets.push(target(What::Config, &places.config, Fate::Backup)),
    }
    // What the scope keeps: a removal never takes one of them.
    let mut kept: Vec<&Path> = Vec::new();
    for (what, path) in folders {
        if !targets.iter().any(|t| t.what == what) {
            kept.push(path);
        }
    }
    if scope != Scope::Everything {
        kept.extend(library.as_deref());
    }
    // A removal holding a place moved too (a cache folder that is the data
    // folder, as on Windows) is a move: the backup covers it.
    let moved: Vec<PathBuf> = targets
        .iter()
        .filter(|t| t.fate == Fate::Backup)
        .map(|t| t.path.clone())
        .collect();
    targets.retain(|t| t.fate == Fate::Backup || !moved.contains(&t.path));
    for t in &mut targets {
        if t.fate == Fate::Remove && moved.iter().any(|m| m.starts_with(&t.path)) {
            t.fate = Fate::Backup;
        }
    }
    for t in &targets {
        if t.fate == Fate::Remove
            && let Some(k) = kept.iter().find(|k| k.starts_with(&t.path))
        {
            return Err(Refused(format!(
                "removing {} would take {}, which this reset keeps",
                t.path.display(),
                k.display()
            )));
        }
    }
    // A path inside another target of the same fate goes with it.
    let all = targets.clone();
    targets.retain(|t| {
        !all.iter()
            .any(|o| o.path != t.path && o.fate == t.fate && t.path.starts_with(&o.path))
    });
    let mut seen = Vec::new();
    targets.retain(|t| {
        let new = !seen.contains(&t.path);
        seen.push(t.path.clone());
        new
    });
    // The safety core: no photo folder is a target or inside one.
    for root in photo_roots {
        for t in &targets {
            if holds(&t.path, root) {
                return Err(Refused(format!(
                    "{} holds photos ({}): a reset never touches a photo folder",
                    t.path.display(),
                    root.display()
                )));
            }
        }
    }
    // Removals first: a cache inside a moved folder goes before it.
    targets.sort_by_key(|t| t.fate != Fate::Remove);
    let backup = if targets.iter().any(|t| t.fate == Fate::Backup) {
        Some(backup_folder(&places.data, stamp, &targets)?)
    } else {
        None
    };
    Ok(Plan {
        scope,
        targets,
        backup,
        photos: photo_roots.to_vec(),
    })
}

fn target(what: What, path: &Path, fate: Fate) -> Target {
    Target {
        what,
        path: path.to_path_buf(),
        fate,
    }
}

/// The thumbnails beside a library file (`helios_thumbs::cache_dir`).
fn thumbnails(library: &Path) -> PathBuf {
    helios_side(library).join("thumbs")
}

/// The `.helios` folder beside a library file.
fn helios_side(library: &Path) -> PathBuf {
    library
        .parent()
        .unwrap_or(Path::new("/"))
        .join(helios_inspect::discovery::DIR)
}

/// The library file and every file Helios keeps beside it.
fn beside(library: &Path) -> Vec<PathBuf> {
    let name = library.file_name().unwrap_or_default();
    let mut paths: Vec<PathBuf> = LIBRARY_SUFFIXES
        .iter()
        .map(|suffix| {
            let mut n = name.to_os_string();
            n.push(suffix);
            library.with_file_name(n)
        })
        .collect();
    paths.push(helios_side(library));
    paths
}

/// Whether `target` is `root` or holds it, as written or once links are
/// resolved.
fn holds(target: &Path, root: &Path) -> bool {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let (t, r) = (abs(target), abs(root));
    if r.starts_with(&t) {
        return true;
    }
    match (t.canonicalize(), r.canonicalize()) {
        (Ok(t), Ok(r)) => r.starts_with(t),
        _ => false,
    }
}

/// A free `helios-reset-<stamp>` folder beside the data folder, outside
/// every target.
fn backup_folder(data: &Path, stamp: &str, targets: &[Target]) -> Result<PathBuf, Refused> {
    let parent = data
        .parent()
        .ok_or_else(|| Refused(format!("{} has no parent folder", data.display())))?;
    let base = format!("{BACKUP_PREFIX}{stamp}");
    let mut path = parent.join(&base);
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = parent.join(format!("{base}-{n}"));
    }
    if let Some(t) = targets.iter().find(|t| path.starts_with(&t.path)) {
        return Err(Refused(format!(
            "the backup folder {} would be inside {}",
            path.display(),
            t.path.display()
        )));
    }
    Ok(path)
}

/// What a reset did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Each path moved, and where to.
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// Each path removed.
    pub removed: Vec<PathBuf>,
    /// Where the moved paths are, when any was.
    pub backup: Option<PathBuf>,
    /// The step that failed, and why; the steps before it are done, the
    /// ones after are not.
    pub failed: Option<(PathBuf, String)>,
}

/// Runs `plan`: the removals, then the moves into the backup folder. A
/// target missing on disk is skipped. Stops at the first step that
/// fails, saying which in the report.
pub fn run(plan: &Plan) -> Report {
    let mut report = Report {
        backup: plan.backup.clone(),
        ..Report::default()
    };
    for t in &plan.targets {
        if std::fs::symlink_metadata(&t.path).is_err() {
            continue;
        }
        let step = match t.fate {
            Fate::Remove => remove(&t.path).map(|()| report.removed.push(t.path.clone())),
            Fate::Backup => {
                let Some(backup) = &plan.backup else {
                    report.failed = Some((t.path.clone(), "no backup folder".into()));
                    break;
                };
                let to = backup.join(backup_name(t));
                std::fs::create_dir_all(backup)
                    .map_err(|e| format!("{}: {e}", backup.display()))
                    .and_then(|()| move_path(&t.path, &to))
                    .map(|()| report.moved.push((t.path.clone(), to)))
            }
        };
        match step {
            Ok(()) => log::info!(
                target: "helios::reset",
                "{:?} {:?}: {}",
                t.fate,
                t.what,
                t.path.display()
            ),
            Err(e) => {
                log::error!(target: "helios::reset", "{}: {e}", t.path.display());
                report.failed = Some((t.path.clone(), e));
                break;
            }
        }
    }
    if plan.scope == Scope::Everything
        && report.failed.is_none()
        && let Some(backup) = &plan.backup
        && let Err(e) = record_reimport(backup, &plan.photos)
    {
        log::warn!(target: "helios::reset", "the photo folders not recorded: {e}");
    }
    report
}

/// The re-import an "everything" reset left to offer: the record's file
/// and the photo folders it lists that are still there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reimport {
    /// The record ([`REIMPORT_FILE`] in the backup folder).
    pub record: PathBuf,
    /// The photo folders, each still a folder on disk.
    pub folders: Vec<PathBuf>,
}

/// Writes the photo folders into `backup`'s [`REIMPORT_FILE`].
fn record_reimport(backup: &Path, folders: &[PathBuf]) -> Result<(), String> {
    std::fs::create_dir_all(backup).map_err(|e| format!("{}: {e}", backup.display()))?;
    let file = backup.join(REIMPORT_FILE);
    let text = serde_json::to_string_pretty(&serde_json::json!({ "photo_folders": folders }))
        .map_err(|e| e.to_string())?;
    std::fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))
}

/// The newest re-import an "everything" reset recorded beside the data
/// folder `data` and not offered yet, its folders that are gone left
/// out; `None` when there is none, or none of its folders is left.
pub fn pending_reimport(data: &Path) -> Option<Reimport> {
    let parent = data.parent()?;
    let newest = std::fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(BACKUP_PREFIX))
        .map(|e| e.path().join(REIMPORT_FILE))
        .filter_map(|record| {
            let at = std::fs::metadata(&record).and_then(|m| m.modified()).ok()?;
            Some((at, record))
        })
        .max()?
        .1;
    let text = std::fs::read_to_string(&newest).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let folders: Vec<PathBuf> = value["photo_folders"]
        .as_array()?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect();
    (!folders.is_empty()).then_some(Reimport {
        record: newest,
        folders,
    })
}

/// Sets `record` aside as offered ([`REIMPORT_OFFERED`]): the re-import
/// is not offered again.
///
/// # Errors
/// The record cannot be renamed.
pub fn reimport_offered(record: &Path) -> Result<(), String> {
    let to = record.with_file_name(REIMPORT_OFFERED);
    std::fs::rename(record, &to).map_err(|e| format!("{}: {e}", record.display()))
}

/// A target's name in the backup folder: its kind, the library's files
/// by their own names.
fn backup_name(t: &Target) -> PathBuf {
    match t.what {
        What::Config => "config".into(),
        What::Data => "data".into(),
        What::State => "state".into(),
        What::Cache => "cache".into(),
        What::Thumbnails => "thumbnails".into(),
        What::Library => Path::new("library").join(t.path.file_name().unwrap_or_default()),
    }
}

fn remove(path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{e}"))?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .map_err(|e| format!("{e}"))
}

/// Moves `from` to `to`: a rename, else (another file system) a copy
/// then the original removed.
fn move_path(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy(from, to).map_err(|e| format!("copy to {}: {e}", to.display()))?;
    remove(from)
}

fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_symlink() {
        return copy_link(&std::fs::read_link(from)?, to);
    }
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

#[cfg(unix)]
fn copy_link(link: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(link, to)
}

/// Elsewhere a link is not copied: what it points to is not Helios's.
#[cfg(not(unix))]
fn copy_link(_link: &Path, _to: &Path) -> std::io::Result<()> {
    Ok(())
}
