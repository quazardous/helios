//! The template format: read, checked against a catalogue, built headless
//! (folded and opened composites), written back, and its schemas in step
//! with its types.

use std::sync::Arc;

use egui_pop_model::ids::PanelId;
use egui_pop_model::layout::{Layout, Node};
use egui_pop_model::panels::{Area, PanelSpec};
use helios_core::{Bus, Engine, ModuleRef, Region};
use helios_template::{Bundle, Catalogue, CompositeFile, parse_composite, parse_template, schema};
use serde_json::{Value as Json, json};

/// Two exposures in a row, one exposed parameter setting both.
fn twice(version: u32) -> CompositeFile {
    parse_composite(
        &json!({
            "format": 1,
            "id": "test.twice",
            "version": version,
            "name": "Twice",
            "inputs": [{ "name": "input", "channels": 4 }],
            "outputs": [{ "name": "output", "channels": 4 }],
            "nodes": [
                { "name": "a", "module": "exposure" },
                { "name": "b", "module": "exposure" }
            ],
            "connections": [
                { "from": "@", "port": "input", "to": "a", "input": "input" },
                { "from": "a", "port": "output", "to": "b", "input": "input" }
            ],
            "exits": [{ "from": "b", "port": "output", "output": "output" }],
            "exposed": [
                { "name": "ev", "node": "a", "param": "exposure" },
                { "name": "ev", "node": "b", "param": "exposure" }
            ],
            "panel": { "panel": "test.params", "area": "end_side" }
        })
        .to_string(),
    )
    .unwrap()
}

fn panels() -> Vec<PanelSpec> {
    let id = |s: &str| PanelId::new(s).unwrap();
    vec![
        PanelSpec::new(id("test.image"), "Image", Area::Center).only_in(&[Area::Center]),
        PanelSpec::new(id("test.params"), "Parameters", Area::EndSide),
        PanelSpec::new(id("test.log"), "Log", Area::Bottom),
    ]
}

fn catalogue() -> Catalogue {
    Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .composite(twice(1))
        .panels(panels())
        .commands(["test.draw".to_string(), "test.erase".to_string()])
}

fn template() -> Json {
    json!({
        "format": 1,
        "id": "test.brighter",
        "version": 1,
        "meta": { "name": "Brighter", "scenario": "Make it brighter, twice" },
        "network": {
            "sources": ["source"],
            "nodes": [
                { "name": "folded", "composite": { "id": "test.twice", "version": 1 },
                  "params": { "ev": { "float": 0.5 } } },
                { "name": "opened", "composite": { "id": "test.twice", "version": 1 },
                  "params": { "ev": { "float": 0.5 } }, "open": true }
            ],
            "connections": [
                { "from": "source", "port": "image", "to": "folded", "input": "input" },
                { "from": "source", "port": "image", "to": "opened", "input": "input" }
            ],
            "view": { "node": "opened", "port": "output" },
            "presets": [
                { "id": "more", "name": "More",
                  "values": { "opened": { "ev": { "float": 1.0 } } } }
            ]
        },
        "tools": {
            "toolbox": ["test.draw", "test.erase"],
            "default": "test.draw",
            "keymap": [{ "keys": "D", "command": "test.draw" }]
        }
    })
}

fn layout() -> Layout {
    Layout {
        areas: vec![(Area::Center, Node::tab(PanelId::new("test.image").unwrap()))],
        hidden: vec![PanelId::new("test.log").unwrap()],
        ..Layout::default()
    }
}

fn bundle(t: &Json) -> Bundle {
    Bundle::new(parse_template(&t.to_string()).unwrap(), layout())
}

