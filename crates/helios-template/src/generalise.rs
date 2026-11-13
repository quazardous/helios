//! Generalising a workshop session (experimental, see
//! `docs/design/workshops-on-the-fly.md`, section 3, "Make it
//! automatic"): from the values the user ended with on the test photos,
//! one row per slider saying how the workshop should set it on another
//! photo, with its evidence and a default.
//!
//! The rules, for each exposed number parameter:
//!
//! 1. **A relative target where a measure explains the value.** Each
//!    candidate measure (the whole image's, each zone's, each pair of
//!    zones' `zones.ratio`) is read on each test photo at the user's
//!    value, as a goal on the output or as an offset from the input; the
//!    goal is their mean. It **explains** the value when aiming it from
//!    the workshop's neutral value lands within tolerance of the user's
//!    value on every test photo (one step of the slider, or 0.1 for an
//!    EV slider), the measure moving one way over the range (as
//!    [`crate::workshop::aim`] checks). With two test photos or more, the
//!    target must also fit them better than one fixed value would; with
//!    one, a target cannot be told from a fixed value, so the fixed
//!    value is the default and the targets that fit are offered.
//! 2. **Otherwise a fixed value**: the mean of the user's values, with
//!    their spread.
//! 3. **AI proposals become deterministic rules where one fits.** A zone
//!    the AI found (a `mask_object` answer, its mask among the case's
//!    zones) is fitted with a `mask_parametric`: luminance and hue bounds
//!    searched on the test photos, kept when the overlap (intersection
//!    over union) is at least [`MIN_OVERLAP`] on every one
//!    ([`generalise_zones`]). A value the AI proposed goes through rules
//!    1 and 2 like a user's.
//! 4. **An AI junction is kept only where nothing fits**: the zone stays
//!    an AI object member, or a value the AI proposed that no target
//!    explains and that differs between the test photos by more than the
//!    tolerance stays a decision asked per photo ([`keep_ai_values`]).
//!    The row says why.
//!
//! Nothing is written here: the rows are what the panel and the
//! `workshop.generalise` verb show for the user to confirm. It is pure:
//! the caller gives the render, as for aiming.

use std::collections::BTreeMap;

use helios_core::{Engine, Graph, Image, Macro, ModuleRef, Params, Region, Value};
use serde::Serialize;

use crate::build::SOURCE_PORT;
use crate::format::{At, Junction, JunctionKind, Measure, Target, Workshop, Zone};
use crate::workshop::{Range, aim_in, measure_in, ranges};

/// A test photo of the session.
#[derive(Clone, Copy, Debug)]
pub struct Case<'a> {
    /// What enters the workshop on it (its proxy).
    pub input: &'a Image,
    /// Each zone's mask over `input`'s region.
    pub zones: &'a BTreeMap<String, Image>,
    /// The workshop's values the user ended with on it.
    pub values: &'a Params,
}

/// What a row proposes by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    /// The same target on every photo, each aimed.
    Target,
    /// The same value on every photo.
    Fixed,
    /// The AI asked on every photo: a junction (rule 4).
    Ai,
}

/// How one candidate target did on the test photos.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Fit {
    pub target: Target,
    /// The value aiming it gives on each test photo.
    pub aimed: Vec<f64>,
    /// The largest distance from the user's value, in the slider's unit.
    pub error: f64,
    /// Within tolerance on every test photo.
    pub fits: bool,
    /// Why it does not fit, when aiming said why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One slider's row.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Row {
    /// The exposed parameter.
    pub param: String,
    /// The user's value on each test photo.
    pub values: Vec<f64>,
    /// Their mean, rounded to the slider's step: the fixed value.
    pub fixed: f64,
    /// The largest minus the smallest.
    pub spread: f64,
    /// The tolerance a target is held to.
    pub tolerance: f64,
    /// The best target that explains the values, if one does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Fit>,
    pub default: Choice,
    /// Every candidate tried, the best first: the evidence.
    pub candidates: Vec<Fit>,
    /// What the row says to the user.
    pub note: String,
}

/// The tolerance a target is held to on `range`: one step, 0.1 for an
/// EV slider, a hundredth of the range without a step.
pub fn tolerance(range: &Range) -> f64 {
    let step = match range.step.filter(|s| *s > 0.0 && s.is_finite()) {
        Some(s) if range.integer => s.max(1.0).round(),
        Some(s) => s,
        None if range.integer => 1.0,
        None => (range.max - range.min) / 100.0,
    };
    if range.unit.as_deref() == Some("EV") {
        step.max(0.1)
    } else {
        step
    }
}

/// The candidate targets for `param` with the workshop's `zones` (in the
/// file's order): the whole image's median and highlights, each zone's
/// median, highlights and contrast, and `zones.ratio` for each pair of
/// zones, the earlier against the later. The value is left at zero; the
/// form (`at`) is the output's.
pub fn candidates(param: &str, zones: &[String]) -> Vec<Target> {
    let t = |measure, zone: Option<&String>, against: Option<&String>| Target {
        param: param.into(),
        measure,
        value: 0.0,
        at: At::Output,
        zone: zone.cloned(),
        against: against.cloned(),
    };
    let mut out = vec![
        t(Measure::LumaMedian, None, None),
        t(Measure::LumaHigh, None, None),
    ];
    for z in zones {
        for m in [
            Measure::LumaMedian,
            Measure::LumaHigh,
            Measure::ZoneContrast,
        ] {
            out.push(t(m, Some(z), None));
        }
    }
    for (i, z) in zones.iter().enumerate() {
        for w in &zones[i + 1..] {
            out.push(t(Measure::ZonesRatio, Some(z), Some(w)));
        }
    }
    out
}

