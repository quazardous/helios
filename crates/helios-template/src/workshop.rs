//! Workshops (experimental, see `docs/design/workshops.md`): aiming a
//! workshop's relative targets on an image.
//!
//! A target says what the image should measure ([`Measure`]), not what
//! the parameter should be, so the same workshop gives each image its
//! own values. [`aim`] solves the targets in the file's order, the other
//! parameters at their current values:
//!
//! - the two ends of the parameter's range are probed first; a goal
//!   beyond what they measure takes the nearer end, and says so;
//! - between them, the range is bisected, at most [`MAX_PROBES`] renders
//!   per target, ends included; a probe measuring outside what the ends
//!   measure (the measure does not move one way over the range) keeps
//!   the parameter's value, and says why;
//! - the answer is interpolated within the last bracket and rounded to
//!   the parameter's step.
//!
//! It is pure: the caller gives the render (the desktop and the node over
//! their engine on a small proxy of the image, tests over synthetic
//! images), so the same input, workshop and targets give the same values.

use std::cell::Cell;
use std::collections::BTreeMap;

use helios_core::{Composite, Image, Macro, ParamHint, ParamKind, ParamSpec, Params, Value};
use helios_scopes::Scale;

use crate::format::{At, Measure, Target};

/// At most this many renders per target, its two ends included.
pub const MAX_PROBES: usize = 12;

/// The luminance histogram the measures read: stops around 1.0, a
/// hundredth of a stop per bin.
const SCALE: Scale = Scale::Stops {
    min: -16.0,
    max: 8.0,
};
const BINS: usize = 2400;

/// The 99th percentile `luma.high` reads.
const HIGH: f64 = 0.99;

/// What `image` measures, in the measure's unit ([`Measure::unit`]):
/// stops around 1.0 for the luminance ones, a percentage for
/// `clip.high`. None for an image without pixels.
pub fn measure(image: &Image, m: Measure) -> Option<f64> {
    match m {
        Measure::LumaMedian => helios_scopes::luminance(image, BINS, SCALE).quantile(SCALE, 0.5),
        Measure::LumaHigh => helios_scopes::luminance(image, BINS, SCALE).quantile(SCALE, HIGH),
        Measure::ClipHigh => {
            // One bin up to 1.0: what is above it is clipped.
            let counts = helios_scopes::luminance(image, 1, Scale::Linear { max: 1.0 });
            let total = counts.total();
            (total > 0).then(|| f64::from(counts.above) * 100.0 / total as f64)
        }
    }
}

/// The range a targeted parameter is searched over (its narrowed range,
/// else the inner hard one), and how its value is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    /// The step a value is rounded to; none: a thousandth of the range
    /// (1 for a whole number).
    pub step: Option<f64>,
    /// A whole number parameter (`int`).
    pub integer: bool,
    /// The unit the notes show (`EV`).
    pub unit: Option<String>,
}

impl Range {
    /// A number parameter's range.
    pub fn new(min: f64, max: f64) -> Self {
        Range {
            min,
            max,
            step: None,
            integer: false,
            unit: None,
        }
    }

    fn step(&self) -> Option<f64> {
        match self.step.filter(|s| *s > 0.0 && s.is_finite()) {
            Some(s) if self.integer => Some(s.max(1.0).round()),
            Some(s) => Some(s),
            None => self.integer.then_some(1.0),
        }
    }

    /// `v` rounded to the step, within the range.
    pub fn round(&self, v: f64) -> f64 {
        let v = match self.step() {
            Some(step) => (v / step).round() * step,
            None => v,
        };
        v.clamp(self.min, self.max)
    }

    /// How close the search goes: half a step.
    fn precision(&self) -> f64 {
        self.step().unwrap_or((self.max - self.min) / 1000.0) / 2.0
    }

    fn value(&self, v: f64) -> Value {
        if self.integer {
            Value::Int(v.round() as i64)
        } else {
            Value::Float(v)
        }
    }

    fn show(&self, v: f64) -> String {
        let v = (v * 1000.0).round() / 1000.0;
        match &self.unit {
            Some(u) => format!("{v} {u}"),
            None => format!("{v}"),
        }
    }
}

