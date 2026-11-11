//! Capture metadata into the vault (`docs/design/library.md`, slice L2):
//! what a file says of how it was taken, kept as the vault's read-only
//! capture fields ([`helios_vault::capture`]).
//!
//! - **At import**, each file's metadata is read (no pixel decoded) and
//!   written with its image ([`read`]).
//! - **The backfill** ([`backfill`]) fills the images imported before:
//!   a job of kind `capture` in the jobs centre that walks the Library in
//!   id order, [`BATCH`] images at a time, each batch written in one
//!   transaction with its progress, so a run stopped anywhere (cancelled,
//!   Helios closed) resumes after the last batch kept. Each image is read
//!   from its first present copy; with none at hand (an offline disk, a
//!   missing file) or one that cannot be read, from what the vault kept:
//!   the import's signals (the time, the settings, the camera) and the
//!   sightings (camera, lens, the file's time). An image that has its
//!   capture already (imported since) is passed over. Once the whole
//!   Library is done the backfill is finished and never runs again.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use helios_core::jobs::{Jobs, Progress, Start};
use helios_vault::capture::{Backfill, Capture};
use helios_vault::{ImageId, Query, Sighting, Store, Vault};
use serde_json::Value;

use crate::signals::exif_time;

/// The jobs-centre kind of the backfill.
pub const KIND: &str = "capture";
/// Images written per transaction.
pub const BATCH: usize = 256;

/// An offset as EXIF writes it (`+09:00`, `-05:30`), in minutes.
pub fn offset_minutes(text: &str) -> Option<i64> {
    let t = text.trim();
    let sign = match t.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (h, m) = t[1..].split_once(':')?;
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    ((0..=14).contains(&h) && (0..60).contains(&m)).then_some(sign * (h * 60 + m))
}

/// A file's type: its extension in lower case.
pub fn file_type(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| !e.is_empty())
}

/// The vault's capture of what a file's metadata says; with no EXIF time,
/// the file's modification time (`modified`, seconds UTC) with an offset
/// of 0.
pub fn of_file(
    path: &Path,
    file: &helios_load::capture::Capture,
    modified: Option<u64>,
) -> Capture {
    let mut c = Capture {
        make: file.make.clone(),
        model: file.model.clone(),
        lens: file.lens.clone(),
        focal_length: file.focal_length,
        aperture: file.aperture,
        shutter: file.exposure_time,
        iso: file.iso.map(|i| i.round() as i64),
        width: file.width.map(i64::from),
        height: file.height.map(i64::from),
        orientation: file.orientation.map(i64::from),
        file_type: file_type(path),
        ..Capture::default()
    };
    match file
        .taken
        .as_deref()
        .and_then(|t| exif_time(t, file.subsec.as_deref()))
    {
        Some(t) => {
            c.captured = Some(t);
            c.offset = file.offset.as_deref().and_then(offset_minutes);
            c.captured_from = Some("exif".into());
        }
        None => file_time(&mut c, modified),
    }
    c
}

fn file_time(c: &mut Capture, modified: Option<u64>) {
    if let Some(m) = modified.filter(|m| *m > 0) {
        c.captured = Some(m as f64);
        c.offset = Some(0);
        c.captured_from = Some("file".into());
    }
}

fn modified(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .as_ref()
        .map(Duration::as_secs)
}

/// Reads a file's capture: its metadata, else its modification time.
///
/// # Errors
/// The file cannot be read or is no raw or picture Helios reads.
pub fn read(path: &Path) -> Result<Capture, String> {
    let file = helios_load::capture::capture(path)?;
    Ok(of_file(path, &file, modified(path)))
}

