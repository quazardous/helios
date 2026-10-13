//! The raw corpus checks: decoder and camera-name regressions.
//!
//! `tests/raw-corpus.toml` lists raw files from raw.pixls.us, and for each
//! one checked (`tier = "core"`) what Helios must read from it, in its
//! `[file.check]` table:
//!
//! - the file decodes (`expect = "decode"`), or fails cleanly with an error
//!   that contains `error` (`expect = "unsupported"`); a panic is a failure
//!   either way;
//! - `camera`: the normalised make and model, `"Make / Model"`, as Helios's
//!   loader names them (rawler's clean names, trimmed);
//! - `sensor` (`WxH`, the full sensor, `WxHxC` when a photosite has `C`
//!   components), `crop` (`WxH+X+Y`), and `cfa`: the
//!   filter pattern seen from the crop's corner, or `monochrome` / `linear`
//!   for a raw without one;
//! - `black` (the 2 x 2 photosites at the crop's corner), `white`, and
//!   `matrix` (XYZ to camera, D65 when the camera has it, row by row);
//! - `helios`: `mosaic` when Helios's loader accepts the file, else a part
//!   of the error it gives.
//!
//! For a mosaic, the levels and the matrix are what Helios's loader hands
//! the modules (black measured on the masked photosites, as darktable
//! does); for any other raw, what rawler reports.
//!
//! A small thumbnail is also demosaiced (block averages of each filter
//! colour, at most [`THUMB_EDGE`] pixels on the long edge, 8 bits) and its
//! hash compared with `tests/raw-corpus.thumbs.toml`, which only
//! `--bless` rewrites.
//!
//! The files themselves are never in the repository: `tools/bench/fill-corpus.sh`
//! downloads them, into the bench box's `raw-corpus` volume or a local
//! directory (see `docs/raw-corpus.md`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rawler::decoders::RawDecodeParams;
use rawler::imgop::xyz::Illuminant;
use rawler::rawimage::{RawImage, RawPhotometricInterpretation};
use rawler::rawsource::RawSource;
use serde::{Deserialize, Serialize};

/// The thumbnail's long edge, at most, in pixels.
pub const THUMB_EDGE: usize = 64;

/// Relative tolerance on the levels and the matrix (they are rationals and
/// `f32` in rawler: compared as decimals written in the manifest).
const TOLERANCE: f64 = 1e-4;

/// The manifest: `tests/raw-corpus.toml`.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub file: Vec<Entry>,
}

/// One file of the manifest (the fields the checks read).
#[derive(Debug, Deserialize)]
pub struct Entry {
    pub id: String,
    pub filename: String,
    pub tier: String,
    pub expect: String,
    pub licence: String,
    #[serde(default)]
    pub check: Check,
}

/// What a file must give: every field is optional in the file, but a
/// checked file without one fails, saying what was observed.
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq)]
pub struct Check {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cfa: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub black: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub white: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matrix: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helios: Option<String>,
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The files checked: the `core` tier.
    pub fn checked(&self) -> impl Iterator<Item = &Entry> {
        self.file.iter().filter(|f| f.tier == "core")
    }
}

impl Entry {
    /// The file's name in the corpus directory: its id and its extension.
    pub fn local_name(&self) -> String {
        let ext = self.filename.rsplit_once('.').map_or("raw", |(_, e)| e);
        format!("{}.{ext}", self.id)
    }
}

/// The stored thumbnail hashes, by file id.
pub type Thumbs = BTreeMap<String, String>;