/// Each exposed number parameter's [`Range`]: its (narrowed) hard range
/// from `params`, its fine step and unit from `hints`. A module's or a
/// macro's manifest and hints, as the graph holds them.
#[allow(clippy::cast_precision_loss)]
pub fn ranges_of(params: &[ParamSpec], hints: &[ParamHint]) -> BTreeMap<String, Range> {
    params
        .iter()
        .filter_map(|p| {
            let (min, max, integer) = match p.kind {
                ParamKind::Float { min, max, .. } => (min, max, false),
                ParamKind::Int { min, max, .. } => (min as f64, max as f64, true),
                _ => return None,
            };
            let hint = hints.iter().find(|h| h.name == p.name);
            let range = Range {
                min,
                max,
                step: hint.and_then(|h| h.step),
                integer,
                unit: hint.and_then(|h| h.unit.clone()),
            };
            Some((p.name.clone(), range))
        })
        .collect()
}

/// A macro's exposed number parameters' ranges ([`ranges_of`]): what
/// [`aim`] searches a workshop's targets over.
pub fn ranges(m: &Macro) -> BTreeMap<String, Range> {
    ranges_of(&Composite::manifest(m).params, &Composite::param_hints(m))
}

/// How one target was aimed.
#[derive(Clone, Debug, PartialEq)]
pub struct Aimed {
    /// The exposed parameter.
    pub param: String,
    pub measure: Measure,
    /// The value the measure was aimed at (an `at: input` target's
    /// offset added to what the input measures).
    pub goal: f64,
    /// The parameter's value after aiming (its value before when the
    /// target could not be aimed).
    pub value: f64,
    /// What the nearest probe measured.
    pub measured: Option<f64>,
    /// Renders it took.
    pub probes: usize,
    /// Whether the goal was within the range and solved.
    pub reached: bool,
    /// Why it was not, for the user ("brightness: target not reached,
    /// kept at 3 EV").
    pub note: Option<String>,
}

/// Solves `targets` in order. `params` are the workshop's exposed values
/// now; `ranges` gives each targeted parameter's range; `input` is what
/// enters the workshop (an `at: input` target is an offset from what it
/// measures); `render` renders the workshop's output for a set of
/// values. Returns each target's answer and the values after aiming
/// (each solved value is the next targets' current one).
pub fn aim<F>(
    mut render: F,
    input: &Image,
    params: &Params,
    targets: &[Target],
    ranges: &BTreeMap<String, Range>,
) -> Result<(Vec<Aimed>, Params), String>
where
    F: FnMut(&Params) -> Result<Image, String>,
{
    let mut params = params.clone();
    let mut answers = Vec::with_capacity(targets.len());
    for t in targets {
        let range = ranges
            .get(&t.param)
            .ok_or_else(|| format!("{}: no range to aim it in", t.param))?;
        if !(range.min.is_finite() && range.max.is_finite() && range.min < range.max) {
            return Err(format!(
                "{}: the range [{}, {}] is empty",
                t.param, range.min, range.max
            ));
        }
        let goal = match t.at {
            At::Output => t.value,
            At::Input => {
                measure(input, t.measure).ok_or("the workshop's input has no pixels")? + t.value
            }
        };
        let current = params
            .get(&t.param)
            .and_then(Value::as_f64)
            .unwrap_or(range.min)
            .clamp(range.min, range.max);
        let answer = solve(&mut render, &params, t, goal, range, current)?;
        params.set(&t.param, range.value(answer.value));
        answers.push(answer);
    }
    Ok((answers, params))
}

