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

use helios_core::{
    Composite, Engine, Graph, Image, Macro, ModuleRef, ParamHint, ParamKind, ParamSpec, Params,
    Region, Value,
};
use helios_scopes::Scale;
use serde_json::json;

use crate::build::SOURCE_PORT;
use crate::catalogue::Catalogue;
use crate::format::{
    At, CompositeFile, CompositeRef, Connection, Endpoint, Measure, NetworkDesc, NodeDesc, Target,
};
use crate::macros::{Moves, Step};
use crate::network::{PortInfo, fresh_name, ports_of, types_fit};
use crate::workspace::State;

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
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
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

/// [`aim`] through `engine`: `built` rendered alone on `input` (what
/// enters it, a small proxy of the image: the desktop's is 256 px on its
/// long side), its exposed parameters at their defaults overridden by
/// `current`, its targets searched over [`ranges`]. The input is a
/// source of its own, so a probe renders only the workshop's steps.
pub fn aim_rendered(
    engine: &Engine,
    built: &Macro,
    input: &Image,
    current: &Params,
    targets: &[Target],
) -> Result<(Vec<Aimed>, Params), String> {
    let r = input.region();
    let region = Region::sized(r.width, r.height);
    let input = Image::new(region, input.channels(), input.to_vec());
    let module = ModuleRef::composite(built.clone());
    let manifest = module.manifest().clone();
    let (Some(entry), Some(exit)) = (manifest.inputs.first(), manifest.outputs.first()) else {
        return Err(format!("`{}` has no input or no output", manifest.name));
    };
    let mut graph = Graph::new();
    let src = graph.add_source("input", input.clone());
    let node = graph
        .add_module("workshop", module, current)
        .map_err(|e| e.to_string())?;
    graph
        .connect(src, SOURCE_PORT, node, &entry.name)
        .map_err(|e| e.to_string())?;
    let params = graph
        .params(node)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let exit = exit.name.clone();
    aim(
        |p: &Params| {
            for (name, value) in p.iter() {
                graph
                    .set_param(node, name, value.clone())
                    .map_err(|e| e.to_string())?;
            }
            engine
                .render(&graph, node, &exit, region)
                .map_err(|e| e.to_string())
        },
        &input,
        &params,
        targets,
        &ranges(built),
    )
}

/// The output a workshop applied after `after` reads: the port the
/// viewport shows when it shows `after`, else `after`'s first output
/// whose type fits the workshop's input (a source's image).
fn insertion_port(
    net: &NetworkDesc,
    catalogue: &Catalogue,
    after: &str,
    input_ty: &str,
) -> Result<String, String> {
    if net.view.node == after {
        return Ok(net.view.port.clone());
    }
    let (_, outputs) =
        ports_of(net, catalogue, after).ok_or_else(|| format!("no node `{after}`"))?;
    outputs
        .iter()
        .find(|p| types_fit(&p.ty, input_ty))
        .map(|p| p.name.clone())
        .ok_or_else(|| format!("`{after}` has no output the workshop can read"))
}

impl State {
    /// Where `workshop.apply` inserts a workshop: `after` (a node), by
    /// default the node the viewport shows the image from, and the port
    /// of it the workshop reads. Refused for a composite without a
    /// workshop block, one with other than one input and one output
    /// ("add it from the graph"), and an `after` the network lacks.
    pub fn workshop_insertion(
        &self,
        catalogue: &Catalogue,
        workshop: &CompositeRef,
        after: Option<&str>,
    ) -> Result<(CompositeFile, Endpoint), String> {
        let cat = self.bundle.catalogue(catalogue);
        let file = cat
            .get_composite(workshop)
            .ok_or_else(|| format!("unknown composite `{workshop}`"))?
            .clone();
        if file.workshop.is_none() {
            return Err(format!(
                "`{workshop}` is a macro without a workshop block: add it from the graph"
            ));
        }
        if file.inputs.len() != 1 || file.outputs.len() != 1 {
            return Err(format!(
                "`{workshop}` has {} inputs and {} outputs: a workshop applied after a node has one of each; add it from the graph",
                file.inputs.len(),
                file.outputs.len()
            ));
        }
        let net = &self.bundle.template.network;
        let after = after.unwrap_or(&net.view.node).to_string();
        let input_ty = PortInfo::of_spec(&file.inputs[0].spec()).ty;
        let port = insertion_port(net, &cat, &after, &input_ty)?;
        Ok((file, Endpoint { node: after, port }))
    }

