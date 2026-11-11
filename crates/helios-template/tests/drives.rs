//! Semantic sliders (`drives`, see `docs/design/workshops-on-the-fly.md`):
//! one exposed number moves several inner numbers along piecewise-linear
//! curves. The format round-trips and fits its schema, refusals name the
//! entry, and the macro renders as the flat chain with the curves' values,
//! with the same content key.

use std::sync::Arc;

use helios_core::{
    Bus, Composite, Context, Engine, Expansion, Graph, Image, ModuleRef, Params, Region, Value,
};
use helios_template::{Catalogue, CompositeFile, parse_composite, schema};
use serde_json::{Value as Json, json};

fn catalogue() -> Catalogue {
    Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(helios_module_sigmoid::Sigmoid))
}

/// `exp → tone`, with `exposed` as its interface.
fn json_of(exposed: &Json) -> Json {
    json!({
        "format": 1,
        "id": "test.backlit",
        "version": 1,
        "name": "Backlit",
        "inputs": [ { "name": "input", "channels": 4, "layout": "rgba" } ],
        "outputs": [ { "name": "output", "channels": 4, "layout": "rgba" } ],
        "nodes": [
            { "name": "exp", "module": "exposure" },
            { "name": "tone", "module": "sigmoid" }
        ],
        "connections": [
            { "from": "@", "port": "input", "to": "exp", "input": "input" },
            { "from": "exp", "port": "output", "to": "tone", "input": "input" }
        ],
        "exits": [ { "from": "tone", "port": "output", "output": "output" } ],
        "exposed": exposed
    })
}

/// "Backlight lift": the exposure up along one curve, the black level
/// down along a knee, the contrast eased; plus a plain slider.
fn backlight() -> Json {
    json!([
        { "name": "backlight_lift", "label": "Backlight lift", "group": "Light",
          "min": 0.0, "max": 1.0,
          "drives": [
            { "node": "exp", "param": "exposure", "curve": [[0, 0], [1, 1.6]] },
            { "node": "exp", "param": "black", "curve": [[0, 0], [0.5, -0.01], [1, -0.03]] },
            { "node": "tone", "param": "contrast", "curve": [[0, 1.5], [1, 1.2]] }
          ] },
        { "name": "skew", "node": "tone", "param": "skew" }
    ])
}

fn build(exposed: &Json) -> Result<helios_core::Macro, String> {
    let text = json_of(exposed).to_string();
    let file: CompositeFile = parse_composite(&text)?;
    helios_template::catalogue::build(&file, &catalogue(), 0)
}

#[test]
fn the_format_fits_its_schema_and_round_trips() {
    let json = json_of(&backlight());
    let problems = schema::check(&schema::composite(), &json);
    assert!(problems.is_empty(), "{problems:?}");
    let file = parse_composite(&json.to_string()).unwrap();
    let x = &file.exposed[0];
    assert!(x.is_driven());
    assert!(x.node.is_empty() && x.param.is_empty());
    assert_eq!(
        x.inner(),
        [("exp", "exposure"), ("exp", "black"), ("tone", "contrast")]
    );
    assert_eq!(x.drives[1].curve, [[0.0, 0.0], [0.5, -0.01], [1.0, -0.03]]);
    // Written back: the same file, with neither `node` nor `param`.
    let text = serde_json::to_string(&file).unwrap();
    let again: Json = serde_json::from_str(&text).unwrap();
    assert!(again["exposed"][0].get("node").is_none(), "{again}");
    assert_eq!(parse_composite(&text).unwrap(), file);
    // Both forms at once fit neither of the schema's options.
    let mut both = backlight();
    both[0]["node"] = json!("exp");
    both[0]["param"] = json!("exposure");
    let problems = schema::check(&schema::composite(), &json_of(&both));
    assert!(!problems.is_empty());
}

