//! Backend contract: a module's GPU variant against its CPU reference.
//! Without any GPU adapter (software ones count), skipped, saying so.

use helios_core::{Context, ModuleRef, Params, Region, Value};
use helios_gpu::{Gpu, check};
use helios_module_exposure::Exposure;

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => {
            eprintln!("GPU: {}", g.adapter);
            Some(g)
        }
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

#[test]
fn exposure_on_the_gpu_matches_the_cpu() {
    let Some(gpu) = gpu() else { return };
    let input = helios_testkit::noise(Region::sized(97, 61), 4, 7);
    let module = ModuleRef::new(Exposure);
    for (exposure, black) in [(0.0, 0.0), (1.5, 0.01), (-3.0, 0.2), (18.0, -1.0)] {
        let params = Params::new()
            .with("exposure", Value::Float(exposure))
            .with("black", Value::Float(black));
        let c = check(
            &gpu,
            &module,
            &params,
            &Context::new(),
            &input,
            input.region(),
        )
        .unwrap();
        assert!(
            c.max_rel < 1e-5,
            "exposure {exposure}, black {black}: {c:?}"
        );
    }
}

#[test]
fn a_module_without_a_gpu_variant_says_so() {
    let Some(gpu) = gpu() else { return };
    let input = helios_testkit::noise(Region::sized(4, 4), 1, 1);
    let module = ModuleRef::new(helios_testkit_blurless::Identity);
    let out = gpu
        .run(
            &module,
            &Params::new(),
            &Context::new(),
            &input,
            input.region(),
        )
        .unwrap();
    assert!(out.is_none());
}

mod helios_testkit_blurless {
    use helios_core::*;

    /// A module with no GPU variant.
    pub struct Identity;

    impl Module for Identity {
        type Plan = ();
        fn manifest(&self) -> Manifest {
            Manifest {
                name: "identity".into(),
                version: 1,
                description: String::new(),
                params: vec![],
                inputs: vec![PortSpec::image("input", 1)],
                outputs: vec![PortSpec::image("output", 1)],
                context: vec![],
                needs: Needs::default(),
                kind: ModuleKind::Process,
                family: Family::Other,
                role: Role::General,
            }
        }
        fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
            Ok(())
        }
        fn process(&self, _: &(), inputs: &[&Image], _: Region) -> Result<Vec<Image>, String> {
            Ok(vec![inputs[0].clone()])
        }
    }
}

#[test]
fn the_blur_on_the_gpu_matches_the_cpu_with_its_overlap() {
    let Some(gpu) = gpu() else { return };
    let input = helios_testkit::noise(Region::sized(80, 60), 4, 3);
    let module = ModuleRef::new(helios_module_blur::Blur);
    // A region inside the image, and one at its corner where the edges are
    // replicated: the kernel reads the overlap the plan asks for.
    for (radius, region) in [
        (0.0, Region::new(10, 10, 40, 30)),
        (2.0, Region::new(10, 10, 40, 30)),
        (6.5, Region::new(0, 0, 33, 21)),
        (20.0, Region::new(5, 5, 30, 20)),
    ] {
        let params = Params::new().with("radius", Value::Float(radius));
        let c = check(&gpu, &module, &params, &Context::new(), &input, region).unwrap();
        assert!(c.max_abs < 1e-4, "radius {radius}: {c:?}");
    }
}

#[test]
fn sigmoid_on_the_gpu_matches_the_cpu() {
    let Some(gpu) = gpu() else { return };
    // Scene values up to 4 (HDR), some negative to exercise desaturation.
    let input = helios_core::Image::from_fn(Region::sized(64, 48), 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        let v = ((x * 7 + y * 13 + c as i64 * 5) % 97) as f32 / 24.0 - 0.1;
        v * if c == 1 { 0.8 } else { 1.0 }
    });
    let module = ModuleRef::new(helios_module_sigmoid::Sigmoid);
    for (contrast, skew, hue) in [(1.5, 0.0, 100.0), (2.5, 0.5, 50.0), (1.0, -0.5, 0.0)] {
        let params = Params::new()
            .with("contrast", Value::Float(contrast))
            .with("skew", Value::Float(skew))
            .with("preserve_hue", Value::Float(hue));
        let c = check(
            &gpu,
            &module,
            &params,
            &Context::new(),
            &input,
            input.region(),
        )
        .unwrap();
        assert!(
            c.max_abs < 1e-4,
            "contrast {contrast}, skew {skew}, hue {hue}: {c:?}"
        );
    }
}