/// How a target reads in a note: `zones.ratio(foreground, sky)`,
/// `sky.luma.high`, `image.luma.median`, `+` an input's offset.
pub fn target_name(t: &Target) -> String {
    let base = match (&t.zone, &t.against) {
        (Some(z), Some(a)) => format!("{}({z}, {a})", t.measure.name()),
        (Some(z), None) => format!("{z}.{}", t.measure.name()),
        _ => format!("image.{}", t.measure.name()),
    };
    match t.at {
        At::Output => base,
        At::Input => format!("{base} from the input"),
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn show(v: f64, range: &Range) -> String {
    match &range.unit {
        Some(u) => format!("{} {u}", round3(v)),
        None => format!("{}", round3(v)),
    }
}

/// The rows for `params` (the sliders the session set, by exposed name)
/// over the session's test photos. `neutral` holds the workshop's
/// values before the user moved them (aiming starts there); `ranges`
/// each slider's range; `zones` the workshop's zones in the file's
/// order; `render(i, values)` renders the workshop's output on test
/// photo `i`. A slider without a number range has no row; a render that
/// fails is an error.
pub fn generalise<F>(
    mut render: F,
    cases: &[Case],
    neutral: &Params,
    params: &[String],
    ranges: &BTreeMap<String, Range>,
    zones: &[String],
) -> Result<Vec<Row>, String>
where
    F: FnMut(usize, &Params) -> Result<Image, String>,
{
    if cases.is_empty() {
        return Err("no test photo to generalise from".into());
    }
    let mut rows = Vec::new();
    for param in params {
        let Some(range) = ranges.get(param) else {
            continue;
        };
        let Some(values) = cases
            .iter()
            .map(|c| c.values.get(param).and_then(Value::as_f64))
            .collect::<Option<Vec<f64>>>()
        else {
            continue;
        };
        let row = row(&mut render, cases, neutral, param, &values, range, zones)?;
        rows.push(row);
    }
    Ok(rows)
}

fn row<F>(
    render: &mut F,
    cases: &[Case],
    neutral: &Params,
    param: &str,
    values: &[f64],
    range: &Range,
    zones: &[String],
) -> Result<Row, String>
where
    F: FnMut(usize, &Params) -> Result<Image, String>,
{
    let n = values.len() as f64;
    let fixed = range.round(values.iter().sum::<f64>() / n);
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let spread = round3(hi - lo);
    let tol = tolerance(range);
    // What the user's values give on each photo.
    let outputs = cases
        .iter()
        .enumerate()
        .map(|(i, c)| render(i, c.values))
        .collect::<Result<Vec<Image>, String>>()?;
    let mut fits = Vec::new();
    for t in candidates(param, zones) {
        for at in [At::Output, At::Input] {
            let mut t = Target { at, ..t.clone() };
            let read = |image: &Image, c: &Case| {
                let mask = |z: &Option<String>| z.as_ref().and_then(|z| c.zones.get(z));
                measure_in(image, t.measure, mask(&t.zone), mask(&t.against))
            };
            // The goal: the mean of what the user's values measure.
            let goals = cases
                .iter()
                .zip(&outputs)
                .map(|(c, out)| {
                    let m = read(out, c)?;
                    match at {
                        At::Output => Some(m),
                        At::Input => Some(m - read(c.input, c)?),
                    }
                })
                .collect::<Option<Vec<f64>>>();
            let Some(goals) = goals else {
                continue;
            };
            t.value = round3(goals.iter().sum::<f64>() / n);
            fits.push(fit(render, cases, neutral, &t, values, range, tol));
        }
    }
    let fixed_error: f64 = values.iter().map(|v| (v - fixed).abs()).sum();
    fits.sort_by(|a, b| b.fits.cmp(&a.fits).then(a.error.total_cmp(&b.error)));
    let best = fits.first().filter(|f| f.fits).cloned();
    let better = |f: &Fit| {
        let sum: f64 = f.aimed.iter().zip(values).map(|(a, v)| (a - v).abs()).sum();
        sum < fixed_error
    };
    let default = match &best {
        Some(f) if cases.len() > 1 && better(f) => Choice::Target,
        _ => Choice::Fixed,
    };
    let shown: Vec<String> = values.iter().map(|v| show(*v, range)).collect();
    let on = match shown.len() {
        1 => format!("{} on the test photo", shown[0]),
        _ => format!("{} on the test photos", shown.join(" and ")),
    };
    let note = match (&best, default) {
        (Some(f), Choice::Target) => format!(
            "{param}: {on}; target `{}` at {} {} fits {} within {}",
            target_name(&f.target),
            f.target.value,
            f.target.measure.unit(),
            if cases.len() == 2 { "both" } else { "each" },
            round3(f.error),
        ),
        (Some(f), Choice::Fixed) if cases.len() == 1 => format!(
            "{param}: {on}; one test photo cannot tell target `{}` from a fixed value, same value {}",
            target_name(&f.target),
            show(fixed, range),
        ),
        (Some(_), Choice::Fixed) => format!(
            "{param}: {on}; a fixed value fits them as well as a target, same value {}",
            show(fixed, range),
        ),
        (None, _) | (Some(_), Choice::Ai) => format!(
            "{param}: {on}; no measure explains it, same value {} (spread {})",
            show(fixed, range),
            spread,
        ),
    };
    Ok(Row {
        param: param.into(),
        values: values.to_vec(),
        fixed,
        spread,
        tolerance: tol,
        target: best,
        default,
        candidates: fits,
        note,
    })
}

/// Aims `t` on each test photo from the neutral value, the other sliders
/// at the user's.
fn fit<F>(
    render: &mut F,
    cases: &[Case],
    neutral: &Params,
    t: &Target,
    values: &[f64],
    range: &Range,
    tol: f64,
) -> Fit
where
    F: FnMut(usize, &Params) -> Result<Image, String>,
{
    let mut aimed = Vec::with_capacity(cases.len());
    let mut note = None;
    let ranges = BTreeMap::from([(t.param.clone(), range.clone())]);
    for (i, c) in cases.iter().enumerate() {
        let mut start = c.values.clone();
        match neutral.get(&t.param) {
            Some(v) => start.set(&t.param, v.clone()),
            None => start.set(&t.param, Value::Float(range.min)),
        }
        match aim_in(
            |p: &Params| render(i, p),
            c.input,
            c.zones,
            &start,
            std::slice::from_ref(t),
            &ranges,
        ) {
            Ok((answers, _)) => {
                let a = &answers[0];
                aimed.push(a.value);
                if !a.reached && note.is_none() {
                    note.clone_from(&a.note);
                }
            }
            Err(e) => {
                aimed.push(f64::NAN);
                note.get_or_insert(e);
            }
        }
    }
    let error = aimed
        .iter()
        .zip(values)
        .map(|(a, v)| (a - v).abs())
        .fold(
            0.0,
            |m: f64, e| if e.is_nan() { f64::INFINITY } else { m.max(e) },
        );
    // Half a thousandth over: what rounding to the step leaves.
    let fits = note.is_none() && error <= tol + 5e-4;
    Fit {
        target: t.clone(),
        aimed,
        error: round3(error),
        fits,
        note,
    }
}

/// Rule 4 for values: each row of `proposed` (the sliders the AI set) that
/// no target explains and whose values differ by more than the tolerance
/// stays a decision asked per photo; one value holding them all is a
/// fixed value, a target a target.
pub fn keep_ai_values(rows: &mut [Row], proposed: &[String]) {
    for row in rows.iter_mut() {
        if !proposed.contains(&row.param) || row.target.is_some() {
            continue;
        }
        if row.spread <= row.tolerance + 5e-4 {
            continue;
        }
        row.default = Choice::Ai;
        let head = row
            .note
            .split("; ")
            .next()
            .unwrap_or(&row.param)
            .to_string();
        row.note = format!(
            "{head}; the AI's values, no measure explains them and they spread {} over a tolerance of {}; kept as AI, 1 call per photo",
            row.spread, row.tolerance,
        );
    }
}

/// The overlap (intersection over union) a parametric fit of an AI zone
/// must reach on every test photo to replace it.
pub const MIN_OVERLAP: f64 = 0.9;

/// What an AI zone's row proposes by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ZoneChoice {
    /// A `mask_parametric` with the fitted bounds: no AI call.
    Parametric,
    /// The AI object member, asked per photo: a junction.
    Ai,
}

/// One parametric mask tried on an AI zone.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ZoneFit {
    /// The `mask_parametric` parameters: `luminance`, and `hue` when
    /// bounded, each low, high and no feather.
    pub params: Params,
    /// Its overlap with the AI's mask on each test photo.
    pub overlaps: Vec<f64>,
    /// The smallest of them.
    pub overlap: f64,
    /// At least [`MIN_OVERLAP`] on every test photo.
    pub fits: bool,
}

