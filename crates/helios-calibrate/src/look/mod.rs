//! A camera look fitted to a raw's embedded JPEG (see
//! `docs/design/camera-look.md`, step 2): headless, pure, no IO.
//!
//! The raw is rendered small (the sensor proxy, scene-linear working RGB)
//! and its embedded preview is reduced to the same proxy (display-linear
//! working RGB). Each proxy pixel is a pair. [`fit`] learns, in order:
//!
//! 1. a 3×3 **matrix** keeping the neutral axis, on luminance-normalised
//!    RGB so the tone stays out of it, ridge-regularised toward the
//!    identity ([`fit_matrix`]), then refined on Delta E 2000 at the
//!    preview's luminance with the chart solver's Levenberg-Marquardt
//!    ([`refine_matrix`]), the refinement kept when it lowers the
//!    held-out error;
//! 2. a **hue/saturation table** in DNG's layout on what the matrix left,
//!    kept only when it earns its place ([`fit_table`]);
//! 3. a **tone curve** on luminance by quantile matching, monotone,
//!    keeping black, its steepness bounded, applied as a luminance ratio
//!    so hues are kept ([`fit_tone`]);
//! 4. a **chroma curve**, a chroma ratio over display luminance, kept only
//!    when it lowers the held-out error ([`fit_chroma`]).
//!
//! A black-and-white preview, or a near-black frame (most of the preview
//! under [`thresholds::NEAR_BLACK`]), gets a tone curve alone.
//!
//! Every choice and gate is judged on held-out pixels taken in 8×8
//! tiles ([`split`]), with Delta E 2000 against the preview, and the look
//! must beat Helios's default render clearly. A fit that cannot is a
//! [`Refusal`] with its [`Gate`] and a reason in words.
//!
//! Same inputs, same bits: f64 throughout, every sum in a fixed order, no
//! threads, no random start, fixed iteration counts.

use helios_photo::color::{IDENTITY, Mat3, apply as mat_apply};
use helios_photo::compare::{delta_e_2000, lab};
use serde::{Deserialize, Serialize};

use crate::solver::{assemble, levenberg_marquardt, normalise, orthogonal_basis};

pub mod edit;
pub mod render;
mod table;
#[cfg(test)]
mod tests;

pub use table::{
    HUES, HueSatTable, Kernels, LOOK_KERNELS, MAX_HUE_SHIFT, MAX_SAT_SCALE, SATURATIONS, VALUES,
    fit_table, fit_table_with, value_coordinate,
};

/// The fitter's version, stored in every [`Look`]: a later fitter changes
/// no stored look.
///
/// 2: the hue/saturation table, the matrix refined on Delta E 2000, the
/// tone-only look of near-black frames.
pub const FITTER_VERSION: u32 = 2;

/// The proxy's long edge, in pixels.
pub const PROXY: usize = 192;

/// The held-out tiles' edge, in proxy pixels.
pub const TILE: usize = 8;

/// Rec.2020 luminance weights (the working space's Y).
pub const LUMA: [f64; 3] = [0.262_700_2, 0.677_998_1, 0.059_301_7];

/// The gates' thresholds, in one place.
pub mod thresholds {
    /// The preview's long edge, at least, in pixels.
    pub const PREVIEW_MIN_EDGE: u32 = 384;
    /// The preview's long edge, at least, as a fraction of the raw's.
    pub const PREVIEW_MIN_FRACTION: f64 = 0.10;
    /// The preview's aspect ratio, at most this far from the raw's.
    pub const ASPECT_TOLERANCE: f64 = 0.02;
    /// The preview's estimated JPEG quality, at least.
    pub const MIN_QUALITY: u8 = 75;
    /// Usable pairs, at least.
    pub const MIN_PAIRS: usize = 256;
    /// Usable held-out pairs, at least.
    pub const MIN_HELD_OUT: usize = 64;
    /// Preview luminance (linear) under which a pair is near black.
    pub const NEAR_BLACK: f64 = 0.015;
    /// A frame is near black when the median preview luminance of its
    /// pairs is under [`NEAR_BLACK`]: it gets a tone-only look, its pairs
    /// taken from this lower floor.
    pub const NEAR_BLACK_FLOOR: f64 = 0.001;
    /// A near-black frame needs this many times the usual pair counts.
    pub const NEAR_BLACK_COUNTS: usize = 2;
    /// Preview luminance (linear) from which a pair is a highlight, left
    /// out of the colour pairs.
    pub const HIGHLIGHT: f64 = 0.85;
    /// The fraction of the proxy clipped on the sensor side, at most.
    pub const SENSOR_CLIP_MAX: f64 = 0.5;
    /// The fraction of the proxy at 255 in the preview where the sensor
    /// is not clipped, at most.
    pub const PREVIEW_CLIP_MAX: f64 = 0.05;
    /// A pair is coloured when its normalised sensor channels spread more.
    pub const COLOUR_SPREAD: f64 = 0.05;
    /// Coloured pairs, at least, as a fraction.
    pub const COLOUR_FRACTION: f64 = 0.05;
    /// The normalised inputs' second-moment matrix: condition number, at
    /// most.
    pub const MAX_CONDITION: f64 = 1e5;
    /// The brightest 1/32 of scene luminances over the darkest, at least.
    pub const TONE_RANGE: f64 = 1.2;
    /// The tone curve's elasticity (d log out / d log in), at most.
    pub const MAX_ELASTICITY: f64 = 4.0;
    /// A matrix coefficient, at most (in absolute value).
    pub const MATRIX_BOUND: f64 = 8.0;
    /// A preview pixel is grey when its channels are within this.
    pub const MONO_TOLERANCE: f64 = 0.002;
    /// A preview is monochrome when at most this fraction is not grey.
    pub const MONO_FRACTION: f64 = 0.001;
    /// Spearman rank correlation of luminance, at least.
    pub const MIN_SPEARMAN: f64 = 0.5;
    /// Log-luminance correlation of the two proxies, at least.
    pub const MIN_STRUCTURE: f64 = 0.85;
    /// The best shift between the proxies, at most, in proxy pixels.
    pub const MAX_SHIFT: i64 = 1;
    /// The look's held-out mean must be below this fraction of the
    /// default render's.
    pub const BAR: f64 = 0.7;
    /// The look's held-out mean Delta E 2000, at most (provisional).
    pub const CEILING: f64 = 6.0;
    /// The look may clip at most this fraction of the proxy more than
    /// both the default render and the preview.
    pub const CLIP_BUDGET: f64 = 0.001;
    /// A stage that is optional stays only when it lowers the held-out
    /// mean by at least this fraction.
    pub const KEEP_GAIN: f64 = 0.02;
}

use thresholds as t;

/// A proxy: `width × height` RGB pixels, row by row.
#[derive(Clone, Debug, PartialEq)]
pub struct Proxy {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<[f64; 3]>,
}

