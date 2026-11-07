//! The `backup` job: verified copies of library photos on the backup
//! volumes of the library's strategy (`docs/storage-backup-research.md`,
//! section 3; `helios_storage::strategy`).
//!
//! - **From the library copy.** Each image is copied from its present copy
//!   in the library folder (or, for a referenced photo, on a library
//!   volume), never from a card, to `Helios Backup/` on the destination
//!   plus the photo's path in the library.
//! - **Verified.** The copy is written to a hidden part file with its hash
//!   computed while copying, read back and checked, then given its name.
//!   Nothing is overwritten: a name taken by another file gets `-1`,
//!   `-2`...; a file there already holding the image is verified and kept.
//!   The copy is recorded as a location (the volume, its path inside it,
//!   present, verified now) and as verified by hash, which is what makes it
//!   count.
//! - **Write permission.** Only to a volume with a backup, archive or
//!   scratch role, never a card or an ingest volume, and only inside
//!   `Helios Backup/` (`helios_storage::strategy::may_write`).
//! - **Pause.** A destination unplugged mid-run pauses the job ("Plug in
//!   `BACKUP_A` to go on") until it is back, at whatever mount point; it can
//!   still be cancelled, a copy under way removed.
//! - **Triggers.** [`run`] backs up what is below the strategy to every
//!   destination plugged in, when the strategy runs on that trigger: after
//!   an import ([`after_import`]), when a destination is plugged in, by
//!   hand (`library.backupNow`). A [`Gate`] keeps one run at a time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use helios_core::jobs::{Jobs, Progress, Start};
use helios_storage::strategy::{self, Scope, Strategy, Trigger};
use helios_storage::{Role, Volume, VolumeId, Volumes, registry};
use helios_vault::{ImageId, Location, Presence, Store, Vault};
use serde::Serialize;

use crate::import::{self, POLL, Part};

/// The kind of a backup's job.
pub const KIND: &str = "backup";

/// The backups' job ids in this process: `backup-1`, `backup-2`...
static NEXT: AtomicU64 = AtomicU64::new(1);

/// What one backup job did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// The destination volume.
    pub destination: Option<VolumeId>,
    /// Images gone through.
    pub images: u64,
    /// New copies written and verified.
    pub copied: u64,
    /// Copies found there already, verified and recorded.
    pub verified: u64,
    /// Images that did not need a copy there.
    pub skipped: u64,
    pub failed: u64,
    pub cancelled: bool,
    /// Bytes read from library copies.
    pub read: u64,
}

/// Keeps one backup run at a time: a run asked while one goes on is
/// dropped (the one going on catches up).
#[derive(Default)]
pub struct Gate(AtomicBool);

impl Gate {
    /// An open gate.
    pub const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    /// Runs `f` unless a run holds the gate.
    pub fn run<T>(&self, f: impl FnOnce() -> T) -> Option<T> {
        struct Open<'a>(&'a AtomicBool);
        impl Drop for Open<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        if self.0.swap(true, Ordering::SeqCst) {
            return None;
        }
        let _open = Open(&self.0);
        Some(f())
    }
}

/// What happened to one image.
enum Done {
    Copied,
    Verified,
    Skipped,
}

/// Where a photo's library copy is: its present file and its path in the
/// library (what the backup copy is placed under).
fn library_copy(
    locations: &[Location],
    root: Option<&Path>,
    volumes: &[Volume],
) -> Option<(PathBuf, PathBuf)> {
    let present = || locations.iter().filter(|l| l.present() && l.path.is_file());
    if let Some(root) = root
        && let Some(l) = present().find(|l| l.path.starts_with(root))
    {
        let rel = l.path.strip_prefix(root).ok()?.to_path_buf();
        return Some((l.path.clone(), rel));
    }
    // A referenced photo: its copy on a library volume, never a card's.
    present().find_map(|l| {
        let volume = volumes.iter().find(|v| Some(&v.id) == l.volume.as_ref())?;
        let library = volume.roles.contains(&Role::Library)
            && !volume.is_card()
            && !volume.roles.contains(&Role::Ingest);
        library.then(|| Some((l.path.clone(), l.rel_path.clone()?)))?
    })
}

/// Whether one of `locations` is a verified copy on `dest`'s drive.
fn on_drive(
    locations: &[Location],
    volumes: &[Volume],
    verified: Option<&BTreeMap<String, u64>>,
    dest: &Volume,
) -> bool {
    let drive = strategy::drive_of(dest);
    locations.iter().any(|l| {
        let (Some(id), Some(rel)) = (&l.volume, &l.rel_path) else {
            return false;
        };
        l.presence != Presence::Missing
            && volumes
                .iter()
                .any(|v| &v.id == id && strategy::drive_of(v) == drive)
            && verified.is_some_and(|v| v.contains_key(&strategy::copy_key(id, rel)))
    })
}

