//! Near-duplicate signals: what an import measures of each image so that
//! bursts and brackets can be found later without reading the files again.
//!
//! - a **perceptual hash** of the embedded preview (or of the picture, or
//!   of a small decode of the mosaic when a raw has no preview): a 64-bit
//!   difference hash (each bit says whether a cell of a 9 x 8 grey
//!   thumbnail is darker than its right neighbour), so near-identical
//!   frames differ by a few bits, and a change of exposure moves few of
//!   them, since it scales the cells together;
//! - the **capture time** (EXIF, as the camera's clock wrote it; the file's
//!   modification time when there is none), the **exposure** (time,
//!   aperture, ISO, bias) and the camera's **frame number** (EXIF, else the
//!   digits a file name ends with);
//! - a **sharpness**: the variance of the Laplacian of a grey thumbnail at
//!   most [`SHARP_SIDE`] pixels a side, to propose the best of a group.

use std::path::Path;

use helios_core::Image;
use serde::{Deserialize, Serialize};

/// The longest side of the thumbnail the sharpness is measured on.
pub const SHARP_SIDE: u32 = 512;

/// What an import measured of an image.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Signals {
    /// The difference hash, 16 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phash: Option<String>,
    /// What the hash was taken from: `preview`, `picture` or `decode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hashed: Option<String>,
    /// When it was taken, in seconds: the camera's clock read as if it
    /// were UTC (only differences matter), with the fraction of a second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taken: Option<f64>,
    /// Where the time came from: `exif` or `file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taken_from: Option<String>,
    /// Seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure_time: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aperture: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iso: Option<f64>,
    /// EV.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure_bias: Option<f64>,
    /// The camera's frame number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<u32>,
    /// Where the frame number came from: `exif` or `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_from: Option<String>,
    /// A burst the camera says the frame belongs to (none of the cameras
    /// read so far says it: the field waits for maker notes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst: Option<String>,
    /// The camera's maker and model, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharpness: Option<f64>,
}

impl Signals {
    /// `maker model`, when either is known.
    pub fn camera(&self) -> Option<String> {
        let both = format!(
            "{} {}",
            self.maker.as_deref().unwrap_or(""),
            self.model.as_deref().unwrap_or("")
        );
        let both = both.trim();
        (!both.is_empty()).then(|| both.to_string())
    }

    /// The hash as a number.
    pub fn hash(&self) -> Option<u64> {
        self.phash
            .as_deref()
            .and_then(|h| u64::from_str_radix(h, 16).ok())
    }

    /// How much light the settings let in, in stops (larger is brighter):
    /// log2(time x ISO / aperture²), when the three are known.
    pub fn exposure(&self) -> Option<f64> {
        let (t, n, iso) = (self.exposure_time?, self.aperture?, self.iso?);
        (t > 0.0 && n > 0.0 && iso > 0.0).then(|| (t * iso / (n * n)).log2())
    }

    /// The capture date, `(year, month, day)`, from `taken`.
    pub fn date(&self) -> Option<(i64, u32, u32)> {
        self.taken.map(|t| civil((t / 86_400.0).floor() as i64))
    }

    /// The capture time of day, `(hour, minute, second)`, from `taken`.
    pub fn time_of_day(&self) -> Option<(u32, u32, u32)> {
        self.taken.map(|t| {
            let s = t.floor().rem_euclid(86_400.0) as u32;
            (s / 3600, s / 60 % 60, s % 60)
        })
    }
}

/// Bits that differ between two hashes.
pub fn distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// A grey thumbnail of `image` at most `side` pixels a side, by box
/// averaging: perceptual lightness (the square root of the linear
/// luminance) for colour images, the normalized value for one channel (a
/// mosaic). `(width, height, values)`.
pub fn grey(image: &Image, side: u32) -> (usize, usize, Vec<f32>) {
    let r = image.region();
    let (w, h) = (r.width as usize, r.height as usize);
    if w == 0 || h == 0 {
        return (0, 0, Vec::new());
    }
    let longest = w.max(h) as f64;
    let factor = (f64::from(side) / longest).min(1.0);
    let tw = ((w as f64 * factor).round() as usize).max(1);
    let th = ((h as f64 * factor).round() as usize).max(1);
    let channels = image.channels() as usize;
    let data = image.data();
    let peak = if channels < 3 {
        data.iter()
            .step_by(channels)
            .fold(0.0f32, |m, v| m.max(*v))
            .max(f32::MIN_POSITIVE)
    } else {
        1.0
    };
    let mut sums = vec![0.0f64; tw * th];
    let mut counts = vec![0u32; tw * th];
    for y in 0..h {
        let ty = y * th / h;
        for x in 0..w {
            let tx = x * tw / w;
            let p = &data[(y * w + x) * channels..(y * w + x + 1) * channels];
            let linear = if channels >= 3 {
                0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2]
            } else {
                p[0] / peak
            };
            sums[ty * tw + tx] += f64::from(linear.max(0.0).sqrt());
            counts[ty * tw + tx] += 1;
        }
    }
    let values = sums
        .iter()
        .zip(&counts)
        .map(|(s, n)| (*s / f64::from((*n).max(1))) as f32)
        .collect();
    (tw, th, values)
}

