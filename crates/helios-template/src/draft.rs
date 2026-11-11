//! Workshop drafts (experimental, see `docs/design/workshops-on-the-fly.md`,
//! section 1): what an AI proposes for an intent, and Helios's own check of
//! it, with no AI.
//!
//! A [`Draft`] is a composite file with a workshop block (its modules, its
//! zones, the sliders it exposes with their ranges and `drives`, its
//! targets) and a rationale per item, so the user can read why each piece
//! is there. [`check_draft`] takes it with a small proxy of the open image
//! and answers a [`Check`]: a report the user and the agent read, and the
//! cleaned draft, ready to apply. Nothing is written.
//!
//! The check:
//! - **schema and catalogue**: the composite fits its schema, every module
//!   exists, every value and curve point is within its hard range, one
//!   input and one output, every target names an exposed slider and a zone
//!   the block has; otherwise the draft is refused, each problem said;
//! - **zones**: each zone's mask is drawn once on the proxy; one covering
//!   less than [`ZONE_EMPTY`] % or more than [`ZONE_FULL`] % of the image
//!   is reported, kept;
//! - **tested at the extremes**: each number slider is rendered at both
//!   ends of its range, the others at their defaults, and measured on the
//!   whole image and in each zone. A slider whose ends measure the same
//!   (within [`DEAD`] stop) is dropped, its value written into the inner
//!   nodes and its targets with it; an end clipping more than
//!   [`CLIP_LIMIT`] % of the image beyond what the defaults clip is pulled
//!   in, by bisection, until it does not.

use std::collections::BTreeMap;

use helios_core::{
    Composite, Engine, Graph, Image, Macro, ModuleRef, ParamKind, ParamSpec, Params, Region, Value,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::build::SOURCE_PORT;
use crate::catalogue::Catalogue;
use crate::format::{CompositeFile, Measure};
use crate::workshop::{Range, measure_in, ranges, zone_masks, zone_share};

/// A slider whose ends measure closer than this, in stops, everywhere
/// measured, changes nothing: it is dropped.
pub const DEAD: f64 = 0.05;

/// An end may clip at most this share of the image (in %) above what the
/// workshop clips at its defaults.
pub const CLIP_LIMIT: f64 = 1.0;

/// A zone covering less than this share of the proxy (in %) is reported
/// empty.
pub const ZONE_EMPTY: f64 = 0.5;

/// A zone covering more than this share of the proxy (in %) is reported
/// as the whole image.
pub const ZONE_FULL: f64 = 99.5;

/// At most this many renders to pull one end of a range in.
const NARROW_PROBES: usize = 10;

/// What an AI proposes: the workshop, and why each piece is there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    /// The user's words it answers ("backlit, the mountain and the sky too
    /// contrasted"); the workshop block's intent when that is left out.
    pub intent: String,
    /// The workshop: a composite file with a workshop block. `format`,
    /// `id` and `version` may be left out (Helios names the workshop when
    /// it applies it), and so may `inputs` and `outputs` (one RGBA image
    /// each, `input` and `output`).
    pub composite: CompositeFile,
    #[serde(default, skip_serializing_if = "Rationale::is_empty")]
    pub rationale: Rationale,
}

/// Why each piece of a draft is there, in English, by the piece's name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rationale {
    /// The workshop as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop: Option<String>,
    /// By inner node name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nodes: BTreeMap<String, String>,
    /// By exposed name (a slider).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exposed: BTreeMap<String, String>,
    /// By zone name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub zones: BTreeMap<String, String>,
    /// By the targeted slider's name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<String, String>,
}

impl Rationale {
    fn is_empty(&self) -> bool {
        self.workshop.is_none()
            && self.nodes.is_empty()
            && self.exposed.is_empty()
            && self.zones.is_empty()
            && self.targets.is_empty()
    }
}

