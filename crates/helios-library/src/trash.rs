//! "Move to Trash…" and "Delete rejected…" (`docs/design/library.md`,
//! L-C4 and L8): photos' files sent to the system trash, as a journaled
//! job.
//!
//! - **Journaled.** [`start`] plans the job and keeps it as a library
//!   document (`trash/<job>`) before any file is touched: every file with
//!   its outcome, `pending` until [`work`] records it `trashed`,
//!   `failed` (with why) or `missing`. A job stopped half way (a kill, a
//!   crash) keeps its pending files; [`work`] on it again picks them up,
//!   and those that failed, so a job can be read ([`get`], [`list`]) and
//!   resumed.
//! - **Refusals.** Files on a memory card or a camera (an import's
//!   source), on a backup or archive volume, in a `Helios Backup/` or
//!   `Helios Archive/` folder, or on a volume not plugged in are never
//!   touched: they are recorded `refused` with the reason, when planned.
//! - **The Library follows.** Each trashed file's location is marked
//!   missing (a file restored from the trash is found again by a scan);
//!   a photo is taken out of the Library once a file of it was trashed
//!   and none is left pending or failed. Its backup copies stay: the
//!   backups protect what the trash does not.
//! - **Not undone by the library's history.** The system trash is the
//!   undo: its "Restore" puts a file back where it was.
//!
//! The trash is the system's ([`SystemBin`], the `trash` crate: the
//! freedesktop.org trash on Linux and the BSDs, the Recycle Bin on
//! Windows, the Finder's Trash on macOS), or a folder ([`FolderBin`]) for
//! tests and sandboxes ([`redirect`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use helios_core::jobs::{Jobs, Progress, Start};
use helios_storage::strategy::{ARCHIVE_FOLDER, BACKUP_FOLDER};
use helios_storage::{Role, Volume, registry};
use helios_vault::{Changes, ImageId, Presence, Store, Vault};
use serde::{Deserialize, Serialize};

/// The library documents' namespace of the jobs.
pub const NS: &str = "trash";

/// The kind of a trash job on the bus.
pub const KIND: &str = "trash";

/// The jobs' ids in this process, after their time: `trash-<at>-1`...
static NEXT: AtomicU64 = AtomicU64::new(1);

/// Where trashed files go.
pub trait Bin {
    /// Moves the file at `path` to the trash.
    ///
    /// # Errors
    /// The file could not be moved.
    fn put(&self, path: &Path) -> Result<(), String>;
}

/// The system trash, through the `trash` crate.
pub struct SystemBin;

impl Bin for SystemBin {
    fn put(&self, path: &Path) -> Result<(), String> {
        trash::delete(path).map_err(|e| e.to_string())
    }
}

/// A folder standing for the trash: the file is moved into it, its name
/// kept (`-1`, `-2`... when taken).
pub struct FolderBin(pub PathBuf);

impl Bin for FolderBin {
    fn put(&self, path: &Path) -> Result<(), String> {
        std::fs::create_dir_all(&self.0).map_err(|e| format!("{}: {e}", self.0.display()))?;
        let name = path
            .file_name()
            .ok_or_else(|| format!("{}: no file name", path.display()))?;
        let mut to = self.0.join(name);
        let mut n = 1;
        while to.exists() {
            let stem = path.file_stem().unwrap_or(name).to_string_lossy();
            let ext = path
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            to = self.0.join(format!("{stem}-{n}{ext}"));
            n += 1;
        }
        std::fs::rename(path, &to).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// The folder standing for the trash in this process, when one is set.
static REDIRECT: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Sends this process's trash jobs to `folder` instead of the system
/// trash (tests, sandboxes); `None` goes back to the system trash.
pub fn redirect(folder: Option<PathBuf>) {
    *REDIRECT
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = folder;
}

/// The trash of this process: the folder [`redirect`] set, else the
/// system's.
pub fn bin() -> Box<dyn Bin> {
    match REDIRECT
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        Some(folder) => Box::new(FolderBin(folder)),
        None => Box::new(SystemBin),
    }
}

/// What became of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// Not done yet.
    Pending,
    /// Moved to the trash.
    Trashed,
    /// Never touched: on a card, a backup or archive volume, or offline.
    Refused,
    /// Tried and failed; tried again when the job is resumed.
    Failed,
    /// Not there to trash.
    Missing,
}

/// One file of a job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    /// The photo (hex id).
    pub image: String,
    pub path: PathBuf,
    /// The volume's name, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    pub outcome: Outcome,
    /// Why it was refused or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A trash job, as its document keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// When it was asked (seconds since the epoch).
    pub at: u64,
    /// Who asked (the call's origin).
    pub by: String,
    /// The photos asked.
    pub images: Vec<String>,
    pub files: Vec<File>,
    /// Every file has an outcome but `failed` ones.
    #[serde(default)]
    pub done: bool,
}

