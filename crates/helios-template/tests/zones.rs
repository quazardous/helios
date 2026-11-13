//! Zone measures and zone targets (see `docs/design/workshops-on-the-fly.md`,
//! slice 2): a workshop names parts of the image (a sky, a foreground),
//! each a mask drawn once from its input; a target measured inside a zone
//! is aimed there. A synthetic backlit image: a bright sky band over a
//! dark foreground.

use std::collections::BTreeMap;
use std::sync::Arc;

use helios_core::{Bus, Engine, Image, ModuleRef, Params, Region};
use helios_template::format::{Measure, Target};
use helios_template::workshop::{self, measure_in, zone_masks, zone_share};
use helios_template::{Catalogue, CompositeFile, parse_composite, schema};
use serde_json::{Value as Json, json};

fn catalogue() -> Catalogue {
    Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(helios_module_masks::MaskParametric))
}

const W: u32 = 32;
const H: u32 = 32;
/// The sky's rows, from the top.
const SKY_ROWS: u32 = 12;

/// The sky's greys run from 0.6 to 1.0, the foreground's from 0.03 to
/// 0.08, across each band's pixels.
fn backlit() -> Image {
    Image::from_fn(Region::sized(W, H), 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let t = x as f32 / (W - 1) as f32;
        #[allow(clippy::cast_sign_loss)]
        if (y as u32) < SKY_ROWS {
            0.6 + 0.4 * t
        } else {
            0.03 + 0.05 * t
        }
    })
}

/// The exposure as a workshop, its zones and `targets` in its block.
fn json_of(targets: &Json) -> Json {
    json!({
        "format": 1,
        "id": "test.backlit",
        "version": 1,
        "name": "Backlit",
        "inputs": [ { "name": "input", "channels": 4, "layout": "rgba" } ],
        "outputs": [ { "name": "output", "channels": 4, "layout": "rgba" } ],
        "nodes": [ { "name": "exp", "module": "exposure" } ],
        "connections": [ { "from": "@", "port": "input", "to": "exp", "input": "input" } ],
        "exits": [ { "from": "exp", "port": "output", "output": "output" } ],
        "exposed": [
            { "name": "brightness", "node": "exp", "param": "exposure", "min": -3.0, "max": 3.0 }
        ],
        "workshop": {
            "intent": "Backlit: the sky and the foreground too far apart",
            "zones": [
                { "name": "sky", "module": "mask_parametric",
                  "params": { "luminance": { "list": [0.4, 4.0, 0.0, 0.0] } } },
                { "name": "foreground", "module": "mask_parametric",
                  "params": { "luminance": { "list": [0.0, 0.2, 0.0, 0.0] } } },
                { "name": "nothing", "module": "mask_parametric",
                  "params": { "luminance": { "list": [2.0, 3.0, 0.0, 0.0] } } }
            ],
            "targets": targets
        }
    })
}

fn file(targets: &Json) -> CompositeFile {
    parse_composite(&json_of(targets).to_string()).unwrap()
}

fn built(file: &CompositeFile) -> Result<helios_core::Macro, String> {
    helios_template::catalogue::build(file, &catalogue(), 0)
}

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

fn masks(file: &CompositeFile) -> BTreeMap<String, Image> {
    let zones = &file.workshop.as_ref().unwrap().zones;
    zone_masks(&engine(), &catalogue(), zones, &backlit()).unwrap()
}

fn stops(v: f64) -> f64 {
    v.log2()
}

#[test]
fn the_block_with_zones_fits_its_schema_and_round_trips() {
    let targets = json!([
        { "param": "brightness", "measure": "zones.ratio", "value": -2.0,
          "zone": "foreground", "against": "sky" }
    ]);
    let json = json_of(&targets);
    let problems = schema::check(&schema::composite(), &json);
    assert!(problems.is_empty(), "{problems:?}");
    let f = file(&targets);
    let again = parse_composite(&serde_json::to_string(&f).unwrap()).unwrap();
    assert_eq!(again, f);
    let w = f.workshop.as_ref().unwrap();
    assert_eq!(w.zones[0].port, "mask", "the default port");
    assert_eq!(w.targets[0].zone.as_deref(), Some("foreground"));
    built(&f).unwrap();
}