/// One AI zone's row.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ZoneRow {
    pub zone: String,
    /// The best fit, when it fits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fit: Option<ZoneFit>,
    pub default: ZoneChoice,
    /// The best fits tried, the best first: the evidence.
    pub candidates: Vec<ZoneFit>,
    /// What the row says to the user.
    pub note: String,
}

/// How many fits a zone's row keeps as evidence.
const ZONE_EVIDENCE: usize = 5;

/// The quantiles the bounds are searched at, of the zone's own pixels on
/// the test photos: the lowest lows and the highest highs.
const LOWS: [f64; 5] = [0.0, 0.01, 0.02, 0.05, 0.1];
const HIGHS: [f64; 5] = [1.0, 0.99, 0.98, 0.95, 0.9];
const HUES: [(f64, f64); 3] = [(0.0, 1.0), (0.02, 0.98), (0.05, 0.95)];

/// A pixel as the parametric mask reads it.
struct Px {
    luminance: f32,
    hue: f32,
    inside: bool,
}

fn quantile(sorted: &[f32], q: f64) -> Option<f32> {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let i = ((sorted.len().checked_sub(1)? as f64) * q).round() as usize;
    sorted.get(i).copied()
}

/// A bound rounded outwards to a ten-thousandth, so the pixel it came
/// from stays inside (the mask is 0 at an unfeathered low).
fn low_bound(v: f32) -> f64 {
    ((f64::from(v) * 1e4).floor() / 1e4 - 1e-4).max(0.0)
}

fn high_bound(v: f32) -> f64 {
    (f64::from(v) * 1e4).ceil() / 1e4
}

