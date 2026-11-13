//! A workshop's limits (experimental, see
//! `docs/design/workshops-on-the-fly.md`, section 3, "Limits"): the
//! explicit bounds a run stays within on each photo, prefilled from the
//! test photos when the workshop is made automatic ([`defaults`]), and
//! the pure checker a run passes each photo through ([`check`]): clamp,
//! flag, hold or stop.
//!
//! A breach of a slider's, a target's or the input's bounds, or a
//! measure aiming cannot decide, follows `on_breach`. A junction that
//! cannot be answered (low confidence, no AI, the run's AI budget spent)
//! takes its fallback and is flagged, but never holds the photo: the run
//! completes and the review shows each fallback.

use std::collections::BTreeMap;

use helios_core::Image;
use serde::Serialize;

use crate::format::{
    Junction, JunctionKind, JunctionLimits, Limits, Measure, OnBreach, SliderLimits, Span,
    TargetLimits, Workshop,
};
use crate::generalise::{Choice, Row, tolerance};
use crate::workshop::{Range, measure};

/// A junction's answer kept only from this confidence up, by default.
pub const MIN_CONFIDENCE: f64 = 0.7;

/// How far from the test photos' measures an input may be, in stops.
pub const INPUT_SPAN: f64 = 1.5;

/// The measures of the input a run compares with the test photos'.
pub const INPUT_MEASURES: [Measure; 2] = [Measure::LumaMedian, Measure::LumaHigh];

/// The limits generalisation prefills (the design's table): for each
/// row's slider, its `range` narrowed to the test values ± half their
/// spread plus one step, within the slider's range, and a
/// `max_deviation` of twice the spread, at least a tenth of the range;
/// for each row set to a target, one step of `tolerance` (0.1 for an EV
/// slider); the `inputs`' (the test photos') measures ± 1.5 stops; each
/// junction's `min_confidence` at 0.7. `max_ai_calls` is left to the
/// run (the junctions times the photos), `on_breach` at `flag`.
pub fn defaults(
    rows: &[Row],
    ranges: &BTreeMap<String, Range>,
    inputs: &[&Image],
    junctions: &[Junction],
) -> Limits {
    let mut out = Limits::default();
    for row in rows {
        let Some(range) = ranges.get(&row.param) else {
            continue;
        };
        let step = tolerance(range);
        let (lo, hi) = row
            .values
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                (lo.min(*v), hi.max(*v))
            });
        if !lo.is_finite() || !hi.is_finite() {
            continue;
        }
        let pad = row.spread / 2.0 + step;
        let min = (lo - pad).max(range.min);
        let max = (hi + pad).min(range.max);
        out.sliders.insert(
            row.param.clone(),
            SliderLimits {
                min,
                max: max.max(min),
                value: row.fixed,
                max_deviation: (2.0 * row.spread).max((range.max - range.min) / 10.0),
            },
        );
        if row.default == Choice::Target {
            out.targets.insert(
                row.param.clone(),
                TargetLimits {
                    tolerance: row.tolerance,
                },
            );
        }
    }
    for m in INPUT_MEASURES {
        let read: Vec<f64> = inputs.iter().filter_map(|i| measure(i, m)).collect();
        if read.is_empty() {
            continue;
        }
        let lo = read.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = read.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        out.input_span.push(Span {
            measure: m,
            min: lo - INPUT_SPAN,
            max: hi + INPUT_SPAN,
        });
    }
    for j in junctions {
        out.junctions.insert(
            j.name.clone(),
            JunctionLimits {
                min_confidence: MIN_CONFIDENCE,
            },
        );
    }
    out
}

/// `limits` without what `w` no longer has: a slider's bounds where
/// `params` (its exposed parameters) lack it, a target's where no target
/// aims its slider, a junction's where no junction is.
pub fn prune(mut limits: Limits, w: &Workshop, params: &[&str]) -> Limits {
    limits.sliders.retain(|p, _| params.contains(&p.as_str()));
    limits
        .targets
        .retain(|p, _| w.targets.iter().any(|t| t.param == *p));
    limits
        .junctions
        .retain(|n, _| w.junctions.iter().any(|j| j.name == *n));
    limits
}

