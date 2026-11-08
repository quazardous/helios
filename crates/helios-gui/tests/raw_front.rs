//! Raws through the built-in templates, on a synthetic mosaic (no raw
//! file needed): every template taking an image develops a raw through
//! the raw front with "Develop one image"'s pixels, and a raw gets
//! darktable's default exposure (+0.7 EV, the camera's bias taken back)
//! where a picture keeps exposure's identity. A raw shot in portrait is
//! turned upright by the `flip` step, unless the user or a darktable edit
//! says otherwise. A raw's colours are calibrated as darktable's
//! scene-referred default does (the white balance to the camera's D65
//! reference, colour calibration from the camera's illuminant), a
//! darktable edit's colour calibration lands on that node, and an edit made
//! without it keeps its look.

use std::fmt::Write as _;
use std::sync::Arc;

use helios_core::{Bus, Context, Engine, Graph, Image, NodeId, Region, Value};
use helios_gui::templates;
use helios_photo::{CameraInfo, Cfa};
use helios_template::{Bundle, Source};

const SIDE: u32 = 48;

/// A Bayer camera shot at `bias` EV of exposure compensation.
fn camera(bias: f64) -> Context {
    CameraInfo {
        maker: "Test".into(),
        model: "Bayer".into(),
        iso: 100.0,
        exposure_bias: bias,
        exposure_time: 0.01,
        aperture: 4.0,
        focal_length: 50.0,
        black: [0.0; 4],
        white: 1.0,
        white_balance: [2.0, 1.0, 1.5, 1.0],
        xyz_to_camera: [0.9, -0.3, -0.1, -0.4, 1.2, 0.2, -0.05, 0.15, 0.6],
        cfa: Cfa::parse("RGGB").unwrap(),
        orientation: 1,
    }
    .to_context()
}

/// A smooth mosaic, dim enough that nothing clips.
fn mosaic() -> Image {
    Image::from_fn(Region::sized(SIDE, SIDE), 1, |x, y, _| {
        0.05 + 0.2 * (x + y) as f32 / (2 * SIDE) as f32
    })
}

fn built(bundle: &Bundle, mosaic: &Image, camera: &Context) -> (Graph, NodeId) {
    let b = bundle
        .build_with(&templates::modules(), Source::Raw { mosaic, camera }, None)
        .unwrap_or_else(|e| panic!("{}: {e}", bundle.template.id));
    (b.graph, b.view.0)
}

/// The middle of the image, away from the demosaic's border.
fn middle() -> Region {
    Region::new(8, 8, SIDE - 16, SIDE - 16)
}

fn render(graph: &Graph, node: NodeId) -> Vec<f32> {
    Engine::new(Arc::new(Bus::new()))
        .render(graph, node, "output", middle())
        .unwrap()
        .to_vec()
}

fn find(id: &str) -> Bundle {
    templates::find(id).unwrap_or_else(|| panic!("{id} is built in"))
}

#[test]
fn batch_export_and_develop_preview_give_a_raw_develop_s_pixels() {
    let (mosaic, camera) = (mosaic(), camera(0.3));
    let (graph, node) = built(&find("helios.develop"), &mosaic, &camera);
    let develop = render(&graph, node);
    assert!(develop.iter().all(|v| v.is_finite()));
    for id in ["helios.batch-export", "helios.develop-preview"] {
        let (graph, node) = built(&find(id), &mosaic, &camera);
        assert!(graph.find("demosaic").is_some(), "{id}: no raw front");
        let ours = render(&graph, node);
        let worst = ours
            .iter()
            .zip(&develop)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "{id}: differs from Develop's by {worst}");
    }
}

/// What the exposure node `node` does to the raw front's output, as a
/// factor (the mean of its output over the mean of flip's, the raw
/// front's last step).
fn exposure_gain(graph: &Graph, node: &str) -> f64 {
    let mean = |n: &str| {
        let px = render(graph, graph.find(n).unwrap());
        let rgb: Vec<f64> = px
            .chunks(4)
            .flat_map(|p| p[..3].iter().map(|v| f64::from(*v)))
            .collect();
        rgb.iter().sum::<f64>() / rgb.len() as f64
    };
    mean(node) / mean("flip")
}

#[test]
fn a_raw_is_exposed_plus_0_7_ev_its_camera_s_bias_taken_back() {
    let mosaic = mosaic();
    for id in ["helios.develop", "helios.batch-export"] {
        for bias in [0.0, 1.0, -0.7] {
            let (graph, _) = built(&find(id), &mosaic, &camera(bias));
            let ev = exposure_gain(&graph, "exposure").log2();
            // darktable's black (-0.000244) adds a hair on these levels.
            assert!(
                (ev - (0.7 - bias)).abs() < 0.02,
                "{id}, bias {bias}: {ev} EV"
            );
        }
    }
}