/// Rule 3 for zones: each of `ai` (the zones the AI found, their masks
/// among each case's zones) fitted with a `mask_parametric` on the test
/// photos' inputs. Luminance bounds at the quantiles of the zone's own
/// pixels, open or not, with or without hue bounds, are each drawn as the
/// module draws them and compared with the AI's mask; the best is kept
/// when it overlaps at least [`MIN_OVERLAP`] on every test photo, else the
/// zone stays an AI junction (rule 4). A zone missing on a case is an
/// error.
pub fn generalise_zones(cases: &[Case], ai: &[String]) -> Result<Vec<ZoneRow>, String> {
    use helios_module_masks::{Channel, Range as Band, jzczhz, luminance};

    if cases.is_empty() {
        return Err("no test photo to generalise from".into());
    }
    let mut rows = Vec::new();
    for zone in ai {
        // Each photo's pixels, and the zone's luminance and hue, sorted.
        let mut photos = Vec::with_capacity(cases.len());
        let (mut lums, mut hues) = (Vec::new(), Vec::new());
        for (i, c) in cases.iter().enumerate() {
            let mask = c
                .zones
                .get(zone)
                .ok_or_else(|| format!("zone `{zone}` has no mask on test photo {}", i + 1))?;
            let n = c.input.channels() as usize;
            let m = mask.channels() as usize;
            if n < 3 || m == 0 || c.input.data().len() / n != mask.data().len() / m {
                return Err(format!(
                    "zone `{zone}`: its mask is not the size of test photo {}",
                    i + 1
                ));
            }
            let px: Vec<Px> = c
                .input
                .data()
                .chunks_exact(n)
                .zip(mask.data().chunks_exact(m))
                .map(|(p, k)| Px {
                    luminance: luminance(p),
                    hue: jzczhz(p)[2],
                    inside: k[0] >= 0.5,
                })
                .collect();
            let mut l: Vec<f32> = px
                .iter()
                .filter(|p| p.inside)
                .map(|p| p.luminance)
                .collect();
            let mut h: Vec<f32> = px.iter().filter(|p| p.inside).map(|p| p.hue).collect();
            l.sort_by(f32::total_cmp);
            h.sort_by(f32::total_cmp);
            lums.push(l);
            hues.push(h);
            photos.push(px);
        }
        // The bounds: the lowest low and the highest high over the photos.
        let low_at = |sets: &[Vec<f32>], q| {
            sets.iter()
                .filter_map(|s| quantile(s, q))
                .min_by(f32::total_cmp)
        };
        let high_at = |sets: &[Vec<f32>], q| {
            sets.iter()
                .filter_map(|s| quantile(s, q))
                .max_by(f32::total_cmp)
        };
        let span = f64::from(Channel::Luminance.span());
        let mut lows: Vec<f64> = LOWS
            .iter()
            .filter_map(|q| low_at(&lums, *q).map(low_bound))
            .collect();
        lows.push(0.0);
        let mut highs: Vec<f64> = HIGHS
            .iter()
            .filter_map(|q| high_at(&lums, *q).map(high_bound))
            .collect();
        highs.push(span);
        lows.dedup();
        highs.dedup();
        let mut bands: Vec<Option<(f64, f64)>> = vec![None];
        for (a, b) in HUES {
            if let (Some(lo), Some(hi)) = (low_at(&hues, a), high_at(&hues, b)) {
                let band = Some((low_bound(lo), high_bound(hi).min(1.0)));
                if !bands.contains(&band) {
                    bands.push(band);
                }
            }
        }
        let mut fits = Vec::new();
        for &lo in &lows {
            for &hi in &highs {
                if hi < lo {
                    continue;
                }
                #[allow(clippy::cast_possible_truncation)]
                let l = Band::new(lo as f32, hi as f32, 0.0, 0.0, span as f32, false);
                for band in &bands {
                    #[allow(clippy::cast_possible_truncation)]
                    let h = band.map(|(a, b)| {
                        Band::new(a as f32, b as f32, 0.0, 0.0, Channel::Hue.span(), false)
                    });
                    let overlaps: Vec<f64> = photos
                        .iter()
                        .map(|px| {
                            let (mut both, mut either) = (0usize, 0usize);
                            for p in px {
                                let f = l.factor(p.luminance) * h.map_or(1.0, |h| h.factor(p.hue));
                                let drawn = f >= 0.5;
                                both += usize::from(drawn && p.inside);
                                either += usize::from(drawn || p.inside);
                            }
                            #[allow(clippy::cast_precision_loss)]
                            if either == 0 {
                                1.0
                            } else {
                                round3(both as f64 / either as f64)
                            }
                        })
                        .collect();
                    let overlap = overlaps.iter().copied().fold(1.0, f64::min);
                    let mut params =
                        Params::new().with("luminance", Value::List(vec![lo, hi, 0.0, 0.0]));
                    if let Some((a, b)) = band {
                        params.set("hue", Value::List(vec![*a, *b, 0.0, 0.0]));
                    }
                    fits.push(ZoneFit {
                        params,
                        overlaps,
                        overlap,
                        fits: overlap >= MIN_OVERLAP,
                    });
                }
            }
        }
        // The best overlap first; on a tie, the fewer channels.
        fits.sort_by(|a, b| {
            b.overlap.total_cmp(&a.overlap).then(
                a.params
                    .get("hue")
                    .is_some()
                    .cmp(&b.params.get("hue").is_some()),
            )
        });
        fits.truncate(ZONE_EVIDENCE);
        let best = fits.first().filter(|f| f.fits).cloned();
        let note = match (&best, fits.first()) {
            (Some(f), _) => format!(
                "{zone}: a parametric mask ({}) overlaps the AI's {} {}; no AI call",
                bounds(&f.params),
                f.overlap,
                if cases.len() == 1 {
                    "on the test photo".to_string()
                } else {
                    "or more on each test photo".to_string()
                },
            ),
            (None, Some(f)) => {
                let worst = f
                    .overlaps
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.total_cmp(b.1))
                    .map_or(0, |(i, _)| i);
                format!(
                    "{zone}: the parametric mask overlaps {} on {}; kept as AI, 1 call per photo",
                    f.overlap,
                    photo_name(worst, cases.len()),
                )
            }
            (None, None) => {
                format!("{zone}: no parametric mask to try; kept as AI, 1 call per photo")
            }
        };
        rows.push(ZoneRow {
            zone: zone.clone(),
            default: if best.is_some() {
                ZoneChoice::Parametric
            } else {
                ZoneChoice::Ai
            },
            fit: best,
            candidates: fits,
            note,
        });
    }
    Ok(rows)
}