/// A draft from JSON (what an agent posts): the composite's omitted
/// `format`, `id`, `version`, `inputs`, `outputs` and block intent filled
/// in, then the composite checked against its schema. Refused with every
/// problem found.
pub fn parse_draft(json: &Json) -> Result<Draft, Vec<String>> {
    let Some(obj) = json.as_object() else {
        return Err(vec![
            "a draft is an object: intent, composite, rationale".into(),
        ]);
    };
    let intent = obj
        .get("intent")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let Some(mut composite) = obj.get("composite").filter(|c| c.is_object()).cloned() else {
        return Err(vec!["`composite`: missing, or not an object".into()]);
    };
    let c = composite.as_object_mut().expect("an object");
    c.entry("format").or_insert(json!(1));
    c.entry("id").or_insert(json!(DRAFT_ID));
    c.entry("version").or_insert(json!(1));
    let image = |name: &str| json!([{ "name": name, "channels": 4, "layout": "rgba" }]);
    c.entry("inputs").or_insert_with(|| image("input"));
    c.entry("outputs").or_insert_with(|| image("output"));
    let block = c.entry("workshop").or_insert_with(|| json!({}));
    if let Some(b) = block.as_object_mut()
        && b.get("intent")
            .and_then(Json::as_str)
            .is_none_or(|i| i.trim().is_empty())
    {
        b.insert("intent".into(), json!(intent));
    }
    let mut problems: Vec<String> = crate::schema::check(&crate::schema::composite(), &composite)
        .into_iter()
        .map(|p| format!("composite{p}"))
        .collect();
    let mut whole = json.clone();
    whole["composite"] = composite;
    if whole.get("intent").is_none() {
        whole["intent"] = json!("");
    }
    let draft: Option<Draft> = match serde_json::from_value(whole) {
        Ok(d) => Some(d),
        Err(e) => {
            if problems.is_empty() {
                problems.push(e.to_string());
            }
            None
        }
    };
    match draft {
        Some(mut d) if problems.is_empty() => {
            d.intent = intent;
            if d.intent.is_empty()
                && let Some(w) = &d.composite.workshop
            {
                d.intent = w.intent.trim().to_string();
            }
            if d.intent.is_empty() {
                return Err(vec!["`intent`: empty: say what the workshop is for".into()]);
            }
            Ok(d)
        }
        _ => Err(problems),
    }
}

/// The id a draft carries until it is applied.
pub const DRAFT_ID: &str = "user.draft";

/// What became of a slider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Kept,
    /// Its range pulled in where an end clipped.
    Narrowed,
    /// Its ends measure the same: it changes nothing here.
    Dropped,
    /// Not a number: not tested.
    Untested,
}

/// One slider, tested at its ends.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SliderCheck {
    pub name: String,
    /// What the user reads.
    pub label: String,
    /// A semantic slider (`drives`).
    pub semantic: bool,
    pub verdict: Verdict,
    /// The range tested.
    pub range: [f64; 2],
    /// The range kept, when narrowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrowed: Option<[f64; 2]>,
    /// The most its two ends differ, in stops, over what was measured.
    pub change: f64,
    /// Where it changes most: `image`, or a zone's name.
    pub most_in: String,
    /// The share of the image clipped at each end, in %.
    pub clip: [f64; 2],
}

/// What became of a zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneVerdict {
    Ok,
    /// Under [`ZONE_EMPTY`] % of the image.
    Empty,
    /// Over [`ZONE_FULL`] % of the image.
    Whole,
}

/// One zone, drawn on the proxy.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ZoneCheck {
    pub name: String,
    /// The share of the proxy in the zone, in %.
    pub share: f64,
    pub verdict: ZoneVerdict,
}

/// What [`check_draft`] answers.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Check {
    /// Whether the draft may be applied: not refused.
    pub ok: bool,
    /// Why it is refused, each problem with where.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<String>,
    pub sliders: Vec<SliderCheck>,
    pub zones: Vec<ZoneCheck>,
    /// The report in sentences, in order: what was dropped, narrowed,
    /// reported.
    pub notes: Vec<String>,
    /// The cleaned draft, when not refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<Draft>,
}

impl Check {
    fn refused(problems: Vec<String>) -> Self {
        let notes = problems.iter().map(|p| format!("refused: {p}")).collect();
        Check {
            ok: false,
            refused: problems,
            notes,
            ..Check::default()
        }
    }

    /// The sliders kept, by name, in the order shown.
    pub fn kept(&self) -> Vec<&str> {
        self.sliders
            .iter()
            .filter(|s| s.verdict != Verdict::Dropped)
            .map(|s| s.name.as_str())
            .collect()
    }
}

/// Checks `draft` on `proxy` (what enters the workshop, a small proxy of
/// the open image), through `engine` and `catalogue`: see the module's
/// description. A refusal is an answer, not an error: the agent reads it
/// and posts again.
pub fn check_draft(draft: &Draft, catalogue: &Catalogue, engine: &Engine, proxy: &Image) -> Check {
    match checked(draft, catalogue, engine, proxy) {
        Ok(c) => c,
        Err(problems) => Check::refused(problems),
    }
}