impl Proxy {
    /// A proxy of `rgb`, which holds `width × height` pixels.
    ///
    /// # Panics
    /// When the sizes disagree.
    pub fn new(width: usize, height: usize, rgb: Vec<[f64; 3]>) -> Proxy {
        assert_eq!(rgb.len(), width * height, "a proxy's pixel count");
        Proxy { width, height, rgb }
    }
}

/// What the files say, beside the pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Source {
    /// The raw's active area, in pixels.
    pub raw_size: [u32; 2],
    /// The embedded preview's size, in pixels; zero when there is none.
    pub preview_size: [u32; 2],
    /// The preview's estimated JPEG quality ([`estimate_quality`]), when
    /// known.
    pub jpeg_quality: Option<u8>,
    /// A picture effect the maker note names that a global model cannot
    /// follow (toy camera, miniature, soft focus…), in words. Reading the
    /// maker notes is for later; the gate is here.
    pub local_effect: Option<String>,
}

/// The fit's inputs: the two proxies, the default render and the masks,
/// all of one size.
#[derive(Clone, Copy, Debug)]
pub struct LookInputs<'a> {
    /// The raw at its as-shot parameters through the default colour steps
    /// and exposure, scene-linear working RGB.
    pub sensor: &'a Proxy,
    /// The embedded preview, linearised, in the working space.
    pub preview: &'a Proxy,
    /// Helios's default render, display-linear working RGB: the baseline.
    pub baseline: &'a Proxy,
    /// A proxy pixel is clipped when a sensor sample under it is at 0.99
    /// of the clip level or above.
    pub clipped: &'a [bool],
    /// A proxy pixel whose preview pixels reach 255 in a channel.
    pub preview_clipped: &'a [bool],
    /// The neutral axis the matrix keeps, in the sensor proxy's space.
    pub neutral: [f64; 3],
    pub source: &'a Source,
}

/// What a fit could be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// Matrix, tone and chroma.
    Full,
    /// Tone and chroma, the matrix at half weight (or none on a grey
    /// scene).
    Partial,
    /// A tone curve alone: the camera's JPEG is black and white, or the
    /// frame is near black.
    ToneOnly,
}

/// Which pairs a look was fitted on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Attempt {
    AllPixels,
    /// Pixels whose 3×3 neighbourhood spans a luminance ratio under 3:
    /// for previews corrected for the lens, which miss the edges.
    AwayFromEdges,
}

/// Every gate, refusing a fit or (for [`Gate::NotEnoughColour`] and
/// [`Gate::Monochrome`]) narrowing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Gate {
    /// The inputs disagree in size, or hold nothing.
    Inputs,
    PreviewTooSmall,
    AspectMismatch,
    TooCompressed,
    LocalEffect,
    TooFewPairs,
    TooFewHeldOut,
    SensorClipped,
    PreviewClipped,
    FlatScene,
    Unrelated,
    Structure,
    /// No matrix: the scene has too little colour (a note, not a refusal).
    NotEnoughColour,
    /// Tone only: the preview is black and white (a note).
    Monochrome,
    /// Tone only: the frame is near black (a note).
    NearBlack,
    TooSteep,
    NonFinite,
    ClipsHighlights,
    TooFar,
    NotBetter,
}

impl Gate {
    /// The gate in words, for the module's header.
    pub fn words(self) -> &'static str {
        match self {
            Gate::Inputs => "the raw and its preview could not be read alike",
            Gate::PreviewTooSmall => "the embedded preview is too small",
            Gate::AspectMismatch => "the preview is framed differently from the raw",
            Gate::TooCompressed => "the embedded preview is too compressed",
            Gate::LocalEffect => "the camera's picture style uses local effects",
            Gate::TooFewPairs => "too little of the picture can be compared",
            Gate::TooFewHeldOut => "too little of the picture is left to check the fit",
            Gate::SensorClipped => "too much of the picture is clipped",
            Gate::PreviewClipped => "the camera's JPEG clipped highlights the raw kept",
            Gate::FlatScene => "the picture has too little contrast to learn a tone curve",
            Gate::Unrelated => "the preview does not match the raw",
            Gate::Structure => "the preview is shifted or shows another frame",
            Gate::NotEnoughColour => "the picture has too little colour for a matrix",
            Gate::Monochrome => "the camera's JPEG is black and white",
            Gate::NearBlack => "the picture is nearly black: tone only",
            Gate::TooSteep => "the camera's tone curve is too steep to follow",
            Gate::NonFinite => "the fit gave invalid numbers",
            Gate::ClipsHighlights => "the look would clip highlights the camera kept",
            Gate::TooFar => "the look stays too far from the camera's JPEG",
            Gate::NotBetter => "the look is not clearly closer to the JPEG than the default",
        }
    }
}

/// Why no look was fitted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Refusal {
    pub gate: Gate,
    /// The numbers behind it.
    pub detail: String,
}

impl Refusal {
    fn new(gate: Gate, detail: impl Into<String>) -> Refusal {
        Refusal {
            gate,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.gate.words(), self.detail)
    }
}

impl std::error::Error for Refusal {}

/// A tone curve on luminance: knots from black, `[scene, display]`,
/// strictly increasing in scene and never decreasing in display; past the
/// last knot a shoulder rises toward 1 with the last segment's slope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneCurve {
    pub knots: Vec<[f64; 2]>,
}

impl ToneCurve {
    /// The display luminance of scene luminance `s`.
    pub fn eval(&self, s: f64) -> f64 {
        let k = &self.knots;
        if s <= 0.0 || k.len() < 2 {
            return 0.0;
        }
        let last = k[k.len() - 1];
        if s >= last[0] {
            let before = k[k.len() - 2];
            let slope = (last[1] - before[1]) / (last[0] - before[0]);
            let room = 1.0 - last[1];
            if room <= 0.0 || slope <= 0.0 {
                return last[1];
            }
            return last[1] + room * (1.0 - (-slope * (s - last[0]) / room).exp());
        }
        let i = k.partition_point(|p| p[0] <= s).max(1);
        let (a, b) = (k[i - 1], k[i]);
        a[1] + (b[1] - a[1]) * (s - a[0]) / (b[0] - a[0])
    }

    /// The steepest segment's elasticity, `slope × s / d` at its middle;
    /// the segment from black (elasticity 1) left out.
    pub fn elasticity(&self) -> f64 {
        self.knots
            .windows(2)
            .skip(1)
            .filter_map(|w| {
                let (a, b) = (w[0], w[1]);
                let d = a[1] + b[1];
                (d > 0.0).then(|| (b[1] - a[1]) / (b[0] - a[0]) * (a[0] + b[0]) / d)
            })
            .fold(0.0, f64::max)
    }
}