/// What the vault kept of an image without its file: the import's
/// signals (time, settings, camera), the sightings (camera, lens, the
/// file's type and time). `None` when it kept nothing.
pub fn of_kept(signals: Option<&Value>, sightings: &[Sighting]) -> Option<Capture> {
    let s = |k: &str| {
        signals
            .and_then(|v| v.get(k))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let n = |k: &str| signals.and_then(|v| v.get(k)).and_then(Value::as_f64);
    let mut c = Capture {
        make: s("maker"),
        model: s("model"),
        aperture: n("aperture"),
        shutter: n("exposure_time"),
        iso: n("iso").map(|i| i.round() as i64),
        ..Capture::default()
    };
    let first = sightings
        .iter()
        .find(|x| x.presence == helios_vault::Presence::Present)
        .or_else(|| sightings.first());
    if c.model.is_none() {
        c.model = sightings.iter().find_map(|x| x.camera.clone());
    }
    c.lens = sightings.iter().find_map(|x| x.lens.clone());
    c.file_type = first.and_then(|x| file_type(&x.path));
    match (n("taken"), s("taken_from").as_deref()) {
        (Some(t), Some("exif")) => {
            c.captured = Some(t);
            c.captured_from = Some("exif".into());
        }
        _ => file_time(&mut c, first.map(|x| x.modified)),
    }
    (c != Capture::default()).then_some(c)
}

/// Reads a file's capture: [`read`], or another reader in tests and
/// benches.
pub trait Reader: Sync {
    fn read(&self, path: &Path) -> Result<Capture, String>;
}

/// The files themselves.
pub struct Files;

impl Reader for Files {
    fn read(&self, path: &Path) -> Result<Capture, String> {
        read(path)
    }
}

/// Fills the capture fields of the Library's images imported before
/// them, as a job of kind [`KIND`] on `jobs`, from where the last run
/// stopped; stops between two batches when cancelled (the job's cancel or
/// `cancel`), and waits while paused. Returns the progress kept. Nothing
/// runs, and no job shows, once it is finished.
pub fn backfill<S: Store>(
    vault: &Vault<S>,
    jobs: &Jobs,
    reader: &dyn Reader,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Backfill, String> {
    backfill_by(vault, jobs, reader, cancel, BATCH)
}

/// [`backfill`], `batch` images per transaction.
pub fn backfill_by<S: Store>(
    vault: &Vault<S>,
    jobs: &Jobs,
    reader: &dyn Reader,
    cancel: Option<Arc<AtomicBool>>,
    batch: usize,
) -> Result<Backfill, String> {
    let mut progress = vault.backfill()?.unwrap_or_default();
    if progress.finished {
        return Ok(progress);
    }
    let cursor = vault.open_cursor(Query::default())?;
    let ids = vault.cursor_ids(cursor.id, 0, cursor.count);
    vault.close_cursor(cursor.id);
    let after = progress
        .after
        .as_deref()
        .map(ImageId::from_hex)
        .transpose()?;
    // In id order: what is left is after the last image done.
    let left: Vec<ImageId> = ids?
        .into_iter()
        .filter(|id| after.is_none_or(|a| *id > a))
        .collect();
    progress.total = progress.done + left.len() as u64;
    let job = jobs.start(
        Start::new(
            &format!("capture-{}", std::process::id()),
            "Read capture metadata",
            KIND,
        )
        .total(progress.total)
        .pausable(),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        job.on_cancel(move || stop.store(true, Ordering::SeqCst));
        let paused = paused.clone();
        job.on_pause(move |p| paused.store(p, Ordering::SeqCst));
    }
    let stopped =
        || stop.load(Ordering::SeqCst) || cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst));
    for batch in left.chunks(batch.max(1)) {
        while paused.load(Ordering::SeqCst) && !stopped() {
            std::thread::sleep(Duration::from_millis(100));
        }
        if stopped() {
            job.log(format!(
                "stopped after {} of {} photos",
                progress.done, progress.total
            ));
            job.cancelled();
            return Ok(progress);
        }
        let had = vault.captures_of(batch)?;
        let mut captures = Vec::with_capacity(batch.len());
        for (id, had) in batch.iter().zip(had) {
            if had.is_some() {
                continue;
            }
            let sightings = vault.sightings(id)?;
            let present = sightings
                .iter()
                .find(|s| s.presence == helios_vault::Presence::Present);
            let from_file = present.map(|s| reader.read(&s.path));
            match from_file {
                Some(Ok(c)) => {
                    progress.read += 1;
                    captures.push((*id, c));
                }
                other => {
                    if let Some(Err(e)) = other {
                        job.log(format!("{}: {e}", id.hex()));
                    }
                    progress.kept += 1;
                    if let Some(c) = of_kept(vault.signals(id)?.as_ref(), &sightings) {
                        captures.push((*id, c));
                    }
                }
            }
        }
        progress.done += batch.len() as u64;
        progress.after = batch.last().map(ImageId::hex);
        vault.set_captures(&captures, Some(&progress))?;
        job.progress(Progress::new().counts(progress.done, progress.total));
    }
    progress.finished = true;
    vault.set_captures(&[], Some(&progress))?;
    job.log(format!(
        "{} photos: {} read from their files, {} from what the catalogue kept",
        progress.done, progress.read, progress.kept
    ));
    job.finished();
    Ok(progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_read_in_minutes() {
        assert_eq!(offset_minutes("+09:00"), Some(540));
        assert_eq!(offset_minutes("-05:30"), Some(-330));
        assert_eq!(offset_minutes("09:00"), None);
        assert_eq!(offset_minutes("+99:00"), None);
    }

    #[test]
    fn exif_time_wins_and_the_file_time_stands_in() {
        let file = helios_load::capture::Capture {
            taken: Some("2024:06:01 23:30:00".into()),
            offset: Some("+09:00".into()),
            iso: Some(799.6),
            ..Default::default()
        };
        let c = of_file(Path::new("/p/A.CR3"), &file, Some(5));
        assert_eq!(c.captured_from.as_deref(), Some("exif"));
        assert_eq!(c.offset, Some(540));
        assert_eq!(c.iso, Some(800));
        assert_eq!(c.file_type.as_deref(), Some("cr3"));
        assert_eq!(c.day(), Some((2024, 6, 1)));
        let none = of_file(
            Path::new("/p/b.png"),
            &helios_load::capture::Capture::default(),
            Some(86_400),
        );
        assert_eq!(none.captured, Some(86_400.0));
        assert_eq!(none.offset, Some(0));
        assert_eq!(none.captured_from.as_deref(), Some("file"));
    }
}
