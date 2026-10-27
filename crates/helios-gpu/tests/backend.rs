//! Backend contract: a module's GPU variant against its CPU reference.
//! Without any GPU adapter (software ones count), skipped, saying so.

use helios_core::{Context, ModuleRef, Params, Region, Value};
use helios_gpu::{Gpu, check};
use helios_module_exposure::Exposure;

fn gpu() -> Option<std::sync::Arc<Gpu>> {
    match helios_gpu::shared::gpu() {
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
    let _gpu = helios_testkit::gpu_lock();
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
    let _gpu = helios_testkit::gpu_lock();
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
    let _gpu = helios_testkit::gpu_lock();
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
    let _gpu = helios_testkit::gpu_lock();
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
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let input = helios_testkit::noise(Region::sized(64, 48), 4, 11);
    let bus = Arc::new(helios_core::Bus::new());
    let cpu = chain(&helios_core::Engine::new(bus.clone()), &input);
    let engine = helios_core::Engine::new(bus).with_accelerator(gpu);
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
fn a_kernel_that_does_not_compile_runs_its_step_on_the_cpu() {
    use std::sync::Arc;
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let mut g = helios_core::Graph::new();
    let src = g.add_source("src", helios_testkit::noise(Region::sized(8, 8), 1, 2));
    let n = g
        .add_module("b", ModuleRef::new(broken::Broken), &Params::new())
        .unwrap();
    g.connect(src, helios_core::graph::SOURCE_PORT, n, "input")
        .unwrap();
    let engine = helios_core::Engine::new(Arc::new(helios_core::Bus::new())).with_accelerator(gpu);
    let out = engine.render(&g, n, "output", Region::sized(8, 8)).unwrap();
    // The CPU code ran instead (an identity), and the engine says why.
    let input = helios_testkit::noise(Region::sized(8, 8), 1, 2);
    assert_eq!(out.data(), input.data());
    assert_eq!(engine.counters().fallbacks, 1);
    let reason = engine.fallback_reason().unwrap();
    assert!(
        reason.starts_with("broken: kernel `main`") && reason.ends_with("(compile)"),
        "{reason}"
    );
}

#[test]
fn vkdt_modules_on_the_gpu_match_their_cpu_reference() {
    let _gpu = helios_testkit::gpu_lock();
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

#[test]
fn vkdt_grade_on_the_gpu_matches_its_cpu_reference() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let input = helios_testkit::noise(Region::sized(70, 50), 4, 13);
    let grade = ModuleRef::new(helios_module_vkdt::VkdtGrade);
    let classic = Params::new()
        .with("lift", Value::List(vec![0.05, -0.02, 0.1, 0.01]))
        .with("gamma", Value::List(vec![1.2, 0.9, 1.0, 0.05]))
        .with("gain", Value::List(vec![1.1, 1.0, 0.8, 0.1]))
        .with("offset", Value::List(vec![0.0, 0.01, -0.02, 0.0]));
    let zoned = classic
        .clone()
        .with("mode", Value::Choice("zoned_log".into()))
        .with("sh_pivot", Value::Float(0.35))
        .with("hi_pivot", Value::Float(0.6));
    for (name, params) in [
        ("defaults", Params::new()),
        ("classic", classic),
        ("zoned log", zoned),
    ] {
        let c = check(
            &gpu,
            &grade,
            &params,
            &Context::new(),
            &input,
            input.region(),
        )
        .unwrap();
        assert!(c.max_abs < 1e-4, "grade {name}: {c:?}");
    }
}

#[test]
fn vkdt_filmcurv_on_the_gpu_matches_its_cpu_reference() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    // Scene-referred light: past 1 too.
    let input = helios_testkit::noise(Region::sized(70, 50), 4, 17);
    let input = helios_core::Image::new(
        input.region(),
        4,
        input.data().iter().map(|v| v * 3.0).collect(),
    );
    let filmcurv = ModuleRef::new(helios_module_vkdt::VkdtFilmcurv);
    for (light, contrast, bias, colour) in [
        (3.0, 1.2, 0.0, "hsl"),
        (1.5, 0.8, 0.01, "hsl"),
        (5.0, 1.6, 0.005, "per_channel"),
    ] {
        let params = Params::new()
            .with("light", Value::Float(light))
            .with("contrast", Value::Float(contrast))
            .with("bias", Value::Float(bias))
            .with("colour", Value::Choice(colour.into()));
        let c = check(
            &gpu,
            &filmcurv,
            &params,
            &Context::new(),
            &input,
            input.region(),
        )
        .unwrap();
        assert!(
            c.max_abs < 1e-4,
            "filmcurv {light} {contrast} {colour}: {c:?}"
        );
    }
}

#[test]
fn vkdt_colour_on_the_gpu_matches_its_cpu_reference() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let input = helios_testkit::noise(Region::sized(70, 50), 4, 19);
    // The camera's description, as context (the Canon EOS 40D's).
    let context = Context::new()
        .with(
            "photo.xyz_to_camera",
            Value::List(vec![
                0.6071, -0.0747, -0.0856, -0.7653, 1.5365, 0.2441, -0.2025, 0.2553, 0.7315,
            ]),
        )
        .with("photo.white_balance", Value::List(vec![2.1, 1.0, 1.4, 1.0]));
    let colour = ModuleRef::new(helios_module_vkdt::VkdtColour);
    let cases = [
        ("as shot", Params::new()),
        (
            "saturated, clipped",
            Params::new()
                .with("sat", Value::Float(1.4))
                .with("exposure", Value::Float(0.5))
                .with("clip", Value::Bool(true))
                .with("clipmax", Value::Float(0.9)),
        ),
        (
            "rec709, desaturated",
            Params::new()
                .with("matrix", Value::Choice("rec709".into()))
                .with("white", Value::List(vec![1.2, 1.0, 0.9]))
                .with("sat", Value::Float(0.6)),
        ),
    ];
    for (name, params) in cases {
        let c = check(&gpu, &colour, &params, &context, &input, input.region()).unwrap();
        assert!(c.max_abs < 2e-4, "colour {name}: {c:?}");
    }
}