fn checked(
    draft: &Draft,
    catalogue: &Catalogue,
    engine: &Engine,
    proxy: &Image,
) -> Result<Check, Vec<String>> {
    let one = |e: String| vec![e];
    let mut file = draft.composite.clone();
    if draft.intent.trim().is_empty() {
        return Err(one("`intent`: empty: say what the workshop is for".into()));
    }
    let schema = serde_json::to_value(&file).map_err(|e| one(e.to_string()))?;
    let problems = crate::schema::check(&crate::schema::composite(), &schema);
    if !problems.is_empty() {
        return Err(problems
            .into_iter()
            .map(|p| format!("composite{p}"))
            .collect());
    }
    if file.workshop.is_none() {
        return Err(one("`composite`: no workshop block".into()));
    }
    if file.inputs.len() != 1 || file.outputs.len() != 1 {
        return Err(one(format!(
            "`composite`: {} inputs and {} outputs: a workshop has one of each",
            file.inputs.len(),
            file.outputs.len()
        )));
    }
    let built = crate::catalogue::build(&file, catalogue, 0).map_err(one)?;
    let block = file.workshop.clone().unwrap_or_default();
    let mut notes = Vec::new();
    // Rationales for pieces the draft lacks: said, dropped.
    let mut rationale = draft.rationale.clone();
    unknown_rationales(&mut rationale, &file, &mut notes);

    // Zones, drawn once on what enters the workshop.
    let masks = zone_masks(engine, catalogue, &block.zones, proxy).map_err(one)?;
    let mut zones = Vec::new();
    for z in &block.zones {
        let share = zone_share(&masks[&z.name]);
        let verdict = if share < ZONE_EMPTY {
            notes.push(format!(
                "zone `{}`: {} % of the image: empty here, its targets are kept at their values",
                z.name,
                show(share)
            ));
            ZoneVerdict::Empty
        } else if share > ZONE_FULL {
            notes.push(format!(
                "zone `{}`: {} % of the image: the whole image, it separates nothing",
                z.name,
                show(share)
            ));
            ZoneVerdict::Whole
        } else {
            ZoneVerdict::Ok
        };
        zones.push(ZoneCheck {
            name: z.name.clone(),
            share,
            verdict,
        });
    }
    let measured: Vec<(String, &Image)> = masks
        .iter()
        .filter(|(_, m)| zone_share(m) >= ZONE_EMPTY)
        .map(|(n, m)| (n.clone(), m))
        .collect();

    // Each slider at its ends.
    let mut render = Renderer::new(engine, &built, proxy).map_err(one)?;
    let base = render.at(&Params::new()).map_err(one)?;
    let base_clip = clip(&base);
    let specs = Composite::manifest(&built).params.clone();
    let ranges = ranges(&built);
    let mut sliders = Vec::new();
    for spec in &specs {
        let exposed = file.exposed.iter().find(|x| x.name == spec.name);
        let label = exposed
            .and_then(|x| x.label.clone())
            .unwrap_or_else(|| spec.name.clone());
        let semantic = exposed.is_some_and(crate::format::Exposed::is_driven);
        let (Some(range), Some(default)) = (ranges.get(&spec.name), number_default(spec)) else {
            sliders.push(SliderCheck {
                name: spec.name.clone(),
                label,
                semantic,
                verdict: Verdict::Untested,
                range: [0.0, 0.0],
                narrowed: None,
                change: 0.0,
                most_in: String::new(),
                clip: [0.0, 0.0],
            });
            continue;
        };
        let at = |v: f64| Params::new().with(&spec.name, value_of(range, v));
        let lo = render.at(&at(range.min)).map_err(one)?;
        let hi = render.at(&at(range.max)).map_err(one)?;
        let (change, most_in) = difference(&lo, &hi, &measured);
        let clip_ends = [clip(&lo), clip(&hi)];
        let mut check = SliderCheck {
            name: spec.name.clone(),
            label: label.clone(),
            semantic,
            verdict: Verdict::Kept,
            range: [range.min, range.max],
            narrowed: None,
            change,
            most_in,
            clip: clip_ends,
        };
        if change < DEAD {
            check.verdict = Verdict::Dropped;
            notes.push(format!(
                "{label}: dropped: from {} to {} it changes the image by {} stop at most, under {DEAD}",
                show(range.min),
                show(range.max),
                show(change)
            ));
            sliders.push(check);
            continue;
        }
        // Each end that clips, pulled in towards the default.
        let mut kept = [range.min, range.max];
        for (i, end) in [range.min, range.max].into_iter().enumerate() {
            if clip_ends[i] <= base_clip + CLIP_LIMIT || end == default {
                continue;
            }
            let pulled =
                pull_in(&mut render, &spec.name, range, default, end, base_clip).map_err(one)?;
            notes.push(format!(
                "{label}: narrowed: at {} it clips {} % of the image; {} is kept",
                show(end),
                show(clip_ends[i]),
                show(pulled)
            ));
            kept[i] = pulled;
        }
        if kept != [range.min, range.max] {
            if kept[0] >= kept[1] {
                check.verdict = Verdict::Dropped;
                notes.push(format!(
                    "{label}: dropped: it clips as soon as it moves from {}",
                    show(default)
                ));
            } else {
                check.verdict = Verdict::Narrowed;
                check.narrowed = Some(kept);
            }
        }
        sliders.push(check);
    }

    // The cleaned draft.
    for s in &sliders {
        match s.verdict {
            Verdict::Dropped => drop_slider(&mut file, &built, &specs, &s.name, &mut rationale),
            Verdict::Narrowed => {
                let [min, max] = s.narrowed.expect("narrowed");
                if let Some(x) = file.exposed.iter_mut().find(|x| x.name == s.name) {
                    x.min = Some(min);
                    x.max = Some(max);
                    if let Some([a, b]) = x.soft {
                        let (a, b) = (a.clamp(min, max), b.clamp(min, max));
                        x.soft = (a < b).then_some([a, b]);
                    }
                }
            }
            Verdict::Kept | Verdict::Untested => {}
        }
    }
    if let Some(w) = &mut file.workshop {
        let before = w.targets.len();
        w.targets
            .retain(|t| file.exposed.iter().any(|x| x.name == t.param));
        if w.targets.len() < before {
            notes.push(format!(
                "{} target(s) of dropped sliders removed",
                before - w.targets.len()
            ));
        }
    }
    // What is left still builds.
    crate::catalogue::build(&file, catalogue, 0).map_err(|e| one(format!("once cleaned: {e}")))?;
    if notes.is_empty() {
        notes.push("every slider moves the image and stays within clipping; every zone holds part of the image".into());
    }
    Ok(Check {
        ok: true,
        refused: Vec::new(),
        sliders,
        zones,
        notes,
        draft: Some(Draft {
            intent: draft.intent.clone(),
            composite: file,
            rationale,
        }),
    })
}

