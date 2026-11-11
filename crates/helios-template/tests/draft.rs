//! Workshop drafts and their check (see
//! `docs/design/workshops-on-the-fly.md`, slice 3): what an AI proposes is
//! checked by Helios alone, on a synthetic backlit proxy (a bright sky
//! band over a dark foreground): useless sliders dropped, clipping ranges
//! narrowed, empty or whole zones reported, bad drafts refused.

use std::sync::Arc;

use egui_pop_model::layout::Layout;
use helios_core::{Bus, Engine, Graph, Image, ModuleRef, Params, Region, Value};
use helios_template::draft::{
    CLIP_LIMIT, Check, Draft, Verdict, ZoneVerdict, check_draft, parse_draft,
};
use helios_template::workspace::{self, State};
use helios_template::{Bundle, Catalogue, SOURCE_PORT, parse_template};
use serde_json::{Value as Json, json};

fn catalogue() -> Catalogue {
    use helios_module_masks as m;
    Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(m::MaskParametric))
        .module(ModuleRef::new(m::MaskBlend))
}

const W: u32 = 32;
const H: u32 = 32;
/// The sky's rows, from the top.
const SKY_ROWS: u32 = 12;

/// The sky's greys run from 0.5 to 0.9, the foreground's from 0.03 to
/// 0.08: nothing clips.
fn backlit() -> Image {
    Image::from_fn(Region::sized(W, H), 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let t = x as f32 / (W - 1) as f32;
        #[allow(clippy::cast_sign_loss)]
        if (y as u32) < SKY_ROWS {
            0.5 + 0.4 * t
        } else {
            0.03 + 0.05 * t
        }
    })
}

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

fn zone(name: &str, lo: f64, hi: f64) -> Json {
    json!({ "name": name, "module": "mask_parametric",
            "params": { "luminance": { "list": [lo, hi, 0.0, 0.0] } } })
}

/// A backlit workshop: an exposure for the whole image, a second one
/// through a sky mask; `brightness` (plain) and `sky` (semantic) exposed.
/// `extra` sliders are added; `zones` and `targets` make the block.
fn draft_json(brightness: [f64; 2], extra: &[Json], zones: &[Json], targets: &Json) -> Json {
    let mut exposed = vec![
        json!({ "name": "brightness", "label": "Brightness", "node": "exp", "param": "exposure",
                "min": brightness[0], "max": brightness[1] }),
        json!({ "name": "sky", "label": "Sky", "min": -1.0, "max": 1.0,
                "drives": [ { "node": "sky_exp", "param": "exposure",
                              "curve": [[-1.0, -1.0], [1.0, 0.0]] } ] }),
    ];
    exposed.extend(extra.iter().cloned());
    json!({
        "intent": "backlit, the mountain and the sky too contrasted",
        "composite": {
            "name": "Backlit mountain",
            "nodes": [
                { "name": "exp", "module": "exposure" },
                { "name": "sky_mask", "module": "mask_parametric",
                  "params": { "luminance": { "list": [0.3, 4.0, 0.05, 0.0] } } },
                { "name": "sky_exp", "module": "exposure" },
                { "name": "blend", "module": "mask_blend" }
            ],
            "connections": [
                { "from": "@", "port": "input", "to": "exp", "input": "input" },
                { "from": "@", "port": "input", "to": "sky_mask", "input": "input" },
                { "from": "exp", "port": "output", "to": "sky_exp", "input": "input" },
                { "from": "exp", "port": "output", "to": "blend", "input": "input" },
                { "from": "sky_exp", "port": "output", "to": "blend", "input": "processed" },
                { "from": "sky_mask", "port": "mask", "to": "blend", "input": "mask" }
            ],
            "exits": [ { "from": "blend", "port": "output", "output": "output" } ],
            "exposed": exposed,
            "workshop": { "intent": "", "zones": zones, "targets": targets }
        },
        "rationale": {
            "workshop": "Lift the foreground, hold the sky",
            "nodes": { "sky_exp": "Darkens the sky alone" },
            "exposed": { "sky": "One move for the sky's brightness" },
            "zones": { "sky": "Where the sky is" }
        }
    })
}

fn good_zones() -> Vec<Json> {
    vec![zone("sky", 0.3, 4.0), zone("foreground", 0.0, 0.2)]
}

fn good_targets() -> Json {
    json!([{ "param": "brightness", "measure": "luma.median", "value": -3.0, "zone": "foreground" }])
}

