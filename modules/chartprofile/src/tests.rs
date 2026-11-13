//! The chart profile on synthetic pixels: the matrix it says, the identity
//! when the profile is the standard matrix, nothing without a profile.

use super::*;

/// The Canon EOS 40D's matrix, as its raw files record it.
const XYZ_TO_CAMERA: [f64; 9] = [
    0.6071, -0.0747, -0.0856, -0.7653, 1.5365, 0.2441, -0.2025, 0.2553, 0.7315,
];

fn raw_context() -> Context {
    Context::new().with(fields::XYZ_TO_CAMERA, Value::List(XYZ_TO_CAMERA.to_vec()))
}

/// A profile's matrix: the standard one bent a little, as a fit gives.
fn profile_matrix(m_s: &Mat3) -> Mat3 {
    let bend: Mat3 = [
        [1.06, -0.04, -0.02],
        [-0.03, 1.01, 0.02],
        [0.01, -0.05, 1.04],
    ];
    color::mul(&bend, m_s)
}

/// Pixels of every kind: greys, saturated colours, a negative, an HDR one.
fn pixels() -> Vec<[f32; 4]> {
    vec![
        [0.18, 0.18, 0.18, 1.0],
        [0.9, 0.1, 0.05, 0.5],
        [0.05, 0.6, 0.2, 0.25],
        [0.1, 0.2, 0.8, 0.0],
        [-0.01, 0.3, 0.5, 1.0],
        [4.0, 3.5, 2.0, 1.0],
    ]
}

fn run(params: &Params, context: &Context) -> Vec<[f32; 4]> {
    let params = ChartProfile.manifest().resolve(params).unwrap();
    let plan = ChartProfile.prepare(&params, context).unwrap();
    let pixels = pixels();
    let region = Region::sized(pixels.len() as u32, 1);
    let input = Image::new(region, 4, pixels.concat());
    let out = ChartProfile.process(&plan, &[&input], region).unwrap();
    out[0].data().as_chunks::<4>().0.to_vec()
}

fn with_matrix(m: &Mat3) -> Params {
    Params::new()
        .with("matrix", Value::List(flat(m)))
        .with("profile", Value::Text("ab".repeat(32)))
}

#[test]
fn without_a_profile_the_pixels_pass_unchanged() {
    assert_eq!(run(&Params::new(), &raw_context()), pixels());
    assert_eq!(run(&Params::new(), &Context::new()), pixels());
}

#[test]
fn the_standard_matrix_as_profile_is_the_identity() {
    let m_s = standard(&raw_context()).unwrap();
    for (out, inp) in run(&with_matrix(&m_s), &raw_context()).iter().zip(pixels()) {
        for c in 0..3 {
            assert!(
                (out[c] - inp[c]).abs() <= 1e-5 * inp[c].abs().max(1.0),
                "{out:?} {inp:?}"
            );
        }
        assert_eq!(out[3], inp[3], "the fourth channel is kept");
    }
}

#[test]
fn a_raw_s_colours_move_as_the_profile_s_matrix_says() {
    // What colorin gives (M_s · camera), through the module, is the
    // profile's matrix on the camera RGB: M_c · camera.
    let context = raw_context();
    let m_s = standard(&context).unwrap();
    let m_c = profile_matrix(&m_s);
    let back = color::inverse(&m_s).unwrap();
    let out = run(&with_matrix(&m_c), &context);
    for (out, working) in out.iter().zip(pixels()) {
        let w = [0, 1, 2].map(|c| f64::from(working[c]));
        let camera = color::apply(&back, w);
        let want = color::apply(&m_c, camera);
        for c in 0..3 {
            assert!(
                (f64::from(out[c]) - want[c]).abs() <= 2e-6 * want[c].abs().max(1.0),
                "{out:?} against {want:?}"
            );
        }
        assert_eq!(out[3], working[3]);
    }
    // And the colours did move.
    assert!((out[1][0] - pixels()[1][0]).abs() > 0.01);
}