/// The images `strategy` still needs a copy of on `dest`: below it, and
/// with no verified copy on `dest`'s drive yet.
pub fn pending<S: Store>(
    vault: &Vault<S>,
    volumes: &[Volume],
    strategy: &Strategy,
    dest: &Volume,
) -> Result<Vec<ImageId>, String> {
    let verified = strategy::verified(vault)?;
    Ok(strategy::statuses(vault, volumes, Some(strategy))?
        .into_iter()
        .filter(|(id, (status, locations))| {
            strategy.needs(status, dest) && !on_drive(locations, volumes, verified.get(id), dest)
        })
        .map(|(id, _)| id)
        .collect())
}

/// The destination as plugged in now: `None` while it is not.
fn plugged(mounted: &dyn Volumes, dest: &VolumeId) -> Option<PathBuf> {
    mounted
        .volumes()
        .into_iter()
        .find(|v| &v.id == dest)
        .and_then(|v| v.mount)
}

fn modified_secs(path: &Path) -> Result<(u64, u64), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Ok((meta.len(), modified))
}

/// The state of one backup.
struct Run<'a, S: Store> {
    vault: &'a Vault<S>,
    dest: Volume,
    root: Option<PathBuf>,
    strategy: Option<Strategy>,
    now: u64,
    stop: Arc<AtomicBool>,
    cancel: Option<Arc<AtomicBool>>,
    job: String,
    report: Report,
}

impl<S: Store> Run<'_, S> {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
            || self
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::SeqCst))
    }

    /// Copies one image to the destination mounted at `mount`.
    fn image(&mut self, id: &ImageId, mount: &Path) -> Result<Done, String> {
        let volumes = registry::known(self.vault)?;
        let locations = self.vault.locations(id)?;
        let verified = strategy::verified(self.vault)?;
        let dest = Volume {
            mount: Some(mount.to_path_buf()),
            ..self.dest.clone()
        };
        if on_drive(&locations, &volumes, verified.get(id), &dest) {
            return Ok(Done::Skipped);
        }
        if let Some(s) = &self.strategy {
            let status = strategy::status(&locations, &volumes, verified.get(id), Some(s));
            if !s.needs(&status, &dest) {
                return Ok(Done::Skipped);
            }
        }
        let (src, in_library) = library_copy(&locations, self.root.as_deref(), &volumes)
            .ok_or_else(|| format!("{}: no library copy at hand", id.hex()))?;
        let target = strategy::backup_path(mount, &in_library);
        strategy::may_write(&dest, &target)?;
        let dir = target.parent().ok_or("a copy needs a folder")?;
        let staging = mount
            .join(strategy::BACKUP_FOLDER)
            .join(".helios-part")
            .join(format!("{}-{}", std::process::id(), self.job));
        strategy::may_write(&dest, &staging)?;
        std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
        let name = target.file_name().ok_or("a copy needs a name")?;
        let part = Part(staging.join(name));
        let (stop, cancel) = (self.stop.clone(), self.cancel.clone());
        let stopped = move || {
            stop.load(Ordering::SeqCst) || cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst))
        };
        let read = import::stage(&src, &part.0, &mut self.report.read, &stopped)?;
        if read != *id {
            return Err(format!(
                "{}: the library copy no longer matches its hash",
                src.display()
            ));
        }
        let stem = target
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("image");
        let ext = target.extension().and_then(|e| e.to_str()).unwrap_or("");
        let (placed, copied) = import::place(&part.0, dir, stem, ext, id)?;
        let rel = placed
            .strip_prefix(mount)
            .map_err(|_| format!("{}: outside its volume", placed.display()))?
            .to_path_buf();
        let (size, modified) = modified_secs(&placed)?;
        let location = Location {
            path: placed.clone(),
            size,
            modified,
            last_verified: self.now,
            volume: Some(self.dest.id.clone()),
            rel_path: Some(rel.clone()),
            presence: Presence::Present,
        };
        self.vault.update_locations(&[(*id, placed, location)])?;
        strategy::record_verified(self.vault, id, &self.dest.id, &rel, self.now)?;
        Ok(if copied { Done::Copied } else { Done::Verified })
    }
}