#[test]
fn a_template_is_checked_and_built_headless() {
    let b = bundle(&template());
    let report = b.validate(&catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    assert!(report.updates.is_empty());
    // The composite's hint docks the panel the layout did not show.
    assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
    let built = b.build(&catalogue()).unwrap();
    assert_eq!(
        built.layout.area_of(&PanelId::new("test.params").unwrap()),
        Some(Area::EndSide)
    );
    // Folded: one node; opened: its inner nodes, by path.
    let names: Vec<&str> = built.nodes.keys().map(String::as_str).collect();
    assert_eq!(names, ["folded", "opened.a", "opened.b", "source"]);
    assert_eq!(built.view.0, built.nodes["opened.b"]);
    // The exposed value reached both inner nodes of the opened one.
    for inner in ["opened.a", "opened.b"] {
        let params = built.graph.params(built.nodes[inner]).unwrap().unwrap();
        assert_eq!(params.float("exposure"), 0.5);
    }
    // Folded and opened render the same pixels.
    let engine = Engine::new(Arc::new(Bus::new()));
    let region = Region::sized(8, 8);
    let folded = engine
        .render(&built.graph, built.nodes["folded"], "output", region)
        .unwrap();
    let opened = engine
        .render(&built.graph, built.view.0, &built.view.1, region)
        .unwrap();
    assert_eq!(folded.to_vec(), opened.to_vec());
    // A preset's values go over the nodes' own.
    let more = b
        .build_with(
            &catalogue(),
            helios_template::Source::picture(&helios_template::placeholder()),
            Some("more"),
        )
        .unwrap();
    let params = more.graph.params(more.nodes["opened.a"]).unwrap().unwrap();
    assert_eq!(params.float("exposure"), 1.0);
}

#[test]
fn unknown_modules_composites_panels_and_commands_are_refused() {
    let mut t = template();
    t["network"]["nodes"][0] = json!({ "name": "folded", "module": "mystery" });
    t["network"]["nodes"][1]["composite"]["version"] = json!(7);
    t["tools"]["toolbox"][1] = json!("test.nothing");
    t["tools"]["default"] = json!("test.elsewhere");
    let mut b = bundle(&t);
    b.layout
        .areas
        .push((Area::Bottom, Node::tab(PanelId::new("test.image").unwrap())));
    b.layout.hidden.push(PanelId::new("test.ghost").unwrap());
    let report = b.validate(&catalogue());
    let has = |s: &str| report.problems.iter().any(|p| p.contains(s));
    assert!(has("unknown module `mystery`"), "{:?}", report.problems);
    assert!(
        has("unknown composite `test.twice@7`"),
        "{:?}",
        report.problems
    );
    assert!(
        has("unknown command `test.nothing`"),
        "{:?}",
        report.problems
    );
    assert!(
        has("`test.elsewhere` is not in the toolbox"),
        "{:?}",
        report.problems
    );
    assert!(has("unknown panel `test.ghost`"), "{:?}", report.problems);
    assert!(has("Image stays out of"), "{:?}", report.problems);
    assert!(b.build(&catalogue()).is_err());
}

#[test]
fn a_newer_composite_is_offered_never_taken_silently() {
    let catalogue = catalogue().composite(twice(2));
    let mut b = bundle(&template());
    let report = b.validate(&catalogue);
    assert!(report.is_ok(), "{:?}", report.problems);
    assert_eq!(report.updates.len(), 2);
    assert_eq!(report.updates[0].available, 2);
    // Built as pinned.
    let built = b.build(&catalogue).unwrap();
    let folded = built
        .graph
        .manifest(built.nodes["folded"])
        .unwrap()
        .unwrap();
    assert_eq!(folded.version, 1);
    // Taken when asked.
    let moved = b.upgrade_composites(&catalogue);
    assert_eq!(moved.len(), 2);
    let built = b.build(&catalogue).unwrap();
    let folded = built
        .graph
        .manifest(built.nodes["folded"])
        .unwrap()
        .unwrap();
    assert_eq!(folded.version, 2);
    assert!(b.validate(&catalogue).updates.is_empty());
}

#[test]
fn a_newer_format_and_unknown_fields_are_refused() {
    let mut t = template();
    t["format"] = json!(2);
    let e = parse_template(&t.to_string()).unwrap_err();
    assert!(e.contains("newer than this Helios reads"), "{e}");
    let mut t = template();
    t["network"]["nodes"][0]["colour"] = json!("red");
    let e = parse_template(&t.to_string()).unwrap_err();
    assert!(e.contains("colour"), "{e}");
    // The schema refuses it as well.
    let problems = schema::check(&schema::template(), &t);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("unknown field `colour`")),
        "{problems:?}"
    );
}

#[test]
fn the_schemas_describe_what_the_types_write() {
    let b = bundle(&template());
    let written = serde_json::to_value(&b.template).unwrap();
    assert_eq!(
        schema::check(&schema::template(), &written),
        Vec::<String>::new()
    );
    let written = serde_json::to_value(twice(1)).unwrap();
    assert_eq!(
        schema::check(&schema::composite(), &written),
        Vec::<String>::new()
    );
    // A node naming both a module and a composite fits neither option.
    let mut t = template();
    t["network"]["nodes"][0]["module"] = json!("exposure");
    assert!(!schema::check(&schema::template(), &t).is_empty());
}

