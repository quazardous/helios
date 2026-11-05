//! Workshops applied on a workspace as a pure step: the workshop's macro
//! inserted on the wires leaving a node, its aimed values the image's;
//! the network after the step renders as the flat chain the macro
//! packages, with the same content key.

use std::sync::Arc;

use egui_pop_model::layout::Layout;
use helios_core::{Bus, Engine, Image, ModuleRef, Params, Region, Value};
use helios_template::macros::Values;
use helios_template::workshop;
use helios_template::workspace::{self, State};
use helios_template::{Bundle, Catalogue, CompositeRef, Source, parse_composite, parse_template};
use serde_json::{Value as Json, json};

fn lift() -> CompositeRef {
    CompositeRef {
        id: "helios.lift-midtones".into(),
        version: 1,
    }
}

fn catalogue() -> Catalogue {
    use helios_module_masks as m;
    let mut c = Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(helios_module_sigmoid::Sigmoid))
        .module(ModuleRef::new(m::MaskCombine))
        .module(ModuleRef::new(m::MaskFeather))
        .module(ModuleRef::new(m::MaskBlend));
    for f in helios_template::shipped::composites() {
        c.add_composite(f);
    }
    c
}

fn state_of(network: &Json) -> State {
    let template = parse_template(
        &json!({
            "format": 1,
            "id": "test.workshops",
            "version": 1,
            "meta": { "name": "Workshops", "scenario": "A chain to apply a workshop on" },
            "network": network,
            "subworkspaces": [
                { "id": "look", "name": "Look", "nodes": ["exp", "tone"] }
            ],
            "positions": { "exp": [10.0, 20.0], "tone": [200.0, 20.0] }
        })
        .to_string(),
    )
    .unwrap();
    State {
        meta: workspace::Meta {
            name: "workshops".into(),
            ..workspace::Meta::default()
        },
        bundle: Bundle::new(template, Layout::default()),
    }
}

/// `source → exp → tone`, the view on `tone`, and `side` reading `exp`.
fn state() -> State {
    state_of(&json!({
        "sources": ["source"],
        "nodes": [
            { "name": "exp", "module": "exposure" },
            { "name": "tone", "module": "sigmoid" },
            { "name": "side", "module": "exposure" }
        ],
        "connections": [
            { "from": "source", "port": "image", "to": "exp", "input": "input" },
            { "from": "exp", "port": "output", "to": "tone", "input": "input" },
            { "from": "exp", "port": "output", "to": "side", "input": "input" }
        ],
        "view": { "node": "tone", "port": "output" }
    }))
}

fn picture() -> Image {
    Image::from_fn(Region::sized(16, 12), 4, |x, y, c| {
        if c == 3 {
            1.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let v = (x * 3 + y * 5 + i64::from(c)) as f32 / 400.0;
            v
        }
    })
}

/// The view's pixels and content key, the image's values set.
fn render(state: &State, values: &Values) -> (Vec<f32>, helios_core::ContentHash) {
    let image = picture();
    let mut built = state
        .bundle
        .build_with(&catalogue(), Source::Picture(&image), None)
        .unwrap();
    for (path, v) in values {
        let (node, param) = path
            .strip_prefix("node/")
            .unwrap()
            .rsplit_once('/')
            .unwrap();
        built
            .graph
            .set_param(built.nodes[node], param, v.clone())
            .unwrap();
    }
    let engine = Engine::new(Arc::new(Bus::new()));
    let region = Region::sized(16, 12);
    let (view, port) = built.view.clone();
    let pixels = engine
        .render(&built.graph, view, &port, region)
        .unwrap()
        .to_vec();
    (pixels, engine.key(&built.graph, view, region).unwrap())
}

fn connections(state: &State) -> Vec<String> {
    let mut v: Vec<String> = state
        .bundle
        .template
        .network
        .connections
        .iter()
        .map(|c| format!("{}.{}>{}.{}", c.from, c.port, c.to, c.input))
        .collect();
    v.sort();
    v
}