impl Job {
    /// How many files have `outcome`.
    pub fn count(&self, outcome: Outcome) -> usize {
        self.files.iter().filter(|f| f.outcome == outcome).count()
    }

    /// The folders its files are in (refused ones left out), sorted.
    pub fn folders(&self) -> Vec<PathBuf> {
        let set: BTreeSet<PathBuf> = self
            .files
            .iter()
            .filter(|f| f.outcome != Outcome::Refused)
            .filter_map(|f| f.path.parent().map(Path::to_path_buf))
            .collect();
        set.into_iter().collect()
    }
}

/// Why Helios never trashes `path` on `volume`, if it does not.
fn refusal(volume: Option<&Volume>, path: &Path) -> Option<String> {
    if let Some(v) = volume {
        if v.is_card() || v.has_dcim || v.roles.contains(&Role::Ingest) {
            return Some(format!(
                "{} is a memory card: Helios never deletes from a card",
                v.name
            ));
        }
        if v.roles.contains(&Role::Backup) {
            return Some(format!("{} is a backup volume: its copies stay", v.name));
        }
        if v.roles.contains(&Role::Archive) {
            return Some(format!("{} is an archive volume: its copies stay", v.name));
        }
    }
    let kept = path.components().any(|c| {
        let c = c.as_os_str();
        c == BACKUP_FOLDER || c == ARCHIVE_FOLDER
    });
    kept.then(|| format!("{}: a backup copy, it stays", path.display()))
}

/// The files of `images`, each with its outcome when refused or missing
/// (pending else). `volumes`: the volumes known, those plugged in with
/// their mount point.
///
/// # Errors
/// The vault cannot be read.
pub fn plan<S: Store>(
    vault: &Vault<S>,
    images: &[ImageId],
    volumes: &[Volume],
) -> Result<Vec<File>, String> {
    let by_id: BTreeMap<_, _> = volumes.iter().map(|v| (v.id.clone(), v)).collect();
    let mut files = Vec::new();
    for id in images {
        for loc in vault.locations(id)? {
            let volume = loc
                .volume
                .as_ref()
                .and_then(|v| by_id.get(v).copied())
                .or_else(|| registry::holding(volumes, &loc.path));
            let mut file = File {
                image: id.hex(),
                path: loc.path.clone(),
                volume: volume.map(|v| v.name.clone()),
                outcome: Outcome::Pending,
                reason: None,
            };
            if let Some(why) = refusal(volume, &loc.path) {
                file.outcome = Outcome::Refused;
                file.reason = Some(why);
            } else if loc.presence == Presence::Offline {
                file.outcome = Outcome::Refused;
                file.reason = Some(format!(
                    "{} is not plugged in",
                    file.volume.as_deref().unwrap_or("its volume")
                ));
            } else if loc.presence == Presence::Missing {
                file.outcome = Outcome::Missing;
            }
            files.push(file);
        }
    }
    Ok(files)
}

/// The volumes the vault knows, those plugged in with their mount point.
///
/// # Errors
/// The vault cannot be read.
pub fn volumes<S: Store>(vault: &Vault<S>) -> Result<Vec<Volume>, String> {
    registry::merged(vault, &helios_storage::system().volumes())
}

fn save<S: Store>(vault: &Vault<S>, job: &Job) -> Result<(), String> {
    let doc = serde_json::to_value(job).map_err(|e| e.to_string())?;
    vault.set_library_document(NS, &job.id, Some(&doc))
}