/// The chroma curve's node count, evenly over display luminance 0 to 1.
pub const CHROMA_NODES: usize = 8;

/// A chroma ratio over display luminance, at constant luminance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChromaCurve {
    pub nodes: [f64; CHROMA_NODES],
}

impl ChromaCurve {
    /// The ratio at display luminance `d`.
    pub fn eval(&self, d: f64) -> f64 {
        let (i, f) = chroma_cell(d);
        self.nodes[i] * (1.0 - f) + self.nodes[i + 1] * f
    }
}

/// The cell and fraction of `d` among the chroma nodes.
fn chroma_cell(d: f64) -> (usize, f64) {
    let x = d.clamp(0.0, 1.0) * (CHROMA_NODES - 1) as f64;
    let i = (x.floor() as usize).min(CHROMA_NODES - 2);
    (i, x - i as f64)
}

/// A mean and a 95th percentile of Delta E 2000.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub mean: f64,
    pub p95: f64,
}

/// How a look was fitted and how well.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// The look against the preview on the held-out pairs.
    pub held_out: Score,
    /// The default render against the preview on the same pairs.
    pub baseline: Score,
    /// Usable pairs, all of them.
    pub pairs: usize,
    pub held_out_pairs: usize,
    pub attempt: Attempt,
    /// Gates that narrowed the look without refusing it.
    pub notes: Vec<Gate>,
    /// The matrix is the Delta E 2000 refinement of the linear fit.
    #[serde(default)]
    pub refined: bool,
}

/// A fitted camera look: the numbers a render needs, with the fitter's
/// version and how well it fitted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Look {
    pub version: u32,
    pub kind: Kind,
    /// Scene-linear working RGB to working RGB, keeping the neutral.
    pub matrix: Mat3,
    /// The hue/saturation table after the matrix, when it earned its
    /// place.
    #[serde(default)]
    pub table: Option<HueSatTable>,
    pub tone: ToneCurve,
    pub chroma: Option<ChromaCurve>,
    pub report: Report,
}

/// The reference render of a look: a scene-linear working RGB pixel to
/// display-linear working RGB, in [0, 1].
pub fn apply(look: &Look, rgb: [f64; 3]) -> [f64; 3] {
    render(
        &look.matrix,
        look.table.as_ref(),
        &look.tone,
        look.chroma.as_ref(),
        rgb,
    )
}

fn render(
    matrix: &Mat3,
    table: Option<&HueSatTable>,
    tone: &ToneCurve,
    chroma: Option<&ChromaCurve>,
    rgb: [f64; 3],
) -> [f64; 3] {
    let o = toned(matrix, table, tone, rgb);
    let o = match chroma {
        Some(c) => chroma_applied(c, o),
        None => o,
    };
    o.map(|v| v.clamp(0.0, 1.0))
}

/// The colour stages: the matrix, then the table.
fn coloured(matrix: &Mat3, table: Option<&HueSatTable>, rgb: [f64; 3]) -> [f64; 3] {
    let p = mat_apply(matrix, rgb);
    match table {
        Some(t) => t.apply(p),
        None => p,
    }
}

/// The colour stages and the tone curve, before the chroma curve and
/// unclamped.
fn toned(matrix: &Mat3, table: Option<&HueSatTable>, tone: &ToneCurve, rgb: [f64; 3]) -> [f64; 3] {
    let p = coloured(matrix, table, rgb);
    let s = luma(p);
    if !positive(s) {
        return [0.0; 3];
    }
    let ratio = tone.eval(s) / s;
    p.map(|v| v * ratio)
}

fn chroma_applied(c: &ChromaCurve, o: [f64; 3]) -> [f64; 3] {
    let d = luma(o);
    let k = c.eval(d);
    o.map(|v| d + k * (v - d))
}

/// Working RGB's luminance.
pub fn luma(rgb: [f64; 3]) -> f64 {
    LUMA[0] * rgb[0] + LUMA[1] * rgb[1] + LUMA[2] * rgb[2]
}

/// `v > 0`, false for NaN.
fn positive(v: f64) -> bool {
    v > 0.0
}

/// `v >= bound`, false for NaN.
fn at_least(v: f64, bound: f64) -> bool {
    v >= bound
}

fn finite(rgb: [f64; 3]) -> bool {
    rgb.iter().all(|v| v.is_finite())
}

/// The held-out split of a `width × height` proxy: `true` for a held-out
/// pixel. The proxy is cut into [`TILE`]-pixel tiles, numbered row by
/// row; every tile whose number is 0 modulo 3 is held out. It depends on
/// the size alone.
pub fn split(width: usize, height: usize) -> Vec<bool> {
    let tiles_x = width.div_ceil(TILE);
    (0..height)
        .flat_map(|y| (0..width).map(move |x| ((y / TILE) * tiles_x + x / TILE).is_multiple_of(3)))
        .collect()
}

/// The usable pairs of one attempt, as proxy indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pairs {
    /// Colour pairs for fitting: finite, not clipped on either side,
    /// neither near black nor a highlight in the preview.
    pub train: Vec<usize>,
    /// The same, held out.
    pub held: Vec<usize>,
    /// Tone pairs for fitting: finite and not clipped, highlights and
    /// shadows kept.
    pub tone: Vec<usize>,
}

impl Pairs {
    pub fn count(&self) -> usize {
        self.train.len() + self.held.len()
    }
}

/// The pairs of `inputs`, held out by `held` ([`split`]); `keep`, when
/// given, narrows them (the away-from-edges attempt).
pub fn pairs(inputs: &LookInputs<'_>, held: &[bool], keep: Option<&[bool]>) -> Pairs {
    pairs_from(inputs, held, keep, t::NEAR_BLACK)
}

/// [`pairs`], the near-black floor at `floor`.
fn pairs_from(inputs: &LookInputs<'_>, held: &[bool], keep: Option<&[bool]>, floor: f64) -> Pairs {
    let mut out = Pairs::default();
    for i in 0..inputs.sensor.rgb.len() {
        let (x, y) = (inputs.sensor.rgb[i], inputs.preview.rgb[i]);
        if !finite(x) || !finite(y) || inputs.clipped[i] || keep.is_some_and(|k| !k[i]) {
            continue;
        }
        if !positive(luma(x)) {
            continue;
        }
        let ly = luma(y);
        if held[i] {
            if ly >= floor {
                out.held.push(i);
            }
            continue;
        }
        out.tone.push(i);
        if (floor..t::HIGHLIGHT).contains(&ly) && !inputs.preview_clipped[i] {
            out.train.push(i);
        }
    }
    out
}