/// The bounds of a fit, as a note reads them: `luminance 0.4 to open`.
fn bounds(params: &Params) -> String {
    let span = f64::from(helios_module_masks::Channel::Luminance.span());
    let one = |name: &str, open: f64| {
        let v = params.get(name).and_then(Value::as_list)?;
        let lo = if v[0] <= 0.0 {
            "open".to_string()
        } else {
            format!("{}", round3(v[0]))
        };
        let hi = if v[1] >= open {
            "open".to_string()
        } else {
            format!("{}", round3(v[1]))
        };
        Some(format!("{name} {lo} to {hi}"))
    };
    [one("luminance", span), one("hue", 1.0)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ")
}

/// `the first photo`, `the second photo`…, `the test photo` alone.
fn photo_name(i: usize, n: usize) -> String {
    const NTH: [&str; 5] = ["first", "second", "third", "fourth", "fifth"];
    match (n, NTH.get(i)) {
        (1, _) => "the test photo".into(),
        (_, Some(nth)) => format!("the {nth} photo"),
        _ => format!("test photo {}", i + 1),
    }
}

/// [`generalise`] through `engine`: `built` (the workshop) rendered alone
/// on each case's input, its sliders' ranges read from it ([`ranges`]),
/// neutral at its defaults.
pub fn generalise_rendered(
    engine: &Engine,
    built: &Macro,
    cases: &[Case],
    params: &[String],
    zones: &[String],
) -> Result<Vec<Row>, String> {
    let module = ModuleRef::composite(built.clone());
    let manifest = module.manifest().clone();
    let (Some(entry), Some(exit)) = (manifest.inputs.first(), manifest.outputs.first()) else {
        return Err(format!("`{}` has no input or no output", manifest.name));
    };
    let mut graphs = Vec::with_capacity(cases.len());
    let mut neutral = Params::new();
    for c in cases {
        let r = c.input.region();
        let region = Region::sized(r.width, r.height);
        let input = Image::new(region, c.input.channels(), c.input.to_vec());
        let mut graph = Graph::new();
        let src = graph.add_source("input", input);
        let node = graph
            .add_module("workshop", module.clone(), &Params::new())
            .map_err(|e| e.to_string())?;
        graph
            .connect(src, SOURCE_PORT, node, &entry.name)
            .map_err(|e| e.to_string())?;
        neutral = graph
            .params(node)
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        graphs.push((graph, node, region));
    }
    let exit = exit.name.clone();
    generalise(
        |i: usize, p: &Params| {
            let (graph, node, region) = &mut graphs[i];
            for (name, value) in p.iter() {
                graph
                    .set_param(*node, name, value.clone())
                    .map_err(|e| e.to_string())?;
            }
            engine
                .render(graph, *node, &exit, *region)
                .map_err(|e| e.to_string())
        },
        cases,
        &neutral,
        params,
        &ranges(built),
        zones,
    )
}

/// How little AI a generalised workshop keeps (section 3, "Minimum AI,
/// measured"): `J`, the junctions left, and `A`, the AI calls expected
/// per photo in a run. Each junction is asked once on a photo the
/// decision book has not answered, so `A = J`; which photos of a set the
/// book already answers (the cache's share) is the run's to say.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Minimum {
    #[serde(rename = "J")]
    pub junctions: usize,
    #[serde(rename = "A")]
    pub calls_per_photo: f64,
}

/// `J` and `A` for these rows, as they stand (their defaults, or the
/// user's choices once [`choose`] amended them).
pub fn minimum(rows: &[Row], zones: &[ZoneRow]) -> Minimum {
    let junctions = rows.iter().filter(|r| r.default == Choice::Ai).count()
        + zones.iter().filter(|z| z.default == ZoneChoice::Ai).count();
    Minimum {
        junctions,
        calls_per_photo: junctions as f64,
    }
}

/// The user's choices on the rows, by slider or zone name: `target`,
/// `fixed` or `ai` for a slider, `parametric` or `ai` for a zone. A
/// target needs one that fits the test photos; a parametric mask needs a
/// fit, the best tried when none reached [`MIN_OVERLAP`]. Refused for a
/// name no row has.
pub fn choose(
    rows: &mut [Row],
    zones: &mut [ZoneRow],
    choices: &BTreeMap<String, String>,
) -> Result<(), String> {
    for (name, choice) in choices {
        if let Some(row) = rows.iter_mut().find(|r| r.param == *name) {
            row.default = match choice.as_str() {
                "target" if row.target.is_some() => Choice::Target,
                "target" => {
                    return Err(format!(
                        "`{name}`: no measure explains its values, so it has no target"
                    ));
                }
                "fixed" => Choice::Fixed,
                "ai" => Choice::Ai,
                other => {
                    return Err(format!("`{name}`: target, fixed or ai, not `{other}`"));
                }
            };
        } else if let Some(zone) = zones.iter_mut().find(|z| z.zone == *name) {
            zone.default = match choice.as_str() {
                "parametric" => {
                    if zone.fit.is_none() {
                        zone.fit = zone.candidates.first().cloned();
                    }
                    if zone.fit.is_none() {
                        return Err(format!("`{name}`: no parametric mask was fitted"));
                    }
                    ZoneChoice::Parametric
                }
                "ai" => ZoneChoice::Ai,
                other => return Err(format!("`{name}`: parametric or ai, not `{other}`")),
            };
        } else {
            return Err(format!("`{name}`: no row of that name"));
        }
    }
    Ok(())
}

