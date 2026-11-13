//! Culling signals: what helps pick the keepers among near-identical
//! frames, measured on the same small working image as the other
//! [`crate::signals`] (the embedded preview, the picture, or a small decode
//! of the mosaic) and kept with them in the image's signals document.
//!
//! - **Focus:** the grey thumbnail (at most [`crate::signals::SHARP_SIDE`]
//!   pixels a side, perceptual lightness) is cut into a fixed
//!   [`GRID`] x [`GRID`] grid; each cell's energy is the mean of the
//!   squared 4-neighbour Laplacian over its pixels, a mean so it does not
//!   grow with the thumbnail's size. The focus is the cells' 90th
//!   percentile, so a sharp subject on a soft background still scores
//!   high, and `focus_at` is the centre of the sharpest cell (fractions of
//!   the width and height, the first cell in reading order on a tie).
//! - **Clipping:** over every pixel of the working image, the share at or
//!   near white ([`WHITE`]) and at or near black ([`BLACK`]), per channel
//!   (red, green, blue).
//! - **Exposure:** the mean, median and standard deviation of the grey
//!   thumbnail's lightness (0 black, 1 white).
//!
//! A one-channel image (a raw mosaic with no preview) is only measured
//! for focus: its levels are scaled to its own peak, so clipping and
//! exposure would mean nothing. Every measure is a plain sum or sort in a
//! fixed order, in `f64`: the same image gives the same numbers.
//!
//! **The backfill** ([`backfill`]) measures the images imported before
//! these signals existed: a job of kind [`KIND`] in the jobs centre that
//! walks the Library in id order, [`BATCH`] images at a time, each batch's
//! signals written in one transaction with its progress, so a run stopped
//! anywhere resumes after the last batch kept. An image whose signals have
//! the current [`VERSION`] is passed over; one with no copy at hand is
//! counted as missing. Once the whole Library is done it never runs again,
//! until [`VERSION`] changes.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use helios_core::Image;
use helios_core::jobs::{Jobs, Progress, Start};
use helios_vault::{ImageId, Presence, Query, Store, Vault};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::signals::Signals;

/// The version of these measures: a change of method bumps it, and the
/// backfill measures every image again.
pub const VERSION: u32 = 1;
/// Cells per side of the focus grid.
pub const GRID: usize = 8;
/// A linear level at or above which a channel counts as clipped white
/// (about 253 of 255 once encoded in sRGB).
pub const WHITE: f32 = 0.98;
/// A linear level at or below which a channel counts as clipped black
/// (about 2 of 255 once encoded in sRGB).
pub const BLACK: f32 = 0.0007;
/// The jobs-centre kind of the backfill.
pub const KIND: &str = "cull";
/// Images written per transaction.
pub const BATCH: usize = 64;
/// Where the backfill keeps its progress (`library/<ns>`, key).
const PROGRESS: (&str, &str) = ("cull", "backfill");

/// The culling measures of an image, as its signals keep them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cull {
    /// The [`VERSION`] they were measured with.
    #[serde(rename = "cull", default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// The 90th percentile of the focus grid's cell energies: higher is
    /// sharper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<f64>,
    /// The centre of the sharpest cell, `[x, y]` in fractions of the
    /// width and height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_at: Option<[f64; 2]>,
    /// The share of pixels at or near white, per channel `[r, g, b]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_white: Option<[f64; 3]>,
    /// The share of pixels at or near black, per channel `[r, g, b]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_black: Option<[f64; 3]>,
    /// The grey thumbnail's mean lightness, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lum_mean: Option<f64>,
    /// Its median lightness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lum_median: Option<f64>,
    /// The standard deviation of its lightness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lum_spread: Option<f64>,
}

impl Cull {
    /// The measures as signals keys, to merge into a kept document.
    pub fn keys(&self) -> serde_json::Map<String, Value> {
        match serde_json::to_value(self).expect("cull serializes") {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        }
    }

    /// Whether a kept signals document has these measures at the current
    /// [`VERSION`].
    pub fn current(signals: &Value) -> bool {
        signals.get("cull").and_then(Value::as_u64) == Some(u64::from(VERSION))
    }
}