/// A photo's flag, as the review shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// A slider clamped to its range.
    AtLimit,
    /// A slider clamped to the test value ± its deviation.
    Deviation,
    /// A target's measure landed outside its tolerance.
    TargetMissed,
    /// The input measures outside the test photos' span.
    UnlikeTestCases,
    /// A junction answered below its confidence: the fallback is used.
    LowConfidence,
    /// No AI answered a junction: the fallback is used.
    Fallback,
    /// Aiming could not decide (the measure does not move one way): the
    /// test value is kept.
    NotMonotonic,
    /// The run's AI calls are spent: the fallback is used.
    AiBudget,
}

impl Flag {
    /// Whether it is a breach, which `on_breach` acts on; a junction's
    /// fallback never holds a photo.
    pub fn is_breach(self) -> bool {
        !matches!(self, Flag::LowConfidence | Flag::Fallback | Flag::AiBudget)
    }
}

/// A flag raised on a photo, with what it is about.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Raised {
    pub flag: Flag,
    /// The slider, target, junction or measure.
    pub name: String,
    pub note: String,
}

/// A junction's answer on a photo.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// The AI answered: a value for a value junction (none for a zone's
    /// mask), with its confidence.
    Answered { value: Option<f64>, confidence: f64 },
    /// No AI is available.
    NoAi,
    /// Not asked: the run's AI budget is spent ([`Budget::ask`]).
    OverBudget,
}

/// What a run found on one photo, before the limits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Photo {
    /// The sliders' values, aimed or fixed.
    pub values: BTreeMap<String, f64>,
    /// By the slider a target aims: the measure aiming landed on minus
    /// the goal, in the measure's unit.
    pub missed_by: BTreeMap<String, f64>,
    /// What the input measures ([`INPUT_MEASURES`]).
    pub input: Vec<(Measure, f64)>,
    /// The sliders whose aiming could not decide.
    pub not_monotonic: Vec<String>,
    /// By junction name.
    pub answers: BTreeMap<String, Answer>,
}

/// What the run does with the photo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Write it (flagged or not).
    Apply,
    /// Do not write it until it is fixed.
    Hold,
    /// Stop the run here.
    Stop,
}

/// A photo through the limits.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Checked {
    /// The values to write: clamped, fallbacks and junction answers in.
    pub values: BTreeMap<String, f64>,
    /// The junctions that took their fallback (a zone's: its mask is
    /// re-asked or the test photo's kept, as the run does).
    pub fallbacks: Vec<String>,
    pub flags: Vec<Raised>,
    pub action: Action,
}

/// A run's AI calls: `max` of them, `used` so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub used: u64,
    pub max: u64,
}

impl Budget {
    /// The budget of a run of `photos` photos: `max_ai_calls`, or the
    /// workshop's junctions times the photos.
    pub fn for_run(limits: &Limits, junctions: usize, photos: usize) -> Self {
        Budget {
            used: 0,
            max: limits
                .max_ai_calls
                .unwrap_or((junctions as u64).saturating_mul(photos as u64)),
        }
    }