/// Drops the rationales naming nothing of `file`, each said.
fn unknown_rationales(r: &mut Rationale, file: &CompositeFile, notes: &mut Vec<String>) {
    let block = file.workshop.as_ref();
    let mut drop =
        |map: &mut BTreeMap<String, String>, what: &str, known: &dyn Fn(&str) -> bool| {
            map.retain(|name, _| {
                let k = known(name);
                if !k {
                    notes.push(format!(
                        "rationale for {what} `{name}`: the draft has none, dropped"
                    ));
                }
                k
            });
        };
    drop(&mut r.nodes, "node", &|n| {
        file.nodes.iter().any(|x| x.name == n)
    });
    drop(&mut r.exposed, "slider", &|n| {
        file.exposed.iter().any(|x| x.name == n)
    });
    drop(&mut r.zones, "zone", &|n| {
        block.is_some_and(|b| b.zones.iter().any(|z| z.name == n))
    });
    drop(&mut r.targets, "target", &|n| {
        block.is_some_and(|b| b.targets.iter().any(|t| t.param == n))
    });
}

/// Removes the slider `name`: its value at its default is written into
/// the inner parameters it set, so the workshop renders as it did.
fn drop_slider(
    file: &mut CompositeFile,
    built: &Macro,
    specs: &[ParamSpec],
    name: &str,
    rationale: &mut Rationale,
) {
    let default = specs
        .iter()
        .find(|s| s.name == name)
        .and_then(|s| match s.kind {
            ParamKind::Float { default, .. } => Some(Value::Float(default)),
            ParamKind::Int { default, .. } => Some(Value::Int(default)),
            _ => None,
        });
    let mut writes: Vec<(String, String, Value)> = Vec::new();
    for x in file.exposed.iter().filter(|x| x.name == name) {
        if x.is_driven() {
            let at = default.as_ref().and_then(Value::as_f64).unwrap_or(0.0);
            writes.extend(built.driven(name, at));
        } else if let Some(v) = &default {
            writes.push((x.node.clone(), x.param.clone(), v.clone()));
        }
    }
    for (node, param, value) in writes {
        if let Some(n) = file.nodes.iter_mut().find(|n| n.name == node) {
            n.params.set(&param, value);
        }
    }
    file.exposed.retain(|x| x.name != name);
    rationale.exposed.remove(name);
    rationale.targets.remove(name);
}