/// Box-averages a grey thumbnail down to `tw` x `th`.
fn shrink(w: usize, h: usize, values: &[f32], tw: usize, th: usize) -> Vec<f32> {
    let mut sums = vec![0.0f64; tw * th];
    let mut counts = vec![0u32; tw * th];
    for y in 0..h {
        for x in 0..w {
            let i = (y * th / h) * tw + x * tw / w;
            sums[i] += f64::from(values[y * w + x]);
            counts[i] += 1;
        }
    }
    sums.iter()
        .zip(&counts)
        .map(|(s, n)| (*s / f64::from((*n).max(1))) as f32)
        .collect()
}

/// The 64-bit difference hash of a grey thumbnail; `None` when it is too
/// small to hash (under 9 x 8).
pub fn dhash(w: usize, h: usize, values: &[f32]) -> Option<u64> {
    if w < 9 || h < 8 {
        return None;
    }
    let cells = shrink(w, h, values, 9, 8);
    let mut hash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            hash <<= 1;
            if cells[y * 9 + x] < cells[y * 9 + x + 1] {
                hash |= 1;
            }
        }
    }
    Some(hash)
}

/// The variance of the 4-neighbour Laplacian of a grey thumbnail: higher
/// is sharper; 0 for a thumbnail too small to have an inside.
pub fn sharpness(w: usize, h: usize, values: &[f32]) -> f64 {
    if w < 3 || h < 3 {
        return 0.0;
    }
    let at = |x: usize, y: usize| f64::from(values[y * w + x]);
    let mut sum = 0.0;
    let mut squares = 0.0;
    let mut n = 0.0;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let l = at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1) - 4.0 * at(x, y);
            sum += l;
            squares += l * l;
            n += 1.0;
        }
    }
    let mean = sum / n;
    (squares / n - mean * mean).max(0.0)
}

/// The day `days` after 1970-01-01, as `(year, month, day)`.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Days from 1970-01-01 to a date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// An EXIF date (`YYYY:MM:DD HH:MM:SS`) and its sub-second digits, in
/// seconds read as UTC; `None` when it does not parse (a camera whose
/// clock was never set writes zeros).
pub fn exif_time(taken: &str, subsec: Option<&str>) -> Option<f64> {
    let t = taken.trim();
    let num = |range: std::ops::Range<usize>| t.get(range)?.parse::<u32>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if year == 0 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(i64::from(year), month, day);
    let mut seconds = (days * 86_400) as f64 + f64::from(hour * 3600 + minute * 60 + second);
    if let Some(digits) = subsec
        .map(str::trim)
        .filter(|d| !d.is_empty() && d.len() <= 9 && d.bytes().all(|b| b.is_ascii_digit()))
    {
        seconds += format!("0.{digits}").parse::<f64>().unwrap_or(0.0);
    }
    Some(seconds)
}

/// The number a file name's stem ends with (`IMG_0042.CR2`: 42), as
/// cameras number their frames.
pub fn name_frame(path: &Path) -> Option<u32> {
    let stem = path.file_stem()?.to_str()?;
    let digits = &stem[stem.trim_end_matches(|c: char| c.is_ascii_digit()).len()..];
    if digits.is_empty() || digits.len() > 9 {
        return None;
    }
    digits.parse().ok()
}

