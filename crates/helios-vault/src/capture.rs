//! Capture metadata: what a file says of how its image was taken, kept as
//! read-only fields (`docs/design/library.md`, slice L2), so a query can
//! filter on it in the store and an index can serve it.
//!
//! - **The capture time** (`captured`) is the camera's clock read as if it
//!   were UTC, in seconds with the fraction: the wall-clock time where the
//!   photo was taken, so its day is the day the photographer lived, and
//!   two photos of one camera compare. When the file says the clock's
//!   offset (EXIF `OffsetTimeOriginal`), `capture_offset` keeps it in
//!   minutes and the instant is `captured - 60 * capture_offset`
//!   ([`Capture::instant`]). A file with no EXIF time takes its
//!   modification time, in UTC with an offset of 0, and `captured_from`
//!   says `file` rather than `exif`.
//! - **The camera and the settings:** maker, model, lens, focal length,
//!   aperture, shutter, ISO; **the picture:** width and height as stored,
//!   EXIF orientation, file type.
//!
//! An import writes them ([`Vault::set_captures`]); a background job fills
//! the images imported before them, its progress kept in the vault
//! ([`Backfill`]) so it resumes where it stopped. No command changes them
//! ([`crate::fields::FieldSpec::read_only`]) and they leave no audit row:
//! the file is their record.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{ImageId, Store, Vault};

pub const CAPTURED: &str = "captured";
pub const CAPTURE_OFFSET: &str = "capture_offset";
pub const CAPTURED_FROM: &str = "captured_from";
pub const CAMERA_MAKE: &str = "camera_make";
pub const CAMERA_MODEL: &str = "camera_model";
pub const LENS: &str = "lens";
pub const FOCAL_LENGTH: &str = "focal_length";
pub const APERTURE: &str = "aperture";
pub const SHUTTER: &str = "shutter";
pub const ISO: &str = "iso";
pub const WIDTH: &str = "width";
pub const HEIGHT: &str = "height";
pub const ORIENTATION: &str = "orientation";
pub const FILE_TYPE: &str = "file_type";

/// Every capture field.
pub const NAMES: [&str; 14] = [
    CAPTURED,
    CAPTURE_OFFSET,
    CAPTURED_FROM,
    CAMERA_MAKE,
    CAMERA_MODEL,
    LENS,
    FOCAL_LENGTH,
    APERTURE,
    SHUTTER,
    ISO,
    WIDTH,
    HEIGHT,
    ORIENTATION,
    FILE_TYPE,
];

/// The library document the backfill's progress is kept in.
/// Announced on the bus when captures were written (`count`: how many
/// images): a library shown reads their rows again.
pub const CAPTURES_SET: &str = "vault.captures_set";

/// The library document the backfill's progress is kept in.
pub const BACKFILL_NS: &str = "library/capture";
pub const BACKFILL_KEY: &str = "backfill";

/// What a file says of how its image was taken; `None`: it does not say.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    /// The camera's clock read as UTC, in seconds.
    pub captured: Option<f64>,
    /// The clock's offset from UTC, in minutes.
    pub offset: Option<i64>,
    /// `exif` or `file`.
    pub captured_from: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    /// Millimetres.
    pub focal_length: Option<f64>,
    /// f-number.
    pub aperture: Option<f64>,
    /// Seconds.
    pub shutter: Option<f64>,
    pub iso: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub orientation: Option<i64>,
    /// The extension, in lower case.
    pub file_type: Option<String>,
}