/// The pixels away from edges: their 3×3 neighbourhood on the sensor
/// spans a luminance ratio under 3.
fn away_from_edges(sensor: &Proxy) -> Vec<bool> {
    let (w, h) = (sensor.width, sensor.height);
    let l: Vec<f64> = sensor.rgb.iter().map(|&p| luma(p)).collect();
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for yy in y.saturating_sub(1)..(y + 2).min(h) {
                for xx in x.saturating_sub(1)..(x + 2).min(w) {
                    let v = l[yy * w + xx];
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            out[y * w + x] = lo > 0.0 && hi / lo < 3.0;
        }
    }
    out
}

/// The ridge weight toward the identity, per unit of pair weight.
const MATRIX_RIDGE: f64 = 1e-5;

/// The matrix from sensor to preview chromaticities: `P = I + C Bᵀ` with
/// `B` a basis orthogonal to `neutral`, so `P neutral = neutral` holds by
/// construction; `C` (six unknowns) by linear least squares of
/// `(I − ŷ Lᵀ) P x̂ = 0` (the render of `x̂` along the preview's `ŷ`, both
/// normalised by luminance), ridge-regularised toward the identity.
/// `None` when the system is singular or a coefficient passes the bound.
pub fn fit_matrix(sensor: &[[f64; 3]], preview: &[[f64; 3]], neutral: [f64; 3]) -> Option<Mat3> {
    let basis = orthogonal_basis(normalise(neutral)?);
    let mut a = [[0.0; 6]; 6];
    let mut b = [0.0; 6];
    let mut weight = 0.0;
    for (&x, &y) in sensor.iter().zip(preview) {
        let (lx, ly) = (luma(x), luma(y));
        if !(lx > 0.0 && ly > 0.0) {
            continue;
        }
        weight += ly;
        let xh = x.map(|v| v / lx);
        let yh = y.map(|v| v / ly);
        // A = I − ŷ Lᵀ.
        let am: Mat3 = std::array::from_fn(|r| {
            std::array::from_fn(|c| f64::from(u8::from(r == c)) - yh[r] * LUMA[c])
        });
        let z = [dot(basis[0], xh), dot(basis[1], xh)];
        // Weighted by the preview's luminance: a dark pixel's
        // chromaticity is the 8-bit preview's least certain.
        let w = ly;
        let r0 = mat_apply(&am, xh);
        // Column (i, j) of the Jacobian: A's column i times z_j.
        let jac: [[f64; 3]; 6] =
            std::array::from_fn(|k| std::array::from_fn(|r| am[r][k / 2] * z[k % 2]));
        for p in 0..6 {
            for q in 0..6 {
                a[p][q] += w * dot(jac[p], jac[q]);
            }
            b[p] -= w * dot(jac[p], r0);
        }
    }
    if !positive(weight) {
        return None;
    }
    for (p, row) in a.iter_mut().enumerate() {
        row[p] += MATRIX_RIDGE * weight;
    }
    let c = solve(a.iter().map(|r| r.to_vec()).collect(), b.to_vec())?;
    let mut m = IDENTITY;
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell += c[i * 2] * basis[0][j] + c[i * 2 + 1] * basis[1][j];
        }
    }
    bounded(m)
}

fn bounded(m: Mat3) -> Option<Mat3> {
    let ok = m
        .iter()
        .flatten()
        .all(|v| v.is_finite() && v.abs() <= t::MATRIX_BOUND);
    ok.then_some(m)
}

/// The pairs the Delta E 2000 refinement reads, at most: evenly picked.
pub const REFINE_PAIRS: usize = 1024;

/// The refinement's Levenberg-Marquardt steps, at most.
pub const REFINE_ITERATIONS: usize = 30;