/// One target, by bisection with its ends tested first.
fn solve<F>(
    render: &mut F,
    params: &Params,
    t: &Target,
    goal: f64,
    range: &Range,
    current: f64,
) -> Result<Aimed, String>
where
    F: FnMut(&Params) -> Result<Image, String>,
{
    let probes = Cell::new(0);
    let mut probe = |v: f64| -> Result<f64, String> {
        probes.set(probes.get() + 1);
        let mut p = params.clone();
        p.set(&t.param, range.value(v));
        let image = render(&p)?;
        measure(&image, t.measure).ok_or_else(|| "the workshop's output has no pixels".into())
    };
    let answer = |value: f64, measured: Option<f64>, note: Option<String>| Aimed {
        param: t.param.clone(),
        measure: t.measure,
        goal,
        value,
        measured,
        probes: probes.get(),
        reached: note.is_none(),
        note,
    };
    let (mut lo, mut hi) = (range.min, range.max);
    let (mut m_lo, mut m_hi) = (probe(lo)?, probe(hi)?);
    if m_lo == m_hi {
        let note = format!(
            "{}: {} does not change over its range, kept at {}",
            t.param,
            t.measure.name(),
            range.show(current)
        );
        return Ok(answer(current, None, Some(note)));
    }
    // Tested at the extremes: a goal beyond them takes the nearer end.
    if goal < m_lo.min(m_hi) || goal > m_lo.max(m_hi) {
        let (end, m) = if (goal - m_lo).abs() <= (goal - m_hi).abs() {
            (lo, m_lo)
        } else {
            (hi, m_hi)
        };
        let end = range.round(end);
        let note = format!(
            "{}: target not reached, kept at {}",
            t.param,
            range.show(end)
        );
        return Ok(answer(end, Some(m), Some(note)));
    }
    let mut nearest = if (goal - m_lo).abs() <= (goal - m_hi).abs() {
        m_lo
    } else {
        m_hi
    };
    while hi - lo > range.precision() && probes.get() < MAX_PROBES {
        // Probed at a value the parameter can take.
        let mid = range.round(lo.midpoint(hi));
        if mid <= lo || mid >= hi {
            break;
        }
        let m = probe(mid)?;
        if m < m_lo.min(m_hi) || m > m_lo.max(m_hi) {
            let note = format!(
                "{}: {} does not move one way over its range, kept at {}",
                t.param,
                t.measure.name(),
                range.show(current)
            );
            return Ok(answer(current, None, Some(note)));
        }
        if (goal - m).abs() < (goal - nearest).abs() {
            nearest = m;
        }
        if m == goal {
            (lo, hi, m_lo, m_hi) = (mid, mid, m, m);
            break;
        }
        // The goal stays between the bracket's two measures.
        if (m < goal) == (m_lo < goal) {
            (lo, m_lo) = (mid, m);
        } else {
            (hi, m_hi) = (mid, m);
        }
    }
    let v = if m_hi == m_lo {
        lo.midpoint(hi)
    } else {
        lo + (goal - m_lo) * (hi - lo) / (m_hi - m_lo)
    };
    Ok(answer(range.round(v.clamp(lo, hi)), Some(nearest), None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::Region;

    /// A horizontal ramp of grey, from 2^-8 to 2^0 in stops: its median
    /// luminance is -4 stops.
    fn ramp() -> Image {
        let w = 4096;
        Image::from_fn(Region::new(0, 0, w, 1), 4, |x, _, c| {
            if c == 3 {
                1.0
            } else {
                (-8.0 + 8.0 * (x as f32 + 0.5) / w as f32).exp2()
            }
        })
    }

    /// The input scaled by 2^ev: an exposure.
    fn exposed(input: &Image, ev: f64) -> Image {
        let gain = ev.exp2() as f32;
        let data = input
            .data()
            .chunks_exact(4)
            .flat_map(|p| [p[0] * gain, p[1] * gain, p[2] * gain, p[3]])
            .collect();
        Image::new(input.region(), 4, data)
    }

    fn target(measure: Measure, value: f64) -> Target {
        Target {
            param: "brightness".into(),
            measure,
            value,
            at: At::Output,
        }
    }

    fn brightness() -> BTreeMap<String, Range> {
        let mut r = Range::new(-3.0, 3.0);
        r.step = Some(0.01);
        r.unit = Some("EV".into());
        BTreeMap::from([("brightness".to_string(), r)])
    }

    fn start() -> Params {
        Params::new().with("brightness", Value::Float(0.0))
    }

    fn run(targets: &[Target], input: &Image) -> (Vec<Aimed>, Params, usize) {
        let mut renders = 0;
        let (answers, params) = aim(
            |p: &Params| {
                renders += 1;
                Ok(exposed(input, p.float("brightness")))
            },
            input,
            &start(),
            targets,
            &brightness(),
        )
        .unwrap();
        (answers, params, renders)
    }

    #[test]
    fn the_measures_read_the_luminance() {
        let input = ramp();
        let median = measure(&input, Measure::LumaMedian).unwrap();
        assert!((median + 4.0).abs() < 0.02, "{median}");
        let high = measure(&input, Measure::LumaHigh).unwrap();
        assert!((high + 0.08).abs() < 0.02, "{high}");
        assert_eq!(measure(&input, Measure::ClipHigh), Some(0.0));
        // One stop up, the ramp's top eighth is above 1.0.
        let clip = measure(&exposed(&input, 1.0), Measure::ClipHigh).unwrap();
        assert!((clip - 12.5).abs() < 0.1, "{clip}");
        let empty = Image::new(Region::new(0, 0, 0, 0), 4, Vec::new());
        assert_eq!(measure(&empty, Measure::LumaMedian), None);
    }

    #[test]
    fn a_ramp_reaches_its_median_within_half_a_step() {
        let input = ramp();
        let (answers, params, renders) = run(&[target(Measure::LumaMedian, -2.5)], &input);
        let a = &answers[0];
        assert!(a.reached && a.note.is_none(), "{a:?}");
        // -4 stops lifted to -2.5: 1.5 EV.
        assert!((a.value - 1.5).abs() <= 0.005, "{a:?}");
        assert_eq!(params.float("brightness"), a.value);
        assert!(a.probes <= MAX_PROBES && renders <= MAX_PROBES, "{a:?}");
        // The same inputs give the same value.
        let (again, ..) = run(&[target(Measure::LumaMedian, -2.5)], &input);
        assert_eq!(again, answers);
    }

    #[test]
    fn each_measure_is_aimed() {
        let input = ramp();
        let (answers, ..) = run(
            &[
                target(Measure::LumaHigh, 0.5),
                target(Measure::ClipHigh, 25.0),
            ],
            &input,
        );
        // The 99th percentile at +0.5 stops: about 0.58 EV.
        assert!(answers[0].reached, "{:?}", answers[0]);
        assert!((answers[0].value - 0.58).abs() < 0.03, "{:?}", answers[0]);
        // A quarter of the ramp above 1.0: two stops up.
        assert!(answers[1].reached, "{:?}", answers[1]);
        assert!((answers[1].value - 2.0).abs() < 0.03, "{:?}", answers[1]);
    }

    #[test]
    fn an_input_target_is_an_offset_from_the_input() {
        let input = ramp();
        let mut t = target(Measure::LumaMedian, 1.0);
        t.at = At::Input;
        let (answers, ..) = run(&[t], &input);
        assert!((answers[0].goal + 3.0).abs() < 0.02, "{:?}", answers[0]);
        assert!((answers[0].value - 1.0).abs() <= 0.02, "{:?}", answers[0]);
    }

    #[test]
    fn a_target_beyond_the_range_takes_the_nearer_end() {
        let input = ramp();
        let (answers, _, renders) = run(&[target(Measure::LumaMedian, 2.0)], &input);
        let a = &answers[0];
        assert!(!a.reached);
        assert_eq!(a.value, 3.0);
        assert_eq!(
            a.note.as_deref(),
            Some("brightness: target not reached, kept at 3 EV")
        );
        assert_eq!(renders, 2, "only the ends are probed");
        let (low, ..) = run(&[target(Measure::LumaMedian, -12.0)], &input);
        assert_eq!(low[0].value, -3.0);
    }

    #[test]
    fn a_measure_that_does_not_move_one_way_keeps_the_value() {
        let input = ramp();
        // Darker at both ends than in the middle: a bump, the middle
        // brighter than either end.
        let (answers, params) = aim(
            |p: &Params| Ok(exposed(&input, -(p.float("brightness") - 1.0).abs())),
            &input,
            &Params::new().with("brightness", Value::Float(0.25)),
            &[target(Measure::LumaMedian, -6.5)],
            &brightness(),
        )
        .unwrap();
        let a = &answers[0];
        assert!(!a.reached);
        assert_eq!(a.value, 0.25);
        assert_eq!(params.float("brightness"), 0.25);
        assert!(
            a.note
                .as_deref()
                .is_some_and(|n| n.contains("does not move one way")),
            "{a:?}"
        );
    }

    #[test]
    fn a_whole_number_is_aimed_to_a_whole_number() {
        let input = ramp();
        let mut r = Range::new(-3.0, 3.0);
        r.integer = true;
        let ranges = BTreeMap::from([("brightness".to_string(), r)]);
        let (answers, params) = aim(
            |p: &Params| {
                let ev = p.get("brightness").and_then(Value::as_f64).unwrap();
                Ok(exposed(&input, ev))
            },
            &input,
            &Params::new().with("brightness", Value::Int(0)),
            &[target(Measure::LumaMedian, -2.2)],
            &ranges,
        )
        .unwrap();
        assert_eq!(answers[0].value, 2.0);
        assert_eq!(params.get("brightness"), Some(&Value::Int(2)));
    }

    #[test]
    fn a_failed_render_is_an_error() {
        let input = ramp();
        let e = aim(
            |_: &Params| Err("no GPU".to_string()),
            &input,
            &start(),
            &[target(Measure::LumaMedian, -2.5)],
            &brightness(),
        )
        .unwrap_err();
        assert_eq!(e, "no GPU");
        let e = aim(
            |_: &Params| Ok(input.clone()),
            &input,
            &start(),
            &[Target {
                param: "contrast".into(),
                ..target(Measure::LumaMedian, 0.0)
            }],
            &brightness(),
        )
        .unwrap_err();
        assert!(e.contains("no range"), "{e}");
    }
}
