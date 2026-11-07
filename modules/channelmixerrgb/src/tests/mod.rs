use super::*;

/// darktable's results, from `dt_cat16.c`: its CAT16 path copied from its
/// sources, run with the same working space matrix.
mod golden;
/// darktable's "as shot in camera" illuminants, from `dt_as_shot.c`: its
/// code copied from its sources, run on four cameras' matrices and a few
/// as-shot coefficients.
mod golden_as_shot;

fn params(case: &golden::Case) -> Params {
    Params::new()
        .with("illuminant", Value::Choice(case.illuminant.into()))
        .with("temperature", Value::Float(case.temperature))
        .with("illum_fluo", Value::Choice(case.fluo.into()))
        .with("x", Value::Float(case.x))
        .with("y", Value::Float(case.y))
        .with("gamut", Value::Float(case.gamut))
        .with("clip", Value::Bool(case.clip))
}

fn plan(p: Params) -> Result<Plan, String> {
    let m = ChannelMixerRgb.manifest();
    let p = m.resolve(&p).map_err(|e| format!("{e:?}"))?;
    ChannelMixerRgb.prepare(&p, &Context::new())
}

fn run(plan: &Plan, pixels: &[[f32; 4]]) -> Vec<f32> {
    let region = Region::sized(pixels.len() as u32, 1);
    let img = Image::new(region, 4, pixels.iter().flatten().copied().collect());
    ChannelMixerRgb.process(plan, &[&img], region).unwrap()[0]
        .data()
        .to_vec()
}

/// Relative to the value above 1, absolute below.
fn close(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance * b.abs().max(1.0)
}

#[test]
fn the_illuminants_match_darktables_chromaticities() {
    for case in golden::CASES {
        let xy = illuminant_xy(
            &ChannelMixerRgb.manifest().resolve(&params(case)).unwrap(),
            &Context::new(),
        )
        .unwrap();
        for c in 0..2 {
            assert!(
                (xy[c] - case.xy[c]).abs() < 1e-6,
                "{} {} K: {xy:?} against {:?}",
                case.illuminant,
                case.temperature,
                case.xy
            );
        }
    }
}

#[test]
fn the_adaptation_matches_darktables_within_1e_4() {
    let mut worst = 0.0f32;
    for case in golden::CASES {
        let plan = plan(params(case)).unwrap();
        let out = run(&plan, &golden::COLOURS);
        for (i, (got, want)) in out.chunks(4).zip(case.out).enumerate() {
            for c in 0..4 {
                worst = worst.max((got[c] - want[c]).abs() / want[c].abs().max(1.0));
                assert!(
                    close(got[c], want[c], 1e-4),
                    "{} {} K, gamut {} clip {}, colour {i}: {got:?} against {want:?}",
                    case.illuminant,
                    case.temperature,
                    case.gamut,
                    case.clip
                );
            }
        }
    }
    // The bound the parity holds today, to see it move.
    assert!(worst < 1e-5, "worst difference {worst}");
}

#[test]
fn d50_leaves_grey_grey_and_a_warm_light_turns_white_cold() {
    let pipe = plan(Params::new().with("illuminant", Value::Choice("pipe".into()))).unwrap();
    // Within darktable's own rounding: its D50 in CAT16 LMS is rounded, and
    // the gamut compression moves grey by about 1e-4.
    let out = run(&pipe, &[[0.18, 0.18, 0.18, 1.0]]);
    for v in &out[..3] {
        assert!((v - 0.18).abs() < 5e-4, "{out:?}");
    }
    // Under tungsten, a grey reflector reads orange: adapted to D50 it is
    // grey again, so the image's orange cast goes.
    let a = plan(Params::new().with("illuminant", Value::Choice("a".into()))).unwrap();
    let out = run(&a, &[[0.3, 0.18, 0.08, 1.0]]);
    assert!(out[2] > 0.08 && out[0] < 0.3, "{out:?}");
}

#[test]
fn a_synthetic_image_keeps_its_size_alpha_and_order() {
    let region = Region::sized(4, 3);
    let img = Image::from_fn(region, 4, |x, y, c| match c {
        3 => 0.25 * x as f32,
        _ => 0.1 + 0.05 * (x + 2 * y + i64::from(c)) as f32,
    });
    let case = &golden::CASES[0];
    let plan = plan(params(case)).unwrap();
    let out = &ChannelMixerRgb.process(&plan, &[&img], region).unwrap()[0];
    assert_eq!(out.region(), region);
    for y in 0..3 {
        for x in 0..4 {
            let p = img.pixel(x, y);
            let want = plan.pixel([p[0], p[1], p[2]]);
            let got = out.pixel(x, y);
            assert_eq!(&got[..3], &want, "({x}, {y})");
            assert_eq!(got[3], p[3]);
        }
    }
}