/// The matrix `start` (keeping `neutral`) refined on Delta E 2000: the
/// six unknowns of `P = start + U Bᵀ` (`B` orthogonal to the neutral, the
/// chart solver's parametrisation, so the neutral stays kept) minimise
/// `Σ ΔE00(Lab(P x scaled to the preview's luminance), Lab(y))²` by the
/// chart solver's Levenberg-Marquardt. The lightness is the tone curve's,
/// so the refinement weighs colour alone, as the linear fit does. `None`
/// when no pair is usable or a coefficient leaves the bound.
pub fn refine_matrix(
    start: &Mat3,
    sensor: &[[f64; 3]],
    preview: &[[f64; 3]],
    neutral: [f64; 3],
) -> Option<Mat3> {
    let basis = orthogonal_basis(normalise(neutral)?);
    let stride = sensor.len().div_ceil(REFINE_PAIRS).max(1);
    let picked: Vec<([f64; 3], f64, [f64; 3])> = sensor
        .iter()
        .zip(preview)
        .step_by(stride)
        .filter_map(|(&x, &y)| {
            let (lx, ly) = (luma(x), luma(y));
            (positive(lx) && positive(ly) && finite(x) && finite(y))
                .then(|| (x.map(|v| v / lx), ly, lab(y)))
        })
        .collect();
    if picked.is_empty() {
        return None;
    }
    let residuals = |u: &[f64]| {
        let m = assemble(start, &basis, u);
        picked
            .iter()
            .map(|&(xh, ly, target)| {
                let o = mat_apply(&m, xh);
                let lo = luma(o);
                if positive(lo) {
                    delta_e_2000(lab(o.map(|v| v * ly / lo)), target)
                } else {
                    100.0
                }
            })
            .collect()
    };
    let u = levenberg_marquardt(residuals, vec![0.0; 6], REFINE_ITERATIONS);
    bounded(assemble(start, &basis, &u))
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Gaussian elimination with partial pivoting; `None` when singular.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let mut pivot = col;
        for r in col + 1..n {
            if a[r][col].abs() > a[pivot][col].abs() {
                pivot = r;
            }
        }
        if !positive(a[pivot][col].abs() - 1e-300) {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let (top, bottom) = a.split_at_mut(col + 1);
        let pivot_row = &top[col];
        for (k, row) in bottom.iter_mut().enumerate() {
            let f = row[col] / pivot_row[col];
            for (x, p) in row[col..].iter_mut().zip(&pivot_row[col..]) {
                *x -= f * p;
            }
            b[col + 1 + k] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|c| a[r][c] * x[c]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// The quantiles the tone curve is matched at: two extra in the deep
/// shadows, the middles of 32 equal shares, one near the top.
fn tone_quantiles() -> Vec<f64> {
    let mut q = vec![0.25 / 32.0, 0.5 / 32.0];
    q.extend((0..32).map(|k| (f64::from(k) + 0.5) / 32.0));
    q.push(1.0 - 0.25 / 32.0);
    q
}

/// The value at quantile `q` of sorted `v`, linearly interpolated.
fn quantile(v: &[f64], q: f64) -> f64 {
    let x = q * (v.len() - 1) as f64;
    let i = x.floor() as usize;
    let j = (i + 1).min(v.len() - 1);
    v[i] + (v[j] - v[i]) * (x - i as f64)
}

fn sorted(v: &[f64]) -> Vec<f64> {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v
}

/// The tone curve from scene to display luminance by quantile matching:
/// the same quantile of each side is a knot, so the curve is monotone by
/// construction and needs no exact registration. Black is kept. A curve
/// steeper than [`thresholds::MAX_ELASTICITY`] is smoothed once (a
/// three-knot window) and refused if it still is.
///
/// # Errors
/// [`Gate::TooFewPairs`] with fewer than two distinct knots,
/// [`Gate::TooSteep`].
pub fn fit_tone(scene: &[f64], display: &[f64]) -> Result<ToneCurve, Refusal> {
    let (s, d) = (sorted(scene), sorted(display));
    if s.len() < 2 || d.len() < 2 {
        return Err(Refusal::new(Gate::TooFewPairs, "no tone pairs"));
    }
    let mut knots = vec![[0.0, 0.0]];
    for q in tone_quantiles() {
        let (x, y) = (quantile(&s, q), quantile(&d, q).max(0.0));
        let last = knots[knots.len() - 1];
        if x > last[0] * (1.0 + 1e-9) && x > 0.0 {
            knots.push([x, y.max(last[1])]);
        }
    }
    if knots.len() < 3 {
        return Err(Refusal::new(
            Gate::TooFewPairs,
            "the scene's luminances are all alike",
        ));
    }
    let mut curve = ToneCurve { knots };
    if curve.elasticity() > t::MAX_ELASTICITY {
        let k = &curve.knots;
        let mut smooth = k.clone();
        for i in 1..k.len() - 1 {
            smooth[i][1] = (k[i - 1][1] + k[i][1] + k[i + 1][1]) / 3.0;
        }
        curve = ToneCurve { knots: smooth };
    }
    let e = curve.elasticity();
    if e > t::MAX_ELASTICITY {
        return Err(Refusal::new(
            Gate::TooSteep,
            format!("elasticity {e:.2} over {}", t::MAX_ELASTICITY),
        ));
    }
    Ok(curve)
}

/// The chroma ridge toward 1, per pair.
const CHROMA_RIDGE: f64 = 1e-6;

/// The chroma curve: the ratio, over display luminance, that takes the
/// chroma of `toned` (renders before the chroma curve) to the preview's,
/// at constant luminance; least squares on linearly interpolated nodes,
/// ridge-regularised toward 1, each node within [0, 2].
pub fn fit_chroma(toned: &[[f64; 3]], preview: &[[f64; 3]]) -> Option<ChromaCurve> {
    const N: usize = CHROMA_NODES;
    let mut a = vec![vec![0.0; N]; N];
    let mut b = vec![0.0; N];
    let mut count = 0usize;
    for (&o, &y) in toned.iter().zip(preview) {
        let d = luma(o);
        let u = o.map(|v| v - d);
        let ly = luma(y);
        let v = y.map(|c| c - ly);
        let (i, f) = chroma_cell(d);
        let h = [(i, 1.0 - f), (i + 1, f)];
        let (uu, uv) = (dot(u, u), dot(u, v));
        for &(j, hj) in &h {
            for &(k, hk) in &h {
                a[j][k] += hj * hk * uu;
            }
            b[j] += hj * uv;
        }
        count += 1;
    }
    let mu = CHROMA_RIDGE * count.max(1) as f64;
    for j in 0..N {
        a[j][j] += mu;
        b[j] += mu;
    }
    let c = solve(a, b)?;
    let nodes = std::array::from_fn(|j| c[j].clamp(0.0, 2.0));
    Some(ChromaCurve { nodes })
}

/// A JPEG quality estimate (1 to 100) from the preview's luminance
/// quantisation table in natural order, as the IJG scaling made it.
pub fn estimate_quality(table: &[u16; 64]) -> u8 {
    const IJG: [u16; 64] = [
        16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69,
        56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81,
        104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
    ];
    let sum = |t: &[u16; 64]| t.iter().map(|&v| f64::from(v)).sum::<f64>();
    let scale = 100.0 * sum(table) / sum(&IJG);
    let q = if scale <= 100.0 {
        (200.0 - scale) / 2.0
    } else {
        5000.0 / scale
    };
    q.round().clamp(1.0, 100.0) as u8
}

/// Delta E 2000 per held-out pair, or the lightness difference alone
/// (`lightness`, for a black-and-white preview).
fn errors(
    render: impl Fn(usize) -> [f64; 3],
    preview: &Proxy,
    held: &[usize],
    lightness: bool,
) -> Vec<f64> {
    held.iter()
        .map(|&i| {
            let (a, b) = (lab(render(i)), lab(preview.rgb[i]));
            if lightness {
                (a[0] - b[0]).abs()
            } else {
                delta_e_2000(a, b)
            }
        })
        .collect()
}

fn score(errors: &[f64]) -> Score {
    if errors.is_empty() {
        return Score {
            mean: f64::INFINITY,
            p95: f64::INFINITY,
        };
    }
    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
    let s = sorted(errors);
    let at = ((0.95 * s.len() as f64).ceil() as usize).clamp(1, s.len()) - 1;
    Score { mean, p95: s[at] }
}

/// Spearman's rank correlation, ties ranked by their mean rank.
fn spearman(a: &[f64], b: &[f64]) -> f64 {
    let ranks = |v: &[f64]| {
        let mut order: Vec<usize> = (0..v.len()).collect();
        order.sort_by(|&i, &j| v[i].total_cmp(&v[j]).then(i.cmp(&j)));
        let mut r = vec![0.0; v.len()];
        let mut i = 0;
        while i < order.len() {
            let mut j = i;
            while j + 1 < order.len() && v[order[j + 1]] == v[order[i]] {
                j += 1;
            }
            let mean = (i + j) as f64 / 2.0;
            for &k in &order[i..=j] {
                r[k] = mean;
            }
            i = j + 1;
        }
        r
    };
    pearson(&ranks(a), &ranks(b))
}

fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    if a.len() < 2 {
        return 0.0;
    }
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (da, db) = (x - ma, y - mb);
        sab += da * db;
        saa += da * da;
        sbb += db * db;
    }
    if saa > 0.0 && sbb > 0.0 {
        sab / (saa * sbb).sqrt()
    } else {
        0.0
    }
}

/// The furthest shift [`structure`] looks at.
const SHIFT_SEARCH: i64 = 3;

/// Log luminance blurred by a 5×5 box, edges clamped.
fn blurred_log_luma(p: &Proxy) -> Vec<f64> {
    let (w, h) = (p.width as i64, p.height as i64);
    let l: Vec<f64> = p
        .rgb
        .iter()
        .map(|&c| {
            let v = luma(c);
            (if v.is_finite() { v.max(0.0) } else { 0.0 } + 1e-4).ln()
        })
        .collect();
    let mut out = vec![0.0; l.len()];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let (xx, yy) = ((x + dx).clamp(0, w - 1), (y + dy).clamp(0, h - 1));
                    s += l[(yy * w + xx) as usize];
                }
            }
            out[(y * w + x) as usize] = s / 25.0;
        }
    }
    out
}