#[test]
fn a_step_over_4_mpx_matches_the_cpu() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    // 5.2 Mpx: along one dimension, 64 pixels a workgroup, it would need
    // 81 250 workgroups, over the 65 535 a dimension is sure to allow.
    let input = helios_testkit::noise(Region::sized(2600, 2000), 4, 5);
    assert!(input.region().area().div_ceil(64) > 65_535);
    let exposure = ModuleRef::new(Exposure);
    let params = Params::new()
        .with("exposure", Value::Float(1.0))
        .with("black", Value::Float(0.01));
    let c = check(
        &gpu,
        &exposure,
        &params,
        &Context::new(),
        &input,
        input.region(),
    )
    .unwrap();
    assert!(c.max_rel < 1e-5, "exposure: {c:?}");
    // A kernel with an overlap: its last rows read the image's last rows.
    let blur = ModuleRef::new(helios_module_blur::Blur);
    let params = Params::new().with("radius", Value::Float(1.0));
    let c = check(
        &gpu,
        &blur,
        &params,
        &Context::new(),
        &input,
        input.region(),
    )
    .unwrap();
    assert!(c.max_abs < 1e-4, "blur: {c:?}");
}

#[test]
fn the_dispatch_grid_covers_every_pixel_within_the_limits() {
    use helios_gpu::grid;
    assert_eq!(grid(64, 65_535), Ok((1, 1)));
    assert_eq!(grid(65, 65_535), Ok((2, 1)));
    assert_eq!(grid(0, 65_535), Ok((1, 1)));
    // 2600 x 2000: 81 250 workgroups, as two rows.
    let (x, y) = grid(2600 * 2000, 65_535).unwrap();
    assert_eq!((x, y), (65_535, 2));
    assert!(u64::from(x) * u64::from(y) * 64 >= 2600 * 2000);
    // Past what two dimensions hold: an error, not a wrong image.
    assert!(grid(u32::MAX, 16).is_err());
}

/// A generator: no input, its value a function of where the pixel is,
/// `x + 1000 y` (negative coordinates too), read from the shape's origins.
mod generator {
    use helios_core::*;

    pub struct Coordinates;

    pub const WGSL: &str = "
@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 3>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let w = shape[1].z;
    let x = bitcast<i32>(shape[2].x) + i32(p % w);
    let y = bitcast<i32>(shape[2].y) + i32(p / w);
    output[p] = f32(x) + 1000.0 * f32(y) + plan[0].x + input[0];
}
";

    impl Module for Coordinates {
        type Plan = ();
        fn manifest(&self) -> Manifest {
            Manifest {
                name: "coordinates".into(),
                version: 1,
                description: String::new(),
                params: vec![],
                inputs: vec![],
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
        fn process(&self, (): &(), _: &[&Image], output: Region) -> Result<Vec<Image>, String> {
            Ok(vec![Image::from_fn(output, 1, |x, y, _| {
                x as f32 + 1000.0 * y as f32 + 0.5
            })])
        }
        fn gpu_kernel(&self) -> Option<GpuKernel> {
            Some(GpuKernel {
                wgsl: WGSL,
                entry: "main",
            })
        }
        fn gpu_uniforms(&self, (): &()) -> Vec<f32> {
            vec![0.5]
        }
    }
}

#[test]
fn a_module_without_inputs_runs_on_the_gpu_where_its_output_is() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let module = ModuleRef::new(generator::Coordinates);
    let unused = helios_testkit::noise(Region::sized(2, 2), 1, 3);
    for region in [Region::new(-7, -3, 19, 5), Region::new(130, 40, 70, 9)] {
        let c = check(
            &gpu,
            &module,
            &Params::new(),
            &Context::new(),
            &unused,
            region,
        )
        .unwrap();
        assert_eq!(c.max_abs, 0.0, "{region}: {c:?}");
    }

    let mut g = helios_core::Graph::new();
    let node = g.add_module("xy", module, &Params::new()).unwrap();
    let engine = helios_core::Engine::new(std::sync::Arc::new(helios_core::Bus::new()))
        .with_accelerator(gpu);
    let region = Region::new(-5, 2, 33, 4);
    let out = engine.render(&g, node, "output", region).unwrap();
    assert_eq!(out.pixel(-5, 2)[0], -5.0 + 2000.0 + 0.5);
    assert_eq!(out.pixel(27, 5)[0], 27.0 + 5000.0 + 0.5);
    let n = engine.counters();
    assert_eq!((n.accelerated, n.uploads, n.fallbacks), (1, 1, 0), "{n:?}");
}