#[test]
fn a_picture_keeps_exposure_s_identity() {
    let picture = Image::from_fn(Region::sized(SIDE, SIDE), 4, |_, _, c| {
        if c == 3 { 1.0 } else { 0.25 }
    });
    let b = find("helios.batch-export")
        .build_with(&templates::modules(), Source::picture(&picture), None)
        .unwrap();
    let id = b.graph.find("exposure").unwrap();
    let out = render(&b.graph, id);
    assert!(
        out.iter()
            .all(|v| (v - 0.25).abs() < 1e-6 || (v - 1.0).abs() < 1e-6)
    );
}

/// A landscape mosaic, wider than tall, dim enough that nothing clips.
fn wide() -> Image {
    Image::from_fn(Region::sized(48, 32), 1, |x, y, _| {
        0.05 + 0.2 * x as f32 / 48.0 + 0.1 * y as f32 / 32.0
    })
}

/// The camera, with its EXIF orientation tag.
fn tagged(orientation: i64) -> Context {
    camera(0.0).with(
        helios_photo::camera::fields::ORIENTATION,
        Value::Int(orientation),
    )
}

fn whole(graph: &Graph, node: NodeId) -> Image {
    let extent = graph.extent(node).unwrap().expect("a source feeds it");
    Engine::new(Arc::new(Bus::new()))
        .render(graph, node, "output", extent)
        .unwrap()
}

#[test]
fn a_portrait_raw_is_turned_upright_in_every_template_taking_a_raw() {
    let mosaic = wide();
    for id in [
        "helios.develop",
        "helios.batch-export",
        "helios.develop-preview",
    ] {
        // EXIF 6: the camera was held turned clockwise.
        let (mut graph, view) = built(&find(id), &mosaic, &tagged(6));
        let flip = graph
            .find("flip")
            .unwrap_or_else(|| panic!("{id}: no flip"));
        let calibrated = graph.find("channelmixerrgb").unwrap();
        assert_eq!(
            graph.extent(calibrated).unwrap(),
            Some(Region::sized(48, 32))
        );
        let extent = graph.extent(view).unwrap().unwrap();
        assert_eq!((extent.width, extent.height), (32, 48), "{id}: {extent}");
        // Turned 90 degrees clockwise: the left column comes from the
        // bottom row.
        let (before, after) = (whole(&graph, calibrated), whole(&graph, flip));
        for v in 0..48 {
            for u in 0..32 {
                assert_eq!(
                    after.pixel(extent.x + u, extent.y + v),
                    before.pixel(v, 31 - u),
                    "{id}: ({u}, {v})"
                );
            }
        }
        // The user's orientation beats the camera's.
        graph.set_param(flip, "orientation", Value::Int(0)).unwrap();
        assert_eq!(graph.extent(view).unwrap(), Some(Region::sized(48, 32)));
        assert_eq!(whole(&graph, flip).to_vec(), before.to_vec(), "{id}");
    }
}

/// Where the template's exposure is a masked adjustment (Portrait
/// retouch, the blank workspace), a base exposure under it gives the
/// whole image a raw's default, and the masked exposure still works on
/// top of it.
#[test]
fn portrait_and_the_blank_workspace_expose_a_raw_under_their_masked_adjustment() {
    let mosaic = mosaic();
    let blank = helios_template::workspace::blank();
    for bundle in [find("helios.portrait-retouch"), blank] {
        let id = bundle.template.id.clone();
        for bias in [0.0, 1.0, -0.7] {
            let (graph, view) = built(&bundle, &mosaic, &camera(bias));
            let ev = exposure_gain(&graph, "base").log2();
            assert!(
                (ev - (0.7 - bias)).abs() < 0.02,
                "{id}, bias {bias}: {ev} EV"
            );
            // The masked exposure starts from the base, its own value
            // kept (Portrait's +0.5 EV, the blank's identity).
            let masked = exposure_gain(&graph, "exposure") / exposure_gain(&graph, "base");
            let own = if id == "helios.portrait-retouch" {
                0.5
            } else {
                0.0
            };
            assert!(
                (masked.log2() - own).abs() < 0.02,
                "{id}, bias {bias}: masked {} EV",
                masked.log2()
            );
            assert!(render(&graph, view).iter().all(|v| v.is_finite()), "{id}");
        }
    }
}

#[test]
fn a_landscape_raw_keeps_its_extent() {
    let (graph, view) = built(&find("helios.develop"), &wide(), &tagged(1));
    assert_eq!(graph.extent(view).unwrap(), Some(Region::sized(48, 32)));
}