#[test]
fn the_slider_is_a_number_of_the_macro_s_own() {
    let built = build(&backlight()).unwrap();
    let manifest = Composite::manifest(&built);
    let names: Vec<&str> = manifest.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["backlight_lift", "skew"]);
    assert_eq!(
        manifest.params[0].kind,
        helios_core::ParamKind::Float {
            min: 0.0,
            max: 1.0,
            default: 0.0
        }
    );
    let hint = &Composite::param_hints(&built)[0];
    assert_eq!(hint.label.as_deref(), Some("Backlight lift"));
    assert_eq!(hint.group.as_deref(), Some("Light"));
    // Without a range, the curves' domain; a default of its own.
    let wide = json!([
        { "name": "sky", "default": { "float": 0.25 },
          "drives": [
            { "node": "exp", "param": "exposure", "curve": [[-1, 0.5], [1, -0.5]] },
            { "node": "tone", "param": "skew", "curve": [[0, 0], [2, 0.4]] }
          ] }
    ]);
    let built = build(&wide).unwrap();
    assert_eq!(
        Composite::manifest(&built).params[0].kind,
        helios_core::ParamKind::Float {
            min: -1.0,
            max: 2.0,
            default: 0.25
        }
    );
}

#[test]
fn moving_backlight_lift_moves_the_inner_values_as_the_curves_say() {
    let built = build(&backlight()).unwrap();
    for (x, exposure, black, contrast) in [
        (0.0, 0.0, 0.0, 1.5),
        (0.25, 0.4, -0.005, 1.425),
        (0.5, 0.8, -0.01, 1.35),
        (1.0, 1.6, -0.03, 1.2),
    ] {
        let mut params = Params::new();
        params.set("backlight_lift", Value::Float(x));
        let network = built
            .expand(&params, &Context::default(), &Expansion::new(1.0, None))
            .unwrap();
        let exp = &network.nodes()[0].params;
        let tone = &network.nodes()[1].params;
        let near = |v: Option<&Value>, want: f64| {
            let v = v.and_then(Value::as_f64).unwrap();
            assert!((v - want).abs() < 1e-12, "at {x}: {v} against {want}");
        };
        near(exp.get("exposure"), exposure);
        near(exp.get("black"), black);
        near(tone.get("contrast"), contrast);
        // What the step layer reads for opening and dissolving.
        let driven = built.driven("backlight_lift", x);
        assert_eq!(driven.len(), 3);
        assert_eq!(driven[0].0, "exp");
        assert_eq!(driven[0].1, "exposure");
    }
}

/// A picture with a ramp of greys.
fn picture() -> Image {
    Image::from_fn(Region::sized(16, 12), 4, |x, y, c| {
        if c == 3 {
            1.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let v = (x * 3 + y * 5 + i64::from(c)) as f32 / 300.0;
            v
        }
    })
}

#[test]
fn the_macro_renders_as_the_flat_chain_with_the_same_content_key() {
    let built = build(&backlight()).unwrap();
    let region = Region::sized(16, 12);
    let engine = Engine::new(Arc::new(Bus::new()));
    // The macro, its slider at 0.5.
    let mut g = Graph::new();
    let src = g.add_source("source", picture());
    let mut params = Params::new();
    params.set("backlight_lift", Value::Float(0.5));
    let m = g
        .add_module("backlit", ModuleRef::composite(built), &params)
        .unwrap();
    g.connect(src, "image", m, "input").unwrap();
    let macro_pixels = engine.render(&g, m, "output", region).unwrap().to_vec();
    let macro_key = engine.key(&g, m, region).unwrap();
    // The flat chain with the curves' values at 0.5.
    let mut flat = Graph::new();
    let src = flat.add_source("source", picture());
    let mut e = Params::new();
    e.set("exposure", Value::Float(0.8));
    e.set("black", Value::Float(-0.01));
    let exp = flat
        .add_module("exp", ModuleRef::new(helios_module_exposure::Exposure), &e)
        .unwrap();
    let mut t = Params::new();
    t.set("contrast", Value::Float(1.35));
    let tone = flat
        .add_module("tone", ModuleRef::new(helios_module_sigmoid::Sigmoid), &t)
        .unwrap();
    flat.connect(src, "image", exp, "input").unwrap();
    flat.connect(exp, "output", tone, "input").unwrap();
    let flat_pixels = engine
        .render(&flat, tone, "output", region)
        .unwrap()
        .to_vec();
    assert_eq!(engine.key(&flat, tone, region).unwrap(), macro_key);
    assert_eq!(flat_pixels, macro_pixels);
    // At another value, another key.
    g.set_param(m, "backlight_lift", Value::Float(1.0)).unwrap();
    assert_ne!(engine.key(&g, m, region).unwrap(), macro_key);
}

