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
//!
//! Nothing is written here: the rows are what the panel and the
//! `workshop.generalise` verb show for the user to confirm. It is pure:
//! the caller gives the render, as for aiming.

use std::collections::BTreeMap;

use helios_core::{Engine, Graph, Image, Macro, ModuleRef, Params, Region, Value};
use serde::Serialize;

use crate::build::SOURCE_PORT;
use crate::format::{At, Measure, Target};
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
        (None, _) => format!(
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
}