/// The two proxies' structure: the best log-luminance correlation over
/// shifts of up to [`SHIFT_SEARCH`] pixels, and that shift (no shift wins
/// ties).
fn structure(sensor: &Proxy, preview: &Proxy) -> (f64, [i64; 2]) {
    let (a, b) = (blurred_log_luma(sensor), blurred_log_luma(preview));
    let (w, h) = (sensor.width as i64, sensor.height as i64);
    let m = SHIFT_SEARCH;
    let at = |dx: i64, dy: i64| {
        let (mut va, mut vb) = (Vec::new(), Vec::new());
        for y in m..h - m {
            for x in m..w - m {
                va.push(a[(y * w + x) as usize]);
                vb.push(b[((y + dy) * w + x + dx) as usize]);
            }
        }
        pearson(&va, &vb)
    };
    if w <= 2 * m || h <= 2 * m {
        return (0.0, [0, 0]);
    }
    let mut best = (at(0, 0), [0, 0]);
    for dy in -m..=m {
        for dx in -m..=m {
            let c = at(dx, dy);
            if c > best.0 + 1e-12 {
                best = (c, [dx, dy]);
            }
        }
    }
    best
}

/// The fraction of `n` pixels for which `f` holds.
fn fraction(n: usize, f: impl Fn(usize) -> bool) -> f64 {
    (0..n).filter(|&i| f(i)).count() as f64 / n.max(1) as f64
}

/// Whether a render clips more of the proxy than both the default render
/// and the preview, by more than [`thresholds::CLIP_BUDGET`]: the three
/// clipped fractions given.
pub fn clips_more(look: f64, baseline: f64, preview: f64) -> bool {
    look > baseline.max(preview) + t::CLIP_BUDGET
}

const CLIP_LEVEL: f64 = 0.999;

fn clipped_render(rgb: [f64; 3]) -> bool {
    rgb.iter().any(|&v| v >= CLIP_LEVEL)
}

/// Fits a camera look to the embedded preview, or says why not.
///
/// # Errors
/// The first gate that refuses, with its reason ([`Refusal`]).
pub fn fit(inputs: &LookInputs<'_>) -> Result<Look, Refusal> {
    check_inputs(inputs)?;
    check_preview(inputs.source)?;
    let (w, h) = (inputs.sensor.width, inputs.sensor.height);
    let n = w * h;
    let held = split(w, h);
    let all = pairs(inputs, &held, None);
    // A near-black frame: a tone curve alone, from pairs down to a lower
    // floor, with stricter counts.
    let near_black = near_black(inputs, &all.tone);
    let (floor, counts) = if near_black {
        (t::NEAR_BLACK_FLOOR, t::NEAR_BLACK_COUNTS)
    } else {
        (t::NEAR_BLACK, 1)
    };
    let all = if near_black {
        pairs_from(inputs, &held, None, floor)
    } else {
        all
    };
    check_counts(&all, counts)?;
    let sensor_clip = fraction(n, |i| inputs.clipped[i]);
    if sensor_clip > t::SENSOR_CLIP_MAX {
        return Err(Refusal::new(
            Gate::SensorClipped,
            format!("{:.0} % of the proxy clipped", sensor_clip * 100.0),
        ));
    }
    let preview_clip = fraction(n, |i| inputs.preview_clipped[i] && !inputs.clipped[i]);
    if preview_clip > t::PREVIEW_CLIP_MAX {
        return Err(Refusal::new(
            Gate::PreviewClipped,
            format!(
                "{:.1} % of the preview at 255 where the raw is not",
                preview_clip * 100.0
            ),
        ));
    }
    // Tone range, on the scene's luminances.
    let scene: Vec<f64> = sorted(
        &all.tone
            .iter()
            .map(|&i| luma(inputs.sensor.rgb[i]))
            .collect::<Vec<_>>(),
    );
    let share = (scene.len() / 32).max(1);
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let range = mean(&scene[scene.len() - share..]) / mean(&scene[..share]);
    if !at_least(range, t::TONE_RANGE) {
        return Err(Refusal::new(
            Gate::FlatScene,
            format!("brightest to darkest {range:.2}, under {}", t::TONE_RANGE),
        ));
    }
    // Related at all, then aligned.
    let every: Vec<usize> = {
        let mut v = all.tone.clone();
        v.extend(&all.held);
        v.sort_unstable();
        v
    };
    let rho = spearman(
        &every
            .iter()
            .map(|&i| luma(inputs.sensor.rgb[i]))
            .collect::<Vec<_>>(),
        &every
            .iter()
            .map(|&i| luma(inputs.preview.rgb[i]))
            .collect::<Vec<_>>(),
    );
    if !at_least(rho, t::MIN_SPEARMAN) {
        return Err(Refusal::new(
            Gate::Unrelated,
            format!("rank correlation {rho:.2}, under {}", t::MIN_SPEARMAN),
        ));
    }
    let (corr, shift) = structure(inputs.sensor, inputs.preview);
    if !at_least(corr, t::MIN_STRUCTURE) || shift.iter().any(|s| s.abs() > t::MAX_SHIFT) {
        return Err(Refusal::new(
            Gate::Structure,
            format!("correlation {corr:.2} at a shift of {shift:?} pixels"),
        ));
    }
    let monochrome = !near_black
        && fraction(n, |i| {
            let p = inputs.preview.rgb[i];
            finite(p) && {
                let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
                hi - lo > t::MONO_TOLERANCE
            }
        }) <= t::MONO_FRACTION;
    let colour = !monochrome && !near_black && enough_colour(inputs, &all.train);
    let mut notes = Vec::new();
    if near_black {
        notes.push(Gate::NearBlack);
    } else if monochrome {
        notes.push(Gate::Monochrome);
    } else if !colour {
        notes.push(Gate::NotEnoughColour);
    }
    let edges = away_from_edges(inputs.sensor);
    let kinds: &[Kind] = if monochrome || near_black {
        &[Kind::ToneOnly]
    } else if colour {
        &[Kind::Full, Kind::Partial]
    } else {
        &[Kind::Partial]
    };
    let context = Context {
        inputs,
        sensor_clip_render: fraction(n, |i| clipped_render(inputs.baseline.rgb[i])),
        preview_clip: fraction(n, |i| inputs.preview_clipped[i]),
        colour,
    };
    let mut first: Option<Refusal> = None;
    for attempt in [Attempt::AllPixels, Attempt::AwayFromEdges] {
        let p = match attempt {
            Attempt::AllPixels => all.clone(),
            Attempt::AwayFromEdges => pairs_from(inputs, &held, Some(&edges), floor),
        };
        if let Err(e) = check_counts(&p, counts) {
            first.get_or_insert(e);
            continue;
        }
        for &kind in kinds {
            match context.try_look(&p, kind, attempt) {
                Ok(mut look) => {
                    look.report.notes = notes;
                    look.report.pairs = all.count();
                    return Ok(look);
                }
                Err(e) => {
                    first.get_or_insert(e);
                }
            }
        }
    }
    Err(first.unwrap_or_else(|| Refusal::new(Gate::TooFewPairs, "no attempt could run")))
}