/// Plans a job over `images` and keeps it, nothing touched yet.
///
/// # Errors
/// No photo, or the vault cannot be read or written.
pub fn start<S: Store>(
    vault: &Vault<S>,
    images: &[ImageId],
    volumes: &[Volume],
    by: &str,
    now: u64,
) -> Result<Job, String> {
    if images.is_empty() {
        return Err("no photo to move to the trash".into());
    }
    let job = Job {
        id: format!("trash-{now}-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        at: now,
        by: by.into(),
        images: images.iter().map(ImageId::hex).collect(),
        files: plan(vault, images, volumes)?,
        done: false,
    };
    save(vault, &job)?;
    Ok(job)
}

/// A job kept, by id.
///
/// # Errors
/// The vault cannot be read, or the document is not a job.
pub fn get<S: Store>(vault: &Vault<S>, id: &str) -> Result<Option<Job>, String> {
    vault
        .library_document(NS, id)?
        .map(|doc| serde_json::from_value(doc).map_err(|e| format!("trash job {id}: {e}")))
        .transpose()
}

/// Every job kept, the newest first.
///
/// # Errors
/// The vault cannot be read.
pub fn list<S: Store>(vault: &Vault<S>) -> Result<Vec<Job>, String> {
    let mut jobs: Vec<Job> = vault
        .library_documents(NS)?
        .into_iter()
        .filter_map(|(_, doc)| serde_json::from_value(doc).ok())
        .collect();
    jobs.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
    Ok(jobs)
}

/// What [`work`] did.
#[derive(Clone, Debug)]
pub struct Worked {
    pub job: Job,
    /// The photos taken out of the Library (not a step of its history).
    pub changes: Changes,
}

/// Works the job `id`: each pending or failed file to `bin`, its outcome
/// recorded as it goes; then the trashed files' locations marked missing
/// and the photos left with nothing to do taken out of the Library.
/// Announced on `jobs` as a job of [`KIND`].
///
/// # Errors
/// No such job, or the vault cannot be read or written.
pub fn work<S: Store>(
    vault: &Vault<S>,
    id: &str,
    bin: &dyn Bin,
    jobs: Option<&Jobs>,
    now: u64,
) -> Result<Worked, String> {
    let mut job = get(vault, id)?.ok_or_else(|| format!("no trash job {id}"))?;
    let todo: Vec<usize> = (0..job.files.len())
        .filter(|&i| matches!(job.files[i].outcome, Outcome::Pending | Outcome::Failed))
        .collect();
    let total = todo.len() as u64;
    let report = jobs.map(|j| {
        j.start(
            Start::new(id, &format!("Move {total} files to the trash"), KIND)
                .total(total)
                .not_cancellable(),
        )
    });
    let mut failed = 0;
    for (done, &i) in todo.iter().enumerate() {
        if let Some(r) = &report {
            r.progress(
                Progress::new()
                    .counts(done as u64, total)
                    .item(&job.files[i].path.to_string_lossy())
                    .failed(failed),
            );
        }
        let file = &mut job.files[i];
        if file.path.exists() {
            match bin.put(&file.path) {
                Ok(()) => {
                    file.outcome = Outcome::Trashed;
                    file.reason = None;
                }
                Err(e) => {
                    failed += 1;
                    if let Some(r) = &report {
                        r.item_failed(&file.path.to_string_lossy(), &e);
                    }
                    file.outcome = Outcome::Failed;
                    file.reason = Some(e);
                }
            }
        } else {
            file.outcome = Outcome::Missing;
            file.reason = None;
        }
        // Each outcome kept as it comes: a job stopped here resumes
        // after it.
        save(vault, &job)?;
    }
    // The trashed files' locations: missing now.
    let mut updates = Vec::new();
    let mut gone: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for f in job.files.iter().filter(|f| f.outcome == Outcome::Trashed) {
        gone.entry(f.image.clone())
            .or_default()
            .push(f.path.clone());
    }
    for (image, paths) in &gone {
        let id = ImageId::from_hex(image)?;
        for mut loc in vault.locations(&id)? {
            if paths.contains(&loc.path) && loc.presence != Presence::Missing {
                loc.presence = Presence::Missing;
                updates.push((id, loc.path.clone(), loc));
            }
        }
    }
    vault.update_locations(&updates)?;
    // The photos with a file trashed and nothing left to do: out of the
    // Library.
    let out: Vec<ImageId> = gone
        .keys()
        .filter(|image| {
            !job.files.iter().any(|f| {
                &&f.image == image && matches!(f.outcome, Outcome::Pending | Outcome::Failed)
            })
        })
        .map(|h| ImageId::from_hex(h))
        .collect::<Result<_, _>>()?;
    let changes = if out.is_empty() {
        Changes::default()
    } else {
        vault.set_in_library(&out, false, &job.by, now)?
    };
    job.done = job.count(Outcome::Failed) == 0;
    save(vault, &job)?;
    if let Some(r) = report {
        r.progress(Progress::new().counts(total, total).failed(failed));
        r.log(format!(
            "{} moved to the trash, {} refused, {} failed",
            job.count(Outcome::Trashed),
            job.count(Outcome::Refused),
            job.count(Outcome::Failed)
        ));
        r.finished();
    }
    Ok(Worked { job, changes })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use helios_storage::{Identity, VolumeId, VolumeKind};
    use helios_vault::Location;
    use helios_vault_sqlite::SqliteStore;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "helios-trash-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A photo whose file is at `path`, in the Library.
    fn photo(vault: &Vault<SqliteStore>, path: &Path, volume: Option<&str>) -> ImageId {
        std::fs::write(path, path.to_string_lossy().as_bytes()).unwrap();
        let id = vault.import_file(path, 0).unwrap();
        if let Some(v) = volume {
            let mut loc: Location = vault.locations(&id).unwrap().remove(0);
            loc.volume = Some(VolumeId(v.into()));
            vault
                .update_locations(&[(id, loc.path.clone(), loc)])
                .unwrap();
        }
        assert!(vault.in_library(&id).unwrap());
        id
    }

    fn backup_volume(mount: &Path) -> Volume {
        let mut v = Volume::remote("x", "y", "BACKUP_A");
        v.id = VolumeId("backup-a".into());
        v.identified_by = Identity::Marker;
        v.kind = VolumeKind::Removable;
        v.roles = [Role::Backup].into();
        v.mount = Some(mount.to_path_buf());
        v
    }

    struct Failing;
    impl Bin for Failing {
        fn put(&self, _: &Path) -> Result<(), String> {
            Err("device busy".into())
        }
    }

    #[test]
    fn a_job_trashes_refuses_on_backups_records_each_file_and_resumes() {
        let root = temp("job");
        let (lib, backup, bin) = (root.join("lib"), root.join("backup"), root.join("bin"));
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        let vault = Arc::new(Vault::new(SqliteStore::open(&root.join("v.db")).unwrap()));
        let a = photo(&vault, &lib.join("a.jpg"), None);
        let b = photo(&vault, &lib.join("b.jpg"), None);
        let c = photo(&vault, &backup.join("c.jpg"), Some("backup-a"));
        let volumes = [backup_volume(&backup)];

        // Planned and kept before any file moves: the backup's refused.
        let job = start(&vault, &[a, b, c], &volumes, "user", 10).unwrap();
        assert_eq!(job.count(Outcome::Pending), 2);
        let refused = &job.files[2];
        assert_eq!(refused.outcome, Outcome::Refused);
        assert_eq!(
            refused.reason.as_deref(),
            Some("BACKUP_A is a backup volume: its copies stay")
        );
        assert_eq!(get(&vault, &job.id).unwrap().as_ref(), Some(&job));
        assert!(lib.join("a.jpg").exists(), "nothing touched by planning");

        // A failing trash: each failure recorded, nothing taken out.
        let worked = work(&*vault, &job.id, &Failing, None, 11).unwrap();
        assert_eq!(worked.job.count(Outcome::Failed), 2);
        assert!(!worked.job.done);
        assert!(worked.changes.is_empty());
        assert!(vault.in_library(&a).unwrap());

        // Resumed: the failed files go this time.
        let worked = work(&*vault, &job.id, &FolderBin(bin.clone()), None, 12).unwrap();
        assert_eq!(worked.job.count(Outcome::Trashed), 2);
        assert!(worked.job.done);
        assert!(!lib.join("a.jpg").exists() && bin.join("a.jpg").exists());
        assert!(bin.join("b.jpg").exists());
        assert!(backup.join("c.jpg").exists(), "the backup copy stays");
        assert!(!vault.in_library(&a).unwrap() && !vault.in_library(&b).unwrap());
        assert!(vault.in_library(&c).unwrap(), "all its files refused");
        assert_eq!(worked.changes.images.len(), 2);
        assert_eq!(vault.locations(&a).unwrap()[0].presence, Presence::Missing);
        let kept = get(&vault, &job.id).unwrap().unwrap();
        assert_eq!(kept, worked.job, "the journal says each outcome");
        assert_eq!(list(&vault).unwrap()[0].id, job.id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_job_stopped_before_its_files_resumes_them() {
        // A kill after the plan was kept: every file still pending.
        let root = temp("kill");
        let vault = Vault::new(SqliteStore::open(&root.join("v.db")).unwrap());
        let a = photo(&vault, &root.join("a.jpg"), None);
        let job = start(&vault, &[a], &[], "user", 1).unwrap();
        drop(vault);
        let vault = Vault::new(SqliteStore::open(&root.join("v.db")).unwrap());
        let pending = list(&vault).unwrap();
        assert_eq!(pending[0].count(Outcome::Pending), 1);
        let worked = work(&vault, &job.id, &FolderBin(root.join("bin")), None, 2).unwrap();
        assert_eq!(worked.job.count(Outcome::Trashed), 1);
        assert!(!vault.in_library(&a).unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cards_and_backup_folders_are_refused() {
        let mut card = Volume::remote("x", "y", "EOS_DIGITAL");
        card.kind = VolumeKind::Removable;
        card.has_dcim = true;
        assert_eq!(
            refusal(Some(&card), Path::new("/m/DCIM/a.cr2")).as_deref(),
            Some("EOS_DIGITAL is a memory card: Helios never deletes from a card")
        );
        assert!(refusal(None, Path::new("/d/Helios Backup/2024/a.cr2")).is_some());
        assert!(refusal(None, Path::new("/d/photos/a.cr2")).is_none());
    }
}