/// Measures `image`, given its grey thumbnail (`w` x `h` lightness
/// values, as [`crate::signals::grey`] makes it).
pub fn measure(image: &Image, w: usize, h: usize, values: &[f32]) -> Cull {
    let (focus, focus_at) = focus(w, h, values);
    let mut cull = Cull {
        version: Some(VERSION),
        focus: Some(focus),
        focus_at,
        ..Cull::default()
    };
    if image.channels() >= 3 && !values.is_empty() {
        let (white, black) = clipping(image);
        cull.clip_white = Some(white);
        cull.clip_black = Some(black);
        let (mean, median, spread) = levels(values);
        cull.lum_mean = Some(mean);
        cull.lum_median = Some(median);
        cull.lum_spread = Some(spread);
    }
    cull
}

/// The focus of a grey thumbnail and where its sharpest cell is; 0 and
/// nowhere for a thumbnail too small to have an inside.
pub fn focus(w: usize, h: usize, values: &[f32]) -> (f64, Option<[f64; 2]>) {
    if w < 3 || h < 3 {
        return (0.0, None);
    }
    let at = |x: usize, y: usize| f64::from(values[y * w + x]);
    let mut sums = [0.0f64; GRID * GRID];
    let mut counts = [0u32; GRID * GRID];
    for y in 1..h - 1 {
        let cy = y * GRID / h;
        for x in 1..w - 1 {
            let l = at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1) - 4.0 * at(x, y);
            let cell = cy * GRID + x * GRID / w;
            sums[cell] += l * l;
            counts[cell] += 1;
        }
    }
    let cells: Vec<(usize, f64)> = (0..GRID * GRID)
        .filter(|&c| counts[c] > 0)
        .map(|c| (c, sums[c] / f64::from(counts[c])))
        .collect();
    let mut energies: Vec<f64> = cells.iter().map(|(_, e)| *e).collect();
    energies.sort_by(f64::total_cmp);
    let p90 = energies[(energies.len() - 1) * 9 / 10];
    let mut best = cells[0];
    for &cell in &cells[1..] {
        if cell.1 > best.1 {
            best = cell;
        }
    }
    let (cx, cy) = (best.0 % GRID, best.0 / GRID);
    let centre = |c: usize| (c as f64 + 0.5) / GRID as f64;
    (p90, Some([centre(cx), centre(cy)]))
}

/// The share of pixels at or near white and at or near black, per
/// channel, over a colour image.
pub fn clipping(image: &Image) -> ([f64; 3], [f64; 3]) {
    let channels = image.channels() as usize;
    let data = image.data();
    let pixels = data.len() / channels.max(1);
    if channels < 3 || pixels == 0 {
        return ([0.0; 3], [0.0; 3]);
    }
    let mut white = [0u64; 3];
    let mut black = [0u64; 3];
    for p in data.chunks_exact(channels) {
        for c in 0..3 {
            white[c] += u64::from(p[c] >= WHITE);
            black[c] += u64::from(p[c] <= BLACK);
        }
    }
    let share = |n: [u64; 3]| n.map(|n| n as f64 / pixels as f64);
    (share(white), share(black))
}

/// The mean, median and standard deviation of lightness values.
pub fn levels(values: &[f32]) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let n = values.len() as f64;
    let mean = values.iter().map(|v| f64::from(*v)).sum::<f64>() / n;
    let variance = values
        .iter()
        .map(|v| (f64::from(*v) - mean).powi(2))
        .sum::<f64>()
        / n;
    let mut sorted: Vec<f64> = values.iter().map(|v| f64::from(*v)).collect();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        f64::midpoint(sorted[mid - 1], sorted[mid])
    } else {
        sorted[mid]
    };
    (mean, median, variance.sqrt())
}

/// How far the backfill went: the images in the Library taken in id
/// order, the last one done, how many so far and of how many.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Backfill {
    /// The [`VERSION`] this run measures; a progress kept for another
    /// starts over.
    pub version: u32,
    /// The last image done (hex): the next run starts after it.
    pub after: Option<String>,
    pub done: u64,
    pub total: u64,
    /// Measured from a copy at hand.
    #[serde(default)]
    pub measured: u64,
    /// Passed over: no copy at hand (an offline disk, a missing file).
    #[serde(default)]
    pub missing: u64,
    /// A copy at hand that could not be measured.
    #[serde(default)]
    pub failed: u64,
    /// Every image of the Library is done.
    pub finished: bool,
    /// The run copies the measures to the vault's culling fields too
    /// (`helios_vault::cull`): a run finished before them starts over, to
    /// fill the fields of the images it measured.
    #[serde(default)]
    pub fields: bool,
}