fn check_inputs(inputs: &LookInputs<'_>) -> Result<(), Refusal> {
    let s = inputs.sensor;
    let n = s.width * s.height;
    let same = |p: &Proxy| p.width == s.width && p.height == s.height && p.rgb.len() == n;
    if n == 0
        || !same(inputs.preview)
        || !same(inputs.baseline)
        || inputs.clipped.len() != n
        || inputs.preview_clipped.len() != n
    {
        return Err(Refusal::new(
            Gate::Inputs,
            "the proxies and masks differ in size",
        ));
    }
    if !finite(inputs.neutral) || !positive(luma(inputs.neutral)) {
        return Err(Refusal::new(Gate::Inputs, "the neutral is not a colour"));
    }
    Ok(())
}

fn check_preview(source: &Source) -> Result<(), Refusal> {
    let [pw, ph] = source.preview_size;
    let [rw, rh] = source.raw_size;
    let long = pw.max(ph);
    let raw_long = rw.max(rh);
    if pw == 0 || ph == 0 {
        return Err(Refusal::new(
            Gate::PreviewTooSmall,
            "the raw carries no preview",
        ));
    }
    if long < t::PREVIEW_MIN_EDGE || f64::from(long) < t::PREVIEW_MIN_FRACTION * f64::from(raw_long)
    {
        return Err(Refusal::new(
            Gate::PreviewTooSmall,
            format!("{pw}×{ph} for a raw of {rw}×{rh}"),
        ));
    }
    if rw > 0 && rh > 0 {
        let ratio = (f64::from(pw) / f64::from(ph)) / (f64::from(rw) / f64::from(rh));
        if (ratio - 1.0).abs() > t::ASPECT_TOLERANCE {
            return Err(Refusal::new(
                Gate::AspectMismatch,
                format!("{pw}×{ph} for a raw of {rw}×{rh}"),
            ));
        }
    }
    if let Some(q) = source.jpeg_quality
        && q < t::MIN_QUALITY
    {
        return Err(Refusal::new(
            Gate::TooCompressed,
            format!("JPEG quality about {q}, under {}", t::MIN_QUALITY),
        ));
    }
    if let Some(effect) = &source.local_effect {
        return Err(Refusal::new(Gate::LocalEffect, effect.clone()));
    }
    Ok(())
}

/// The pair counts, at least `times` the usual minimums.
fn check_counts(p: &Pairs, times: usize) -> Result<(), Refusal> {
    let (pairs, held) = (t::MIN_PAIRS * times, t::MIN_HELD_OUT * times);
    if p.count() < pairs {
        return Err(Refusal::new(
            Gate::TooFewPairs,
            format!("{} usable pairs, under {pairs}", p.count()),
        ));
    }
    if p.held.len() < held {
        return Err(Refusal::new(
            Gate::TooFewHeldOut,
            format!("{} held-out pairs, under {held}", p.held.len()),
        ));
    }
    Ok(())
}

/// Whether the frame is near black: the median preview luminance of the
/// tone pairs `idx` under [`thresholds::NEAR_BLACK`].
fn near_black(inputs: &LookInputs<'_>, idx: &[usize]) -> bool {
    if idx.is_empty() {
        return false;
    }
    let l = sorted(
        &idx.iter()
            .map(|&i| luma(inputs.preview.rgb[i]))
            .collect::<Vec<_>>(),
    );
    quantile(&l, 0.5) < t::NEAR_BLACK
}

/// Whether the training pairs carry colour enough for a matrix.
fn enough_colour(inputs: &LookInputs<'_>, train: &[usize]) -> bool {
    let mut coloured = 0usize;
    let mut m = [[0.0; 3]; 3];
    for &i in train {
        let x = inputs.sensor.rgb[i];
        let l = luma(x);
        let xh = x.map(|v| v / l);
        let (lo, hi) = (xh[0].min(xh[1]).min(xh[2]), xh[0].max(xh[1]).max(xh[2]));
        if hi - lo > t::COLOUR_SPREAD {
            coloured += 1;
        }
        for (r, row) in m.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell += xh[r] * xh[c];
            }
        }
    }
    let enough = coloured as f64 >= t::COLOUR_FRACTION * train.len() as f64;
    enough && condition(&m) <= t::MAX_CONDITION
}

/// A symmetric 3×3 matrix's condition number, from its eigenvalues
/// (the trigonometric closed form).
fn condition(m: &Mat3) -> f64 {
    let p1 = m[0][1] * m[0][1] + m[0][2] * m[0][2] + m[1][2] * m[1][2];
    let q = (m[0][0] + m[1][1] + m[2][2]) / 3.0;
    let p2 = (m[0][0] - q).powi(2) + (m[1][1] - q).powi(2) + (m[2][2] - q).powi(2) + 2.0 * p1;
    let p = (p2 / 6.0).sqrt();
    if !positive(p) {
        return 1.0;
    }
    let bm: Mat3 = std::array::from_fn(|r| {
        std::array::from_fn(|c| (m[r][c] - if r == c { q } else { 0.0 }) / p)
    });
    let det = bm[0][0] * (bm[1][1] * bm[2][2] - bm[1][2] * bm[2][1])
        - bm[0][1] * (bm[1][0] * bm[2][2] - bm[1][2] * bm[2][0])
        + bm[0][2] * (bm[1][0] * bm[2][1] - bm[1][1] * bm[2][0]);
    let phi = (det / 2.0).clamp(-1.0, 1.0).acos() / 3.0;
    let e1 = q + 2.0 * p * phi.cos();
    let e3 = q + 2.0 * p * (phi + 2.0 * std::f64::consts::PI / 3.0).cos();
    if e3 <= 0.0 { f64::INFINITY } else { e1 / e3 }
}

struct Context<'a> {
    inputs: &'a LookInputs<'a>,
    /// The default render's clipped fraction.
    sensor_clip_render: f64,
    /// The preview's clipped fraction.
    preview_clip: f64,
    colour: bool,
}

/// A look before its last gates: its stages and its held-out score.
struct Candidate {
    matrix: Mat3,
    table: Option<HueSatTable>,
    tone: ToneCurve,
    chroma: Option<ChromaCurve>,
    held_out: Score,
}