pub fn read_thumbs(path: &Path) -> Result<Thumbs, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Thumbs::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn write_thumbs(path: &Path, thumbs: &Thumbs) -> Result<(), String> {
    let mut text = String::from(
        "# Hashes of the raw corpus's demosaiced thumbnails (crates/helios-corpus),\n\
         # by file id. Rewritten only by `--bless`, after checking the change is wanted.\n\n",
    );
    text.push_str(&toml::to_string(thumbs).map_err(|e| e.to_string())?);
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// What a file gave.
#[derive(Debug, Default, Serialize)]
pub struct Observed {
    #[serde(flatten)]
    pub check: Check,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
}

/// A small 8-bit RGB picture.
pub struct Thumb {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

impl Thumb {
    /// Short hash of the size and the pixels.
    pub fn hash(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(&(self.width as u32).to_le_bytes());
        h.update(&(self.height as u32).to_le_bytes());
        h.update(&self.rgb);
        h.finalize().to_hex()[..16].to_string()
    }

    /// As a binary PPM, to look at.
    pub fn ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.extend_from_slice(&self.rgb);
        out
    }
}

/// The result for one file.
pub struct Outcome {
    pub id: String,
    pub failures: Vec<String>,
    pub observed: Observed,
    pub thumb: Option<Thumb>,
    pub seconds: f64,
}

fn ratio(n: u32, d: u32) -> f64 {
    if d == 0 {
        0.0
    } else {
        f64::from(n) / f64::from(d)
    }
}

/// `(x, y, w, h)`: the crop, else the active area, else the sensor.
fn area(raw: &RawImage) -> (usize, usize, usize, usize) {
    raw.crop_area
        .or(raw.active_area)
        .map_or((0, 0, raw.width, raw.height), |r| {
            (r.p.x, r.p.y, r.d.w, r.d.h)
        })
}

fn black_at(raw: &RawImage, x: usize, y: usize) -> f64 {
    let b = &raw.blacklevel;
    if b.levels.is_empty() || b.width == 0 || b.height == 0 {
        return 0.0;
    }
    let i = ((y % b.height) * b.width + x % b.width) * b.cpp.max(1);
    let r = b.levels[i.min(b.levels.len() - 1)];
    ratio(r.n, r.d)
}

fn matrix(raw: &RawImage) -> Vec<f64> {
    raw.color_matrix
        .get(&Illuminant::D65)
        .or_else(|| raw.color_matrix.values().next())
        .filter(|m| m.len() >= 9)
        .map_or_else(
            || vec![0.0; 9],
            |m| m.iter().take(9).map(|v| f64::from(*v)).collect(),
        )
}

/// Rounds for the manifest: levels and matrices are short decimals.
fn round(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// Block averages of each filter colour (or each component), black to
/// white mapped to 0..1, a square-root curve, 8 bits.
fn thumbnail(raw: &RawImage) -> Thumb {
    let (x0, y0, w, h) = area(raw);
    let cfa = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(c) if raw.cpp == 1 => Some(c.cfa.shift(x0, y0)),
        _ => None,
    };
    let period = cfa.as_ref().map_or(1, |c| c.width.max(c.height).max(1));
    let long = w.max(h);
    let block = (long.div_ceil(THUMB_EDGE)).div_ceil(period).max(1) * period;
    let (tw, th) = ((w / block).max(1), (h / block).max(1));
    let data = raw.data.as_f32();
    let cpp = raw.cpp.max(1);
    let black = {
        let b = &raw.blacklevel.levels;
        if b.is_empty() {
            0.0
        } else {
            b.iter().map(|r| ratio(r.n, r.d)).sum::<f64>() / b.len() as f64
        }
    };
    let white = f64::from(
        raw.whitelevel
            .0
            .first()
            .copied()
            .unwrap_or(u32::from(u16::MAX)),
    );
    let span = (white - black).max(1.0);
    let mut rgb = Vec::with_capacity(tw * th * 3);
    for ty in 0..th {
        for tx in 0..tw {
            let mut sum = [0.0f64; 3];
            let mut n = [0u32; 3];
            for y in ty * block..((ty + 1) * block).min(h) {
                for x in tx * block..((tx + 1) * block).min(w) {
                    let at = ((y0 + y) * raw.width + x0 + x) * cpp;
                    match &cfa {
                        Some(c) => {
                            let k = c.color_at(y, x).min(2);
                            sum[k] += f64::from(data[at]);
                            n[k] += 1;
                        }
                        None if cpp >= 3 => {
                            for k in 0..3 {
                                sum[k] += f64::from(data[at + k]);
                                n[k] += 1;
                            }
                        }
                        None => {
                            for k in 0..3 {
                                sum[k] += f64::from(data[at]);
                                n[k] += 1;
                            }
                        }
                    }
                }
            }
            for k in 0..3 {
                let v = if n[k] == 0 {
                    0.0
                } else {
                    sum[k] / f64::from(n[k])
                };
                let v = ((v - black) / span).clamp(0.0, 1.0).sqrt();
                rgb.push((v * 255.0).round() as u8);
            }
        }
    }
    Thumb {
        width: tw,
        height: th,
        rgb,
    }
}

fn decode(path: &Path) -> Result<RawImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read: {e}"))?;
    let source = RawSource::new_from_shared_vec(Arc::new(bytes));
    let decoder = rawler::get_decoder(&source).map_err(|e| e.to_string())?;
    decoder
        .raw_image(&source, &RawDecodeParams::default(), false)
        .map_err(|e| e.to_string())
}