/// Measures a file: [`crate::signals::measure`], or another measurer in
/// tests.
pub trait Measurer: Sync {
    fn measure(&self, path: &Path) -> Result<Signals, String>;
}

/// The files themselves.
pub struct Files;

impl Measurer for Files {
    fn measure(&self, path: &Path) -> Result<Signals, String> {
        crate::signals::measure(path)
    }
}

/// How far the backfill went; `None` before its first batch.
pub fn progress<S: Store>(vault: &Vault<S>) -> Result<Option<Backfill>, String> {
    vault
        .library_document(PROGRESS.0, PROGRESS.1)?
        .map(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
        .transpose()
}

/// Measures the culling signals of the Library's images that lack them,
/// as a job of kind [`KIND`] on `jobs`, from where the last run stopped;
/// stops between two batches when cancelled (the job's cancel or
/// `cancel`), and waits while paused. An image with no signals at all
/// gets every signal an import measures; one with signals gets the
/// culling keys added, the others kept. Returns the progress kept.
/// Nothing runs, and no job shows, once it is finished.
pub fn backfill<S: Store>(
    vault: &Vault<S>,
    jobs: &Jobs,
    measurer: &dyn Measurer,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Backfill, String> {
    backfill_by(vault, jobs, measurer, cancel, BATCH)
}

/// [`backfill`], `batch` images per transaction.
pub fn backfill_by<S: Store>(
    vault: &Vault<S>,
    jobs: &Jobs,
    measurer: &dyn Measurer,
    cancel: Option<Arc<AtomicBool>>,
    batch: usize,
) -> Result<Backfill, String> {
    let mut progress = progress(vault)?
        .filter(|p| p.version == VERSION && p.fields)
        .unwrap_or(Backfill {
            version: VERSION,
            fields: true,
            ..Backfill::default()
        });
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
    let mut left: Vec<ImageId> = ids?
        .into_iter()
        .filter(|id| after.is_none_or(|a| *id > a))
        .collect();
    left.sort();
    progress.total = progress.done + left.len() as u64;
    let job = jobs.start(
        Start::new(
            &format!("cull-{}", std::process::id()),
            "Measure focus, clipping and exposure",
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
        let mut merges = Vec::with_capacity(batch.len());
        for id in batch {
            let kept = vault.signals(id)?;
            if kept.as_ref().is_some_and(Cull::current) {
                // Measured before the fields existed: written again as
                // kept, which fills them.
                let filled = vault.fields_of(&[*id], &[helios_vault::cull::FOCUS])?;
                if filled.first().is_none_or(|f| f.is_empty()) {
                    merges.push((*id, serde_json::Map::new()));
                }
                continue;
            }
            let present = vault
                .sightings(id)?
                .into_iter()
                .find(|s| s.presence == Presence::Present);
            let Some(sighting) = present else {
                progress.missing += 1;
                continue;
            };
            match measurer.measure(&sighting.path) {
                Ok(signals) => {
                    progress.measured += 1;
                    let keys = if kept.is_some() {
                        signals.cull.keys()
                    } else {
                        match serde_json::to_value(&signals).expect("signals serialize") {
                            Value::Object(map) => map,
                            _ => serde_json::Map::new(),
                        }
                    };
                    merges.push((*id, keys));
                }
                Err(e) => {
                    progress.failed += 1;
                    job.log(format!("{}: {e}", id.hex()));
                }
            }
        }
        progress.done += batch.len() as u64;
        progress.after = batch.last().map(ImageId::hex);
        let doc = serde_json::to_value(&progress).map_err(|e| e.to_string())?;
        vault.merge_signals(&merges, Some((PROGRESS.0, PROGRESS.1, &doc)))?;
        job.progress(Progress::new().counts(progress.done, progress.total));
    }
    progress.finished = true;
    let doc = serde_json::to_value(&progress).map_err(|e| e.to_string())?;
    vault.merge_signals(&[], Some((PROGRESS.0, PROGRESS.1, &doc)))?;
    job.log(format!(
        "{} photos: {} measured, {} with no copy at hand, {} failed",
        progress.done, progress.measured, progress.missing, progress.failed
    ));
    job.finished();
    Ok(progress)
}

#[cfg(test)]
mod tests {
    use helios_core::Region;

    use super::*;
    use crate::signals::{SHARP_SIDE, grey};

    fn scene(x: i64, y: i64) -> f32 {
        let checker = ((x / 6 + y / 6) % 2) as f32;
        0.1 + 0.5 * checker
    }

    fn blurred(region: Region) -> Image {
        Image::from_fn(region, 3, |x, y, _| {
            let mut s = 0.0;
            for dy in -3..=3 {
                for dx in -3..=3 {
                    s += scene(
                        (x + dx).clamp(0, i64::from(region.width) - 1),
                        (y + dy).clamp(0, i64::from(region.height) - 1),
                    );
                }
            }
            s / 49.0
        })
    }

    fn cull_of(image: &Image) -> Cull {
        let (w, h, values) = grey(image, SHARP_SIDE);
        measure(image, w, h, &values)
    }

    #[test]
    fn a_sharp_frame_scores_above_its_blurred_copy() {
        let region = Region::sized(96, 64);
        let sharp = cull_of(&Image::from_fn(region, 3, |x, y, _| scene(x, y)));
        let soft = cull_of(&blurred(region));
        assert!(
            sharp.focus.unwrap() > 2.0 * soft.focus.unwrap(),
            "{sharp:?} {soft:?}"
        );
        assert_eq!(sharp.version, Some(VERSION));
    }

    #[test]
    fn the_sharpest_cell_is_found_where_the_detail_is() {
        // Flat grey, with a checker in the top right corner only.
        let region = Region::sized(128, 128);
        let image = Image::from_fn(
            region,
            3,
            |x, y, _| {
                if x >= 96 && y < 32 { scene(x, y) } else { 0.3 }
            },
        );
        let c = cull_of(&image);
        let [x, y] = c.focus_at.unwrap();
        assert!(x > 0.75 && y < 0.25, "{x} {y}");
    }

    #[test]
    fn clipped_pixels_are_counted_per_channel() {
        // A quarter of the frame blown in red only, an eighth black.
        let region = Region::sized(64, 64);
        let image = Image::from_fn(region, 3, |x, y, c| {
            if x < 32 && y < 32 {
                if c == 0 { 1.0 } else { 0.5 }
            } else if x >= 32 && y >= 48 {
                0.0
            } else {
                0.4
            }
        });
        let c = cull_of(&image);
        assert_eq!(c.clip_white, Some([0.25, 0.0, 0.0]));
        assert_eq!(c.clip_black, Some([0.125, 0.125, 0.125]));
        let mean = c.lum_mean.unwrap();
        assert!(mean > 0.0 && mean < 1.0);
        assert!(c.lum_spread.unwrap() > 0.0);
    }

    #[test]
    fn the_levels_are_mean_median_and_spread() {
        let (mean, median, spread) = levels(&[0.0, 0.25, 0.25, 1.0]);
        assert!((mean - 0.375).abs() < 1e-12);
        assert!((median - 0.25).abs() < 1e-12);
        assert!((spread - 0.375).abs() < 1e-12);
        assert_eq!(levels(&[]), (0.0, 0.0, 0.0));
    }

    #[test]
    fn the_same_image_measures_the_same_and_a_mosaic_only_for_focus() {
        let region = Region::sized(80, 60);
        let image = Image::from_fn(region, 3, |x, y, c| {
            scene(x, y) * (1.0 + 0.1 * c as f32) + 0.001 * (x * y) as f32
        });
        let a = cull_of(&image);
        let b = cull_of(&image.clone());
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
        let mosaic = cull_of(&Image::from_fn(region, 1, |x, y, _| 1000.0 * scene(x, y)));
        assert!(mosaic.focus.unwrap() > 0.0);
        assert_eq!((mosaic.clip_white, mosaic.lum_mean), (None, None));
    }

    #[test]
    fn signals_keep_the_culling_keys_flat() {
        let s = Signals {
            sharpness: Some(1.0),
            cull: Cull {
                version: Some(VERSION),
                focus: Some(0.5),
                ..Cull::default()
            },
            ..Signals::default()
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["cull"], VERSION);
        assert_eq!(v["focus"], 0.5);
        assert!(Cull::current(&v));
        let back: Signals = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
        assert!(!Cull::current(&serde_json::json!({ "sharpness": 1.0 })));
    }
}