#[test]
fn a_bundle_is_written_and_read_back_with_its_composites_and_texts() {
    let dir = std::env::temp_dir().join(format!("helios-template-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut b = bundle(&template());
    b.texts.push(helios_template::Text {
        locale: "fr".into(),
        name: "template.ftl".into(),
        source: "test-brighter-name = Plus clair\n".into(),
    });
    b.bundle_composites(&catalogue()).unwrap();
    assert_eq!(b.composites, [twice(1)]);
    b.save(&dir).unwrap();
    assert!(
        dir.join("composites/test.twice@1.helios-composite.json")
            .is_file()
    );
    let read = Bundle::load(&dir).unwrap();
    assert_eq!(read, b);
    // Its bundled composite builds it without the catalogue's.
    let bare = Catalogue::new().module(ModuleRef::new(helios_module_exposure::Exposure));
    assert!(read.build(&bare).is_ok());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_template_writes_only_its_own_text() {
    let mut b = bundle(&template());
    b.texts.push(helios_template::Text {
        locale: "fr".into(),
        name: "template.ftl".into(),
        source: "test-brighter-name = Plus clair\ngui-open-workspace-verb = Pirate\n".into(),
    });
    let report = b.validate(&catalogue());
    assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
    assert!(report.problems[0].contains("gui-open-workspace-verb"));
}

#[test]
fn a_node_placed_bypassed_renders_as_its_input() {
    let mut t = template();
    t["network"]["nodes"][0]["bypassed"] = json!(true);
    let b = bundle(&t);
    let built = b.build(&catalogue()).unwrap();
    let folded = built.nodes["folded"];
    assert!(built.graph.is_bypassed(folded).unwrap());
    let engine = Engine::new(Arc::new(Bus::new()));
    let region = Region::sized(8, 8);
    let out = engine
        .render(&built.graph, folded, "output", region)
        .unwrap();
    let source = engine
        .render(&built.graph, built.nodes["source"], "image", region)
        .unwrap();
    assert_eq!(out.to_vec(), source.to_vec());
    // Written back as placed, and the schema knows the field.
    let written = serde_json::to_value(&b.template).unwrap();
    assert_eq!(written["network"]["nodes"][0]["bypassed"], true);
    assert!(written["network"]["nodes"][1].get("bypassed").is_none());
    assert_eq!(
        schema::check(&schema::template(), &written),
        Vec::<String>::new()
    );
    // An opened composite is not bypassed whole.
    t["network"]["nodes"][1]["bypassed"] = json!(true);
    match bundle(&t).build(&catalogue()) {
        Err(e) => assert!(e.contains("opened composite"), "{e}"),
        Ok(_) => panic!("an opened composite was bypassed whole"),
    }
}

/// A raw source: a mosaic straight into rawprepare, whose levels come
/// from the camera the source is given.
fn raw_template() -> Json {
    json!({
        "format": 1,
        "id": "test.raw",
        "version": 1,
        "meta": { "name": "Raw", "scenario": "A raw's levels." },
        "network": {
            "sources": ["raw"],
            "raw_sources": ["raw"],
            "nodes": [
                { "name": "prep", "module": "rawprepare" },
                { "name": "exp", "module": "exposure" }
            ],
            "connections": [
                { "from": "raw", "port": "image", "to": "prep", "input": "input" }
            ],
            "view": { "node": "prep", "port": "output" }
        }
    })
}

fn raw_catalogue() -> Catalogue {
    catalogue().module(ModuleRef::new(helios_module_rawprepare::RawPrepare))
}

#[test]
fn a_raw_source_takes_a_mosaic_and_gives_its_camera_to_what_reads_it() {
    use helios_core::{Context, Image, Value};
    let t = raw_template();
    assert_eq!(schema::check(&schema::template(), &t), Vec::<String>::new());
    let b = bundle(&t);
    assert_eq!(
        b.template.network.source_kind("raw"),
        helios_template::SourceKind::Raw
    );
    assert!(b.template.network.reads_raw());
    // Checked and built headless on a one-channel placeholder.
    let report = b.validate(&raw_catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    // Given a mosaic and its camera: rawprepare reads the levels.
    let mosaic = Image::from_fn(Region::sized(4, 4), 1, |_, _, _| 0.6);
    let camera = Context::new()
        .with("photo.black", Value::List(vec![0.1; 4]))
        .with("photo.white", Value::Float(1.1));
    let built = b
        .build_with(
            &raw_catalogue(),
            helios_template::Source::Raw {
                mosaic: &mosaic,
                camera: &camera,
            },
            None,
        )
        .unwrap();
    let out = Engine::new(Arc::new(Bus::new()))
        .render(
            &built.graph,
            built.view.0,
            &built.view.1,
            Region::sized(4, 4),
        )
        .unwrap();
    assert!(
        out.to_vec().iter().all(|v| (v - 0.5).abs() < 1e-6),
        "{:?}",
        out.to_vec()
    );
    // A raw source refuses a picture, a picture source a raw.
    let e = b
        .build_with(
            &raw_catalogue(),
            helios_template::Source::picture(&helios_template::placeholder()),
            None,
        )
        .err()
        .unwrap();
    assert!(e.contains("takes a raw file's sensor data"), "{e}");
    let picture = bundle(&template());
    let e = picture
        .build_with(
            &catalogue(),
            helios_template::Source::Raw {
                mosaic: &mosaic,
                camera: &camera,
            },
            None,
        )
        .err()
        .unwrap();
    assert!(e.contains("takes a picture"), "{e}");
}

#[test]
fn a_raw_source_is_one_of_the_sources() {
    let mut t = raw_template();
    t["network"]["raw_sources"] = json!(["prep", "raw", "raw"]);
    let problems = bundle(&t).validate(&raw_catalogue()).problems;
    for expected in [
        "network.raw_sources[0]: `prep` is not a source",
        "network.raw_sources[2]: `raw` is not a source, or named twice",
    ] {
        assert!(
            problems.iter().any(|p| p.starts_with(expected)),
            "{expected}: {problems:?}"
        );
    }
}

/// An image source: a picture straight into exposure, a raw through the
/// raw front first; the network has a `demosaic` node of its own.
fn image_template() -> Json {
    json!({
        "format": 1,
        "id": "test.image",
        "version": 1,
        "meta": { "name": "Image", "scenario": "A picture or a raw, brighter." },
        "network": {
            "sources": ["source"],
            "image_sources": ["source"],
            "nodes": [
                { "name": "exp", "module": "exposure" },
                { "name": "demosaic", "module": "exposure" }
            ],
            "connections": [
                { "from": "source", "port": "image", "to": "exp", "input": "input" },
                { "from": "exp", "port": "output", "to": "demosaic", "input": "input" }
            ],
            "view": { "node": "demosaic", "port": "output" }
        }
    })
}

/// The raw modules and the raw front Helios ships.
fn image_catalogue() -> Catalogue {
    let mut c = raw_catalogue()
        .module(ModuleRef::new(helios_module_whitebalance::WhiteBalance))
        .module(ModuleRef::new(helios_module_highlights::Highlights))
        .module(ModuleRef::new(helios_module_demosaic::Demosaic))
        .module(ModuleRef::new(helios_module_colorin::ColorIn));
    for file in helios_template::shipped::composites() {
        c.add_composite(file);
    }
    c
}

#[test]
fn a_raw_through_the_raw_front_gets_darktables_colour_calibration() {
    use helios_core::{Image, Value};
    let catalogue = image_catalogue().module(ModuleRef::new(
        helios_module_channelmixerrgb::ChannelMixerRgb,
    ));
    let b = bundle(&image_template());
    // A Nikon D750 (its XYZ to camera matrix), shot under a warm light.
    let white_balance = [1.32, 1.0, 2.45, 1.0];
    let camera = helios_photo::CameraInfo {
        maker: "Nikon".into(),
        model: "D750".into(),
        iso: 100.0,
        exposure_bias: 0.0,
        exposure_time: 0.01,
        aperture: 4.0,
        focal_length: 50.0,
        black: [0.0; 4],
        white: 1.0,
        white_balance,
        xyz_to_camera: [
            0.9020, -0.2890, -0.0715, -0.4535, 1.2436, 0.2348, -0.0934, 0.1919, 0.7086,
        ],
        cfa: helios_photo::Cfa::parse("RGGB").unwrap(),
        orientation: 1,
    }
    .to_context();
    // A grey card under that light: what the as-shot coefficients make
    // neutral.
    let mosaic = Image::from_fn(Region::sized(16, 16), 1, |x, y, _| {
        let c = match (x % 2, y % 2) {
            (0, 0) => 0,
            (1, 1) => 2,
            _ => 1,
        };
        0.2 / white_balance[c] as f32
    });
    let built = b
        .build_with(
            &catalogue,
            helios_template::Source::Raw {
                mosaic: &mosaic,
                camera: &camera,
            },
            None,
        )
        .unwrap();
    // The white balance to the camera's D65 reference, then colour
    // calibration from the camera's illuminant, after colorin.
    let wb = built.graph.find("whitebalance").unwrap();
    assert_eq!(
        built.graph.params(wb).unwrap().unwrap().get("mode"),
        Some(&Value::Choice("camera_reference".into()))
    );
    let cat = built.graph.find("channelmixerrgb").unwrap();
    assert_eq!(
        built.graph.params(cat).unwrap().unwrap().get("illuminant"),
        Some(&Value::Choice("camera".into()))
    );
    let colorin = built.graph.find("colorin").unwrap();
    assert_eq!(built.graph.input_nodes(cat).unwrap(), [Some(colorin)]);
    let exp = built.graph.find("exp").unwrap();
    assert_eq!(built.graph.input_nodes(exp).unwrap(), [Some(cat)]);
    assert!(
        built
            .graph
            .context(cat)
            .unwrap()
            .is_some_and(|c| !c.is_empty())
    );
    let render = |node| {
        Engine::new(Arc::new(Bus::new()))
            .render(&built.graph, node, "output", Region::sized(16, 16))
            .unwrap()
    };
    // The card is neutral again once adapted (CAT16 against the
    // matrix's Bradford, so within a few percent), not before.
    let spread = |img: &Image| {
        let p = img.pixel(8, 8);
        let max = p[..3].iter().copied().fold(f32::MIN, f32::max);
        let min = p[..3].iter().copied().fold(f32::MAX, f32::min);
        assert!(min > 0.0 && max.is_finite(), "{p:?}");
        max / min
    };
    let before = spread(&render(colorin));
    let after = spread(&render(cat));
    assert!(after < 1.08, "after colour calibration: {after}");
    assert!(before > 1.5, "before colour calibration: {before}");
}

#[test]
fn the_raw_front_has_a_bypassed_chart_profile_that_changes_colours_once_on() {
    use helios_core::{Image, Value};
    use helios_module_chartprofile::{ChartProfile, flat, standard};
    let without = image_catalogue().module(ModuleRef::new(
        helios_module_channelmixerrgb::ChannelMixerRgb,
    ));
    let with = without.clone().module(ModuleRef::new(ChartProfile));
    let b = bundle(&image_template());
    let camera = helios_photo::CameraInfo {
        maker: "Nikon".into(),
        model: "D750".into(),
        iso: 100.0,
        exposure_bias: 0.0,
        exposure_time: 0.01,
        aperture: 4.0,
        focal_length: 50.0,
        black: [0.0; 4],
        white: 1.0,
        white_balance: [2.0, 1.0, 1.5, 1.0],
        xyz_to_camera: [
            0.9020, -0.2890, -0.0715, -0.4535, 1.2436, 0.2348, -0.0934, 0.1919, 0.7086,
        ],
        cfa: helios_photo::Cfa::parse("RGGB").unwrap(),
        orientation: 1,
    }
    .to_context();
    let mosaic = Image::from_fn(Region::sized(16, 16), 1, |x, y, _| {
        0.05 + 0.01 * (x as f32) + 0.02 * ((x + y) % 3) as f32
    });
    let build = |catalogue: &Catalogue| {
        b.build_with(
            catalogue,
            helios_template::Source::Raw {
                mosaic: &mosaic,
                camera: &camera,
            },
            None,
        )
        .unwrap()
    };
    let render = |graph: &helios_core::Graph, node| {
        Engine::new(Arc::new(Bus::new()))
            .render(graph, node, "output", Region::sized(16, 16))
            .unwrap()
    };
    let plain = build(&without);
    assert!(plain.graph.find("chartprofile").is_none());
    let mut built = build(&with);
    // After colorin, before colour calibration, bypassed.
    let profile = built.graph.find("chartprofile").unwrap();
    let colorin = built.graph.find("colorin").unwrap();
    let cat = built.graph.find("channelmixerrgb").unwrap();
    assert_eq!(built.graph.input_nodes(profile).unwrap(), [Some(colorin)]);
    assert_eq!(built.graph.input_nodes(cat).unwrap(), [Some(profile)]);
    assert!(built.graph.is_bypassed(profile).unwrap());
    // Bypassed, the render is the one without it, bit for bit.
    let at = |g: &helios_core::Graph| g.find("channelmixerrgb").unwrap();
    let base = render(&plain.graph, at(&plain.graph));
    assert_eq!(render(&built.graph, cat).data(), base.data());
    // On, with a profile's matrix: colorin's output moves as M_c M_s^-1.
    let m_s = standard(built.graph.context(profile).unwrap().unwrap()).unwrap();
    let bend = [
        [1.06, -0.04, -0.02],
        [-0.03, 1.01, 0.02],
        [0.01, -0.05, 1.04],
    ];
    let m_c = helios_photo::color::mul(&bend, &m_s);
    built
        .graph
        .set_param(profile, "matrix", Value::List(flat(&m_c)))
        .unwrap();
    built.graph.set_bypass(profile, false).unwrap();
    let input = render(&built.graph, colorin);
    let output = render(&built.graph, profile);
    for (i, o) in input
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(output.data().as_chunks::<4>().0)
    {
        let want = helios_photo::color::apply(&bend, [0, 1, 2].map(|c| f64::from(i[c])));
        for c in 0..3 {
            assert!(
                (f64::from(o[c]) - want[c]).abs() <= 1e-5 * want[c].abs().max(1.0),
                "{o:?} against {want:?}"
            );
        }
        assert_eq!(o[3], i[3]);
    }
    assert_ne!(render(&built.graph, cat).data(), base.data());
}

#[test]
fn an_image_source_takes_a_picture_as_it_is_and_a_raw_through_the_raw_front() {
    use helios_core::Image;
    let t = image_template();
    assert_eq!(schema::check(&schema::template(), &t), Vec::<String>::new());
    let b = bundle(&t);
    let net = &b.template.network;
    assert_eq!(
        net.source_kind("source"),
        helios_template::SourceKind::Image
    );
    assert!(net.takes_raw() && !net.reads_raw());
    let report = b.validate(&image_catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    // A picture goes straight on: no raw node.
    let picture = Image::from_fn(Region::sized(4, 4), 4, |_, _, _| 0.25);
    let built = b
        .build_with(
            &image_catalogue(),
            helios_template::Source::picture(&picture),
            None,
        )
        .unwrap();
    assert!(built.graph.find("rawprepare").is_none());
    assert_eq!(
        built.nodes.keys().collect::<Vec<_>>(),
        ["demosaic", "exp", "source"]
    );
    // A raw: the mosaic, then the raw front under the names a darktable
    // edit's items take, `demosaic` renamed where the network has one.
    let camera = helios_photo::CameraInfo {
        maker: "Test".into(),
        model: "Bayer".into(),
        iso: 100.0,
        exposure_bias: 0.0,
        exposure_time: 0.01,
        aperture: 4.0,
        focal_length: 50.0,
        black: [0.0; 4],
        white: 1.0,
        white_balance: [2.0, 1.0, 1.5, 1.0],
        xyz_to_camera: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        cfa: helios_photo::Cfa::parse("RGGB").unwrap(),
        orientation: 1,
    }
    .to_context();
    let mosaic = Image::from_fn(Region::sized(8, 8), 1, |_, _, _| 0.3);
    let built = b
        .build_with(
            &image_catalogue(),
            helios_template::Source::Raw {
                mosaic: &mosaic,
                camera: &camera,
            },
            None,
        )
        .unwrap();
    for name in [
        "rawprepare",
        "whitebalance",
        "highlights",
        "source.demosaic",
        "colorin",
    ] {
        let id = built.graph.find(name).unwrap_or_else(|| panic!("{name}"));
        // Each reads the raw: it is given the camera.
        assert!(
            built
                .graph
                .context(id)
                .unwrap()
                .is_some_and(|c| !c.is_empty()),
            "{name}"
        );
    }
    let exp = built.graph.find("exp").unwrap();
    let colorin = built.graph.find("colorin").unwrap();
    assert_eq!(built.graph.input_nodes(exp).unwrap(), [Some(colorin)]);
    let out = Engine::new(Arc::new(Bus::new()))
        .render(
            &built.graph,
            built.view.0,
            &built.view.1,
            Region::sized(8, 8),
        )
        .unwrap();
    assert_eq!(out.channels(), 4);
    assert!(out.to_vec().iter().all(|v| v.is_finite()));
    // Not a source, a raw source too, or named twice: refused.
    let mut t = image_template();
    t["network"]["raw_sources"] = json!(["source"]);
    t["network"]["image_sources"] = json!(["source", "exp"]);
    let problems = bundle(&t).validate(&image_catalogue()).problems;
    for expected in [
        "network.image_sources[0]: `source` is not a source, or named twice",
        "network.image_sources[1]: `exp` is not a source",
    ] {
        assert!(
            problems.iter().any(|p| p.starts_with(expected)),
            "{expected}: {problems:?}"
        );
    }
}

#[test]
fn raw_params_go_over_params_when_the_network_is_built_with_a_raw() {
    use helios_core::{Image, Value};
    let mut t = image_template();
    t["network"]["nodes"][0]["params"] = json!({ "black": { "float": 0.01 } });
    t["network"]["nodes"][0]["raw_params"] = json!({ "exposure": { "float": 0.7 } });
    assert_eq!(schema::check(&schema::template(), &t), Vec::<String>::new());
    let b = bundle(&t);
    let param = |built: &helios_template::Built, k: &str| {
        let id = built.graph.find("exp").unwrap();
        built.graph.params(id).unwrap().unwrap().get(k).cloned()
    };
    // A picture: the node's own parameters.
    let picture = Image::from_fn(Region::sized(4, 4), 4, |_, _, _| 0.25);
    let built = b
        .build_with(
            &image_catalogue(),
            helios_template::Source::picture(&picture),
            None,
        )
        .unwrap();
    assert_eq!(param(&built, "exposure"), Some(Value::Float(0.0)));
    assert_eq!(param(&built, "black"), Some(Value::Float(0.01)));
    // A raw: the raw's defaults over them.
    let mosaic = Image::from_fn(Region::sized(8, 8), 1, |_, _, _| 0.3);
    let camera = helios_core::Context::new();
    let built = b
        .build_with(
            &image_catalogue(),
            helios_template::Source::Raw {
                mosaic: &mosaic,
                camera: &camera,
            },
            None,
        )
        .unwrap();
    assert_eq!(param(&built, "exposure"), Some(Value::Float(0.7)));
    assert_eq!(param(&built, "black"), Some(Value::Float(0.01)));
    // Checked against the manifest, whatever the build is given.
    t["network"]["nodes"][0]["raw_params"] = json!({ "loudness": { "float": 0.7 } });
    let e = bundle(&t).build(&image_catalogue()).err().unwrap();
    assert!(e.contains("raw_params"), "{e}");
}

/// A one-exposure macro with a workshop block (experimental).
fn workshop(targets: Json) -> Json {
    json!({
        "format": 1,
        "id": "test.lift",
        "version": 1,
        "name": "Lift",
        "inputs": [{ "name": "input", "channels": 4 }],
        "outputs": [{ "name": "output", "channels": 4 }],
        "nodes": [{ "name": "a", "module": "exposure" }],
        "connections": [{ "from": "@", "port": "input", "to": "a", "input": "input" }],
        "exits": [{ "from": "a", "port": "output", "output": "output" }],
        "exposed": [
            { "name": "brightness", "node": "a", "param": "exposure", "unit": "EV",
              "min": -3.0, "max": 3.0 },
            { "name": "black", "node": "a", "param": "black" },
            { "name": "bias", "node": "a", "param": "compensate_bias" }
        ],
        "workshop": {
            "intent": "Brighter midtones",
            "keywords": ["brighten", "midtones"],
            "targets": targets
        }
    })
}

/// Built against a catalogue with the exposure module.
fn built(file: &Json) -> Result<(), String> {
    let file = parse_composite(&file.to_string())?;
    let catalogue = Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .composite(file.clone());
    catalogue.build_composite(&file.reference(), 0).map(|_| ())
}

#[test]
fn a_workshop_block_round_trips_and_fits_the_schema() {
    let j = workshop(json!([
        { "param": "brightness", "measure": "luma.median", "value": -2.5 },
        { "param": "black", "measure": "clip.high", "value": 1.0, "at": "input" }
    ]));
    assert_eq!(
        schema::check(&schema::composite(), &j),
        Vec::<String>::new()
    );
    let file = parse_composite(&j.to_string()).unwrap();
    let w = file.workshop.as_ref().unwrap();
    assert_eq!(w.targets[0].measure, helios_template::Measure::LumaMedian);
    assert_eq!(w.targets[1].at, helios_template::format::At::Input);
    // Written as read: `at: output` is the default, left out.
    let written = serde_json::to_value(&file).unwrap();
    assert_eq!(written["workshop"], j["workshop"]);
    assert_eq!(parse_composite(&written.to_string()).unwrap(), file);
    assert_eq!(
        schema::check(&schema::composite(), &written),
        Vec::<String>::new()
    );
    let mut back = j.clone();
    back["workshop"]["targets"][0]["at"] = json!("output");
    assert_eq!(parse_composite(&back.to_string()).unwrap(), file);
    built(&j).unwrap();
    // Without the block, the file is the macro it was.
    let mut plain = j.clone();
    plain.as_object_mut().unwrap().remove("workshop");
    assert!(
        parse_composite(&plain.to_string())
            .unwrap()
            .workshop
            .is_none()
    );
    // The catalogue's description, which agents read, carries it.
    let catalogue = Catalogue::new().composite(file);
    assert_eq!(
        catalogue.describe()["composites"][0]["workshop"]["intent"],
        "Brighter midtones"
    );
}

#[test]
fn the_schema_and_the_types_refuse_the_same_blocks() {
    let bad = [
        json!([{ "param": "brightness", "measure": "luma.mean", "value": 0.0 }]),
        json!([{ "param": "brightness", "measure": "luma.median" }]),
        json!([{ "param": "brightness", "measure": "luma.median", "value": 0.0, "at": "mask" }]),
        json!([{ "param": "brightness", "measure": "luma.median", "value": 0.0, "zone": 3 }]),
    ];
    for targets in bad {
        let j = workshop(targets.clone());
        assert!(
            !schema::check(&schema::composite(), &j).is_empty(),
            "the schema takes {targets}"
        );
        assert!(
            parse_composite(&j.to_string()).is_err(),
            "serde takes {targets}"
        );
    }
    let mut no_intent = workshop(json!([]));
    no_intent["workshop"]
        .as_object_mut()
        .unwrap()
        .remove("intent");
    assert!(!schema::check(&schema::composite(), &no_intent).is_empty());
    assert!(parse_composite(&no_intent.to_string()).is_err());
}

#[test]
fn a_workshop_target_is_on_one_exposed_number() {
    let refused = |targets: Json, why: &str| {
        let e = built(&workshop(targets.clone())).unwrap_err();
        assert!(e.contains(why), "{targets}: {e}");
        assert!(e.contains("workshop: targets["), "{e}");
    };
    refused(
        json!([{ "param": "exposure", "measure": "luma.median", "value": 0.0 }]),
        "not an exposed parameter",
    );
    refused(
        json!([{ "param": "bias", "measure": "luma.median", "value": 0.0 }]),
        "for a number parameter",
    );
    refused(
        json!([
            { "param": "brightness", "measure": "luma.median", "value": 0.0 },
            { "param": "brightness", "measure": "luma.high", "value": 1.0 }
        ]),
        "a second target",
    );
    refused(
        json!([{ "param": "brightness", "measure": "clip.high", "value": 120.0 }]),
        "between 0 and 100",
    );
}

#[test]
fn the_raw_front_has_a_bypassed_camera_look_colour_half_after_colour_calibration() {
    use helios_core::Image;
    use helios_module_cameracolour::CameraColour;
    use helios_module_chartprofile::ChartProfile;
    let without = image_catalogue()
        .module(ModuleRef::new(
            helios_module_channelmixerrgb::ChannelMixerRgb,
        ))
        .module(ModuleRef::new(ChartProfile));
    let with = without.clone().module(ModuleRef::new(CameraColour));
    let b = bundle(&image_template());
    let camera = helios_photo::CameraInfo {
        maker: "Nikon".into(),
        model: "D750".into(),
        iso: 100.0,
        exposure_bias: 0.0,
        exposure_time: 0.01,
        aperture: 4.0,
        focal_length: 50.0,
        black: [0.0; 4],
        white: 1.0,
        white_balance: [2.0, 1.0, 1.5, 1.0],
        xyz_to_camera: [
            0.9020, -0.2890, -0.0715, -0.4535, 1.2436, 0.2348, -0.0934, 0.1919, 0.7086,
        ],
        cfa: helios_photo::Cfa::parse("RGGB").unwrap(),
        orientation: 1,
    }
    .to_context();
    let mosaic = Image::from_fn(Region::sized(16, 16), 1, |x, y, _| {
        0.05 + 0.01 * (x as f32) + 0.02 * ((x + y) % 3) as f32
    });
    let build = |catalogue: &Catalogue| {
        b.build_with(
            catalogue,
            helios_template::Source::Raw {
                mosaic: &mosaic,
                camera: &camera,
            },
            None,
        )
        .unwrap()
    };
    let render = |graph: &helios_core::Graph, node: &str| {
        let at = graph.find(node).unwrap();
        Engine::new(Arc::new(Bus::new()))
            .render(graph, at, "output", Region::sized(16, 16))
            .unwrap()
    };
    let plain = build(&without);
    assert!(plain.graph.find("cameracolour").is_none());
    let built = build(&with);
    let g = &built.graph;
    let (profile, calibration, look) = (
        g.find("chartprofile").unwrap(),
        g.find("channelmixerrgb").unwrap(),
        g.find("cameracolour").unwrap(),
    );
    // The chart profile after the camera colours, colour calibration,
    // then the look's colour half: it is fitted on what calibration
    // gives.
    assert_eq!(g.input_nodes(calibration).unwrap(), [Some(profile)]);
    assert_eq!(g.input_nodes(look).unwrap(), [Some(calibration)]);
    assert_eq!(g.input_nodes(g.find("exp").unwrap()).unwrap(), [Some(look)]);
    assert!(g.is_bypassed(look).unwrap());
    // Bypassed, the render is the one without it, bit for bit.
    assert_eq!(
        render(g, "cameracolour").data(),
        render(&plain.graph, "channelmixerrgb").data()
    );
}

#[test]
fn a_raw_shown_through_sigmoid_gets_a_bypassed_camera_look_tone_half_before_it() {
    use helios_core::Image;
    use helios_module_cameratone::CameraTone;
    // A template with a display transform and no `cameratone`: built
    // with a raw, the tone half goes on sigmoid's input, so an edit with
    // a look (sigmoid off, cameratone on) never renders scene-linear.
    let t = json!({
        "format": 1,
        "id": "test.shown",
        "version": 1,
        "meta": { "name": "Shown", "scenario": "A raw, shown." },
        "network": {
            "sources": ["source"],
            "image_sources": ["source"],
            "nodes": [
                { "name": "exp", "module": "exposure" },
                { "name": "tone", "module": "sigmoid" }
            ],
            "connections": [
                { "from": "source", "port": "image", "to": "exp", "input": "input" },
                { "from": "exp", "port": "output", "to": "tone", "input": "input" }
            ],
            "view": { "node": "tone", "port": "output" }
        }
    });
    let b = bundle(&t);
    let without = image_catalogue().module(ModuleRef::new(helios_module_sigmoid::Sigmoid));
    let with = without.clone().module(ModuleRef::new(CameraTone));
    let camera = helios_photo::CameraInfo {
        maker: "Nikon".into(),
        model: "D750".into(),
        iso: 100.0,
        exposure_bias: 0.0,
        exposure_time: 0.01,
        aperture: 4.0,
        focal_length: 50.0,
        black: [0.0; 4],
        white: 1.0,
        white_balance: [2.0, 1.0, 1.5, 1.0],
        xyz_to_camera: [
            0.9020, -0.2890, -0.0715, -0.4535, 1.2436, 0.2348, -0.0934, 0.1919, 0.7086,
        ],
        cfa: helios_photo::Cfa::parse("RGGB").unwrap(),
        orientation: 1,
    }
    .to_context();
    let mosaic = Image::from_fn(Region::sized(16, 16), 1, |x, y, _| {
        0.05 + 0.01 * (x as f32) + 0.02 * ((x + y) % 3) as f32
    });
    let raw = |catalogue: &Catalogue| {
        b.build_with(
            catalogue,
            helios_template::Source::Raw {
                mosaic: &mosaic,
                camera: &camera,
            },
            None,
        )
        .unwrap()
    };
    let render = |graph: &helios_core::Graph| {
        let at = graph.find("tone").unwrap();
        Engine::new(Arc::new(Bus::new()))
            .render(graph, at, "output", Region::sized(16, 16))
            .unwrap()
    };
    let plain = raw(&without);
    assert!(plain.graph.find("cameratone").is_none());
    let built = raw(&with);
    let g = &built.graph;
    let look = g.find("cameratone").unwrap();
    assert_eq!(g.input_nodes(look).unwrap(), [g.find("exp")]);
    assert_eq!(
        g.input_nodes(g.find("tone").unwrap()).unwrap(),
        [Some(look)]
    );
    assert!(g.is_bypassed(look).unwrap());
    assert!(built.nodes.contains_key("cameratone"));
    // Bypassed, the render is the one without it, bit for bit.
    assert_eq!(render(g).data(), render(&plain.graph).data());
    // Built without a raw (no look can be fitted), the network is as
    // written.
    assert!(b.build(&with).unwrap().graph.find("cameratone").is_none());
}