#[test]
fn vkdt_guided_kernels_on_the_gpu_match_their_cpu_reference() {
    use helios_module_vkdt::guided::{Blur, BlurPass, BlurSmall, Guided1, Guided2, Resample};
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let rgba = helios_testkit::noise(Region::new(-8, -6, 60, 44), 4, 3);
    let two = helios_testkit::noise(Region::new(-8, -6, 60, 44), 2, 4);
    // Regions off the origin, so the kernels' positions are absolute.
    let region = Region::new(-3, 2, 37, 23);
    let half = Region::new(-2, 1, 18, 11);
    let cases: Vec<(&str, ModuleRef, Params, &helios_core::Image, Region)> = vec![
        (
            "guided1",
            ModuleRef::new(Guided1),
            Params::new(),
            &rgba,
            region,
        ),
        (
            "guided2",
            ModuleRef::new(Guided2),
            Params::new().with("edges", Value::Float(0.5)),
            &two,
            region,
        ),
        ("blur", ModuleRef::new(Blur), Params::new(), &two, region),
        (
            "blur down",
            ModuleRef::new(Blur),
            Params::new().with("down", Value::Bool(true)),
            &two,
            half,
        ),
        (
            "blurs",
            ModuleRef::new(BlurSmall),
            Params::new().with("sigma", Value::Float(0.7)),
            &two,
            region,
        ),
        (
            "blurh",
            ModuleRef::new(BlurPass),
            Params::new().with("radius", Value::Float(4.5)),
            &two,
            region,
        ),
        (
            "blurv",
            ModuleRef::new(BlurPass),
            Params::new()
                .with("radius", Value::Float(2.4))
                .with("vertical", Value::Bool(true)),
            &two,
            region,
        ),
        (
            "resample",
            ModuleRef::new(Resample),
            Params::new().with("levels", Value::Int(2)),
            &two,
            Region::new(-21, 5, 40, 30),
        ),
    ];
    for (name, module, params, input, region) in cases {
        let c = check(&gpu, &module, &params, &Context::new(), input, region).unwrap();
        assert!(c.max_abs < 1e-4, "{name}: {c:?}");
    }
}

/// vkdt's contrast, a multi-kernel module, with an accelerator: its
/// kernels with a GPU variant run on the GPU (the image staying there from
/// one to the next), the two reading two images on the CPU, and the result
/// matches the CPU render.
#[test]
fn a_multi_kernel_module_runs_its_kernels_on_the_gpu() {
    use std::sync::Arc;

    use helios_core::{Bus, Engine, Graph};
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let input = helios_testkit::noise(Region::sized(128, 96), 4, 11);
    for radius in [0.01, 0.03] {
        let params = Params::new()
            .with("radius", Value::Float(radius))
            .with("edges", Value::Float(0.5))
            .with("detail", Value::Float(2.0));
        let mut g = Graph::new();
        let src = g.add_source("src", input.clone());
        let n = g
            .add_module(
                "contrast",
                ModuleRef::composite(helios_module_vkdt::VkdtContrast),
                &params,
            )
            .unwrap();
        g.connect(src, helios_core::graph::SOURCE_PORT, n, "input")
            .unwrap();
        let region = Region::new(10, 8, 100, 70);
        let cpu = Engine::new(Arc::new(Bus::new()))
            .render(&g, n, "output", region)
            .unwrap();
        let engine = Engine::new(Arc::new(Bus::new())).with_accelerator(gpu.clone());
        let out = engine.render(&g, n, "output", region).unwrap();
        let c = helios_gpu::compare(&out, &cpu).unwrap();
        assert!(c.max_abs < 1e-3, "radius {radius}: {c:?}");
        let k = engine.counters();
        // guided1, guided2 and the blurs; guided3 and combine on the CPU.
        assert!(
            k.accelerated >= 4 && k.fallbacks == 0,
            "radius {radius}: {k:?}"
        );
    }
}