#[test]
fn a_darktable_edit_s_orientation_carries_over() {
    // darktable's flip at 6 (ORIENTATION_ROTATE_CCW_90_DEG), little-endian.
    let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/"
   darktable:xmp_version="5" darktable:iop_order_version="2" darktable:history_end="1">
   <darktable:history><rdf:Seq>
    <rdf:li darktable:num="0" darktable:operation="flip" darktable:enabled="1" darktable:modversion="2" darktable:params="06000000"/>
   </rdf:Seq></darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
    let (_, imported) = helios_xmp::import_text(xmp, &helios_xmp::Layouts::built_in()).unwrap();
    // The camera says upright; the edit turns it counter-clockwise.
    let (mut graph, view) = built(&find("helios.develop"), &wide(), &tagged(1));
    let skipped = helios_vault::edit::apply_to_graph(&mut graph, &imported.edit()).unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    let flip = graph.find("flip").unwrap();
    assert_eq!(
        graph.params(flip).unwrap().unwrap().get("orientation"),
        Some(&Value::Int(6))
    );
    // Counter-clockwise about the origin: x from 0, y up to 0.
    assert_eq!(
        graph.extent(view).unwrap(),
        Some(Region::new(0, -48, 32, 48))
    );
}

/// A node's whole output written as an 8-bit PNG, as an export writes it,
/// read back.
fn exported(graph: &Graph, node: NodeId) -> image::RgbaImage {
    let png = helios_export::encode(&whole(graph, node), &helios_export::Format::Png { bits: 8 })
        .map_err(|m| m.english)
        .unwrap();
    image::load_from_memory(&png).unwrap().to_rgba8()
}

#[test]
fn a_turned_raw_exports_upright() {
    let (mut graph, view) = built(&find("helios.batch-export"), &wide(), &tagged(6));
    let turned = exported(&graph, view);
    assert_eq!(turned.dimensions(), (32, 48));
    let flip = graph.find("flip").unwrap();
    graph.set_param(flip, "orientation", Value::Int(0)).unwrap();
    let upright = exported(&graph, view);
    assert_eq!(upright.dimensions(), (48, 32));
    assert_eq!(turned, image::imageops::rotate90(&upright));
}

#[test]
fn a_picture_keeps_its_exposure_under_the_masked_adjustment() {
    let picture = Image::from_fn(Region::sized(SIDE, SIDE), 4, |_, _, c| {
        if c == 3 { 1.0 } else { 0.25 }
    });
    for bundle in [
        find("helios.portrait-retouch"),
        helios_template::workspace::blank(),
    ] {
        let b = bundle
            .build_with(&templates::modules(), Source::picture(&picture), None)
            .unwrap();
        let out = render(&b.graph, b.graph.find("base").unwrap());
        assert!(
            out.iter()
                .all(|v| (v - 0.25).abs() < 1e-6 || (v - 1.0).abs() < 1e-6),
            "{}",
            bundle.template.id
        );
    }
}

/// darktable's params of `operation` (its newest layout) from `fields`, as
/// a sidecar's hex.
fn params_hex(operation: &str, fields: &serde_json::Value) -> (u32, String) {
    let layouts = helios_xmp::Layouts::built_in();
    let version = *layouts.versions(operation).iter().max().unwrap();
    let bytes = helios_xmp::encode(layouts.get(operation, version).unwrap(), fields).unwrap();
    let hex = bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    (version, hex)
}

/// A darktable edit of `items` (operation, params), in that order.
fn sidecar(items: &[(&str, serde_json::Value)]) -> String {
    let mut lis = String::new();
    for (i, (op, fields)) in items.iter().enumerate() {
        let (version, hex) = params_hex(op, fields);
        let _ = write!(
            lis,
            r#"<rdf:li darktable:num="{i}" darktable:operation="{op}" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{hex}"/>"#
        );
    }
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/"
   darktable:xmp_version="5" darktable:iop_order_version="4" darktable:history_end="{}">
   <darktable:history><rdf:Seq>{lis}</rdf:Seq></darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#,
        items.len()
    )
}

fn param(graph: &Graph, node: &str, name: &str) -> Option<Value> {
    let id = graph.find(node).unwrap();
    graph.params(id).unwrap().unwrap().get(name).cloned()
}

