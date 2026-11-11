//! Parity of exposure's brick form with the sealed module: the same bits on
//! the CPU, over the images exposure's tests use (darktable parity's chart,
//! the test kit's noise and ramp, its adversarial inputs) and a sweep of
//! its parameters and of the camera's bias.

use std::sync::Arc;

use helios_core::{Bus, Context, Engine, Graph, Image, ModuleRef, Params, Region, Value};
use helios_module_exposure::{Exposure, from_bricks};
use helios_photo::camera::fields::EXPOSURE_BIAS;

/// darktable parity's chart (`helios-parity`'s `exposure_matches_darktable`):
/// ramps past 1, a grey band, saturated patches, a few negative values.
fn chart() -> Image {
    Image::from_fn(Region::sized(96, 64), 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        if y < 8 {
            return 0.18;
        }
        let (fx, fy) = (x as f32 / 95.0, y as f32 / 63.0);
        match c {
            0 => fx * fx * 1.6,
            1 => fy * 0.9 - 0.02,
            _ => ((x + y) % 17) as f32 / 16.0 * 0.7,
        }
    })
}

fn images() -> Vec<(String, Image)> {
    let noise = helios_testkit::noise(Region::sized(97, 61), 4, 7);
    let mut images = vec![
        ("chart".to_string(), chart()),
        (
            "ramp".to_string(),
            helios_testkit::ramp(Region::sized(64, 16), 4),
        ),
        ("noise".to_string(), noise.clone()),
    ];
    for (name, fill) in helios_testkit::ADVERSARIAL {
        let data = noise
            .data()
            .iter()
            .enumerate()
            .map(|(i, v)| fill(*v, i as u64))
            .collect();
        images.push(((*name).to_string(), Image::new(noise.region(), 4, data)));
    }
    images
}

fn render(
    engine: &Engine,
    module: &ModuleRef,
    params: &Params,
    context: &Context,
    input: &Image,
    region: Region,
) -> Result<Vec<u32>, String> {
    let mut g = Graph::new();
    let src = g.add_source("src", input.clone());
    let n = g
        .add_module("exposure", module.clone(), params)
        .map_err(|e| e.to_string())?;
    g.set_context(n, context.clone()).unwrap();
    g.connect(src, helios_core::graph::SOURCE_PORT, n, "input")
        .unwrap();
    let out = engine
        .render(&g, n, "output", region)
        .map_err(|e| e.to_string())?;
    Ok(out.data().iter().map(|v| v.to_bits()).collect())
}

struct Pair {
    sealed: (Engine, ModuleRef),
    bricks: (Engine, ModuleRef),
}

impl Pair {
    fn new() -> Self {
        let engine = || Engine::new(Arc::new(Bus::new()));
        Self {
            sealed: (engine(), ModuleRef::new(Exposure)),
            bricks: (engine(), from_bricks()),
        }
    }

    /// Both forms render the same bits, or both refuse.
    fn same(&self, case: &str, params: &Params, context: &Context, input: &Image, region: Region) {
        let sealed = render(
            &self.sealed.0,
            &self.sealed.1,
            params,
            context,
            input,
            region,
        );
        let bricks = render(
            &self.bricks.0,
            &self.bricks.1,
            params,
            context,
            input,
            region,
        );
        match (sealed, bricks) {
            (Ok(a), Ok(b)) => {
                if let Some(i) = (0..a.len()).find(|&i| a[i] != b[i]) {
                    panic!(
                        "{case}: first difference at {i}: sealed {}, bricks {}",
                        f32::from_bits(a[i]),
                        f32::from_bits(b[i])
                    );
                }
                assert_eq!(a.len(), b.len(), "{case}");
            }
            (Err(a), Err(b)) => {
                assert!(a.contains(EXPOSURE_BIAS), "{case}: {a}");
                assert!(b.contains(EXPOSURE_BIAS), "{case}: {b}");
            }
            (a, b) => panic!("{case}: sealed {a:?}, bricks {b:?}"),
        }
    }
}

fn params(exposure: f64, black: f64) -> Params {
    Params::new()
        .with("exposure", Value::Float(exposure))
        .with("black", Value::Float(black))
}

#[test]
fn the_brick_form_renders_the_sealed_bits_over_a_sweep() {
    let pair = Pair::new();
    let images = images();
    let mut exposures: Vec<f64> = (0..=50).map(|i| -18.0 + f64::from(i) * 0.72).collect();
    exposures.extend([0.0, 1.0, -2.0, 3.5, 0.5, -0.5, 1.0 / 3.0, 18.0, 1.5, -3.0]);
    let blacks = [-1.0, -0.1, -0.02, 0.0, 0.01, 0.05, 0.2, 0.8, 1.0];
    for exposure in &exposures {
        for black in blacks {
            for (name, image) in &images[..2] {
                pair.same(
                    &format!("{name}, exposure {exposure}, black {black}"),
                    &params(*exposure, black),
                    &Context::new(),
                    image,
                    image.region(),
                );
            }
        }
    }
}

#[test]
fn the_brick_form_renders_the_sealed_bits_on_every_test_image() {
    let pair = Pair::new();
    for (name, image) in images() {
        for (exposure, black) in [
            (0.0, 0.0),
            (1.5, 0.01),
            (-3.0, 0.2),
            (18.0, -1.0),
            (1.0, 0.8),
        ] {
            let p = params(exposure, black);
            let case = format!("{name}, exposure {exposure}, black {black}");
            pair.same(&case, &p, &Context::new(), &image, image.region());
            // A region inside the image, as a viewport asks.
            let inside = Region::new(8, 6, 48, 36);
            pair.same(
                &format!("{case}, inside"),
                &p,
                &Context::new(),
                &image,
                inside,
            );
        }
    }
}

#[test]
fn the_brick_form_takes_the_cameras_bias_back_as_the_sealed_one() {
    let pair = Pair::new();
    let image = chart();
    let compensate = |exposure: f64, black: f64| {
        params(exposure, black).with("compensate_bias", Value::Bool(true))
    };
    let biases = [
        Value::Float(1.0),
        Value::Float(-0.7),
        Value::Float(1.0 / 3.0),
        Value::Float(5.0),
        Value::Float(-5.0),
        Value::Float(5.000_001),
        Value::Float(12.0),
        Value::Float(f64::NAN),
        Value::Float(f64::NEG_INFINITY),
        Value::Int(2),
    ];
    for (exposure, black) in [(0.0, 0.0), (1.0, 0.01), (-2.0, 0.05), (0.37, -0.02)] {
        for bias in &biases {
            let context = Context::new().with(EXPOSURE_BIAS, bias.clone());
            let case = format!("exposure {exposure}, black {black}, bias {bias:?}");
            pair.same(
                &case,
                &compensate(exposure, black),
                &context,
                &image,
                image.region(),
            );
            // Not asked: the bias changes nothing in either form.
            pair.same(
                &format!("{case}, not compensated"),
                &params(exposure, black),
                &context,
                &image,
                image.region(),
            );
        }
        // Asked without the bias: both refuse, naming the field.
        pair.same(
            &format!("exposure {exposure}, no bias"),
            &compensate(exposure, black),
            &Context::new(),
            &image,
            image.region(),
        );
    }
}