/// A number parameter's default.
#[allow(clippy::cast_precision_loss)]
fn number_default(spec: &ParamSpec) -> Option<f64> {
    match spec.kind {
        ParamKind::Float { default, .. } => Some(default),
        ParamKind::Int { default, .. } => Some(default as f64),
        _ => None,
    }
}

fn value_of(range: &Range, v: f64) -> Value {
    if range.integer {
        Value::Int(v.round() as i64)
    } else {
        Value::Float(v)
    }
}

/// The furthest value from `default` towards `end` (which clips) that
/// does not clip, by bisection, rounded to the slider's step towards the
/// default.
fn pull_in(
    render: &mut Renderer,
    name: &str,
    range: &Range,
    default: f64,
    end: f64,
    base_clip: f64,
) -> Result<f64, String> {
    let (mut good, mut bad) = (default, end);
    for _ in 0..NARROW_PROBES {
        let mid = good.midpoint(bad);
        let image = render.at(&Params::new().with(name, value_of(range, mid)))?;
        if clip(&image) <= base_clip + CLIP_LIMIT {
            good = mid;
        } else {
            bad = mid;
        }
    }
    // Rounded to the step, never past what was found good.
    let rounded = range.round(good);
    let kept = if (rounded - default).abs() > (good - default).abs() {
        let step = range.step.unwrap_or(0.0);
        range.round(good - step.copysign(good - default))
    } else {
        rounded
    };
    Ok(kept)
}

/// The share of `image` clipped, in %.
fn clip(image: &Image) -> f64 {
    measure_in(image, Measure::ClipHigh, None, None).unwrap_or(0.0)
}

/// How much two renders differ, in stops, and where most: on the whole
/// image and in each zone, the medians, the highlights, and the mean
/// difference per channel (which a change of colour alone moves).
fn difference(a: &Image, b: &Image, zones: &[(String, &Image)]) -> (f64, String) {
    let mut most = (0.0, "image".to_string());
    // Zones first: on a tie, the zone names where it acts.
    let regions = zones
        .iter()
        .map(|(n, m)| (n.clone(), Some(*m)))
        .chain(std::iter::once(("image".to_string(), None)));
    for (name, zone) in regions {
        let mut d = mean_channel_stops(a, b, zone);
        for m in [Measure::LumaMedian, Measure::LumaHigh] {
            if let (Some(x), Some(y)) = (measure_in(a, m, zone, None), measure_in(b, m, zone, None))
            {
                d = d.max((x - y).abs());
            }
        }
        if d > most.0 {
            most = (d, name);
        }
    }
    most
}

/// The mean over pixels (in `zone`, else all) of the largest colour
/// channel difference, in stops.
fn mean_channel_stops(a: &Image, b: &Image, zone: Option<&Image>) -> f64 {
    const FLOOR: f32 = 1.0 / 1024.0;
    let n = a.channels() as usize;
    let (da, db) = (a.data(), b.data());
    if n == 0 || da.len() != db.len() {
        return 0.0;
    }
    let colours = n.min(3);
    let (mut sum, mut count) = (0.0_f64, 0_usize);
    for (i, (pa, pb)) in da.chunks_exact(n).zip(db.chunks_exact(n)).enumerate() {
        if let Some(m) = zone {
            let mc = m.channels() as usize;
            if m.data().get(i * mc).is_none_or(|v| *v < 0.5) {
                continue;
            }
        }
        let d = (0..colours)
            .map(|c| (pa[c].max(FLOOR).log2() - pb[c].max(FLOOR).log2()).abs())
            .fold(0.0_f32, f32::max);
        sum += f64::from(d);
        count += 1;
    }
    if count == 0 {
        0.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        let mean = sum / count as f64;
        mean
    }
}

/// A number for a sentence: three decimals at most.
fn show(v: f64) -> String {
    format!("{}", (v * 1000.0).round() / 1000.0)
}