fn draft(json: &Json) -> Draft {
    parse_draft(json).unwrap()
}

fn check(json: &Json) -> Check {
    check_draft(&draft(json), &catalogue(), &engine(), &backlit())
}

fn slider<'a>(c: &'a Check, name: &str) -> &'a helios_template::draft::SliderCheck {
    c.sliders.iter().find(|s| s.name == name).unwrap()
}

/// The workshop's output on the proxy at its defaults.
fn render_file(file: &helios_template::CompositeFile) -> Vec<f32> {
    let m = helios_template::catalogue::build(file, &catalogue(), 0).unwrap();
    let mut graph = Graph::new();
    let src = graph.add_source("input", backlit());
    let node = graph
        .add_module("w", ModuleRef::composite(m), &Params::new())
        .unwrap();
    graph.connect(src, SOURCE_PORT, node, "input").unwrap();
    engine()
        .render(&graph, node, "output", Region::sized(W, H))
        .unwrap()
        .to_vec()
}

#[test]
fn a_good_draft_is_kept_as_it_is_and_says_so() {
    let json = draft_json([-3.0, 0.0], &[], &good_zones(), &good_targets());
    let d = draft(&json);
    // What was left out is filled in.
    assert_eq!(d.composite.format, 1);
    assert_eq!(d.composite.inputs[0].name, "input");
    assert_eq!(
        d.composite.workshop.as_ref().unwrap().intent,
        "backlit, the mountain and the sky too contrasted"
    );
    let c = check(&json);
    assert!(c.ok, "{:?}", c.refused);
    assert_eq!(c.kept(), ["brightness", "sky"]);
    assert!(
        c.sliders.iter().all(|s| s.verdict == Verdict::Kept),
        "{c:?}"
    );
    assert_eq!(
        slider(&c, "sky").most_in,
        "sky",
        "the sky slider moves the sky"
    );
    assert!(slider(&c, "sky").semantic);
    assert!(c.zones.iter().all(|z| z.verdict == ZoneVerdict::Ok));
    #[allow(clippy::cast_precision_loss)]
    let sky_share = f64::from(SKY_ROWS) * 100.0 / f64::from(H);
    assert!((c.zones[0].share - sky_share).abs() < 1e-9, "{:?}", c.zones);
    assert_eq!(c.notes.len(), 1, "{:?}", c.notes);
    assert!(c.notes[0].starts_with("every slider moves the image"));
    let cleaned = c.draft.as_ref().unwrap();
    assert_eq!(cleaned, &d, "nothing to clean");
    // The report reads as JSON, for an agent.
    let report = serde_json::to_value(&c).unwrap();
    assert_eq!(report["ok"], json!(true));
    assert_eq!(report["sliders"][0]["verdict"], json!("kept"));
}

#[test]
fn a_dead_slider_is_dropped_and_the_workshop_renders_as_before() {
    // A haze slider whose curve is flat: it moves nothing.
    let haze = json!({ "name": "haze", "label": "Haze", "min": 0.0, "max": 1.0, "default": { "float": 0.0 },
                       "drives": [ { "node": "exp", "param": "black", "curve": [[0.0, 0.0], [1.0, 0.0]] } ] });
    let mut targets = good_targets();
    targets
        .as_array_mut()
        .unwrap()
        .push(json!({ "param": "haze", "measure": "luma.median", "value": -1.0 }));
    let mut json = draft_json([-3.0, 0.0], &[haze], &good_zones(), &targets);
    json["rationale"]["exposed"]["haze"] = json!("Clears the haze");
    let original = draft(&json);
    let c = check(&json);
    assert!(c.ok, "{:?}", c.refused);
    let haze = slider(&c, "haze");
    assert_eq!(haze.verdict, Verdict::Dropped);
    assert!(haze.change < helios_template::draft::DEAD, "{haze:?}");
    assert_eq!(c.kept(), ["brightness", "sky"]);
    assert!(
        c.notes.iter().any(|n| n.starts_with("Haze: dropped")),
        "{:?}",
        c.notes
    );
    assert!(
        c.notes
            .iter()
            .any(|n| n.contains("1 target(s) of dropped sliders removed")),
        "{:?}",
        c.notes
    );
    let cleaned = c.draft.unwrap();
    let file = &cleaned.composite;
    assert!(file.exposed.iter().all(|x| x.name != "haze"));
    assert!(
        file.workshop
            .as_ref()
            .unwrap()
            .targets
            .iter()
            .all(|t| t.param != "haze")
    );
    assert!(!cleaned.rationale.exposed.contains_key("haze"));
    // Its value at its default is written where it drove.
    let exp = file.nodes.iter().find(|n| n.name == "exp").unwrap();
    assert_eq!(exp.params.get("black"), Some(&Value::Float(0.0)));
    assert_eq!(render_file(file), render_file(&original.composite));
}