    /// Applies a workshop (`workshop.apply`): its macro's node, named
    /// after the workshop (a free name), is inserted on the wires leaving
    /// `after`'s output ([`State::workshop_insertion`]): it reads that
    /// output, and every reader of it whose input its output fits reads
    /// the workshop instead, the viewport too when it showed that output.
    /// It joins `after`'s sub-workspaces and is placed beside it.
    /// `values` (the aimed ones, by exposed name) are the step's values
    /// for the open image, under `node/<workshop node>/<exposed>`; the
    /// network keeps the workshop's defaults, as for any macro.
    pub fn workshop_applied(
        &self,
        catalogue: &Catalogue,
        workshop: &CompositeRef,
        after: Option<&str>,
        values: &Params,
    ) -> Result<Step, String> {
        let (file, at) = self.workshop_insertion(catalogue, workshop, after)?;
        let cat = self.bundle.catalogue(catalogue);
        let mut next = self.clone();
        let t = &mut next.bundle.template;
        let node = fresh_name(&t.network, &file.name);
        let output = file.outputs[0].name.clone();
        let output_ty = PortInfo::of_spec(&file.outputs[0].spec()).ty;
        // The readers of `after`'s output its output fits.
        let mut rewired = Vec::new();
        for c in &mut t.network.connections {
            if c.from != at.node || c.port != at.port {
                continue;
            }
            let fits =
                ports_of(&self.bundle.template.network, &cat, &c.to).is_some_and(|(ins, _)| {
                    ins.iter()
                        .any(|p| p.name == c.input && types_fit(&output_ty, &p.ty))
                });
            if fits {
                c.from.clone_from(&node);
                c.port.clone_from(&output);
                rewired.push(format!("{}.{}", c.to, c.input));
            }
        }
        t.network.connections.push(Connection {
            from: at.node.clone(),
            port: at.port.clone(),
            to: node.clone(),
            input: file.inputs[0].name.clone(),
        });
        let view = t.network.view == at;
        if view {
            t.network.view = Endpoint {
                node: node.clone(),
                port: output.clone(),
            };
        }
        t.network.nodes.push(NodeDesc {
            name: node.clone(),
            composite: Some(workshop.clone()),
            ..NodeDesc::default()
        });
        if let Some([x, y]) = t.positions.get(&at.node).copied() {
            t.positions.insert(node.clone(), [x + PLACE, y + PLACE]);
        }
        for s in &mut t.subworkspaces {
            if let Some(i) = s.nodes.iter().position(|n| *n == at.node) {
                s.nodes.insert(i + 1, node.clone());
            }
        }
        next.bundle.carry_composites(&cat);
        let next = State::checked(next, catalogue)?;
        let moves: Moves = values
            .iter()
            .map(|(p, v)| (format!("node/{node}/{p}"), Some(v.clone())))
            .collect();
        // Numbers as numbers, for whoever reads the answer.
        let values: BTreeMap<&str, serde_json::Value> = values
            .iter()
            .map(|(p, v)| (p, v.as_f64().map_or_else(|| json!(v), |f| json!(f))))
            .collect();
        Ok(Step {
            state: next,
            values: moves,
            answer: json!({
                "node": node,
                "workshop": workshop.to_string(),
                "after": { "node": at.node, "port": at.port },
                "rewired": rewired,
                "view": view,
                "values": values,
            }),
        })
    }
}

/// Where a workshop is ranked by [`suggest`]: what the user's words
/// matched first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Matched {
    /// Its name.
    Name,
    /// One of its keywords.
    Keyword,
    /// A word of its intent.
    Intent,
    /// Nothing asked: every workshop, by name.
    All,
}

/// A workshop found by [`suggest`].
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Suggestion {
    pub workshop: CompositeRef,
    pub name: String,
    pub intent: String,
    pub matched: Matched,
    /// How many of the user's words it matched, anywhere.
    pub words: usize,
}

/// Words too common to find anything by.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "at", "by", "for", "from", "i", "in", "is", "it", "my", "of", "on", "or",
    "the", "this", "to", "too", "with", "without",
];

/// The words of `text`, lower case, common ones left out.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .filter(|w| !STOP_WORDS.contains(&w.as_str()))
        .collect()
}

/// The workshops among `files` the user's words (`intent`) find, the
/// stub where an AI will later find them (`workshop.suggest`): a word
/// matches a word of a workshop that starts with it ("bright" finds
/// "brighten"). Ranked by where the first match is (the name, then the
/// keywords, then the intent's words), then by how many words matched,
/// then by name; files without a workshop block are left out. Empty
/// words: every workshop, by name. The newest version of each id only.
pub fn suggest<'a>(
    files: impl IntoIterator<Item = &'a CompositeFile>,
    intent: &str,
) -> Vec<Suggestion> {
    let mut newest: BTreeMap<&str, &CompositeFile> = BTreeMap::new();
    for f in files {
        if f.workshop.is_none() {
            continue;
        }
        let at = newest.entry(f.id.as_str()).or_insert(f);
        if f.version > at.version {
            *at = f;
        }
    }
    let asked = words(intent);
    let hits = |of: &[String]| -> Vec<&String> {
        asked
            .iter()
            .filter(|w| of.iter().any(|x| x.starts_with(w.as_str())))
            .collect()
    };
    let mut out: Vec<Suggestion> = newest
        .into_values()
        .filter_map(|f| {
            let w = f.workshop.as_ref()?;
            let suggestion = |matched, words| Suggestion {
                workshop: f.reference(),
                name: f.name.clone(),
                intent: w.intent.clone(),
                matched,
                words,
            };
            if asked.is_empty() {
                return Some(suggestion(Matched::All, 0));
            }
            let name = hits(&words(&f.name));
            let keywords = hits(&words(&w.keywords.join(" ")));
            let intent = hits(&words(&w.intent));
            let matched = if !name.is_empty() {
                Matched::Name
            } else if !keywords.is_empty() {
                Matched::Keyword
            } else if !intent.is_empty() {
                Matched::Intent
            } else {
                return None;
            };
            let mut all: Vec<&String> = name.into_iter().chain(keywords).chain(intent).collect();
            all.sort();
            all.dedup();
            Some(suggestion(matched, all.len()))
        })
        .collect();
    out.sort_by(|a, b| {
        a.matched
            .cmp(&b.matched)
            .then(b.words.cmp(&a.words))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.workshop.id.cmp(&b.workshop.id))
    });
    out
}

/// How far from `after` an applied workshop is placed, across and down.
const PLACE: f32 = 90.0;

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