/// The workshop block and the new slider defaults the rows write, from
/// the block the workshop has (`block`): a target row's target replaces
/// any target of its slider; a fixed row drops it and its value becomes
/// the slider's default; a zone fitted parametric is drawn by a
/// `mask_parametric` with the fitted bounds. A row left on the AI is
/// written as a junction (asked per photo by a run): a slider's with its
/// fixed value as the default and the fallback, a zone's as the AI zone
/// it is. Every other junction of the rows' sliders and zones is dropped.
pub fn written(
    block: &Workshop,
    rows: &[Row],
    zones: &[ZoneRow],
) -> Result<(Workshop, Params), String> {
    let mut out = block.clone();
    let mut defaults = Params::new();
    for row in rows {
        out.targets.retain(|t| t.param != row.param);
        out.junctions.retain(|j| j.name != row.param);
        match (row.default, &row.target) {
            (Choice::Target, Some(fit)) => out.targets.push(fit.target.clone()),
            (Choice::Ai, _) => {
                defaults.set(&row.param, Value::Float(row.fixed));
                out.junctions.push(Junction {
                    kind: JunctionKind::Value,
                    name: row.param.clone(),
                    fallback: Some(row.fixed),
                    note: Some(row.note.clone()),
                });
            }
            _ => defaults.set(&row.param, Value::Float(row.fixed)),
        }
    }
    for z in zones {
        out.junctions.retain(|j| j.name != z.zone);
        let Some(zone) = out.zones.iter_mut().find(|x| x.name == z.zone) else {
            return Err(format!(
                "`{}`: the workshop has no zone of that name",
                z.zone
            ));
        };
        match (z.default, &z.fit) {
            (ZoneChoice::Parametric, Some(fit)) => {
                *zone = Zone {
                    name: z.zone.clone(),
                    module: PARAMETRIC.into(),
                    params: fit.params.clone(),
                    port: "mask".into(),
                };
            }
            (ZoneChoice::Parametric, None) => {
                return Err(format!("`{}`: no parametric mask was fitted", z.zone));
            }
            (ZoneChoice::Ai, _) => {
                if zone.module != AI_ZONE {
                    return Err(format!(
                        "`{}`: drawn by `{}`, it cannot be asked to the AI",
                        z.zone, zone.module
                    ));
                }
                out.junctions.push(Junction {
                    kind: JunctionKind::Zone,
                    name: z.zone.clone(),
                    fallback: None,
                    note: Some(z.note.clone()),
                });
            }
        }
    }
    Ok((out, defaults))
}

/// The module a fitted zone is drawn by.
pub const PARAMETRIC: &str = "mask_parametric";

/// The module of a zone the AI draws, per photo: such a zone is fitted
/// by rule 3.
pub const AI_ZONE: &str = "mask_object";

#[cfg(test)]
mod tests {
    use super::*;

    const W: i64 = 32;
    const SKY_ROWS: i64 = 12;

    /// A backlit photo: a sky band at `sky` stops over a foreground at
    /// `ground` stops, each a small ramp of a tenth of a stop.
    fn backlit(sky: f64, ground: f64) -> Image {
        Image::from_fn(Region::sized(W as u32, W as u32), 4, |x, y, c| {
            if c == 3 {
                return 1.0;
            }
            let base = if y < SKY_ROWS { sky } else { ground };
            let t = x as f64 / (W - 1) as f64;
            (base + 0.1 * t).exp2() as f32
        })
    }

    fn masks() -> BTreeMap<String, Image> {
        let band = |sky: bool| {
            Image::from_fn(Region::sized(W as u32, W as u32), 1, move |_, y, _| {
                f32::from(u8::from((y < SKY_ROWS) == sky))
            })
        };
        BTreeMap::from([
            ("foreground".to_string(), band(false)),
            ("sky".to_string(), band(true)),
        ])
    }

    /// The workshop: `lift` EV on the foreground alone.
    fn lifted(input: &Image, lift: f64) -> Image {
        let gain = lift.exp2() as f32;
        Image::from_fn(input.region(), 4, |x, y, c| {
            let v = input.data()[((y * W + x) * 4 + i64::from(c)) as usize];
            if c == 3 || y < SKY_ROWS { v } else { v * gain }
        })
    }

    fn lift_range() -> BTreeMap<String, Range> {
        let mut r = Range::new(0.0, 4.0);
        r.step = Some(0.01);
        r.unit = Some("EV".into());
        BTreeMap::from([("backlight_lift".to_string(), r)])
    }