#[test]
fn a_clipping_range_is_narrowed_until_it_does_not_clip() {
    let json = draft_json([-3.0, 2.0], &[], &good_zones(), &good_targets());
    let c = check(&json);
    assert!(c.ok, "{:?}", c.refused);
    let b = slider(&c, "brightness");
    assert_eq!(b.verdict, Verdict::Narrowed, "{b:?}");
    assert!(b.clip[1] > 30.0, "two stops up, the sky clips: {b:?}");
    let [min, max] = b.narrowed.unwrap();
    assert_eq!(min, -3.0, "the dark end does not clip");
    // The Sky slider at its default holds the sky half a stop down: its
    // top (0.9, so 0.64) reaches 1.0 at about +0.65 EV.
    assert!(max > 0.45 && max < 0.7, "{max}");
    assert!(
        c.notes
            .iter()
            .any(|n| n.starts_with("Brightness: narrowed: at 2 it clips")),
        "{:?}",
        c.notes
    );
    let cleaned = c.draft.clone().unwrap();
    let x = &cleaned.composite.exposed[0];
    assert_eq!((x.min, x.max), (Some(min), Some(max)));
    // At the kept end, the image clips no more than the limit.
    let mut file = cleaned.composite.clone();
    let exp = file.nodes.iter_mut().find(|n| n.name == "exp").unwrap();
    exp.params.set("exposure", Value::Float(max));
    file.exposed.remove(0);
    file.workshop.as_mut().unwrap().targets.clear();
    let out = render_file(&file);
    #[allow(clippy::cast_precision_loss)]
    let clipped = out
        .chunks_exact(4)
        .filter(|p| p[..3].iter().any(|v| *v > 1.0))
        .count() as f64
        * 100.0
        / f64::from(W * H);
    assert!(clipped <= CLIP_LIMIT, "{clipped} %");
    // The other slider is untouched.
    assert_eq!(slider(&c, "sky").verdict, Verdict::Kept);
}

#[test]
fn empty_and_whole_zones_are_reported_and_kept() {
    let mut zones = good_zones();
    zones.push(zone("moon", 2.0, 3.0));
    zones.push(zone("everything", 0.0, 4.0));
    let json = draft_json([-3.0, 0.0], &[], &zones, &good_targets());
    let c = check(&json);
    assert!(c.ok, "{:?}", c.refused);
    let verdict = |n: &str| c.zones.iter().find(|z| z.name == n).unwrap().verdict;
    assert_eq!(verdict("sky"), ZoneVerdict::Ok);
    assert_eq!(verdict("moon"), ZoneVerdict::Empty);
    assert_eq!(verdict("everything"), ZoneVerdict::Whole);
    assert!(
        c.notes
            .iter()
            .any(|n| n.starts_with("zone `moon`: 0 % of the image: empty")),
        "{:?}",
        c.notes
    );
    assert!(
        c.notes
            .iter()
            .any(|n| n.starts_with("zone `everything`: 100 % of the image")),
        "{:?}",
        c.notes
    );
    let zones = &c.draft.unwrap().composite.workshop.unwrap().zones;
    assert_eq!(zones.len(), 4, "reported, not removed");
}