#[test]
fn each_zone_is_measured_on_its_own_pixels() {
    let f = file(&json!([]));
    let masks = masks(&f);
    let image = backlit();
    let sky = &masks["sky"];
    let fg = &masks["foreground"];
    #[allow(clippy::cast_precision_loss)]
    let sky_share = f64::from(SKY_ROWS) * 100.0 / f64::from(H);
    assert!(
        (zone_share(sky) - sky_share).abs() < 1e-9,
        "{}",
        zone_share(sky)
    );
    assert!((zone_share(fg) - (100.0 - sky_share)).abs() < 1e-9);
    assert_eq!(zone_share(&masks["nothing"]), 0.0);
    // Medians: the middle of each band's ramp (luminance of a grey is the
    // grey), within a histogram bin.
    let near = |v: Option<f64>, want: f64| {
        let v = v.unwrap();
        assert!((v - want).abs() < 0.03, "{v} against {want}");
    };
    near(
        measure_in(&image, Measure::LumaMedian, Some(sky), None),
        stops(0.8),
    );
    near(
        measure_in(&image, Measure::LumaMedian, Some(fg), None),
        stops(0.055),
    );
    // The backlight's gap, in stops; and each zone's own contrast.
    near(
        measure_in(&image, Measure::ZonesRatio, Some(fg), Some(sky)),
        stops(0.055) - stops(0.8),
    );
    near(
        measure_in(&image, Measure::ZoneContrast, Some(sky), None),
        stops(0.98) - stops(0.62),
    );
    // The whole image's median sits in the foreground (20 rows of 32).
    let whole = workshop::measure(&image, Measure::LumaMedian).unwrap();
    assert!(whole < stops(0.1), "{whole}");
    // Nothing in a zone, nothing measured; a ratio needs two zones.
    assert_eq!(
        measure_in(&image, Measure::LumaMedian, Some(&masks["nothing"]), None),
        None
    );
    assert_eq!(
        measure_in(&image, Measure::ZonesRatio, Some(fg), None),
        None
    );
    assert_eq!(workshop::measure(&image, Measure::ZonesRatio), None);
}

/// Aims `targets` on the backlit image; the answers and the output.
fn aim(targets: &Json) -> (Vec<workshop::Aimed>, Image) {
    let f = file(targets);
    let m = built(&f).unwrap();
    let masks = masks(&f);
    let block = f.workshop.as_ref().unwrap();
    let input = backlit();
    let (answers, values) = workshop::aim_rendered(
        &engine(),
        &m,
        &input,
        &masks,
        &Params::new(),
        &block.targets,
    )
    .unwrap();
    // The output at the aimed values.
    let mut graph = helios_core::Graph::new();
    let src = graph.add_source("input", input);
    let node = graph
        .add_module("w", ModuleRef::composite(m), &values)
        .unwrap();
    graph.connect(src, "image", node, "input").unwrap();
    let out = engine()
        .render(&graph, node, "output", Region::sized(W, H))
        .unwrap();
    (answers, out)
}

#[test]
fn sky_midtones_are_aimed_inside_the_sky() {
    let (answers, out) = aim(&json!([
        { "param": "brightness", "measure": "luma.median", "value": -1.0, "zone": "sky" }
    ]));
    let a = &answers[0];
    assert!(a.reached, "{a:?}");
    let masks = masks(&file(&json!([])));
    let sky = measure_in(&out, Measure::LumaMedian, Some(&masks["sky"]), None).unwrap();
    assert!((sky + 1.0).abs() < 0.05, "sky midtones at {sky}");
    // About -0.68 EV: from 0.8 (-0.32 stops) down to -1 stop.
    assert!((a.value - (-1.0 - stops(0.8))).abs() < 0.06, "{a:?}");
    // The whole image's median would have asked for something else.
    let whole = workshop::measure(&out, Measure::LumaMedian).unwrap();
    assert!((whole + 1.0).abs() > 1.0, "{whole}");
}

#[test]
fn the_foreground_is_aimed_at_its_own_median() {
    let (answers, out) = aim(&json!([
        { "param": "brightness", "measure": "luma.median", "value": -1.5, "zone": "foreground" }
    ]));
    assert!(answers[0].reached, "{answers:?}");
    let masks = masks(&file(&json!([])));
    let fg = measure_in(&out, Measure::LumaMedian, Some(&masks["foreground"]), None).unwrap();
    assert!((fg + 1.5).abs() < 0.05, "foreground midtones at {fg}");
}

#[test]
fn an_empty_zone_keeps_the_value_and_says_so() {
    let (answers, _) = aim(&json!([
        { "param": "brightness", "measure": "luma.median", "value": -1.0, "zone": "nothing" }
    ]));
    let a = &answers[0];
    assert!(!a.reached);
    assert_eq!(a.probes, 0);
    let note = a.note.as_deref().unwrap();
    assert!(note.contains("zone `nothing` is empty"), "{note}");
    // A gap a brightness cannot move (both zones scale alike): kept.
    let (answers, _) = aim(&json!([
        { "param": "brightness", "measure": "zones.ratio", "value": -2.0,
          "zone": "foreground", "against": "sky" }
    ]));
    assert!(!answers[0].reached, "{answers:?}");
}