#[test]
fn applied_after_a_node_every_reader_reads_the_workshop() {
    let now = state();
    let values = Params::new().with("brightness", Value::Float(1.5));
    let step = now
        .workshop_applied(&catalogue(), &lift(), Some("exp"), &values)
        .unwrap();
    let node = step.answer["node"].as_str().unwrap();
    assert_eq!(node, "lift_midtones");
    assert_eq!(
        connections(&step.state),
        [
            "exp.output>lift_midtones.input",
            "lift_midtones.output>side.input",
            "lift_midtones.output>tone.input",
            "source.image>exp.input",
        ]
    );
    assert_eq!(step.answer["rewired"], json!(["tone.input", "side.input"]));
    assert_eq!(step.answer["view"], json!(false));
    let t = &step.state.bundle.template;
    assert_eq!(t.network.view.node, "tone");
    // In `exp`'s sub-workspace, right after it, and placed beside it.
    assert_eq!(t.subworkspaces[0].nodes, ["exp", "lift_midtones", "tone"]);
    assert_eq!(t.positions["lift_midtones"], [100.0, 110.0]);
    // The aimed value is the image's, not the network's.
    let n = t.network.nodes.iter().find(|n| n.name == node).unwrap();
    assert_eq!(n.composite, Some(lift()));
    assert!(n.params.iter().next().is_none());
    let path = "node/lift_midtones/brightness";
    assert_eq!(step.values[path], Some(Value::Float(1.5)));
    // A shipped workshop is not copied into the workspace.
    assert!(step.state.bundle.composites.is_empty());
}

#[test]
fn applied_where_the_image_is_shown_the_view_follows_and_renders_as_the_flat_chain() {
    let now = state();
    let values = Params::new().with("brightness", Value::Float(1.5));
    let step = now
        .workshop_applied(&catalogue(), &lift(), None, &values)
        .unwrap();
    assert_eq!(
        step.answer["after"],
        json!({ "node": "tone", "port": "output" })
    );
    assert_eq!(step.answer["view"], json!(true));
    assert_eq!(step.answer["rewired"], json!([]));
    let view = &step.state.bundle.template.network.view;
    assert_eq!(
        (view.node.as_str(), view.port.as_str()),
        ("lift_midtones", "output")
    );
    let image: Values = step
        .values
        .iter()
        .map(|(p, v)| (p.clone(), v.clone().unwrap()))
        .collect();
    let applied = render(&step.state, &image);
    // The chain the macro packages, written out.
    let flat = state_of(&json!({
        "sources": ["source"],
        "nodes": [
            { "name": "exp", "module": "exposure" },
            { "name": "tone", "module": "sigmoid" },
            { "name": "side", "module": "exposure" },
            { "name": "inner_exp", "module": "exposure",
              "params": { "exposure": { "float": 1.5 } } },
            { "name": "inner_tone", "module": "sigmoid" }
        ],
        "connections": [
            { "from": "source", "port": "image", "to": "exp", "input": "input" },
            { "from": "exp", "port": "output", "to": "tone", "input": "input" },
            { "from": "exp", "port": "output", "to": "side", "input": "input" },
            { "from": "tone", "port": "output", "to": "inner_exp", "input": "input" },
            { "from": "inner_exp", "port": "output", "to": "inner_tone", "input": "input" }
        ],
        "view": { "node": "inner_tone", "port": "output" }
    }));
    let flat = render(&flat, &Values::new());
    assert_eq!(applied, flat);
}

#[test]
fn applying_is_refused_for_a_plain_macro_two_inputs_or_an_unknown_node() {
    let now = state();
    let none = Params::new();
    let e = now
        .workshop_applied(&catalogue(), &lift(), Some("nothing"), &none)
        .unwrap_err();
    assert!(e.contains("no node `nothing`"), "{e}");
    let unknown = CompositeRef {
        id: "helios.nothing".into(),
        version: 1,
    };
    let e = now
        .workshop_applied(&catalogue(), &unknown, None, &none)
        .unwrap_err();
    assert!(e.contains("unknown composite"), "{e}");
    let masked = CompositeRef {
        id: "helios.masked-adjustment".into(),
        version: 1,
    };
    let e = now
        .workshop_applied(&catalogue(), &masked, None, &none)
        .unwrap_err();
    assert!(e.contains("without a workshop block"), "{e}");
    // The masked adjustment given a block: two inputs.
    let mut file = serde_json::to_value(
        helios_template::shipped::composites()
            .into_iter()
            .find(|f| f.id == masked.id)
            .unwrap(),
    )
    .unwrap();
    file["id"] = json!("test.masked-workshop");
    file["workshop"] = json!({ "intent": "A masked look" });
    let mut c = catalogue();
    c.add_composite(parse_composite(&file.to_string()).unwrap());
    let two = CompositeRef {
        id: "test.masked-workshop".into(),
        version: 1,
    };
    let e = now.workshop_applied(&c, &two, None, &none).unwrap_err();
    assert!(
        e.contains("2 inputs") && e.contains("from the graph"),
        "{e}"
    );
}