#[test]
fn bad_drafts_are_refused_each_problem_said() {
    let refused = |json: Json, want: &str| {
        let c = match parse_draft(&json) {
            Ok(d) => check_draft(&d, &catalogue(), &engine(), &backlit()),
            Err(problems) => {
                let all = problems.join("; ");
                assert!(all.contains(want), "{want}: {all}");
                return;
            }
        };
        assert!(!c.ok, "{want}: {c:?}");
        assert!(c.draft.is_none());
        let all = c.refused.join("; ");
        assert!(all.contains(want), "{want}: {all}");
        assert!(c.notes.iter().all(|n| n.starts_with("refused: ")));
    };
    let good = || draft_json([-3.0, 0.0], &[], &good_zones(), &good_targets());
    // A target naming a zone the block lacks.
    let mut json = good();
    json["composite"]["workshop"]["targets"] =
        json!([{ "param": "brightness", "measure": "luma.median", "value": -1.0, "zone": "moon" }]);
    refused(json, "no zone `moon`");
    // A module the catalogue lacks.
    let mut json = good();
    json["composite"]["nodes"][0]["module"] = json!("toneequal");
    refused(json, "unknown module `toneequal`");
    // A curve point outside the inner range.
    let mut json = good();
    json["composite"]["exposed"][1]["drives"][0]["curve"] = json!([[-1.0, -40.0], [1.0, 0.0]]);
    refused(json, "sky");
    // A field the format does not have.
    let mut json = good();
    json["composite"]["colour"] = json!(1);
    refused(json, "colour");
    // Two outputs.
    let mut json = good();
    json["composite"]["outputs"] = json!([
        { "name": "output", "channels": 4 }, { "name": "mask", "channels": 1 }
    ]);
    refused(json, "a workshop has one of each");
    // No intent anywhere.
    let mut json = good();
    json["intent"] = json!("");
    refused(json, "`intent`: empty");
    // No composite.
    refused(json!({ "intent": "brighter" }), "`composite`: missing");
}

#[test]
fn a_rationale_for_nothing_is_said_and_dropped() {
    let mut json = draft_json([-3.0, 0.0], &[], &good_zones(), &good_targets());
    json["rationale"]["zones"]["moon"] = json!("Where the moon is");
    let c = check(&json);
    assert!(c.ok);
    assert!(
        c.notes
            .iter()
            .any(|n| n == "rationale for zone `moon`: the draft has none, dropped"),
        "{:?}",
        c.notes
    );
    assert!(!c.draft.unwrap().rationale.zones.contains_key("moon"));
}

fn state() -> State {
    let template = parse_template(
        &json!({
            "format": 1,
            "id": "test.design",
            "version": 1,
            "meta": { "name": "Design", "scenario": "A picture to design a workshop on" },
            "network": {
                "sources": ["source"],
                "nodes": [ { "name": "exposure", "module": "exposure" } ],
                "connections": [
                    { "from": "source", "port": "image", "to": "exposure", "input": "input" }
                ],
                "view": { "node": "exposure", "port": "output" }
            }
        })
        .to_string(),
    )
    .unwrap();
    State {
        meta: workspace::Meta {
            name: "design".into(),
            ..workspace::Meta::default()
        },
        bundle: Bundle::new(template, Layout::default()),
    }
}

#[test]
fn a_checked_draft_is_applied_as_one_step_and_a_new_design_replaces_it() {
    let json = draft_json([-3.0, 2.0], &[], &good_zones(), &good_targets());
    let c = check(&json);
    let cleaned = c.draft.unwrap();
    let now = state();
    let values = Params::new().with("brightness", Value::Float(-1.0));
    let step = now
        .draft_applied(&catalogue(), &cleaned, None, None, &values)
        .unwrap();
    assert_eq!(
        step.answer["designed"],
        json!("user.backlit-mountain@1"),
        "{}",
        step.answer
    );
    let node = step.answer["node"].as_str().unwrap().to_string();
    let net = &step.state.bundle.template.network;
    assert_eq!(net.view.node, node, "the view shows the workshop");
    let file = step
        .state
        .bundle
        .composites
        .iter()
        .find(|f| f.id == "user.backlit-mountain")
        .unwrap();
    assert_eq!(
        file.exposed, cleaned.composite.exposed,
        "exactly its sliders"
    );
    assert_eq!(
        file.workshop.as_ref().unwrap().intent,
        "backlit, the mountain and the sky too contrasted"
    );
    assert_eq!(
        step.values.get(&format!("node/{node}/brightness")),
        Some(&Some(Value::Float(-1.0)))
    );
    // Asked again: the same node, pinned to the next version.
    let again = step
        .state
        .draft_applied(&catalogue(), &cleaned, None, Some(&node), &Params::new())
        .unwrap();
    assert_eq!(again.answer["designed"], json!("user.backlit-mountain@2"));
    assert_eq!(again.answer["replaced"], json!("user.backlit-mountain@1"));
    assert_eq!(
        again.state.bundle.template.network.nodes.len(),
        step.state.bundle.template.network.nodes.len()
    );
    // A module is not a design to replace.
    let e = step
        .state
        .draft_applied(
            &catalogue(),
            &cleaned,
            None,
            Some("exposure"),
            &Params::new(),
        )
        .unwrap_err();
    assert!(e.contains("not a macro"), "{e}");
}
