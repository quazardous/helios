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
        .run(&module, &Params::new(), &Context::new(), &input)
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