/// Observes a file that decodes: rawler's view, then Helios's loader.
fn observe(path: &Path, raw: &RawImage) -> (Observed, Thumb) {
    let (x0, y0, w, h) = area(raw);
    let mut check = Check {
        camera: Some(format!(
            "{} / {}",
            raw.clean_make.trim(),
            raw.clean_model.trim()
        )),
        sensor: Some(if raw.cpp == 1 {
            format!("{}x{}", raw.width, raw.height)
        } else {
            format!("{}x{}x{}", raw.width, raw.height, raw.cpp)
        }),
        crop: Some(format!("{w}x{h}+{x0}+{y0}")),
        cfa: Some(match &raw.photometric {
            RawPhotometricInterpretation::Cfa(c) => c.cfa.shift(x0, y0).name,
            RawPhotometricInterpretation::BlackIsZero => "monochrome".into(),
            RawPhotometricInterpretation::LinearRaw => "linear".into(),
        }),
        black: Some(
            [(0, 0), (1, 0), (0, 1), (1, 1)]
                .map(|(dx, dy)| round(black_at(raw, x0 + dx, y0 + dy)))
                .to_vec(),
        ),
        white: Some(f64::from(
            raw.whitelevel
                .0
                .first()
                .copied()
                .unwrap_or(u32::from(u16::MAX)),
        )),
        matrix: Some(matrix(raw).into_iter().map(round).collect()),
        ..Check::default()
    };
    match helios_load::load(path) {
        Ok(loaded) => {
            check.helios = Some("mosaic".into());
            if let Some(cam) = loaded.camera {
                check.camera = Some(format!("{} / {}", cam.maker, cam.model));
                check.cfa = Some(cam.cfa.name());
                check.black = Some(cam.black.iter().copied().map(round).collect());
                check.white = Some(round(cam.white));
                check.matrix = Some(cam.xyz_to_camera.iter().copied().map(round).collect());
            }
        }
        Err(e) => {
            // The loader names the file first: keep the reason.
            let prefix = format!("{}: ", path.display());
            check.helios = Some(e.strip_prefix(&prefix).unwrap_or(&e).to_string());
        }
    }
    let thumb = thumbnail(raw);
    (
        Observed {
            check,
            thumb: Some(thumb.hash()),
        },
        thumb,
    )
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOLERANCE * a.abs().max(b.abs()).max(1.0)
}

fn compare<T: std::fmt::Debug + PartialEq>(
    failures: &mut Vec<String>,
    what: &str,
    expected: Option<&T>,
    observed: Option<&T>,
    same: impl Fn(&T, &T) -> bool,
) {
    match (expected, observed) {
        (Some(e), Some(o)) if same(e, o) => {}
        (Some(e), Some(o)) => failures.push(format!("{what}: expected {e:?}, got {o:?}")),
        (None, Some(o)) => failures.push(format!("{what}: no expectation (got {o:?})")),
        (_, None) => {}
    }
}

fn close_all(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| close(*x, *y))
}

/// Checks one file of the corpus directory.
pub fn check_file(dir: &Path, entry: &Entry, thumbs: &Thumbs) -> Outcome {
    let start = Instant::now();
    let path = dir.join(entry.local_name());
    let mut failures = Vec::new();
    let mut observed = Observed::default();
    let mut thumb = None;
    if entry.licence != "CC0-1.0" {
        failures.push(format!("licence {}: the corpus is CC0 only", entry.licence));
    }
    if path.is_file() {
        let decoded = catch_unwind(AssertUnwindSafe(|| {
            decode(&path).map(|raw| observe(&path, &raw))
        }));
        let want = &entry.check;
        match (decoded, entry.expect.as_str()) {
            (Err(_), _) => failures.push("panicked while decoding".into()),
            (Ok(Err(e)), "unsupported") => {
                observed.check.error = Some(e.clone());
                match &want.error {
                    Some(part) if e.contains(part.as_str()) => {}
                    Some(part) => failures.push(format!(
                        "error: expected one containing {part:?}, got {e:?}"
                    )),
                    None => failures.push(format!("error: no expectation (got {e:?})")),
                }
            }
            (Ok(Err(e)), _) => failures.push(format!("does not decode: {e}")),
            (Ok(Ok(_)), "unsupported") => failures.push(
                "decodes, but the manifest expects it unsupported: a new decoder? update the entry"
                    .into(),
            ),
            (Ok(Ok((obs, t))), _) => {
                let got = &obs.check;
                let f = &mut failures;
                compare(
                    f,
                    "camera",
                    want.camera.as_ref(),
                    got.camera.as_ref(),
                    PartialEq::eq,
                );
                compare(
                    f,
                    "sensor",
                    want.sensor.as_ref(),
                    got.sensor.as_ref(),
                    PartialEq::eq,
                );
                compare(
                    f,
                    "crop",
                    want.crop.as_ref(),
                    got.crop.as_ref(),
                    PartialEq::eq,
                );
                compare(f, "cfa", want.cfa.as_ref(), got.cfa.as_ref(), PartialEq::eq);
                compare(
                    f,
                    "black",
                    want.black.as_ref(),
                    got.black.as_ref(),
                    |a, b| close_all(a, b),
                );
                compare(
                    f,
                    "white",
                    want.white.as_ref(),
                    got.white.as_ref(),
                    |a, b| close(*a, *b),
                );
                compare(
                    f,
                    "matrix",
                    want.matrix.as_ref(),
                    got.matrix.as_ref(),
                    |a, b| close_all(a, b),
                );
                match (&want.helios, &got.helios) {
                    (Some(w), Some(g)) if g == w || (w != "mosaic" && g.contains(w.as_str())) => {}
                    (w, g) => f.push(format!("helios: expected {w:?}, got {g:?}")),
                }
                compare(
                    f,
                    "thumbnail",
                    thumbs.get(&entry.id),
                    obs.thumb.as_ref(),
                    PartialEq::eq,
                );
                observed = obs;
                thumb = Some(t);
            }
        }
    } else {
        failures.push(format!(
            "missing from the corpus directory: {}",
            path.display()
        ));
    }
    Outcome {
        id: entry.id.clone(),
        failures,
        observed,
        thumb,
        seconds: start.elapsed().as_secs_f64(),
    }
}