    fn run(photos: &[(Image, f64)]) -> Row {
        let zones = masks();
        let values: Vec<Params> = photos
            .iter()
            .map(|(_, v)| Params::new().with("backlight_lift", Value::Float(*v)))
            .collect();
        let cases: Vec<Case> = photos
            .iter()
            .zip(&values)
            .map(|((input, _), values)| Case {
                input,
                zones: &zones,
                values,
            })
            .collect();
        let mut rows = generalise(
            |i, p: &Params| Ok(lifted(&photos[i].0, p.float("backlight_lift"))),
            &cases,
            &Params::new().with("backlight_lift", Value::Float(0.0)),
            &["backlight_lift".to_string()],
            &lift_range(),
            &["foreground".to_string(), "sky".to_string()],
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        rows.remove(0)
    }

    #[test]
    fn a_backlight_gap_kept_by_the_user_becomes_a_zones_ratio_target() {
        // The user closed the gap to 2 stops on both: lifted by 2 EV on
        // the first, by 1 EV on the second, whose sky is a stop darker.
        let row = run(&[(backlit(0.0, -4.0), 2.0), (backlit(-1.0, -4.0), 1.0)]);
        assert_eq!(row.default, Choice::Target, "{}", row.note);
        let fit = row.target.as_ref().unwrap();
        assert_eq!(fit.target.measure, Measure::ZonesRatio, "{fit:?}");
        assert_eq!(fit.target.zone.as_deref(), Some("foreground"));
        assert_eq!(fit.target.against.as_deref(), Some("sky"));
        assert_eq!(fit.target.at, At::Output);
        assert!((fit.target.value + 2.0).abs() < 0.02, "{fit:?}");
        assert!(fit.error <= 0.1, "{fit:?}");
        assert_eq!(row.fixed, 1.5);
        assert_eq!(row.spread, 1.0);
        assert!(
            row.note.starts_with(
                "backlight_lift: 2 EV and 1 EV on the test photos; target `zones.ratio(foreground, sky)` at -2"
            ),
            "{}",
            row.note
        );
        // The sky does not move with the lift: not a candidate that fits.
        let sky = row
            .candidates
            .iter()
            .find(|f| f.target.zone.as_deref() == Some("sky") && f.target.against.is_none())
            .unwrap();
        assert!(!sky.fits && sky.note.is_some(), "{sky:?}");
        // The foreground's median alone differs on the two: it does not fit.
        let ground = row
            .candidates
            .iter()
            .find(|f| {
                f.target.zone.as_deref() == Some("foreground")
                    && f.target.measure == Measure::LumaMedian
                    && f.target.at == At::Output
            })
            .unwrap();
        assert!(!ground.fits, "{ground:?}");
    }

    #[test]
    fn the_same_value_on_both_photos_stays_a_fixed_value() {
        let row = run(&[(backlit(0.0, -4.0), 1.5), (backlit(-1.0, -4.0), 1.5)]);
        assert_eq!(row.default, Choice::Fixed, "{}", row.note);
        assert_eq!(row.fixed, 1.5);
        assert_eq!(row.spread, 0.0);
    }

    #[test]
    fn values_no_measure_explains_stay_fixed_with_their_spread() {
        // The darker sky got the larger lift: the gap differs, 2 and 3.5.
        let row = run(&[(backlit(0.0, -4.0), 2.0), (backlit(-1.0, -4.0), 0.5)]);
        assert_eq!(row.default, Choice::Fixed, "{}", row.note);
        assert!(row.target.is_none(), "{row:?}");
        assert_eq!(row.fixed, 1.25);
        assert_eq!(row.spread, 1.5);
    }

    #[test]
    fn one_test_photo_offers_the_target_and_keeps_the_value() {
        let row = run(&[(backlit(0.0, -4.0), 2.0)]);
        assert_eq!(row.default, Choice::Fixed);
        assert!(row.target.is_some(), "{row:?}");
        assert!(row.note.contains("one test photo"), "{}", row.note);
        assert_eq!(row.fixed, 2.0);
    }

    /// The sky band of [`masks`] with `extra`'s foreground pixels added.
    fn ai_mask(extra: impl Fn(i64, i64) -> bool + 'static) -> Image {
        Image::from_fn(Region::sized(W as u32, W as u32), 1, move |x, y, _| {
            f32::from(u8::from(y < SKY_ROWS || extra(x, y)))
        })
    }

    fn zone_rows(photos: &[Image], ai: &[(&str, &Image)]) -> Vec<ZoneRow> {
        let mut zones = masks();
        for (name, mask) in ai {
            zones.insert((*name).to_string(), (*mask).clone());
        }
        let values = Params::new();
        let cases: Vec<Case> = photos
            .iter()
            .map(|input| Case {
                input,
                zones: &zones,
                values: &values,
            })
            .collect();
        let names: Vec<String> = ai.iter().map(|(n, _)| (*n).to_string()).collect();
        generalise_zones(&cases, &names).unwrap()
    }

    #[test]
    fn an_ai_sky_a_parametric_mask_matches_becomes_parametric() {
        let sky = ai_mask(|_, _| false);
        let rows = zone_rows(
            &[backlit(0.0, -4.0), backlit(-1.0, -4.0)],
            &[("ai_sky", &sky)],
        );
        let row = &rows[0];
        assert_eq!(row.default, ZoneChoice::Parametric, "{}", row.note);
        let fit = row.fit.as_ref().unwrap();
        assert!(fit.overlaps.iter().all(|o| *o >= MIN_OVERLAP), "{fit:?}");
        assert_eq!(fit.overlap, 1.0, "{fit:?}");
        // Luminance alone does it: no hue bound.
        assert!(fit.params.get("hue").is_none(), "{fit:?}");
        let l = fit
            .params
            .get("luminance")
            .and_then(Value::as_list)
            .unwrap();
        assert!(l[0] > 0.0625 && l[0] <= 0.5, "{l:?}");
        assert!(
            row.note.starts_with("ai_sky: a parametric mask (luminance"),
            "{}",
            row.note
        );
        assert!(row.note.ends_with("no AI call"), "{}", row.note);
    }

    #[test]
    fn an_ai_zone_no_parametric_mask_overlaps_enough_stays_ai() {
        // The AI's sky also holds a figure in the foreground, as dark as
        // what surrounds it: no range of luminance or hue tells them apart.
        let sky = ai_mask(|x, y| y >= 16 && (8..16).contains(&x) && y % 2 == 0);
        let rows = zone_rows(&[backlit(0.0, -4.0)], &[("sky", &sky)]);
        let row = &rows[0];
        assert_eq!(row.default, ZoneChoice::Ai, "{}", row.note);
        assert!(row.fit.is_none(), "{row:?}");
        let best = &row.candidates[0];
        assert!(best.overlap < MIN_OVERLAP, "{best:?}");
        assert_eq!(
            row.note,
            format!(
                "sky: the parametric mask overlaps {} on the test photo; kept as AI, 1 call per photo",
                best.overlap
            )
        );
    }

    #[test]
    fn a_mix_of_ai_zones_keeps_only_the_one_nothing_fits() {
        let sky = ai_mask(|_, _| false);
        // Every other row of the foreground: half of the same tones.
        let subject = Image::from_fn(Region::sized(W as u32, W as u32), 1, |_, y, _| {
            f32::from(u8::from(y >= SKY_ROWS && y % 2 == 0))
        });
        let rows = zone_rows(
            &[backlit(0.0, -4.0), backlit(-1.0, -4.0)],
            &[("ai_sky", &sky), ("subject", &subject)],
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].default, ZoneChoice::Parametric, "{}", rows[0].note);
        assert_eq!(rows[1].default, ZoneChoice::Ai, "{}", rows[1].note);
        assert!(
            rows[1]
                .note
                .contains("on the first photo; kept as AI, 1 call per photo"),
            "{}",
            rows[1].note
        );
        assert!(
            rows[1].candidates[0].overlap <= 0.5,
            "{:?}",
            rows[1].candidates[0]
        );
    }

