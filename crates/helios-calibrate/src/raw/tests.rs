//! Tests of the raw measurement graph on a synthetic mosaic: the chart
//! drawn in white-balanced camera RGB, turned into a Bayer mosaic in
//! counts by the inverse of the raw front (`helios_testkit::mosaic`), and
//! measured back through the graph the calibration builds.

use super::*;
use crate::run::tests::{CAMERA, CHART, CORNERS, H, W, adaptation_from, correction, draw, lit};
use helios_module_channelmixerrgb::blackbody_xy;
use helios_module_whitebalance::d65_coefficients;
use helios_photo::Cfa;
use helios_photo::color::{apply, inverse, mul};

/// A camera with [`CAMERA`]'s matrix, a 14-bit sensor with a black level
/// that differs on one photosite of the block, as some sensors record.
fn camera() -> CameraInfo {
    CameraInfo {
        maker: "Synthetic".into(),
        model: "Chart".into(),
        iso: 100.0,
        exposure_bias: 0.0,
        exposure_time: 1.0 / 60.0,
        aperture: 8.0,
        focal_length: 50.0,
        black: [512.0, 510.0, 514.0, 512.0],
        white: 16383.0,
        white_balance: [2.1, 1.0, 1.5, 1.0],
        xyz_to_camera: CAMERA,
        cfa: Cfa::parse("RGGB").unwrap(),
        orientation: 1,
    }
}

/// A raw shot under 3200 K: the camera's true matrix `M_true` off from
/// `colorin`'s standard one by a correction keeping the neutral, each
/// patch's white-balanced camera RGB `c_k = M_true⁻¹ t_k` at half
/// exposure.
struct Scene {
    truth: Mat3,
    camera_rgb: Vec<[f64; 3]>,
    xy: [f32; 2],
}

fn scene() -> Scene {
    let xy = blackbody_xy(3200.0);
    let targets = lit(&adaptation_from(xy));
    let standard = camera_to_working(&CAMERA).unwrap();
    let truth = mul(&correction(&targets), &standard);
    let truth_inv = inverse(&truth).unwrap();
    let camera_rgb = targets
        .iter()
        .map(|t| apply(&truth_inv, *t).map(|c| c * 0.5))
        .collect();
    Scene {
        truth,
        camera_rgb,
        xy,
    }
}

/// The scene's chart drawn at [`CORNERS`] and mosaicked as the camera
/// records it.
fn mosaic_of(values: &[[f64; 3]]) -> Image {
    let wb = d65_coefficients(&CAMERA).unwrap();
    helios_testkit::mosaic(
        &camera(),
        wb,
        Region::sized(W as u32, H as u32),
        &draw(values),
    )
}

#[test]
fn the_region_holds_the_chart_on_whole_bayer_blocks() {
    let extent = Region::sized(W as u32, H as u32);
    let r = measured_region(CHART, CORNERS, extent).unwrap();
    assert_eq!((r.x % 2, r.y % 2, r.width % 2, r.height % 2), (0, 0, 0, 0));
    assert!(extent.contains(&r));
    // Smaller than the image, larger than the handles.
    assert!(r.area() < extent.area());
    for [x, y] in CORNERS {
        assert!(x > r.x as f64 + 30.0 && x < r.right() as f64 - 30.0);
        assert!(y > r.y as f64 + 30.0 && y < r.bottom() as f64 - 30.0);
    }
    // Moved off the image, nothing is left.
    let away = CORNERS.map(|[x, y]| [x + 2000.0, y]);
    assert_eq!(measured_region(CHART, away, extent), None);
}

#[test]
fn the_graph_gives_each_patch_its_camera_rgb_back() {
    let scene = scene();
    let mosaic = mosaic_of(&scene.camera_rgb);
    let region = measured_region(CHART, CORNERS, mosaic.region()).unwrap();
    let measured = measure_raw(mosaic, &camera(), region).unwrap();
    assert_eq!(measured.image.region(), region);
    assert_eq!(measured.clip.channels(), 1);
    assert_eq!(measured.standard, camera_to_working(&CAMERA).unwrap());
    let shifted = CORNERS.map(|[x, y]| [x - region.x as f64, y - region.y as f64]);
    let layout = Layout::new(CHART, shifted).unwrap();
    let mut worst = 0.0f64;
    for (k, truth) in scene.camera_rgb.iter().enumerate() {
        let stats = layout.measure(&view(&measured.image), k).unwrap();
        for c in 0..3 {
            let off = (stats.mean[c] - truth[c]).abs() / truth[c].abs().max(0.01);
            worst = worst.max(off);
        }
    }
    eprintln!("worst relative error of a patch's mean: {worst:e}");
    assert!(worst < 1e-4, "{worst}");
}