#[test]
fn a_raw_gets_darktables_colour_calibration_in_every_template_taking_a_raw() {
    let mosaic = mosaic();
    for id in [
        "helios.develop",
        "helios.batch-export",
        "helios.develop-preview",
        "helios.portrait-retouch",
    ] {
        let (graph, view) = built(&find(id), &mosaic, &camera(0.0));
        assert_eq!(
            param(&graph, "whitebalance", "mode"),
            Some(Value::Choice("camera_reference".into())),
            "{id}"
        );
        assert_eq!(
            param(&graph, "channelmixerrgb", "illuminant"),
            Some(Value::Choice("camera".into())),
            "{id}"
        );
        let calibrated = graph.find("channelmixerrgb").unwrap();
        let colorin = graph.find("colorin").unwrap();
        assert_eq!(graph.input_nodes(calibrated).unwrap(), [Some(colorin)]);
        // The as-shot coefficients give an illuminant: the colours move.
        let (before, after) = (render(&graph, colorin), render(&graph, calibrated));
        assert!(after.iter().all(|v| v.is_finite()), "{id}");
        assert!(
            before.iter().zip(&after).any(|(a, b)| (a - b).abs() > 1e-3),
            "{id}: colour calibration changed nothing"
        );
        assert!(render(&graph, view).iter().all(|v| v.is_finite()), "{id}");
    }
}

#[test]
fn a_darktable_edit_s_camera_colour_calibration_applies() {
    // darktable's scene-referred default: the white balance at the
    // camera's D65 coefficients (camera reference), colour calibration
    // CAT16 from the camera's illuminant.
    let identity = |c: usize| {
        let mut row = vec![0.0; 4];
        row[c] = 1.0;
        row
    };
    let xmp = sidecar(&[
        (
            "temperature",
            serde_json::json!({
                "red": 2.2, "green": 1.0, "blue": 1.25, "various": 1.0, "preset": 1
            }),
        ),
        (
            "channelmixerrgb",
            serde_json::json!({
                "red": identity(0), "green": identity(1), "blue": identity(2),
                "illuminant": "DT_ILLUMINANT_CAMERA",
                "illum_fluo": "DT_ILLUMINANT_FLUO_F3",
                "illum_led": "DT_ILLUMINANT_LED_B5",
                "adaptation": "DT_ADAPTATION_CAT16",
                "x": 0.3648, "y": 0.3773, "temperature": 4447.0,
                "gamut": 1.0, "clip": true,
                "version": "CHANNELMIXERRGB_V_3",
            }),
        ),
    ]);
    let (_, imported) = helios_xmp::import_text(&xmp, &helios_xmp::Layouts::built_in()).unwrap();
    assert!(
        imported.report.not_applied.is_empty(),
        "{:#?}",
        imported.report
    );
    for id in ["helios.develop", "helios.batch-export"] {
        let (mut graph, view) = built(&find(id), &mosaic(), &camera(0.0));
        let skipped = helios_vault::edit::apply_to_graph(&mut graph, &imported.edit()).unwrap();
        assert!(skipped.is_empty(), "{id}: {skipped:?}");
        assert_eq!(
            param(&graph, "whitebalance", "mode"),
            Some(Value::Choice("user".into())),
            "{id}"
        );
        assert_eq!(
            param(&graph, "channelmixerrgb", "illuminant"),
            Some(Value::Choice("camera".into())),
            "{id}"
        );
        assert!(render(&graph, view).iter().all(|v| v.is_finite()), "{id}");
    }
}

#[test]
fn an_edit_made_without_colour_calibration_keeps_its_look() {
    // A darktable edit balancing as shot, with no colour calibration
    // (display-referred), and an edit Helios saved before it had one: the
    // white balance as shot.
    let xmp = sidecar(&[(
        "temperature",
        serde_json::json!({
            "red": 2.0, "green": 1.0, "blue": 1.5, "various": 1.0, "preset": 0
        }),
    )]);
    let (_, darktable) = helios_xmp::import_text(&xmp, &helios_xmp::Layouts::built_in()).unwrap();
    let helios = serde_json::json!({ "modules": [
        { "name": "whitebalance", "module": "whitebalance", "params": { "mode": { "choice": "as_shot" } } }
    ] });
    for edit in [darktable.edit(), helios] {
        let (mut graph, _) = built(&find("helios.develop"), &mosaic(), &camera(0.0));
        let skipped = helios_vault::edit::apply_to_graph(&mut graph, &edit).unwrap();
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(
            param(&graph, "channelmixerrgb", "illuminant"),
            Some(Value::Choice("pipe".into())),
            "{edit}"
        );
    }
    // An edit naming its colour calibration keeps it as it is.
    let edit = serde_json::json!({ "modules": [
        { "name": "whitebalance", "module": "whitebalance", "params": { "mode": { "choice": "as_shot" } } },
        { "name": "channelmixerrgb", "module": "channelmixerrgb", "params": { "illuminant": { "choice": "a" } } }
    ] });
    let (mut graph, _) = built(&find("helios.develop"), &mosaic(), &camera(0.0));
    helios_vault::edit::apply_to_graph(&mut graph, &edit).unwrap();
    assert_eq!(
        param(&graph, "channelmixerrgb", "illuminant"),
        Some(Value::Choice("a".into()))
    );
}