#[test]
fn refusals_name_the_target_or_the_zone() {
    let refused = |targets: Json, zones: Option<Json>, want: &str| {
        let mut json = json_of(&targets);
        if let Some(z) = zones {
            json["workshop"]["zones"] = z;
        }
        let f = parse_composite(&json.to_string()).unwrap();
        let e = built(&f).map(|_| ()).unwrap_err();
        assert!(e.contains("workshop") && e.contains(want), "{want}: {e}");
    };
    refused(
        json!([{ "param": "brightness", "measure": "luma.median", "value": 0.0, "zone": "moon" }]),
        None,
        "no zone `moon`",
    );
    refused(
        json!([{ "param": "brightness", "measure": "zones.ratio", "value": 0.0, "zone": "sky" }]),
        None,
        "needs a `zone` and the zone it is `against`",
    );
    refused(
        json!([{ "param": "brightness", "measure": "zones.ratio", "value": 0.0,
                 "zone": "sky", "against": "sky" }]),
        None,
        "not one with itself",
    );
    refused(
        json!([{ "param": "brightness", "measure": "luma.median", "value": 0.0,
                 "zone": "sky", "against": "foreground" }]),
        None,
        "`against` is for `zones.ratio`",
    );
    refused(
        json!([]),
        Some(json!([{ "name": "sky", "module": "exposure" }])),
        "no output `mask`",
    );
    refused(
        json!([]),
        Some(json!([{ "name": "sky", "module": "exposure", "port": "output" }])),
        "not a mask",
    );
    refused(
        json!([]),
        Some(json!([{ "name": "Sky", "module": "mask_parametric" }])),
        "not a name",
    );
    refused(
        json!([]),
        Some(json!([
            { "name": "sky", "module": "mask_parametric" },
            { "name": "sky", "module": "mask_parametric" }
        ])),
        "a second zone",
    );
    refused(
        json!([]),
        Some(json!([{ "name": "sky", "module": "nope" }])),
        "unknown module",
    );
    // A target naming a zone the caller did not draw.
    let f = file(&json!([
        { "param": "brightness", "measure": "luma.median", "value": -1.0, "zone": "sky" }
    ]));
    let input = backlit();
    let e = workshop::aim_rendered(
        &engine(),
        &built(&f).unwrap(),
        &input,
        &BTreeMap::new(),
        &Params::new(),
        &f.workshop.unwrap().targets,
    )
    .unwrap_err();
    assert!(e.contains("no zone `sky` drawn"), "{e}");
    let _: Option<Target> = None;
}

#[test]
fn junctions_fit_the_schema_and_refusals_name_them() {
    let with = |junctions: Json, targets: Json| {
        let mut json = json_of(&targets);
        json["workshop"]["junctions"] = junctions;
        json
    };
    // A value asked per photo, with its fallback: schema, round trip,
    // built.
    let json = with(
        json!([{ "kind": "value", "name": "brightness", "fallback": 0.5,
                 "note": "kept as AI, 1 call per photo" }]),
        json!([]),
    );
    let problems = schema::check(&schema::composite(), &json);
    assert!(problems.is_empty(), "{problems:?}");
    let f = parse_composite(&json.to_string()).unwrap();
    let again = parse_composite(&serde_json::to_string(&f).unwrap()).unwrap();
    assert_eq!(again, f);
    built(&f).unwrap();
    let refused = |junctions: Json, targets: Json, want: &str| {
        let f = parse_composite(&with(junctions, targets).to_string()).unwrap();
        let e = built(&f).map(|_| ()).unwrap_err();
        assert!(e.contains("junctions[") && e.contains(want), "{want}: {e}");
    };
    refused(
        json!([{ "kind": "value", "name": "nope", "fallback": 0.0 }]),
        json!([]),
        "not an exposed parameter",
    );
    refused(
        json!([{ "kind": "value", "name": "brightness" }]),
        json!([]),
        "needs a `fallback`",
    );
    refused(
        json!([{ "kind": "value", "name": "brightness", "fallback": 0.0 }]),
        json!([{ "param": "brightness", "measure": "luma.median", "value": 0.0 }]),
        "aimed at a target",
    );
    refused(
        json!([
            { "kind": "value", "name": "brightness", "fallback": 0.0 },
            { "kind": "value", "name": "brightness", "fallback": 1.0 }
        ]),
        json!([]),
        "a second junction",
    );
    refused(
        json!([{ "kind": "zone", "name": "moon" }]),
        json!([]),
        "no zone of that name",
    );
    refused(
        json!([{ "kind": "zone", "name": "sky" }]),
        json!([]),
        "not asked to the AI",
    );
}