#[test]
fn a_raw_shot_is_solved_through_the_request() {
    let scene = scene();
    let answer = calibrate_raw(
        &ChartArgs::new(CORNERS),
        mosaic_of(&scene.camera_rgb),
        &camera(),
    )
    .unwrap();
    assert!(answer.dry_run);
    // The corners in the raw's own coordinates, in the chart's order.
    assert_eq!(answer.corners, CORNERS);
    assert_eq!(
        (
            answer.orientation.quarter_turns,
            answer.orientation.mirrored
        ),
        (0, false)
    );
    for c in 0..2 {
        let off = (answer.illuminant.xy[c] - f64::from(scene.xy[c])).abs();
        assert!(
            off < 1e-4,
            "{:?} against {:?}",
            answer.illuminant.xy,
            scene.xy
        );
    }
    let mut worst = 0.0f64;
    for i in 0..3 {
        for j in 0..3 {
            worst = worst.max((answer.camera_matrix[i][j] - scene.truth[i][j]).abs());
        }
    }
    eprintln!(
        "camera matrix within {worst:e}; Delta E before {:.3}, after {:.4} (max {:.4})",
        answer.before.mean, answer.after.mean, answer.after.max
    );
    assert!(worst < 1e-3, "{worst}");
    assert!(answer.after.max < 0.01, "{}", answer.after.max);
    assert!(answer.before.mean > 10.0 * answer.after.mean);
    assert!(answer.patches.iter().all(|p| p.status == "kept"));
}

#[test]
fn a_patch_clipped_on_the_sensor_is_left_out() {
    let scene = scene();
    let mut values = scene.camera_rgb.clone();
    // The white patch over-exposed: its photosites at the sensor's white.
    let white = CHART.neutrals[0];
    values[white] = values[white].map(|v| v * 4.0);
    let answer = calibrate_raw(&ChartArgs::new(CORNERS), mosaic_of(&values), &camera()).unwrap();
    let name = CHART.patches[white].name;
    for p in &answer.patches {
        let expected = if p.name == name { "clipped" } else { "kept" };
        assert_eq!(p.status, expected, "{}", p.name);
    }
    // The other patches still give the matrix.
    let worst = (0..9)
        .map(|n| (answer.camera_matrix[n / 3][n % 3] - scene.truth[n / 3][n % 3]).abs())
        .fold(0.0, f64::max);
    assert!(worst < 1e-3, "{worst}");
}

#[test]
fn a_chart_placed_off_the_image_is_refused_with_its_reason() {
    let scene = scene();
    let away = CORNERS.map(|[x, y]| [x + 2000.0, y + 2000.0]);
    let error = calibrate_raw(
        &ChartArgs::new(away),
        mosaic_of(&scene.camera_rgb),
        &camera(),
    )
    .unwrap_err();
    assert_eq!(error, ChartError::OutsideImage);
    assert_eq!(
        error.to_string(),
        "the chart's patches fall outside the image"
    );
}

#[test]
fn a_camera_without_a_matrix_is_refused() {
    let scene = scene();
    let mut no_matrix = camera();
    no_matrix.xyz_to_camera = [0.0; 9];
    let error = calibrate_raw(
        &ChartArgs::new(CORNERS),
        mosaic_of(&scene.camera_rgb),
        &no_matrix,
    )
    .unwrap_err();
    assert_eq!(error, ChartError::NoCameraMatrix);
}

#[test]
fn the_same_raw_gives_the_same_answer() {
    let scene = scene();
    let run = || {
        let answer = calibrate_raw(
            &ChartArgs::new(CORNERS),
            mosaic_of(&scene.camera_rgb),
            &camera(),
        )
        .unwrap();
        serde_json::to_string(&answer).unwrap()
    };
    assert_eq!(run(), run());
}