/// Measures a file: its preview (or picture, or a small decode of its
/// mosaic) hashed and measured, how it was shot, and when.
pub fn measure(path: &Path) -> Result<Signals, String> {
    let look = helios_load::look(path)?;
    let (image, hashed) = match (look.preview, &look.shot) {
        (Some(p), Some(_)) => (p, "preview"),
        (Some(p), None) => (p, "picture"),
        (None, _) => (helios_load::load(path)?.image, "decode"),
    };
    let (w, h, values) = grey(&image, SHARP_SIDE);
    let mut signals = Signals {
        phash: dhash(w, h, &values).map(|v| format!("{v:016x}")),
        hashed: Some(hashed.into()),
        sharpness: Some(sharpness(w, h, &values)),
        ..Signals::default()
    };
    if let Some(shot) = look.shot {
        signals.taken = shot
            .taken
            .as_deref()
            .and_then(|t| exif_time(t, shot.subsec.as_deref()));
        signals.exposure_time = shot.exposure_time;
        signals.aperture = shot.aperture;
        signals.iso = shot.iso;
        signals.exposure_bias = shot.exposure_bias;
        if shot.frame.is_some() {
            signals.frame = shot.frame;
            signals.frame_from = Some("exif".into());
        }
        signals.maker = Some(shot.maker).filter(|m| !m.is_empty());
        signals.model = Some(shot.model).filter(|m| !m.is_empty());
    }
    if signals.taken.is_some() {
        signals.taken_from = Some("exif".into());
    } else if let Some(modified) = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
    {
        signals.taken = Some(modified.as_secs_f64());
        signals.taken_from = Some("file".into());
    }
    if signals.frame.is_none()
        && let Some(n) = name_frame(path)
    {
        signals.frame = Some(n);
        signals.frame_from = Some("name".into());
    }
    Ok(signals)
}

#[cfg(test)]
mod tests {
    use helios_core::Region;

    use super::*;

    #[test]
    fn exif_times_read_as_seconds_and_dates_come_back() {
        let t = exif_time("2024:06:01 12:34:56", Some("25")).unwrap();
        let s = Signals {
            taken: Some(t),
            ..Signals::default()
        };
        assert_eq!(s.date(), Some((2024, 6, 1)));
        assert_eq!(s.time_of_day(), Some((12, 34, 56)));
        assert!((t.fract() - 0.25).abs() < 1e-6);
        let next = exif_time("2024:06:01 12:34:57", None).unwrap();
        assert!((next - t - 0.75).abs() < 1e-6);
        assert_eq!(exif_time("0000:00:00 00:00:00", None), None);
        assert_eq!(exif_time("garbage", None), None);
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }

    #[test]
    fn frames_come_from_a_name_s_last_digits() {
        assert_eq!(name_frame(Path::new("IMG_0042.CR2")), Some(42));
        assert_eq!(name_frame(Path::new("DSC01234.ARW")), Some(1234));
        assert_eq!(name_frame(Path::new("holiday.jpg")), None);
    }

    #[test]
    fn a_brighter_copy_hashes_alike_and_a_blur_is_less_sharp() {
        let region = Region::sized(64, 48);
        let scene = |x: i64, y: i64| {
            let checker = ((x / 8 + y / 8) % 2) as f32;
            0.05 + 0.4 * checker + 0.002 * x as f32
        };
        let image = Image::from_fn(region, 3, |x, y, _| scene(x, y));
        let brighter = Image::from_fn(region, 3, |x, y, _| 1.3 * scene(x, y));
        let blurred = Image::from_fn(region, 3, |x, y, _| {
            let mut s = 0.0;
            for dy in -2..=2 {
                for dx in -2..=2 {
                    s += scene((x + dx).clamp(0, 63), (y + dy).clamp(0, 47));
                }
            }
            s / 25.0
        });
        let (w, h, a) = grey(&image, SHARP_SIDE);
        let (_, _, b) = grey(&brighter, SHARP_SIDE);
        let (_, _, c) = grey(&blurred, SHARP_SIDE);
        let (ha, hb) = (dhash(w, h, &a).unwrap(), dhash(w, h, &b).unwrap());
        assert!(distance(ha, hb) <= 4, "{}", distance(ha, hb));
        assert!(sharpness(w, h, &a) > sharpness(w, h, &c));
        let mosaic = Image::from_fn(region, 1, |x, y, _| 1000.0 * scene(x, y));
        let (_, _, m) = grey(&mosaic, SHARP_SIDE);
        assert!(distance(ha, dhash(w, h, &m).unwrap()) <= 8);
    }

    #[test]
    fn the_exposure_is_in_stops() {
        let s = Signals {
            exposure_time: Some(1.0 / 100.0),
            aperture: Some(4.0),
            iso: Some(100.0),
            ..Signals::default()
        };
        let brighter = Signals {
            exposure_time: Some(1.0 / 50.0),
            ..s.clone()
        };
        let d = brighter.exposure().unwrap() - s.exposure().unwrap();
        assert!((d - 1.0).abs() < 1e-9);
    }
}