#[test]
fn a_picture_s_profile_is_its_matrix() {
    let m: Mat3 = [[1.1, -0.1, 0.0], [0.0, 0.9, 0.1], [0.05, 0.0, 0.95]];
    let out = run(&with_matrix(&m), &Context::new());
    for (out, inp) in out.iter().zip(pixels()) {
        let want = color::apply(&m, [0, 1, 2].map(|c| f64::from(inp[c])));
        for c in 0..3 {
            assert!((f64::from(out[c]) - want[c]).abs() <= 2e-6 * want[c].abs().max(1.0));
        }
    }
}

#[test]
fn the_same_parameters_give_the_same_bits() {
    let m_c = profile_matrix(&standard(&raw_context()).unwrap());
    let a = run(&with_matrix(&m_c), &raw_context());
    let b = run(&with_matrix(&m_c), &raw_context());
    let bits = |v: &[[f32; 4]]| v.iter().flatten().map(|f| f.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&a), bits(&b));
}

#[test]
fn an_unusable_camera_matrix_is_named() {
    let zero = Context::new().with(fields::XYZ_TO_CAMERA, Value::List(vec![0.0; 9]));
    let params = ChartProfile
        .manifest()
        .resolve(&with_matrix(&IDENTITY))
        .unwrap();
    assert!(ChartProfile.prepare(&params, &zero).is_err());
}

#[test]
fn it_keeps_the_module_contract() {
    helios_testkit::assert_contract_in(ChartProfile, &raw_context());
}

/// A residual table turning every coloured hue by `shift` degrees.
fn turning_table(shift: f64) -> Vec<f64> {
    let mut flat = vec![1.0, 1.0, 1.0];
    for _ in 0..huesat::VALUES * huesat::HUES {
        flat.extend([0.0, 1.0]);
        for _ in 1..huesat::SATURATIONS {
            flat.extend([shift, 1.0]);
        }
    }
    flat
}

#[test]
fn a_profile_without_a_table_is_the_matrix_step_bit_for_bit() {
    // A profile saved before residual tables (no `lut`) renders as the
    // matrix step did.
    let context = raw_context();
    let m_s = standard(&context).unwrap();
    let m_c = profile_matrix(&m_s);
    let out = run(&with_matrix(&m_c), &context);
    let p = correction(&m_c, &m_s).unwrap();
    let mut want = pixels().concat();
    matrix3::apply(&matrix3::plan(&flat(&p)).unwrap(), &mut want);
    let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&out.concat()), bits(&want));
    let empty = with_matrix(&m_c).with("lut", Value::List(Vec::new()));
    assert_eq!(bits(&run(&empty, &context).concat()), bits(&want));
}

#[test]
fn the_residual_table_turns_the_hues_after_the_matrix() {
    let m: Mat3 = [[1.1, -0.1, 0.0], [0.0, 0.9, 0.1], [0.05, 0.0, 0.95]];
    let plain = run(&with_matrix(&m), &Context::new());
    let params = with_matrix(&m).with("lut", Value::List(turning_table(8.0)));
    let turned = run(&params, &Context::new());
    // The grey keeps its value, the colours turn and keep their
    // luminance, the fourth channel is kept.
    for c in 0..3 {
        assert!((turned[0][c] - plain[0][c]).abs() < 1e-6, "{turned:?}");
    }
    let luma =
        |p: &[f32; 4]| huesat::LUMA[0] * p[0] + huesat::LUMA[1] * p[1] + huesat::LUMA[2] * p[2];
    for (t, p) in turned.iter().zip(&plain).skip(1).take(3) {
        assert!((luma(t) - luma(p)).abs() < 1e-5, "{t:?} {p:?}");
        assert!((t[0] - p[0]).abs() + (t[1] - p[1]).abs() + (t[2] - p[2]).abs() > 1e-3);
        assert_eq!(t[3], p[3]);
    }
    // A table of the wrong size is refused, by its name.
    let bad = ChartProfile
        .manifest()
        .resolve(&with_matrix(&m).with("lut", Value::List(vec![1.0; 7])))
        .unwrap();
    let e = ChartProfile.prepare(&bad, &Context::new()).unwrap_err();
    assert!(e.contains("`lut`"), "{e}");
}