/// The workshop rendered alone on the proxy, a set of values over its
/// defaults.
struct Renderer<'a> {
    engine: &'a Engine,
    graph: Graph,
    node: helios_core::NodeId,
    exit: String,
    region: Region,
    defaults: Params,
}

impl<'a> Renderer<'a> {
    fn new(engine: &'a Engine, built: &Macro, input: &Image) -> Result<Self, String> {
        let r = input.region();
        let region = Region::sized(r.width, r.height);
        let input = Image::new(region, input.channels(), input.to_vec());
        let module = ModuleRef::composite(built.clone());
        let manifest = module.manifest().clone();
        let (Some(entry), Some(exit)) = (manifest.inputs.first(), manifest.outputs.first()) else {
            return Err(format!("`{}` has no input or no output", manifest.name));
        };
        let mut graph = Graph::new();
        let src = graph.add_source("input", input);
        let node = graph
            .add_module("workshop", module, &Params::new())
            .map_err(|e| e.to_string())?;
        graph
            .connect(src, SOURCE_PORT, node, &entry.name)
            .map_err(|e| e.to_string())?;
        let defaults = graph
            .params(node)
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        Ok(Renderer {
            engine,
            graph,
            node,
            exit: exit.name.clone(),
            region,
            defaults,
        })
    }

    fn at(&mut self, values: &Params) -> Result<Image, String> {
        for (name, value) in self.defaults.iter() {
            let v = values.get(name).unwrap_or(value);
            self.graph
                .set_param(self.node, name, v.clone())
                .map_err(|e| e.to_string())?;
        }
        self.engine
            .render(&self.graph, self.node, &self.exit, self.region)
            .map_err(|e| e.to_string())
    }
}

impl crate::workspace::State {
    /// Applies a checked draft (`workshop.design`): its composite saved in
    /// the workspace as `user.<slug of its name>@1` (a free id), its
    /// block's intent the draft's when left empty, and inserted as
    /// [`State::workshop_applied`] does, as one step. With `replace` (a
    /// node a former design applied, a user's workshop), that node is
    /// pinned to the next version of its file instead: asking again
    /// replaces the design. `values` are the image's values for the node
    /// (the aimed ones), by exposed name.
    ///
    /// [`State::workshop_applied`]: crate::workspace::State::workshop_applied
    pub fn draft_applied(
        &self,
        catalogue: &Catalogue,
        draft: &Draft,
        after: Option<&str>,
        replace: Option<&str>,
        values: &Params,
    ) -> Result<crate::macros::Step, String> {
        use crate::format::CompositeRef;
        use crate::macros::{USER_PREFIX, free_id, next_ref, node_path};
        let cat = self.bundle.catalogue(catalogue);
        let mut file = draft.composite.clone();
        if let Some(w) = &mut file.workshop
            && w.intent.trim().is_empty()
        {
            w.intent.clone_from(&draft.intent);
        }
        let Some(node) = replace else {
            let r = CompositeRef {
                id: free_id(&cat, &file.name),
                version: 1,
            };
            file.id.clone_from(&r.id);
            file.version = r.version;
            let mut with = self.clone();
            with.bundle.composites.push(file);
            let mut step = with.workshop_applied(catalogue, &r, after, values)?;
            step.answer["designed"] = json!(r.to_string());
            return Ok(step);
        };
        let (i, desc, old) = self.macro_node(node)?;
        if desc.open {
            return Err(format!("`{node}` is open: fold it first"));
        }
        if !old.id.starts_with(USER_PREFIX) {
            return Err(format!(
                "`{node}` is `{old}`, not a designed workshop: a design replaces only a user's workshop"
            ));
        }
        let r = next_ref(&cat, &old, &file.name);
        file.id.clone_from(&r.id);
        file.version = r.version;
        crate::catalogue::build(&file, &cat, 0)?;
        let mut next = self.clone();
        next.bundle.template.network.nodes[i].composite = Some(r.clone());
        next.bundle.composites.push(file);
        let next = crate::workspace::State::checked(next, catalogue)?;
        let moves: crate::macros::Moves = values
            .iter()
            .map(|(p, v)| (node_path(node, p), Some(v.clone())))
            .collect();
        Ok(crate::macros::Step {
            state: next,
            values: moves,
            answer: json!({
                "node": node,
                "workshop": r.to_string(),
                "designed": r.to_string(),
                "replaced": old.to_string(),
            }),
        })
    }
}