#[test]
fn aimed_through_the_engine_then_applied_the_median_lands_on_the_target() {
    let now = state();
    let c = catalogue();
    let (file, at) = now.workshop_insertion(&c, &lift(), None).unwrap();
    assert_eq!(at.node, "tone");
    let built = c.build_composite(&lift(), 0).unwrap();
    let engine = Engine::new(Arc::new(Bus::new()));
    // What enters the workshop: the view's render.
    let image = picture();
    let b = now
        .bundle
        .build_with(&c, Source::Picture(&image), None)
        .unwrap();
    let input = engine
        .render(&b.graph, b.nodes["tone"], "output", Region::sized(16, 12))
        .unwrap();
    let targets = &file.workshop.unwrap().targets;
    let (answers, values) =
        workshop::aim_rendered(&engine, &built, &input, &Params::new(), targets).unwrap();
    assert!(answers[0].reached, "{answers:?}");
    let step = now.workshop_applied(&c, &lift(), None, &values).unwrap();
    let image: Values = step
        .values
        .iter()
        .map(|(p, v)| (p.clone(), v.clone().unwrap()))
        .collect();
    let (pixels, _) = render(&step.state, &image);
    let out = Image::new(Region::sized(16, 12), 4, pixels);
    let median = workshop::measure(&out, helios_template::format::Measure::LumaMedian).unwrap();
    assert!((median + 2.5).abs() < 0.1, "{median}");
}

/// A one-node macro of `exposure` named `name`, a workshop when `w` is
/// given.
fn tiny(id: &str, name: &str, w: Option<Json>) -> helios_template::CompositeFile {
    let mut file = json!({
        "format": 1,
        "id": id,
        "version": 1,
        "name": name,
        "inputs": [ { "name": "input", "channels": 4, "layout": "rgba" } ],
        "outputs": [ { "name": "output", "channels": 4, "layout": "rgba" } ],
        "nodes": [ { "name": "exposure", "module": "exposure" } ],
        "connections": [ { "from": "@", "port": "input", "to": "exposure", "input": "input" } ],
        "exits": [ { "from": "exposure", "port": "output", "output": "output" } ],
        "exposed": [ { "name": "brightness", "node": "exposure", "param": "exposure" } ]
    });
    if let Some(w) = w {
        file["workshop"] = w;
    }
    parse_composite(&file.to_string()).unwrap()
}

#[test]
fn suggest_ranks_the_name_then_the_keywords_then_the_intent() {
    let files = [
        tiny(
            "user.a",
            "Warm skin",
            Some(json!({ "intent": "Faces glowing in shade", "keywords": ["portrait"] })),
        ),
        tiny(
            "user.b",
            "Night city",
            Some(json!({ "intent": "Neon without noise", "keywords": ["glow", "street"] })),
        ),
        tiny(
            "user.c",
            "Glow",
            Some(json!({ "intent": "A soft halo", "keywords": ["dreamy"] })),
        ),
        // A plain macro is never suggested.
        tiny("user.d", "Glowing", None),
    ];
    let found = workshop::suggest(&files, "the glow");
    let ids: Vec<&str> = found.iter().map(|s| s.workshop.id.as_str()).collect();
    assert_eq!(ids, ["user.c", "user.b", "user.a"]);
    let matched: Vec<workshop::Matched> = found.iter().map(|s| s.matched).collect();
    assert_eq!(
        matched,
        [
            workshop::Matched::Name,
            workshop::Matched::Keyword,
            workshop::Matched::Intent
        ]
    );
    // More words matched rank first within a kind; a word's start finds it.
    let found = workshop::suggest(&files, "neon stre");
    assert_eq!(found[0].workshop.id, "user.b");
    assert_eq!(found[0].words, 2);
    assert!(workshop::suggest(&files, "landscape").is_empty());
    // Nothing asked: every workshop, by name.
    let all: Vec<String> = workshop::suggest(&files, " ")
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(all, ["Glow", "Night city", "Warm skin"]);
    // The newest version of an id only.
    let mut newer = files[2].clone();
    newer.version = 2;
    let found = workshop::suggest(files.iter().chain([&newer]), "glow");
    assert_eq!(found[0].workshop.version, 2);
    assert_eq!(
        found.iter().filter(|s| s.workshop.id == "user.c").count(),
        1
    );
}