fn text(v: Option<&String>) -> Option<Value> {
    v.map(String::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(Value::from)
}

fn number(v: Option<f64>) -> Option<Value> {
    v.filter(|n| n.is_finite()).map(Value::from)
}

impl Capture {
    /// When it was taken, in seconds since 1970 UTC, when the offset is
    /// known.
    pub fn instant(&self) -> Option<f64> {
        Some(self.captured? - 60.0 * self.offset? as f64)
    }

    /// Its capture day, `(year, month, day)`, as the camera's clock said.
    pub fn day(&self) -> Option<(i64, u32, u32)> {
        self.captured.map(|t| civil((t / 86_400.0).floor() as i64))
    }

    /// The fields it sets, every capture field named: `None` clears one.
    pub fn values(&self) -> [(&'static str, Option<Value>); 14] {
        [
            (CAPTURED, number(self.captured)),
            (CAPTURE_OFFSET, self.offset.map(Value::from)),
            (CAPTURED_FROM, text(self.captured_from.as_ref())),
            (CAMERA_MAKE, text(self.make.as_ref())),
            (CAMERA_MODEL, text(self.model.as_ref())),
            (LENS, text(self.lens.as_ref())),
            (FOCAL_LENGTH, number(self.focal_length)),
            (APERTURE, number(self.aperture)),
            (SHUTTER, number(self.shutter)),
            (ISO, self.iso.map(Value::from)),
            (WIDTH, self.width.map(Value::from)),
            (HEIGHT, self.height.map(Value::from)),
            (ORIENTATION, self.orientation.map(Value::from)),
            (FILE_TYPE, text(self.file_type.as_ref())),
        ]
    }

    /// A capture from its stored fields; `None` when none is set.
    pub fn of_fields(fields: &BTreeMap<String, Value>) -> Option<Self> {
        let f = |name: &str| fields.get(name);
        let s = |name: &str| f(name).and_then(Value::as_str).map(str::to_string);
        let n = |name: &str| f(name).and_then(Value::as_f64);
        let i = |name: &str| f(name).and_then(Value::as_i64);
        let c = Self {
            captured: n(CAPTURED),
            offset: i(CAPTURE_OFFSET),
            captured_from: s(CAPTURED_FROM),
            make: s(CAMERA_MAKE),
            model: s(CAMERA_MODEL),
            lens: s(LENS),
            focal_length: n(FOCAL_LENGTH),
            aperture: n(APERTURE),
            shutter: n(SHUTTER),
            iso: i(ISO),
            width: i(WIDTH),
            height: i(HEIGHT),
            orientation: i(ORIENTATION),
            file_type: s(FILE_TYPE),
        };
        (c != Self::default()).then_some(c)
    }
}

/// How far the backfill went: the images in the Library taken in id
/// order, the last one done, how many so far and of how many.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Backfill {
    /// The last image done (hex): the next run starts after it.
    pub after: Option<String>,
    pub done: u64,
    pub total: u64,
    /// Filled from the file itself.
    #[serde(default)]
    pub read: u64,
    /// Filled from what the vault kept (the import's signals, the file's
    /// time): its file was not at hand or could not be read.
    #[serde(default)]
    pub kept: u64,
    /// Every image of the Library is done.
    pub finished: bool,
}

/// The day `days` after 1970-01-01, as `(year, month, day)`.
pub fn civil(days: i64) -> (i64, u32, u32) {
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
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The capture time a day (`YYYY-MM-DD`) starts at; `None` when it is
/// no such date.
pub fn day_start(text: &str) -> Option<i64> {
    let t = text.trim();
    if t.len() != 10 || t.as_bytes()[4] != b'-' || t.as_bytes()[7] != b'-' {
        return None;
    }
    let year: i64 = t[0..4].parse().ok()?;
    let month: u32 = t[5..7].parse().ok()?;
    let day: u32 = t[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400)
}

/// The day of a capture time, `YYYY-MM-DD`, as its camera's clock said.
pub fn day_text(captured: f64) -> String {
    let (y, m, d) = civil((captured / 86_400.0).floor() as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

impl<S: Store> Vault<S> {
    /// Records what files say of how images were taken: each image's
    /// capture fields replaced whole (one the reading did not find is
    /// cleared), in one transaction with the backfill's progress when
    /// given, so a run stopped anywhere resumes after the last batch
    /// kept. An image gone meanwhile is passed over. No audit row and no
    /// field event: the file is the record. The images count as placed (a
    /// cursor's next refresh lists them) and one [`CAPTURES_SET`] event
    /// says so, so a library reads their rows again. Returns how many
    /// images were written.
    pub fn set_captures(
        &self,
        captures: &[(ImageId, Capture)],
        progress: Option<&Backfill>,
    ) -> Result<usize, String> {
        let progress = progress
            .map(|p| serde_json::to_value(p).map_err(|e| e.to_string()))
            .transpose()?;
        let written = self.transaction(|store| {
            let mut written = Vec::new();
            for (id, capture) in captures {
                if store.image(id)?.is_none() {
                    continue;
                }
                for (name, value) in capture.values() {
                    let value = value
                        .map(|v| crate::fields::normalize(name, &v))
                        .transpose()?;
                    store.set_field(id, name, value.as_ref())?;
                }
                written.push(*id);
            }
            if let Some(p) = &progress {
                store.set_document(BACKFILL_NS, BACKFILL_KEY, Some(p))?;
            }
            Ok(written)
        })?;
        let count = written.len();
        if count > 0 {
            self.placed(written);
            if let Some(bus) = &self.bus {
                bus.publish_value("vault", None, CAPTURES_SET, json!({ "count": count }));
            }
        }
        Ok(count)
    }

    /// The capture of some images, in the order of `ids`: one store query.
    pub fn captures_of(&self, ids: &[ImageId]) -> Result<Vec<Option<Capture>>, String> {
        Ok(self
            .fields_of(ids, &NAMES)?
            .iter()
            .map(Capture::of_fields)
            .collect())
    }

    /// How far the backfill went; `None` before its first batch.
    pub fn backfill(&self) -> Result<Option<Backfill>, String> {
        self.store
            .lock()
            .expect("store lock")
            .document(BACKFILL_NS, BACKFILL_KEY)?
            .map(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_and_instants() {
        assert_eq!(day_start("1970-01-02"), Some(86_400));
        assert_eq!(day_start("2024-06-01"), Some(1_717_200_000));
        assert_eq!(day_start("2024-6-1"), None);
        assert_eq!(day_start("2024-13-01"), None);
        assert_eq!(day_text(1_717_200_000.5 + 3600.0), "2024-06-01");
        let c = Capture {
            captured: Some(1_717_200_000.0 + 7200.0),
            offset: Some(120),
            ..Capture::default()
        };
        assert_eq!(c.instant(), Some(1_717_200_000.0));
        assert_eq!(c.day(), Some((2024, 6, 1)));
        assert_eq!(Capture::of_fields(&BTreeMap::new()), None);
    }
}