impl Candidate {
    fn render(&self, rgb: [f64; 3]) -> [f64; 3] {
        render(
            &self.matrix,
            self.table.as_ref(),
            &self.tone,
            self.chroma.as_ref(),
            rgb,
        )
    }
}

impl Context<'_> {
    /// The look completed after its colour stages: the tone curve, then
    /// the chroma curve when it helps (none for a tone-only look), and
    /// the held-out score.
    fn complete(
        &self,
        p: &Pairs,
        kind: Kind,
        matrix: Mat3,
        table: Option<HueSatTable>,
    ) -> Result<Candidate, Refusal> {
        let inputs = self.inputs;
        let (sensor, preview) = (&inputs.sensor.rgb, &inputs.preview.rgb);
        let tab = table.as_ref();
        // The table keeps the luminance: the tone curve reads the matrix's.
        let scene: Vec<f64> = p
            .tone
            .iter()
            .map(|&i| luma(mat_apply(&matrix, sensor[i])))
            .collect();
        let display: Vec<f64> = p.tone.iter().map(|&i| luma(preview[i])).collect();
        let tone = fit_tone(&scene, &display)?;
        let lightness = kind == Kind::ToneOnly;
        let held = &p.held;
        let without = score(&errors(
            |i| render(&matrix, tab, &tone, None, sensor[i]),
            inputs.preview,
            held,
            lightness,
        ));
        let chroma = if lightness {
            None
        } else {
            let fit_on: Vec<usize> = p
                .tone
                .iter()
                .copied()
                .filter(|&i| luma(preview[i]) >= t::NEAR_BLACK)
                .collect();
            let toned_px: Vec<[f64; 3]> = fit_on
                .iter()
                .map(|&i| toned(&matrix, tab, &tone, sensor[i]))
                .collect();
            let targets: Vec<[f64; 3]> = fit_on.iter().map(|&i| preview[i]).collect();
            fit_chroma(&toned_px, &targets).filter(|c| {
                let with = score(&errors(
                    |i| render(&matrix, tab, &tone, Some(c), sensor[i]),
                    inputs.preview,
                    held,
                    false,
                ));
                with.mean < without.mean * (1.0 - t::KEEP_GAIN)
            })
        };
        let held_out = match &chroma {
            Some(c) => score(&errors(
                |i| render(&matrix, tab, &tone, Some(c), sensor[i]),
                inputs.preview,
                held,
                false,
            )),
            None => without,
        };
        Ok(Candidate {
            matrix,
            table,
            tone,
            chroma,
            held_out,
        })
    }

    fn try_look(&self, p: &Pairs, kind: Kind, attempt: Attempt) -> Result<Look, Refusal> {
        let inputs = self.inputs;
        let (sensor, preview) = (&inputs.sensor.rgb, &inputs.preview.rgb);
        let pick = |v: &[[f64; 3]], idx: &[usize]| idx.iter().map(|&i| v[i]).collect::<Vec<_>>();
        let (train_x, train_y) = (pick(sensor, &p.train), pick(preview, &p.train));
        let (matrix, refined) = match kind {
            Kind::Full => {
                let m = fit_matrix(&train_x, &train_y, inputs.neutral).ok_or_else(|| {
                    Refusal::new(Gate::NotEnoughColour, "the matrix is ill-conditioned")
                })?;
                (m, refine_matrix(&m, &train_x, &train_y, inputs.neutral))
            }
            Kind::Partial if self.colour => {
                let m = match fit_matrix(&train_x, &train_y, inputs.neutral) {
                    Some(m) => std::array::from_fn(|r| {
                        std::array::from_fn(|c| f64::midpoint(m[r][c], IDENTITY[r][c]))
                    }),
                    None => IDENTITY,
                };
                (m, None)
            }
            Kind::Partial | Kind::ToneOnly => (IDENTITY, None),
        };
        // The linear matrix, or its Delta E 2000 refinement when that
        // lowers the held-out error.
        let mut best = self.complete(p, kind, matrix, None)?;
        let mut was_refined = false;
        if let Some(m) = refined
            && let Ok(c) = self.complete(p, kind, m, None)
            && c.held_out.mean < best.held_out.mean
        {
            best = c;
            was_refined = true;
        }
        // The table on what the matrix left, kept when it earns its place.
        if kind == Kind::Full {
            let scene: Vec<[f64; 3]> = train_x
                .iter()
                .map(|&x| mat_apply(&best.matrix, x))
                .collect();
            if let Some(table) = fit_table(&scene, &train_y, inputs.neutral)
                && let Ok(c) = self.complete(p, kind, best.matrix, Some(table))
                && c.held_out.mean < best.held_out.mean * (1.0 - t::KEEP_GAIN)
            {
                best = c;
            }
        }
        let held = &p.held;
        let lightness = kind == Kind::ToneOnly;
        let held_out = best.held_out;
        let baseline = score(&errors(
            |i| inputs.baseline.rgb[i],
            inputs.preview,
            held,
            lightness,
        ));
        let numbers = best
            .matrix
            .iter()
            .flatten()
            .chain(best.table.iter().flat_map(table::numbers))
            .chain(best.tone.knots.iter().flatten())
            .chain(best.chroma.iter().flat_map(|c| c.nodes.iter()));
        if !held_out.mean.is_finite() || numbers.into_iter().any(|v| !v.is_finite()) {
            return Err(Refusal::new(
                Gate::NonFinite,
                "a fitted number is not finite",
            ));
        }
        let n = sensor.len();
        let look_clip = fraction(n, |i| {
            finite(sensor[i]) && clipped_render(best.render(sensor[i]))
        });
        if clips_more(look_clip, self.sensor_clip_render, self.preview_clip) {
            return Err(Refusal::new(
                Gate::ClipsHighlights,
                format!(
                    "{:.2} % clipped, the default {:.2} %, the preview {:.2} %",
                    look_clip * 100.0,
                    self.sensor_clip_render * 100.0,
                    self.preview_clip * 100.0
                ),
            ));
        }
        if held_out.mean > t::CEILING {
            return Err(Refusal::new(
                Gate::TooFar,
                format!("Delta E {:.1}, over {}", held_out.mean, t::CEILING),
            ));
        }
        if !positive(t::BAR * baseline.mean - held_out.mean) {
            return Err(Refusal::new(
                Gate::NotBetter,
                format!(
                    "Delta E {:.1}, the default {:.1}",
                    held_out.mean, baseline.mean
                ),
            ));
        }
        Ok(Look {
            version: FITTER_VERSION,
            kind,
            matrix: best.matrix,
            table: best.table,
            tone: best.tone,
            chroma: best.chroma,
            report: Report {
                held_out,
                baseline,
                pairs: p.count(),
                held_out_pairs: held.len(),
                attempt,
                notes: Vec::new(),
                refined: was_refined,
            },
        })
    }
}