    /// Whether one more call may be made; counted when it may.
    pub fn ask(&mut self) -> bool {
        if self.used < self.max {
            self.used += 1;
            true
        } else {
            false
        }
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// `photo` through `limits`, with the workshop's `junctions`: a value
/// junction's answer kept from its `min_confidence` up, its fallback
/// otherwise; a slider aiming could not decide set to the test value; each
/// slider clamped to the test value ± `max_deviation`, then to its range;
/// each target's miss and the input's measures compared. The flags raised
/// in that order; the action from `on_breach` when one is a breach.
pub fn check(limits: &Limits, junctions: &[Junction], photo: &Photo) -> Checked {
    let mut values = photo.values.clone();
    let mut fallbacks = Vec::new();
    let mut flags = Vec::new();
    let mut raise = |flag, name: &str, note: String| {
        flags.push(Raised {
            flag,
            name: name.to_string(),
            note,
        });
    };
    for j in junctions {
        let min = limits
            .junctions
            .get(&j.name)
            .map_or(MIN_CONFIDENCE, |l| l.min_confidence);
        let fallback = match photo.answers.get(&j.name).unwrap_or(&Answer::NoAi) {
            Answer::Answered { value, confidence } if *confidence >= min => {
                if let (JunctionKind::Value, Some(v)) = (j.kind, value) {
                    values.insert(j.name.clone(), *v);
                }
                None
            }
            Answer::Answered { confidence, .. } => Some((
                Flag::LowConfidence,
                format!(
                    "answered at {} confidence, under {min}: the fallback is used",
                    round3(*confidence)
                ),
            )),
            Answer::NoAi => Some((
                Flag::Fallback,
                "no AI answered: the fallback is used".to_string(),
            )),
            Answer::OverBudget => Some((
                Flag::AiBudget,
                "the run's AI calls are spent: the fallback is used".to_string(),
            )),
        };
        if let Some((flag, note)) = fallback {
            if let (JunctionKind::Value, Some(v)) = (j.kind, j.fallback) {
                values.insert(j.name.clone(), v);
            }
            fallbacks.push(j.name.clone());
            raise(flag, &j.name, note);
        }
    }
    for name in &photo.not_monotonic {
        let kept = limits.sliders.get(name).map(|s| s.value);
        if let Some(v) = kept {
            values.insert(name.clone(), v);
        }
        raise(
            Flag::NotMonotonic,
            name,
            match kept {
                Some(v) => format!("aiming cannot decide here: the test value {v} is kept"),
                None => "aiming cannot decide here".to_string(),
            },
        );
    }
    for (name, s) in &limits.sliders {
        let Some(v) = values.get_mut(name) else {
            continue;
        };
        let (lo, hi) = (s.value - s.max_deviation, s.value + s.max_deviation);
        if *v < lo || *v > hi {
            let was = *v;
            *v = v.clamp(lo, hi);
            raise(
                Flag::Deviation,
                name,
                format!(
                    "{} is more than {} from the test value {}: set to {}",
                    round3(was),
                    round3(s.max_deviation),
                    round3(s.value),
                    round3(*v)
                ),
            );
        }
        if *v < s.min || *v > s.max {
            let was = *v;
            *v = v.clamp(s.min, s.max);
            raise(
                Flag::AtLimit,
                name,
                format!(
                    "{} is outside {} to {}: set to {}",
                    round3(was),
                    round3(s.min),
                    round3(s.max),
                    round3(*v)
                ),
            );
        }
    }
    for (name, t) in &limits.targets {
        if let Some(miss) = photo.missed_by.get(name)
            && miss.abs() > t.tolerance
        {
            raise(
                Flag::TargetMissed,
                name,
                format!(
                    "the target is missed by {}, more than {}",
                    round3(*miss),
                    round3(t.tolerance)
                ),
            );
        }
    }
    for span in &limits.input_span {
        let Some((_, v)) = photo.input.iter().find(|(m, _)| *m == span.measure) else {
            continue;
        };
        if *v < span.min || *v > span.max {
            raise(
                Flag::UnlikeTestCases,
                span.measure.name(),
                format!(
                    "the photo's {} is {}, the test photos' span {} to {}",
                    span.measure.name(),
                    round3(*v),
                    round3(span.min),
                    round3(span.max)
                ),
            );
        }
    }
    let breach = flags.iter().any(|r| r.flag.is_breach());
    let action = match (breach, limits.on_breach) {
        (false, _) | (true, OnBreach::Flag) => Action::Apply,
        (true, OnBreach::Hold) => Action::Hold,
        (true, OnBreach::Stop) => Action::Stop,
    };
    Checked {
        values,
        fallbacks,
        flags,
        action,
    }
}

/// What is wrong with `limits` on a workshop whose exposed number
/// parameters are `params`: a slider's bounds on one of them, `min` up
/// to `max`, a deviation above zero; a target's on a slider a target
/// aims, a tolerance above zero; a junction's on a junction, a
/// confidence from 0 to 1; a span from `min` up to `max` on a measure of
/// the whole image.
pub fn validate(limits: &Limits, w: &Workshop, params: &[&str]) -> Result<(), String> {
    let finite = |v: &[f64]| v.iter().all(|x| x.is_finite());
    for (name, s) in &limits.sliders {
        let at = |e: &str| format!("limits.sliders.{name}: {e}");
        if !params.contains(&name.as_str()) {
            return Err(at("not an exposed number parameter"));
        }
        if !finite(&[s.min, s.max, s.value, s.max_deviation]) {
            return Err(at("not a number"));
        }
        if s.min > s.max {
            return Err(at("`min` is above `max`"));
        }
        if s.max_deviation <= 0.0 {
            return Err(at("`max_deviation` is above zero"));
        }
    }
    for (name, t) in &limits.targets {
        let at = |e: &str| format!("limits.targets.{name}: {e}");
        if !w.targets.iter().any(|x| x.param == *name) {
            return Err(at("no target aims that parameter"));
        }
        if !(t.tolerance.is_finite() && t.tolerance > 0.0) {
            return Err(at("`tolerance` is a number above zero"));
        }
    }
    for (name, j) in &limits.junctions {
        let at = |e: &str| format!("limits.junctions.{name}: {e}");
        if !w.junctions.iter().any(|x| x.name == *name) {
            return Err(at("no junction of that name"));
        }
        if !(0.0..=1.0).contains(&j.min_confidence) {
            return Err(at("`min_confidence` is from 0 to 1"));
        }
    }
    for (i, s) in limits.input_span.iter().enumerate() {
        let at = |e: &str| format!("limits.input_span[{i}]: {e}");
        if !Measure::ALL.contains(&s.measure) {
            return Err(at("a measure of the whole image"));
        }
        if !finite(&[s.min, s.max]) || s.min > s.max {
            return Err(at("`min` up to `max`"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{At, Target};

    fn limits() -> Limits {
        Limits {
            sliders: BTreeMap::from([(
                "lift".to_string(),
                SliderLimits {
                    min: 0.5,
                    max: 3.0,
                    value: 2.0,
                    max_deviation: 0.8,
                },
            )]),
            targets: BTreeMap::from([("lift".to_string(), TargetLimits { tolerance: 0.1 })]),
            input_span: vec![Span {
                measure: Measure::LumaMedian,
                min: -4.0,
                max: -1.0,
            }],
            junctions: BTreeMap::from([(
                "haze".to_string(),
                JunctionLimits {
                    min_confidence: 0.7,
                },
            )]),
            max_ai_calls: None,
            on_breach: OnBreach::Flag,
        }
    }

    fn junctions() -> Vec<Junction> {
        vec![
            Junction {
                kind: JunctionKind::Value,
                name: "haze".into(),
                fallback: Some(0.25),
                note: None,
            },
            Junction {
                kind: JunctionKind::Zone,
                name: "sky".into(),
                fallback: None,
                note: None,
            },
        ]
    }

    /// A photo like the test photos: within every limit, the junctions
    /// answered with confidence.
    fn fine() -> Photo {
        Photo {
            values: BTreeMap::from([("lift".to_string(), 2.1)]),
            missed_by: BTreeMap::from([("lift".to_string(), 0.05)]),
            input: vec![(Measure::LumaMedian, -2.5), (Measure::LumaHigh, 0.5)],
            not_monotonic: Vec::new(),
            answers: BTreeMap::from([
                (
                    "haze".to_string(),
                    Answer::Answered {
                        value: Some(0.4),
                        confidence: 0.9,
                    },
                ),
                (
                    "sky".to_string(),
                    Answer::Answered {
                        value: None,
                        confidence: 0.95,
                    },
                ),
            ]),
        }
    }

    fn flags(c: &Checked) -> Vec<Flag> {
        c.flags.iter().map(|r| r.flag).collect()
    }

    #[test]
    fn a_photo_like_the_test_photos_raises_nothing() {
        let c = check(&limits(), &junctions(), &fine());
        assert!(c.flags.is_empty(), "{:?}", c.flags);
        assert_eq!(c.action, Action::Apply);
        assert_eq!(c.values["lift"], 2.1);
        assert_eq!(c.values["haze"], 0.4);
        assert!(c.fallbacks.is_empty());
    }

    #[test]
    fn each_flag_is_raised_by_a_crafted_input() {
        let one = |photo: Photo| {
            let c = check(&limits(), &junctions(), &photo);
            assert_eq!(c.flags.len(), 1, "{:?}", c.flags);
            c
        };
        // at_limit: within the deviation, under the range's floor.
        let mut p = fine();
        p.values.insert("lift".into(), 1.3);
        let mut tighter = limits();
        tighter.sliders.get_mut("lift").unwrap().min = 1.5;
        let c = check(&tighter, &junctions(), &p);
        assert_eq!(flags(&c), [Flag::AtLimit]);
        assert_eq!(c.values["lift"], 1.5);
        // deviation: within the range, too far from the test value.
        let mut p = fine();
        p.values.insert("lift".into(), 2.95);
        let c = one(p);
        assert_eq!(flags(&c), [Flag::Deviation]);
        assert!((c.values["lift"] - 2.8).abs() < 1e-9);
        // target_missed.
        let mut p = fine();
        p.missed_by.insert("lift".into(), -0.3);
        assert_eq!(flags(&one(p)), [Flag::TargetMissed]);
        // unlike_test_cases.
        let mut p = fine();
        p.input[0].1 = 0.5;
        assert_eq!(flags(&one(p)), [Flag::UnlikeTestCases]);
        // low_confidence: the fallback replaces the answer.
        let mut p = fine();
        p.answers.insert(
            "haze".into(),
            Answer::Answered {
                value: Some(0.9),
                confidence: 0.4,
            },
        );
        let c = one(p);
        assert_eq!(flags(&c), [Flag::LowConfidence]);
        assert_eq!(c.values["haze"], 0.25);
        assert_eq!(c.fallbacks, ["haze"]);
        // fallback: no AI for the zone.
        let mut p = fine();
        p.answers.insert("sky".into(), Answer::NoAi);
        let c = one(p);
        assert_eq!(flags(&c), [Flag::Fallback]);
        assert_eq!(c.fallbacks, ["sky"]);
        // not_monotonic: the test value is kept.
        let mut p = fine();
        p.not_monotonic.push("lift".into());
        let c = one(p);
        assert_eq!(flags(&c), [Flag::NotMonotonic]);
        assert_eq!(c.values["lift"], 2.0);
        // ai_budget: one junction, two photos, a cap of one call.
        let mut capped = limits();
        capped.max_ai_calls = Some(1);
        let mut budget = Budget::for_run(&capped, 1, 2);
        assert!(budget.ask());
        assert!(!budget.ask());
        assert_eq!(Budget::for_run(&limits(), 2, 3).max, 6);
        let mut p = fine();
        p.answers.insert("haze".into(), Answer::OverBudget);
        let c = one(p);
        assert_eq!(flags(&c), [Flag::AiBudget]);
        assert_eq!(c.values["haze"], 0.25);
    }

    #[test]
    fn on_breach_flags_holds_or_stops_and_a_fallback_never_holds() {
        let mut p = fine();
        p.values.insert("lift".into(), 2.95);
        for (on, want) in [
            (OnBreach::Flag, Action::Apply),
            (OnBreach::Hold, Action::Hold),
            (OnBreach::Stop, Action::Stop),
        ] {
            let mut l = limits();
            l.on_breach = on;
            assert_eq!(check(&l, &junctions(), &p).action, want);
        }
        let mut l = limits();
        l.on_breach = OnBreach::Stop;
        let mut p = fine();
        p.answers.insert("haze".into(), Answer::NoAi);
        assert_eq!(check(&l, &junctions(), &p).action, Action::Apply);
    }

    fn row(param: &str, values: &[f64], default: Choice) -> Row {
        let n = values.len() as f64;
        let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Row {
            param: param.into(),
            values: values.to_vec(),
            fixed: values.iter().sum::<f64>() / n,
            spread: hi - lo,
            tolerance: 0.1,
            target: None,
            default,
            candidates: Vec::new(),
            note: String::new(),
        }
    }

    #[test]
    fn defaults_come_from_the_test_photos() {
        let mut ev = Range::new(0.0, 4.0);
        ev.step = Some(0.01);
        ev.unit = Some("EV".into());
        let ranges = BTreeMap::from([
            ("lift".to_string(), ev),
            ("haze".to_string(), Range::new(0.0, 1.0)),
        ]);
        let grey = |stops: f64| {
            Image::from_fn(helios_core::Region::sized(8, 8), 4, move |_, _, c| {
                if c == 3 { 1.0 } else { stops.exp2() as f32 }
            })
        };
        let (a, b) = (grey(-3.0), grey(-1.0));
        let l = defaults(
            &[
                row("lift", &[1.8, 2.2], Choice::Target),
                row("haze", &[0.3, 0.3], Choice::Ai),
            ],
            &ranges,
            &[&a, &b],
            &junctions()[..1],
        );
        // lift: 1.8 to 2.2, ± (0.2 + 0.1 for an EV slider's step).
        let s = &l.sliders["lift"];
        assert!(
            (s.min - 1.5).abs() < 1e-9 && (s.max - 2.5).abs() < 1e-9,
            "{s:?}"
        );
        assert!((s.value - 2.0).abs() < 1e-9);
        // Twice the spread (0.8) beats a tenth of the range (0.4).
        assert!((s.max_deviation - 0.8).abs() < 1e-9);
        // haze: no spread, a tenth of the range.
        assert!((l.sliders["haze"].max_deviation - 0.1).abs() < 1e-9);
        assert_eq!(l.targets.keys().collect::<Vec<_>>(), ["lift"]);
        let median = &l.input_span[0];
        assert_eq!(median.measure, Measure::LumaMedian);
        assert!(
            (median.min + 4.5).abs() < 0.1 && (median.max - 0.5).abs() < 0.1,
            "{median:?}"
        );
        assert_eq!(l.junctions["haze"].min_confidence, MIN_CONFIDENCE);
        assert_eq!(l.on_breach, OnBreach::Flag);
        assert_eq!(l.max_ai_calls, None);
        // Valid on the workshop it is for.
        let w = Workshop {
            intent: "backlit".into(),
            targets: vec![Target {
                param: "lift".into(),
                measure: Measure::LumaMedian,
                value: 0.0,
                at: At::Output,
                zone: None,
                against: None,
            }],
            junctions: junctions()[..1].to_vec(),
            ..Workshop::default()
        };
        validate(&l, &w, &["lift", "haze"]).unwrap();
        let e = validate(&l, &w, &["lift"]).unwrap_err();
        assert!(e.contains("limits.sliders.haze"), "{e}");
        let bare = Workshop {
            intent: "backlit".into(),
            ..Workshop::default()
        };
        let e = validate(&l, &bare, &["lift", "haze"]).unwrap_err();
        assert!(e.contains("limits.targets.lift"), "{e}");
        let pruned = prune(l, &bare, &["lift"]);
        assert!(pruned.targets.is_empty() && pruned.junctions.is_empty());
        assert_eq!(pruned.sliders.keys().collect::<Vec<_>>(), ["lift"]);
        validate(&pruned, &bare, &["lift", "haze"]).unwrap();
    }
}