#[test]
fn refusals_name_the_entry_and_the_reason() {
    for (exposed, want) in [
        // A point out of the inner range (the black level is -1 to 1).
        (
            json!([{ "name": "s", "drives": [
                { "node": "exp", "param": "black", "curve": [[0, 0], [1, 2]] } ] }]),
            "outside the inner range",
        ),
        // x not increasing.
        (
            json!([{ "name": "s", "drives": [
                { "node": "exp", "param": "black", "curve": [[1, 0], [0, 0.1]] } ] }]),
            "strictly increasing",
        ),
        // One point.
        (
            json!([{ "name": "s", "drives": [
                { "node": "exp", "param": "black", "curve": [[0, 0]] } ] }]),
            "at least 2",
        ),
        // The same inner parameter driven twice by one slider.
        (
            json!([{ "name": "s", "drives": [
                { "node": "exp", "param": "black", "curve": [[0, 0], [1, 0.1]] },
                { "node": "exp", "param": "black", "curve": [[0, 0], [1, 0.2]] } ] }]),
            "driven twice",
        ),
        // Two sliders on one inner parameter: a cycle of drives, refused
        // whichever comes first.
        (
            json!([
                { "name": "a", "drives": [
                    { "node": "exp", "param": "black", "curve": [[0, 0], [1, 0.1]] } ] },
                { "name": "b", "drives": [
                    { "node": "exp", "param": "black", "curve": [[0, 0], [1, 0.2]] } ] }
            ]),
            "already set by `a`",
        ),
        (
            json!([
                { "name": "black", "node": "exp", "param": "black" },
                { "name": "b", "drives": [
                    { "node": "exp", "param": "black", "curve": [[0, 0], [1, 0.2]] } ] }
            ]),
            "already set by `black`",
        ),
        (
            json!([
                { "name": "b", "drives": [
                    { "node": "exp", "param": "black", "curve": [[0, 0], [1, 0.2]] } ] },
                { "name": "black", "node": "exp", "param": "black" }
            ]),
            "already set by `b`",
        ),
        // A name already exposed.
        (
            json!([
                { "name": "b", "node": "exp", "param": "exposure" },
                { "name": "b", "drives": [
                    { "node": "exp", "param": "black", "curve": [[0, 0], [1, 0.2]] } ] }
            ]),
            "already exposed",
        ),
        // Both forms at once.
        (
            json!([{ "name": "s", "node": "exp", "param": "black", "drives": [
                { "node": "exp", "param": "black", "curve": [[0, 0], [1, 0.1]] } ] }]),
            "not `node` and `param`",
        ),
        // An unknown node or parameter.
        (
            json!([{ "name": "s", "drives": [
                { "node": "nope", "param": "black", "curve": [[0, 0], [1, 0.1]] } ] }]),
            "no node `nope`",
        ),
        (
            json!([{ "name": "s", "drives": [
                { "node": "exp", "param": "nope", "curve": [[0, 0], [1, 0.1]] } ] }]),
            "unknown parameter",
        ),
        // A default outside its range.
        (
            json!([{ "name": "s", "min": 0.0, "max": 1.0, "default": { "float": 3.0 },
                "drives": [
                { "node": "exp", "param": "black", "curve": [[0, 0], [1, 0.1]] } ] }]),
            "default",
        ),
    ] {
        let e = build(&exposed).map(|_| ()).unwrap_err();
        assert!(e.contains("exposed[") && e.contains(want), "{want}: {e}");
    }
}