/// Backs up `images` to the volume `destination`, as a job on `jobs`
/// (kind `backup`); `mounted` says what is plugged in now. Cancelled (the
/// job's cancel or `cancel`), it stops before its next image.
pub fn backup<S: Store>(
    vault: &Vault<S>,
    jobs: &Jobs,
    mounted: &dyn Volumes,
    destination: &VolumeId,
    images: &[ImageId],
    now: u64,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Report, String> {
    let id = format!("backup-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let known = registry::merged(vault, &mounted.volumes())?;
    let title = |name: &str| format!("Back up to {name}");
    let checked = known
        .iter()
        .find(|v| &v.id == destination)
        .ok_or_else(|| format!("no volume {destination}"))
        .and_then(|dest| {
            strategy::may_back_up_to(dest)?;
            if dest.identified_by == helios_storage::Identity::Weak {
                return Err(format!(
                    "{}: known only by its label and size, a copy there would not count (mark the disk first)",
                    dest.name
                ));
            }
            Ok(dest.clone())
        });
    let dest = match checked {
        Ok(d) => d,
        Err(e) => {
            jobs.start(Start::new(&id, &title(destination.as_str()), KIND))
                .failed(&e);
            return Err(e);
        }
    };
    let total = images.len() as u64;
    let job = jobs.start(Start::new(&id, &title(&dest.name), KIND).total(total));
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        job.on_cancel(move || stop.store(true, Ordering::SeqCst));
    }
    let mut run = Run {
        vault,
        root: import::library_root(vault)?,
        strategy: strategy::load(vault, &Scope::Library)?,
        dest,
        now,
        stop,
        cancel,
        job: id.clone(),
        report: Report {
            destination: Some(destination.clone()),
            ..Report::default()
        },
    };
    let mut last_mount = None;
    for (done, image) in images.iter().enumerate() {
        if run.stopped() {
            break;
        }
        job.progress(
            Progress::new()
                .counts(done as u64, total)
                .item(&image.hex()[..12])
                .failed(run.report.failed)
                .skipped(run.report.skipped),
        );
        let mut outcome = Err(String::new());
        while !run.stopped() {
            let mut mount = plugged(mounted, destination);
            if mount.is_none() {
                // The destination went away: wait for it rather than fail
                // every image left.
                job.paused(&format!("Plug in {} to go on", run.dest.name));
                while mount.is_none() && !run.stopped() {
                    std::thread::sleep(POLL);
                    mount = plugged(mounted, destination);
                }
                if run.stopped() {
                    break;
                }
                job.resumed();
                job.log(format!("{} is back", run.dest.name));
            }
            let Some(at) = mount else { break };
            outcome = run.image(image, &at);
            last_mount = Some(at);
            // An image that failed as the destination went away is tried
            // again once it is back.
            if outcome.is_ok() || plugged(mounted, destination).is_some() {
                break;
            }
        }
        if run.stopped() && outcome.is_err() {
            break;
        }
        run.report.images += 1;
        match outcome {
            Ok(Done::Copied) => run.report.copied += 1,
            Ok(Done::Verified) => run.report.verified += 1,
            Ok(Done::Skipped) => run.report.skipped += 1,
            Err(e) => {
                run.report.failed += 1;
                job.item_failed(&image.hex(), &e);
            }
        }
    }
    if let Some(mount) = last_mount {
        let staging = mount.join(strategy::BACKUP_FOLDER).join(".helios-part");
        let _ = std::fs::remove_dir_all(staging.join(format!("{}-{id}", std::process::id())));
        let _ = std::fs::remove_dir(staging);
    }
    if run.stopped() && run.report.images < total {
        run.report.cancelled = true;
        job.log(format!(
            "cancelled after {} of {total} photos",
            run.report.images
        ));
        job.cancelled();
        return Ok(run.report);
    }
    job.progress(
        Progress::new()
            .counts(total, total)
            .failed(run.report.failed)
            .skipped(run.report.skipped),
    );
    job.log(format!(
        "{} copied, {} found there and verified, {} not needed, {} failed",
        run.report.copied, run.report.verified, run.report.skipped, run.report.failed
    ));
    job.finished();
    Ok(run.report)
}

/// Backs up what is below the library's strategy, on `trigger`, to each
/// destination plugged in: one job per destination with photos to copy.
/// Nothing runs without a strategy, or when it does not run on `trigger`;
/// by hand, those are errors to show.
pub fn run<S: Store>(
    vault: &Vault<S>,
    jobs: &Jobs,
    mounted: &dyn Volumes,
    trigger: Trigger,
    now: u64,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Vec<Report>, String> {
    let by_hand = trigger == Trigger::ByHand;
    let Some(strategy) = strategy::load(vault, &Scope::Library)? else {
        return if by_hand {
            Err("no backup strategy set: choose one on the Storage page".into())
        } else {
            Ok(Vec::new())
        };
    };
    if !strategy.runs_on(trigger) {
        return Ok(Vec::new());
    }
    let volumes = registry::merged(vault, &mounted.volumes())?;
    let dests = strategy::destinations_online(&strategy, &volumes);
    if dests.is_empty() && by_hand {
        return Err("no backup disk of the strategy is plugged in".into());
    }
    let mut reports = Vec::new();
    for dest in dests {
        let todo = pending(vault, &volumes, &strategy, &dest)?;
        if todo.is_empty() {
            continue;
        }
        reports.push(backup(
            vault,
            jobs,
            mounted,
            &dest.id,
            &todo,
            now,
            cancel.clone(),
        )?);
        if reports.last().is_some_and(|r| r.cancelled) {
            break;
        }
    }
    Ok(reports)
}

/// After an import: backs up when the strategy runs on import and the
/// import brought photos in (not when it was cancelled).
pub fn after_import<S: Store>(
    vault: &Vault<S>,
    jobs: &Jobs,
    mounted: &dyn Volumes,
    imported: &import::Report,
    now: u64,
) -> Result<Vec<Report>, String> {
    if imported.cancelled || imported.added + imported.located == 0 {
        return Ok(Vec::new());
    }
    run(vault, jobs, mounted, Trigger::OnImport, now, None)
}
