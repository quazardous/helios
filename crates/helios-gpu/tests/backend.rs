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
        fn process(&self, (): &(), inputs: &[&Image], _: Region) -> Result<Vec<Image>, String> {
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
        let v = ((x * 7 + y * 13 + i64::from(c) * 5) % 97) as f32 / 24.0 - 0.1;
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

/// exposure → blur → sigmoid over a region, on an engine with or without
/// the GPU.
fn chain(engine: &helios_core::Engine, input: &helios_core::Image) -> helios_core::Image {
    use helios_core::graph::SOURCE_PORT;
    let mut g = helios_core::Graph::new();
    let src = g.add_source("src", input.clone());
    let mut prev = src;
    let mut port = SOURCE_PORT.to_string();
    let steps: [(&str, ModuleRef, Params); 3] = [
        (
            "exposure",
            ModuleRef::new(Exposure),
            Params::new().with("exposure", Value::Float(1.0)),
        ),
        (
            "blur",
            ModuleRef::new(helios_module_blur::Blur),
            Params::new().with("radius", Value::Float(3.0)),
        ),
        (
            "sigmoid",
            ModuleRef::new(helios_module_sigmoid::Sigmoid),
            Params::new().with("contrast", Value::Float(1.8)),
        ),
    ];
    for (name, module, params) in steps {
        let n = g.add_module(name, module.clone(), &params).unwrap();
        g.connect(prev, &port, n, &module.manifest().inputs[0].name)
            .unwrap();
        prev = n;
        port.clone_from(&module.manifest().outputs[0].name);
    }
    engine
        .render(&g, prev, &port, Region::new(8, 6, 48, 36))
        .unwrap()
}

#[test]
fn an_engine_with_the_gpu_renders_a_chain_as_the_cpu_does() {
    use std::sync::Arc;
    let Some(gpu) = gpu() else { return };
    let input = helios_testkit::noise(Region::sized(64, 48), 4, 11);
    let bus = Arc::new(helios_core::Bus::new());
    let cpu = chain(&helios_core::Engine::new(bus.clone()), &input);
    let engine = helios_core::Engine::new(bus).with_accelerator(Arc::new(gpu));
    let out = chain(&engine, &input);
    let c = helios_gpu::compare(&out, &cpu).unwrap();
    assert!(c.max_abs < 1e-4, "{c:?}");
    assert_eq!(engine.counters().accelerated, 3);
    // Asked again, everything comes from the cache.
    chain(&engine, &input);
    assert_eq!(engine.counters().accelerated, 3);
}

mod broken {
    use helios_core::*;

    /// A module whose GPU variant does not compile.
    pub struct Broken;

    impl Module for Broken {
        type Plan = ();
        fn manifest(&self) -> Manifest {
            Manifest {
                name: "broken".into(),
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
        fn process(&self, (): &(), inputs: &[&Image], _: Region) -> Result<Vec<Image>, String> {
            Ok(vec![inputs[0].clone()])
        }
        fn gpu_kernel(&self) -> Option<GpuKernel> {
            Some(GpuKernel {
                wgsl: "@compute @workgroup_size(64) fn main() { let x: f32 = nope; }",
                entry: "main",
            })
        }
    }
}

#[test]
fn a_kernel_that_does_not_compile_is_an_error_not_a_panic() {
    use std::sync::Arc;
    let Some(gpu) = gpu() else { return };
    let mut g = helios_core::Graph::new();
    let src = g.add_source("src", helios_testkit::noise(Region::sized(8, 8), 1, 2));
    let n = g
        .add_module("b", ModuleRef::new(broken::Broken), &Params::new())
        .unwrap();
    g.connect(src, helios_core::graph::SOURCE_PORT, n, "input")
        .unwrap();
    let engine =
        helios_core::Engine::new(Arc::new(helios_core::Bus::new())).with_accelerator(Arc::new(gpu));
    let err = engine
        .render(&g, n, "output", Region::sized(8, 8))
        .unwrap_err();
    assert!(
        matches!(&err, helios_core::Error::Module { module, reason } if module == "broken" && reason.starts_with("GPU: kernel `main`")),
        "{err}"
    );
}

#[test]
fn vkdt_modules_on_the_gpu_match_their_cpu_reference() {
    let Some(gpu) = gpu() else { return };
    let input = helios_testkit::noise(Region::sized(70, 50), 4, 9);
    let exposure = ModuleRef::new(helios_module_vkdt::VkdtExposure);
    for ev in [-2.0, 0.0, 1.5] {
        let params = Params::new().with("exposure", Value::Float(ev));
        let c = check(
            &gpu,
            &exposure,
            &params,
            &Context::new(),
            &input,
            input.region(),
        )
        .unwrap();
        assert!(c.max_rel < 1e-6, "exposure {ev}: {c:?}");
    }
    let usm = ModuleRef::new(helios_module_vkdt::VkdtUsm);
    for (amount, thrs, region) in [
        (2.0, 0.2, Region::new(6, 5, 40, 30)),
        (0.5, 0.05, Region::new(0, 0, 30, 20)),
    ] {
        let params = Params::new()
            .with("amount", Value::Float(amount))
            .with("thrs", Value::Float(thrs));
        let c = check(&gpu, &usm, &params, &Context::new(), &input, region).unwrap();
        assert!(c.max_abs < 1e-4, "usm {amount}, {thrs}: {c:?}");
    }
}
