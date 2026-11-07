//! Raws through the built-in templates, on a synthetic mosaic (no raw
//! file needed): every template taking an image develops a raw through
//! the raw front with "Develop one image"'s pixels, and a raw gets
//! darktable's default exposure (+0.7 EV, the camera's bias taken back)
//! where a picture keeps exposure's identity.

use std::sync::Arc;

use helios_core::{Bus, Context, Engine, Graph, Image, NodeId, Region};
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