/// Checks every core file of the manifest in `dir`.
pub fn run(dir: &Path, manifest: &Manifest, thumbs: &Thumbs) -> Vec<Outcome> {
    manifest
        .checked()
        .map(|e| check_file(dir, e, thumbs))
        .collect()
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A JUnit report: one test case per file.
pub fn junit(outcomes: &[Outcome]) -> String {
    let failed = outcomes.iter().filter(|o| !o.failures.is_empty()).count();
    let total: f64 = outcomes.iter().map(|o| o.seconds).sum();
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"raw-corpus\" tests=\"{}\" failures=\"{failed}\" time=\"{total:.3}\">\n<testsuite name=\"raw-corpus\" tests=\"{}\" failures=\"{failed}\" time=\"{total:.3}\">\n",
        outcomes.len(),
        outcomes.len()
    );
    for o in outcomes {
        let _ = write!(
            out,
            "<testcase classname=\"raw-corpus\" name=\"{}\" time=\"{:.3}\"",
            xml(&o.id),
            o.seconds
        );
        if o.failures.is_empty() {
            out.push_str("/>\n");
        } else {
            let _ = writeln!(
                out,
                "><failure message=\"{}\">{}</failure></testcase>",
                xml(&o.failures[0]),
                xml(&o.failures.join("\n"))
            );
        }
    }
    out.push_str("</testsuite>\n</testsuites>\n");
    out
}

/// A Markdown summary: the counts, then a line per file.
pub fn summary(outcomes: &[Outcome]) -> String {
    let failed: Vec<&Outcome> = outcomes.iter().filter(|o| !o.failures.is_empty()).collect();
    let mut out = format!(
        "# Raw corpus\n\n{} files, {} passed, {} failed.\n\n| file | camera | result | s |\n|---|---|---|---|\n",
        outcomes.len(),
        outcomes.len() - failed.len(),
        failed.len()
    );
    for o in outcomes {
        let result = if o.failures.is_empty() {
            "ok".to_string()
        } else {
            format!("FAIL: {}", o.failures.join("; ").replace('|', "\\|"))
        };
        let camera = o
            .observed
            .check
            .camera
            .clone()
            .or_else(|| o.observed.check.error.clone().map(|e| format!("({e})")))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {:.2} |",
            o.id,
            camera.replace('|', "\\|"),
            result,
            o.seconds
        );
    }
    out
}

/// What each file gave, as `[file.check]` tables to paste into the manifest.
pub fn observed_toml(outcomes: &[Outcome]) -> String {
    let mut out = String::new();
    for o in outcomes {
        let check = &o.observed.check;
        if check.helios.is_none() && check.error.is_none() {
            continue;
        }
        let _ = writeln!(out, "# {}\n[file.check]", o.id);
        out.push_str(&toml::to_string(check).unwrap_or_default());
        if let Some(t) = &o.observed.thumb {
            let _ = writeln!(out, "# thumb = {t:?}");
        }
        out.push('\n');
    }
    out
}

/// The repository's root, from this crate's place in it.
pub fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}