    #[test]
    fn ai_values_no_measure_explains_stay_a_junction() {
        let mut rows = vec![
            run(&[(backlit(0.0, -4.0), 2.0), (backlit(-1.0, -4.0), 0.5)]),
            run(&[(backlit(0.0, -4.0), 1.5), (backlit(-1.0, -4.0), 1.5)]),
            run(&[(backlit(0.0, -4.0), 2.0), (backlit(-1.0, -4.0), 1.0)]),
        ];
        // The AI proposed all three; a user's values stay as they were.
        let mut user = rows[0].clone();
        keep_ai_values(&mut rows, &["backlight_lift".to_string()]);
        keep_ai_values(std::slice::from_mut(&mut user), &[]);
        assert_eq!(rows[0].default, Choice::Ai, "{}", rows[0].note);
        assert!(
            rows[0]
                .note
                .starts_with("backlight_lift: 2 EV and 0.5 EV on the test photos; the AI's values"),
            "{}",
            rows[0].note
        );
        assert!(
            rows[0].note.ends_with("kept as AI, 1 call per photo"),
            "{}",
            rows[0].note
        );
        assert_eq!(rows[1].default, Choice::Fixed, "{}", rows[1].note);
        assert_eq!(rows[2].default, Choice::Target, "{}", rows[2].note);
        assert_eq!(user.default, Choice::Fixed);
    }

    #[test]
    fn the_choices_write_targets_defaults_and_fitted_zones_and_count_the_ai() {
        let target = run(&[(backlit(0.0, -4.0), 2.0), (backlit(-1.0, -4.0), 1.0)]);
        let mut fixed = run(&[(backlit(0.0, -4.0), 2.0), (backlit(-1.0, -4.0), 1.0)]);
        fixed.param = "haze".into();
        fixed.default = Choice::Ai;
        let fit = ZoneFit {
            params: Params::new().with("luminance", Value::List(vec![0.5, 1.0, 0.0, 0.0])),
            overlaps: vec![0.95],
            overlap: 0.95,
            fits: true,
        };
        let mut rows = vec![target, fixed];
        let mut zones = vec![ZoneRow {
            zone: "sky".into(),
            fit: None,
            default: ZoneChoice::Ai,
            candidates: vec![fit.clone()],
            note: String::new(),
        }];
        assert_eq!(
            minimum(&rows, &zones),
            Minimum {
                junctions: 2,
                calls_per_photo: 2.0
            }
        );
        let block = Workshop {
            intent: "backlit".into(),
            keywords: Vec::new(),
            targets: vec![Target {
                param: "haze".into(),
                measure: Measure::LumaMedian,
                value: 0.0,
                at: At::Output,
                zone: None,
                against: None,
            }],
            zones: vec![Zone {
                name: "sky".into(),
                module: AI_ZONE.into(),
                params: Params::new(),
                port: "mask".into(),
            }],
            junctions: Vec::new(),
            limits: None,
        };
        // Junctions left: written as such, the slider's fixed value its
        // default and fallback, the zone still the AI's.
        let (asked, defaults) = written(&block, &rows, &zones).unwrap();
        let kinds: Vec<(JunctionKind, &str, Option<f64>)> = asked
            .junctions
            .iter()
            .map(|j| (j.kind, j.name.as_str(), j.fallback))
            .collect();
        assert_eq!(
            kinds,
            [
                (JunctionKind::Value, "haze", Some(1.5)),
                (JunctionKind::Zone, "sky", None)
            ]
        );
        assert!(asked.targets.iter().all(|t| t.param != "haze"));
        assert_eq!(asked.zones[0].module, AI_ZONE);
        assert_eq!(defaults.float("haze"), 1.5);
        // An unknown name, or a target where none fits: refused.
        let ask = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
                .collect()
        };
        assert!(choose(&mut rows, &mut zones, &ask(&[("nope", "fixed")])).is_err());
        rows[1].target = None;
        assert!(choose(&mut rows, &mut zones, &ask(&[("haze", "target")])).is_err());
        choose(
            &mut rows,
            &mut zones,
            &ask(&[("haze", "fixed"), ("sky", "parametric")]),
        )
        .unwrap();
        assert_eq!(minimum(&rows, &zones).junctions, 0);
        let (out, defaults) = written(&block, &rows, &zones).unwrap();
        // The target row's target; the fixed row's target dropped, its
        // value the default; the AI sky drawn by the fitted mask.
        assert_eq!(out.targets.len(), 1);
        assert_eq!(out.targets[0].param, "backlight_lift");
        assert_eq!(out.targets[0].measure, Measure::ZonesRatio);
        assert_eq!(defaults.float("haze"), 1.5);
        assert!(defaults.get("backlight_lift").is_none());
        assert_eq!(out.zones[0].module, PARAMETRIC);
        assert_eq!(out.zones[0].params, fit.params);
        // The junctions the earlier version had are dropped once decided.
        let (again, _) = written(&asked, &rows, &zones).unwrap();
        assert!(again.junctions.is_empty(), "{again:?}");
    }
}
