//! Tests of the one-call calibration. The light is estimated and adapted
//! in colour calibration's f32: a shot solved exactly lands within a few
//! thousandths of Delta E, not 1e-6 as the solver alone.

use super::*;
use crate::chart::XRITE_CC24_2014;
use crate::sampler::{PICTURE_CLIP_LEVEL, Warning};
use crate::solver::{normalise, orthogonal_basis};
use helios_module_channelmixerrgb::{blackbody_xy, daylight_xy, rec2020_to_xyz_d50};
use helios_photo::color::{IDENTITY, camera_to_working, inverse, mul};

const CHART: &Chart = &XRITE_CC24_2014;
const W: usize = 640;
const H: usize = 440;
/// The chart upright, slightly in perspective.
const CORNERS: [Point; 4] = [[95.0, 88.0], [548.0, 70.0], [560.0, 352.0], [80.0, 340.0]];

/// XYZ to camera RGB of a real body (dcraw's table, Canon EOS 5D Mark II).
const CAMERA: [f64; 9] = [
    0.4716, 0.0603, -0.0830, -0.7798, 1.5474, 0.2480, -0.1496, 0.1937, 0.6651,
];

/// Colour calibration's adaptation from `xy` to D50, in f64.
fn adaptation_from(xy: [f32; 2]) -> Mat3 {
    to_f64(
        &Plan::new(xy, &rec2020_to_xyz_d50(), 0.0, false)
            .unwrap()
            .adaptation(),
    )
}

/// Every patch's reference seen under the light `a` adapts from: `t_k =
/// A⁻¹ W⁻¹ xyz(L_k)`, in the working space.
fn lit(a: &Mat3) -> Vec<[f64; 3]> {
    let back = mul(&inverse(a).unwrap(), &inverse(&working_to_xyz()).unwrap());
    CHART
        .patches
        .iter()
        .map(|p| apply(&back, lab_to_xyz(p.lab)))
        .collect()
}

/// A correction keeping the neutrals' direction of `targets`.
fn correction(targets: &[[f64; 3]]) -> Mat3 {
    let sum = CHART.neutrals.iter().fold([0.0; 3], |acc, &k| {
        [0, 1, 2].map(|c| acc[c] + targets[k][c])
    });
    let basis = orthogonal_basis(normalise(sum).unwrap());
    let u = [[0.08, -0.05], [-0.03, 0.11], [0.06, 0.04]];
    std::array::from_fn(|i| {
        std::array::from_fn(|c| IDENTITY[i][c] + u[i][0] * basis[0][c] + u[i][1] * basis[1][c])
    })
}

/// The chart drawn upright at [`CORNERS`], each patch's square filled with
/// its value, the rest a dark grey; three channels.
fn draw(values: &[[f64; 3]]) -> Vec<f32> {
    let placed = Layout::new(CHART, CORNERS).unwrap();
    let half = 0.5 * CHART.patch_side;
    let mut rgb = vec![0.01_f32; W * H * 3];
    for y in 0..H {
        for x in 0..W {
            let [u, v] = placed.to_chart.apply([x as f64 + 0.5, y as f64 + 0.5]);
            let (i, j) = (u.round(), v.round());
            if i < 0.0 || j < 0.0 || i >= CHART.columns as f64 || j >= CHART.rows as f64 {
                continue;
            }
            if (u - i).abs() > half || (v - j).abs() > half {
                continue;
            }
            let k = CHART.index(i as usize, j as usize);
            for c in 0..3 {
                rgb[(y * W + x) * 3 + c] = values[k][c] as f32;
            }
        }
    }
    rgb
}

fn view(data: &[f32]) -> ImageView<'_> {
    ImageView {
        data,
        width: W,
        height: H,
        channels: 3,
    }
}

/// A raw shot under 3200 K: the camera's true matrix `M_true`, the
/// standard one off from it (keeping the neutral), each patch's camera RGB
/// `c_k = M_true⁻¹ t_k` at half exposure.
struct RawShot {
    truth: Mat3,
    standard: Mat3,
    camera: Vec<[f64; 3]>,
    xy: [f32; 2],
}

fn raw_shot() -> RawShot {
    let xy = blackbody_xy(3200.0);
    let targets = lit(&adaptation_from(xy));
    let truth = camera_to_working(&CAMERA).unwrap();
    let standard = mul(&inverse(&correction(&targets)).unwrap(), &truth);
    let truth_inv = inverse(&truth).unwrap();
    let camera = targets
        .iter()
        .map(|t| apply(&truth_inv, *t).map(|c| c * 0.5))
        .collect();
    RawShot {
        truth,
        standard,
        camera,
        xy,
    }
}

fn assert_xy(found: [f64; 2], truth: [f32; 2]) {
    for c in 0..2 {
        assert!(
            (found[c] - f64::from(truth[c])).abs() < 1e-4,
            "{found:?} against {truth:?}"
        );
    }
}