/// A darktable edit of exposure `instances` (instance number, name, EV),
/// in that order.
fn exposures(instances: &[(u32, &str, f64)]) -> String {
    let mut lis = String::new();
    for (i, (priority, name, ev)) in instances.iter().enumerate() {
        let (version, hex) = params_hex(
            "exposure",
            &serde_json::json!({ "exposure": ev, "compensate_exposure_bias": false }),
        );
        let _ = write!(
            lis,
            r#"<rdf:li darktable:num="{i}" darktable:operation="exposure" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{hex}" darktable:multi_priority="{priority}" darktable:multi_name="{name}"/>"#
        );
    }
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/"
   darktable:xmp_version="5" darktable:iop_order_version="4" darktable:history_end="{}">
   <darktable:history><rdf:Seq>{lis}</rdf:Seq></darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#,
        instances.len()
    )
}

/// darktable's base exposure is its exposure module's first instance:
/// where the template's `exposure` is a masked adjustment over a `base`
/// (Portrait retouch, the blank workspace), it lands on `base`, the
/// masked exposure kept; in "Develop one image" it is the `exposure`,
/// whatever the instance's name.
#[test]
fn darktable_s_first_exposure_instance_lands_on_the_base_exposure() {
    // The instances (number, name, EV), and those left out.
    type Case<'a> = (&'a [(u32, &'a str, f64)], &'a [&'a str]);
    let ev = |graph: &Graph, node: &str| match param(graph, node, "exposure") {
        Some(Value::Float(v)) => v,
        other => panic!("{node}: {other:?}"),
    };
    let cases: [Case; 3] = [
        (&[(0, "", 1.5)], &[]),
        (&[(0, "", 1.5), (1, "", -0.5)], &["exposure 1"]),
        (&[(1, "", -0.5), (0, "global", 1.5)], &["exposure 1"]),
    ];
    for (instances, extra) in cases {
        let (_, imported) =
            helios_xmp::import_text(&exposures(instances), &helios_xmp::Layouts::built_in())
                .unwrap();
        assert!(
            imported.report.not_applied.is_empty(),
            "{:#?}",
            imported.report
        );
        let skipped_extra: Vec<String> = extra
            .iter()
            .map(|n| format!("`{n}`: no such node"))
            .collect();
        for bundle in [
            find("helios.portrait-retouch"),
            helios_template::workspace::blank(),
        ] {
            let id = bundle.template.id.clone();
            let (mut graph, _) = built(&bundle, &mosaic(), &camera(0.0));
            let masked = ev(&graph, "exposure");
            let skipped = helios_vault::edit::apply_to_graph(&mut graph, &imported.edit()).unwrap();
            assert_eq!(skipped, skipped_extra, "{id}: {instances:?}");
            assert!(
                (ev(&graph, "base") - 1.5).abs() < 1e-9,
                "{id}: {instances:?}"
            );
            assert_eq!(
                param(&graph, "base", "compensate_bias"),
                Some(Value::Bool(false)),
                "{id}"
            );
            assert_eq!(ev(&graph, "exposure"), masked, "{id}: {instances:?}");
        }
        // Without a base, the first instance is the template's exposure.
        let (mut graph, _) = built(&find("helios.develop"), &mosaic(), &camera(0.0));
        let skipped = helios_vault::edit::apply_to_graph(&mut graph, &imported.edit()).unwrap();
        assert_eq!(skipped, skipped_extra, "{instances:?}");
        assert!((ev(&graph, "exposure") - 1.5).abs() < 1e-9, "{instances:?}");
    }
    // An edit Helios saved names its nodes itself: its `exposure` is the
    // masked one.
    let helios = serde_json::json!({ "modules": [
        { "name": "exposure", "module": "exposure", "params": { "exposure": { "float": 2.0 } } }
    ] });
    let (mut graph, _) = built(&find("helios.portrait-retouch"), &mosaic(), &camera(0.0));
    let base = ev(&graph, "base");
    helios_vault::edit::apply_to_graph(&mut graph, &helios).unwrap();
    assert!((ev(&graph, "exposure") - 2.0).abs() < 1e-9);
    assert_eq!(ev(&graph, "base"), base);
}