#[test]
fn unknown_parts_are_refused_by_name() {
    // The mixer rows are not ported: the manifest refuses them.
    let err = plan(Params::new().with("red", Value::List(vec![1.0, 0.0, 0.0]))).unwrap_err();
    assert!(err.contains("red"), "{err}");
    let err = plan(Params::new().with("adaptation", Value::Choice("bradford".into()))).unwrap_err();
    assert!(err.contains("adaptation"), "{err}");
    let err =
        plan(Params::new().with("illuminant", Value::Choice("detect_edges".into()))).unwrap_err();
    assert!(err.contains("illuminant"), "{err}");
}

fn camera_context(camera: &golden_as_shot::Camera, wb: [f64; 3]) -> Context {
    use helios_photo::camera::fields;
    Context::new()
        .with(
            fields::XYZ_TO_CAMERA,
            Value::List(camera.xyz_to_camera.to_vec()),
        )
        .with(
            fields::WHITE_BALANCE,
            Value::List(vec![wb[0], wb[1], wb[2], wb[1]]),
        )
}

#[test]
fn the_as_shot_illuminant_matches_darktables() {
    let mut worst = 0.0f32;
    let mut worst_t = 0.0f32;
    for camera in &golden_as_shot::CAMERAS {
        // The white balance's camera reference, darktable's multipliers.
        let d65 = helios_module_whitebalance::d65_coefficients(&camera.xyz_to_camera).unwrap();
        for c in 0..3 {
            assert!(
                (d65[c] - camera.d65[c]).abs() < 1e-6,
                "{}: {d65:?} against {:?}",
                camera.name,
                camera.d65
            );
        }
        for shot in &camera.shots {
            let got = as_shot_illuminant_xy(&camera.xyz_to_camera, &shot.wb);
            let ctx = camera_context(camera, shot.wb);
            assert_eq!(camera_illuminant_xy(&ctx), got, "{}", camera.name);
            match (got, shot.xy) {
                (None, None) => {}
                (Some(xy), Some(want)) => {
                    for c in 0..2 {
                        worst = worst.max((xy[c] - want[c]).abs());
                    }
                    let (t, kind) = close_to_daylight(xy);
                    worst_t = worst_t.max((t - shot.temperature).abs() / shot.temperature);
                    assert_eq!(kind, shot.kind, "{} {:?}", camera.name, shot.wb);
                }
                other => panic!("{} {:?}: {other:?}", camera.name, shot.wb),
            }
        }
    }
    // The bounds the parity holds today, to see them move.
    assert!(worst < 1e-6, "worst chromaticity difference {worst}");
    assert!(
        worst_t < 1e-4,
        "worst relative temperature difference {worst_t}"
    );
}

#[test]
fn the_camera_illuminant_adapts_from_the_shots_white() {
    let camera = &golden_as_shot::CAMERAS[0];
    // A tungsten shot: the illuminant is warm, and its adaptation is the
    // custom illuminant's at darktable's chromaticity.
    let shot = &camera.shots[1];
    let p = ChannelMixerRgb
        .manifest()
        .resolve(&Params::new().with("illuminant", Value::Choice(CAMERA.into())))
        .unwrap();
    let got = ChannelMixerRgb
        .prepare(&p, &camera_context(camera, shot.wb))
        .unwrap();
    let want = shot.xy.unwrap();
    let custom = plan(
        Params::new()
            .with("illuminant", Value::Choice("custom".into()))
            .with("x", Value::Float(f64::from(want[0])))
            .with("y", Value::Float(f64::from(want[1]))),
    )
    .unwrap();
    for c in 0..3 {
        assert!(
            (got.illuminant[c] - custom.illuminant[c]).abs() < 1e-5,
            "{:?} against {:?}",
            got.illuminant,
            custom.illuminant
        );
    }
    // The camera's own D65 coefficients give D50: next to nothing to adapt.
    let d65 = camera.d65;
    let plan = ChannelMixerRgb
        .prepare(&p, &camera_context(camera, d65))
        .unwrap();
    let out = run(&plan, &[[0.18, 0.18, 0.18, 1.0]]);
    for v in &out[..3] {
        assert!((v - 0.18).abs() < 1e-3, "{out:?}");
    }
}

#[test]
fn a_camera_illuminant_not_found_is_d50() {
    let p = ChannelMixerRgb
        .manifest()
        .resolve(&Params::new().with("illuminant", Value::Choice(CAMERA.into())))
        .unwrap();
    let pipe = plan(Params::new().with("illuminant", Value::Choice("pipe".into()))).unwrap();
    // No camera in the context, or a coefficient of zero.
    let camera = &golden_as_shot::CAMERAS[0];
    for ctx in [Context::new(), camera_context(camera, [0.0, 1.0, 1.5])] {
        assert_eq!(ChannelMixerRgb.prepare(&p, &ctx).unwrap(), pipe);
    }
}

#[test]
fn it_keeps_the_module_contract() {
    helios_testkit::assert_contract(ChannelMixerRgb);
}
