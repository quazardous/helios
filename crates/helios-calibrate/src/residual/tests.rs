//! The residual table on synthetic charts: a camera with a twist no matrix
//! can undo keeps it, a linear camera drops it; a profile carries it and
//! the `chartprofile` module renders it as fitted.

use helios_core::{Context, Image, Module, Params, Region, Value};
use helios_module_channelmixerrgb::daylight_xy;
use helios_module_chartprofile::ChartProfile;
use helios_photo::color::{apply, inverse, mul};
use helios_photo::compare::delta_e_2000;

use crate::lab::xyz_to_lab;
use crate::look::{HUES, HueSatTable, SATURATIONS, VALUES};
use crate::profile::{PROFILE_FORMAT, Profile, ProfileSource};
use crate::request::{ChartAnswer, ChartArgs, calibrate_picture};
use crate::run::tests::{CHART, CORNERS, H, W, adaptation_from, correction, draw, lit};
use crate::run::working_to_xyz;
use crate::solver::normalise;

/// The chart's references under 6500 K, in the working space.
fn references() -> Vec<[f64; 3]> {
    lit(&adaptation_from(daylight_xy(6500.0)))
}

/// A camera's non-linear twist: hues turned by up to 10° and saturations
/// scaled by up to 20 %, both as the third harmonic of the hue (no matrix
/// makes it), greys kept, the luminance kept.
fn twist(neutral: [f64; 3]) -> HueSatTable {
    let mut table = HueSatTable::identity(neutral);
    for v in 0..VALUES {
        for h in 0..HUES {
            let hue = (h as f64 * 360.0 / HUES as f64).to_radians();
            for s in 1..SATURATIONS {
                table.nodes[(v * HUES + h) * SATURATIONS + s] = [
                    10.0 * (3.0 * hue).sin(),
                    1.0 + 0.2 * (3.0 * hue + 1.0).cos(),
                    1.0,
                ];
            }
        }
    }
    table
}

fn neutral_of(values: &[[f64; 3]]) -> [f64; 3] {
    let sum = CHART
        .neutrals
        .iter()
        .fold([0.0; 3], |acc, &k| [0, 1, 2].map(|c| acc[c] + values[k][c]));
    normalise(sum).unwrap()
}

/// `values` as a decoded picture at 80 % exposure, answered.
fn answer(values: &[[f64; 3]]) -> ChartAnswer {
    let scaled: Vec<[f64; 3]> = values.iter().map(|v| v.map(|c| 0.8 * c)).collect();
    let rgb = draw(&scaled);
    let data: Vec<f32> = rgb
        .chunks(3)
        .flat_map(|p| [p[0], p[1], p[2], 1.0])
        .collect();
    calibrate_picture(&ChartArgs::new(CORNERS), &data, W, H, 4).unwrap()
}

fn twisted() -> Vec<[f64; 3]> {
    let t = references();
    let camera = twist(neutral_of(&t));
    t.iter().map(|&v| camera.apply(v)).collect()
}

#[test]
fn a_twisted_camera_keeps_the_table_and_it_lowers_delta_e() {
    let a = answer(&twisted());
    let r = a.residual.as_ref().expect("a residual check");
    assert_eq!(r.status, "kept", "{r:?}");
    assert!(
        r.held_out_with <= (1.0 - super::MARGIN) * r.held_out_without
            && r.held_out_without - r.held_out_with >= super::MIN_GAIN,
        "{r:?}"
    );
    assert!(r.after.mean < a.after.mean, "{:?} {:?}", r.after, a.after);
    assert!(r.table.as_ref().is_some_and(HueSatTable::is_valid));
    // A profile saved from it carries the table, in format 2.
    let source = ProfileSource {
        camera: None,
        image: "ab",
        edit: None,
        excluded: &[],
    };
    let profile = Profile::new(&a, &source, "twisted", 0);
    assert_eq!(profile.format, PROFILE_FORMAT);
    assert_eq!(profile.lut.as_ref(), r.table.as_ref());
    let check = profile.report.residual.as_ref().unwrap();
    assert_eq!(check.status, "kept");
    assert_eq!(check.table, None, "the table is stored once, in `lut`");
}

#[test]
fn a_linear_camera_drops_the_table() {
    let t = references();
    // A camera a matrix describes exactly: the solve undoes it.
    let back = inverse(&correction(&t)).unwrap();
    let values: Vec<[f64; 3]> = t.iter().map(|&v| apply(&back, v)).collect();
    let a = answer(&values);
    let r = a.residual.as_ref().expect("a residual check");
    assert_eq!(r.status, "dropped", "{r:?}");
    assert_eq!(r.table, None);
    let source = ProfileSource {
        camera: None,
        image: "ab",
        edit: None,
        excluded: &[],
    };
    assert_eq!(Profile::new(&a, &source, "linear", 0).lut, None);
}

#[test]
fn the_same_chart_gives_the_same_table_bit_for_bit() {
    let values = twisted();
    let (a, b) = (answer(&values), answer(&values));
    let bits = |a: &ChartAnswer| {
        let r = a.residual.as_ref().unwrap();
        let mut v: Vec<u64> = r
            .table
            .as_ref()
            .unwrap()
            .to_param()
            .iter()
            .map(|x| x.to_bits())
            .collect();
        v.extend([r.held_out_without.to_bits(), r.held_out_with.to_bits()]);
        v
    };
    assert_eq!(bits(&a), bits(&b));
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

#[test]
fn the_module_renders_the_profile_with_its_table_as_fitted() {
    let a = answer(&twisted());
    let table = a.residual.as_ref().unwrap().table.clone().unwrap();
    // A picture: M_s is the identity, so the module's matrix is P.
    let p = a.matrix;
    let params = ChartProfile
        .manifest()
        .resolve(
            &Params::new()
                .with("matrix", Value::List(p.iter().flatten().copied().collect()))
                .with("lut", Value::List(table.to_param())),
        )
        .unwrap();
    let plan = ChartProfile.prepare(&params, &Context::new()).unwrap();
    let kept: Vec<usize> = (0..CHART.len())
        .filter(|&k| a.patches[k].status == "kept")
        .collect();
    let pixels: Vec<f32> = kept
        .iter()
        .flat_map(|&k| {
            let m = a.patches[k].measured;
            [m[0] as f32, m[1] as f32, m[2] as f32, 1.0]
        })
        .collect();
    let region = Region::sized(kept.len() as u32, 1);
    let input = Image::new(region, 4, pixels);
    let out = ChartProfile.process(&plan, &[&input], region).unwrap();
    let rendered = out[0].data().as_chunks::<4>().0;
    // The adaptation the answer's light gives, as the solve used it.
    let adaptation = adaptation_from(a.illuminant.xy.map(|v| v as f32));
    let forward = mul(&working_to_xyz(), &adaptation);
    let residual = a.residual.as_ref().unwrap();
    let mut mean = 0.0;
    for (i, &k) in kept.iter().enumerate() {
        let want = table.apply(apply(&p, a.patches[k].measured));
        let got = rendered[i];
        for c in 0..3 {
            let g = f64::from(got[c]);
            assert!(
                (g - want[c]).abs() <= 1e-4 * want[c].abs().max(0.05),
                "{}: {got:?} {want:?}",
                a.patches[k].name
            );
        }
        let lab = xyz_to_lab(apply(&forward, [0, 1, 2].map(|c| f64::from(got[c]))));
        mean += delta_e_2000(lab, a.patches[k].reference);
    }
    mean /= kept.len() as f64;
    assert!(
        (mean - residual.after.mean).abs() < 0.05,
        "rendered {mean}, fitted {}",
        residual.after.mean
    );
}
