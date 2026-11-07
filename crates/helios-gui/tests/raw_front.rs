//! Raws through the built-in templates, on a synthetic mosaic (no raw
//! file needed): every template taking an image develops a raw through
//! the raw front with "Develop one image"'s pixels, and a raw gets
//! darktable's default exposure (+0.7 EV, the camera's bias taken back)
//! where a picture keeps exposure's identity. A raw shot in portrait is
//! turned upright by the `flip` step, unless the user or a darktable edit
//! says otherwise.

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

/// What the exposure node does to its input, as a factor (the mean of
/// its output over the mean of colorin's).
fn exposure_gain(graph: &Graph) -> f64 {
    let mean = |n: &str| {
        let px = render(graph, graph.find(n).unwrap());
        let rgb: Vec<f64> = px
            .chunks(4)
            .flat_map(|p| p[..3].iter().map(|v| f64::from(*v)))
            .collect();
        rgb.iter().sum::<f64>() / rgb.len() as f64
    };
    mean("exposure") / mean("colorin")
}

#[test]
fn a_raw_is_exposed_plus_0_7_ev_its_camera_s_bias_taken_back() {
    let mosaic = mosaic();
    for id in ["helios.develop", "helios.batch-export"] {
        for bias in [0.0, 1.0, -0.7] {
            let (graph, _) = built(&find(id), &mosaic, &camera(bias));
            let ev = exposure_gain(&graph).log2();
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
        .build_with(&templates::modules(), Source::Picture(&picture), None)
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
        let colorin = graph.find("colorin").unwrap();
        assert_eq!(graph.extent(colorin).unwrap(), Some(Region::sized(48, 32)));
        let extent = graph.extent(view).unwrap().unwrap();
        assert_eq!((extent.width, extent.height), (32, 48), "{id}: {extent}");
        // Turned 90 degrees clockwise: the left column comes from the
        // bottom row.
        let (before, after) = (whole(&graph, colorin), whole(&graph, flip));
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