#[test]
fn a_raw_shot_gives_its_light_matrix_and_orientation_back() {
    let shot = raw_shot();
    let rgb = draw(&shot.camera);
    let measured = Shot {
        image: view(&rgb),
        clip: None,
        standard: shot.standard,
    };
    for given in [
        Orientation::ALL[0],
        Orientation::ALL[2],
        Orientation::ALL[5],
    ] {
        let request = Request::new(CHART, given.handles(CORNERS), &[]);
        let report = calibrate(&measured, &request).unwrap();
        assert_eq!(report.orientation, given);
        assert_eq!(report.layout.corners, CORNERS);
        assert_xy(report.illuminant.xy, shot.xy);
        // Below 4000 K the daylight locus is the black body's: darktable
        // words it `d`.
        assert_eq!(report.illuminant.kind, "d");
        assert!(
            (report.illuminant.cct - 3200.0).abs() < 25.0,
            "{}",
            report.illuminant.cct
        );
        let solution = &report.solution;
        assert_eq!(solution.stage, Stage::DeltaE2000);
        assert!(solution.after.max < 5e-3, "{:?}", solution.after);
        assert!(solution.before.mean > 1.0, "{:?}", solution.before);
        assert!(
            (solution.exposure_ev - 1.0).abs() < 1e-3,
            "{}",
            solution.exposure_ev
        );
        for r in 0..3 {
            for c in 0..3 {
                let (found, truth) = (report.camera_matrix[r][c], shot.truth[r][c]);
                assert!(
                    (found - truth).abs() < 1e-4,
                    "M_c[{r}][{c}] {found} against {truth}"
                );
            }
        }
        // The shot's warm cast is the light's, which the adaptation takes
        // out: no other warning.
        let warnings: Vec<Warning> = report
            .check
            .warnings()
            .into_iter()
            .filter(|w| !matches!(w, Warning::NeutralCast(_)))
            .collect();
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}

#[test]
fn a_decoded_picture_needs_no_correction() {
    let xy = daylight_xy(6500.0);
    let picture = lit(&adaptation_from(xy));
    let rgb = draw(&picture);
    let measured = Shot {
        image: view(&rgb),
        clip: Some(ClipSource {
            image: view(&rgb),
            level: PICTURE_CLIP_LEVEL,
        }),
        standard: IDENTITY,
    };
    let handles = Orientation::ALL[6].handles(CORNERS);
    let report = calibrate(&measured, &Request::new(CHART, handles, &[])).unwrap();
    assert_eq!(report.orientation, Orientation::ALL[6]);
    assert_xy(report.illuminant.xy, xy);
    assert_eq!(report.illuminant.kind, "d");
    assert!(
        report.solution.after.max < 5e-3,
        "{:?}",
        report.solution.after
    );
    for (found, identity) in report.solution.matrix.iter().zip(&IDENTITY) {
        for (f, i) in found.iter().zip(identity) {
            assert!((f - i).abs() < 1e-4, "{:?}", report.solution.matrix);
        }
    }
}

#[test]
fn excluded_and_clipped_patches_are_left_out() {
    let shot = raw_shot();
    let mut camera = shot.camera.clone();
    // B3 measured wrong and excluded by name; C4 wrong and clipped.
    let (b3, _) = CHART.patch("B3").unwrap();
    let (c4, _) = CHART.patch("C4").unwrap();
    camera[b3] = [0.3, 0.05, 0.4];
    camera[c4] = [0.9, 0.9, 0.1];
    let rgb = draw(&camera);
    let mut clipped = shot.camera.iter().map(|_| [0.0; 3]).collect::<Vec<_>>();
    clipped[c4] = [1.0; 3];
    let mosaic: Vec<f32> = draw(&clipped).chunks(3).map(|p| p[0]).collect();
    let measured = Shot {
        image: view(&rgb),
        clip: Some(ClipSource {
            image: ImageView {
                data: &mosaic,
                width: W,
                height: H,
                channels: 1,
            },
            level: crate::sampler::RAW_CLIP_LEVEL,
        }),
        standard: shot.standard,
    };
    let exclude = vec!["B3".to_string()];
    let report = calibrate(&measured, &Request::new(CHART, CORNERS, &exclude)).unwrap();
    assert!(report.check.warnings().contains(&Warning::Clipped(c4)));
    assert_eq!(report.solution.patches[b3].weight, 0.0);
    assert_eq!(report.solution.patches[c4].weight, 0.0);
    assert!(
        report.solution.after.max < 5e-3,
        "{:?}",
        report.solution.after
    );
    assert_xy(report.illuminant.xy, shot.xy);

    let unknown = vec!["Z9".to_string()];
    assert_eq!(
        calibrate(&measured, &Request::new(CHART, CORNERS, &unknown)).unwrap_err(),
        RunError::UnknownPatch("Z9".into())
    );
}

#[test]
fn a_chart_outside_the_image_is_unsolved() {
    let rgb = vec![0.2_f32; W * H * 3];
    let measured = Shot {
        image: view(&rgb),
        clip: None,
        standard: IDENTITY,
    };
    let far = CORNERS.map(|[x, y]| [x + 5000.0, y + 5000.0]);
    assert!(matches!(
        calibrate(&measured, &Request::new(CHART, far, &[])),
        Err(RunError::Unsolved(_))
    ));
}
