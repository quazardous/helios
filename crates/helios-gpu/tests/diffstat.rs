//! The diffstat itself, without a GPU: two CPU engines report zero
//! everywhere (the sanity mode), and an accelerator that breaks one
//! module's kernel is caught at that module, its worst tile where it
//! broke it.

use std::sync::Arc;

use helios_core::{
    Accelerator, AcceleratorError, Bus, DeviceImage, Engine, Failure, GpuJob, Graph, Image,
    ModuleRef, Params, Region, Value,
};
use helios_gpu::diffstat::{self, Options};

/// A pipeline of three modules: identity exposure, a blur, an exposure.
fn pipeline() -> (Graph, Region) {
    let region = Region::sized(200, 130);
    let mut g = Graph::new();
    let src = g.add_source("src", helios_testkit::noise(region, 4, 3));
    let ident = Params::new()
        .with("exposure", Value::Float(0.0))
        .with("black", Value::Float(0.0));
    let e = g
        .add_module(
            "first",
            ModuleRef::new(helios_module_exposure::Exposure),
            &ident,
        )
        .unwrap();
    let b = g
        .add_module(
            "blur",
            ModuleRef::new(helios_module_blur::Blur),
            &Params::new().with("radius", Value::Float(1.5)),
        )
        .unwrap();
    let l = g
        .add_module(
            "last",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new().with("exposure", Value::Float(1.0)),
        )
        .unwrap();
    g.connect(src, "image", e, "input").unwrap();
    g.connect(e, "output", b, "input").unwrap();
    g.connect(b, "output", l, "input").unwrap();
    (g, region)
}

fn cpu() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

#[test]
fn two_cpu_engines_report_zero_for_every_module() {
    let (g, region) = pipeline();
    let diffs = diffstat::run(&g, region, &cpu(), &cpu(), Options::default(), None).unwrap();
    let names: Vec<_> = diffs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["first", "blur", "last"]);
    for d in &diffs {
        let (alone, chain) = (d.alone.as_ref().unwrap(), d.chain.as_ref().unwrap());
        assert!(alone.is_zero() && chain.is_zero(), "{}: {d:?}", d.name);
        assert_eq!((alone.mean_abs, alone.over), (0.0, 0.0));
    }
    let only = diffstat::run(&g, region, &cpu(), &cpu(), Options::default(), Some("blur")).unwrap();
    assert_eq!(only.len(), 1);
}

/// Where the broken kernel adds its error.
const BUMP: Region = Region::new(130, 70, 8, 8);

/// An accelerator whose exposure kernel is the identity plus 0.5 over
/// [`BUMP`]: right for the identity exposure, wrong there. Any other
/// kernel fails, so the engine runs that step on the CPU.
struct Broken;

impl Accelerator for Broken {
    fn id(&self) -> &'static str {
        "broken"
    }

    fn upload(&self, image: &Image) -> Result<DeviceImage, AcceleratorError> {
        Ok(DeviceImage::new(
            image.region(),
            image.channels(),
            image.clone(),
        ))
    }

    fn download(&self, image: &DeviceImage) -> Result<Image, AcceleratorError> {
        image
            .handle::<Image>()
            .cloned()
            .ok_or_else(|| "not ours".into())
    }

    fn run(
        &self,
        module: &str,
        _job: &GpuJob,
        input: &DeviceImage,
        output: Region,
        _channels: u32,
    ) -> Result<DeviceImage, AcceleratorError> {
        if module != "exposure" {
            return Err(AcceleratorError::new(Failure::Compile, "no kernel here"));
        }
        let src = input.handle::<Image>().ok_or("not ours")?.sample(output);
        let out = Image::from_fn(output, src.channels(), |x, y, c| {
            let v = src.pixel(x, y)[c as usize];
            let inside =
                (BUMP.x..BUMP.right()).contains(&x) && (BUMP.y..BUMP.bottom()).contains(&y);
            if inside { v + 0.5 } else { v }
        });
        Ok(DeviceImage::new(output, out.channels(), out))
    }
}

#[test]
fn a_broken_kernel_is_caught_at_its_module_and_tile() {
    let (g, region) = pipeline();
    let broken = cpu().with_accelerator(Arc::new(Broken));
    let options = Options {
        threshold: 1.0 / 255.0,
        tile: 32,
    };
    let diffs = diffstat::run(&g, region, &cpu(), &broken, options, None).unwrap();
    let by = |n: &str| diffs.iter().find(|d| d.name == n).unwrap();

    // The broken exposure, alone: 0.5 over the bump's 64 pixels only.
    let first = by("first").alone.clone().unwrap();
    assert!((first.max_abs - 0.5).abs() < 1e-6, "{first:?}");
    let share = 64.0 / region.area() as f64;
    assert!((first.over - share).abs() < 1e-9, "{first:?}");
    assert!(first.worst_tile.overlaps(&BUMP), "{first:?}");
    assert_eq!(first.worst_tile, Region::new(128, 64, 32, 32));

    // The blur runs on the CPU on both sides: alone, nothing; through the
    // chain, the error it inherits, spread around the bump.
    let blur = by("blur");
    assert!(blur.alone.as_ref().unwrap().is_zero(), "{blur:?}");
    let chain = blur.chain.as_ref().unwrap();
    assert!(chain.max_abs > 0.0 && chain.over > share, "{chain:?}");
    assert!(chain.worst_tile.overlaps(&BUMP), "{chain:?}");

    // The last exposure (×2) is broken the same way on its own input.
    let last = by("last").alone.clone().unwrap();
    assert!(last.max_abs > 0.4, "{last:?}");

    // The diff image puts the distance where the bump is.
    let (alone, n) = diffstat::alone(&g, by("first").node, region).unwrap();
    let r = cpu().render(&alone, n, "output", region).unwrap();
    let o = broken.render(&alone, n, "output", region).unwrap();
    let d = diffstat::diff_image(&r, &o).unwrap();
    assert_eq!(d.channels(), 1);
    assert!((d.pixel(BUMP.x, BUMP.y)[0] - 0.5).abs() < 1e-6);
    assert_eq!(d.pixel(0, 0)[0], 0.0);
}

#[test]
fn a_value_that_turns_not_finite_is_an_infinite_distance() {
    let region = Region::sized(4, 4);
    let a = Image::from_fn(region, 1, |_, _, _| 1.0);
    let b = Image::from_fn(
        region,
        1,
        |x, y, _| if (x, y) == (3, 3) { f32::NAN } else { 1.0 },
    );
    let s = diffstat::measure(&a, &b, Options::default()).unwrap();
    assert!(s.max_abs.is_infinite());
    assert!((s.over - 1.0 / 16.0).abs() < 1e-12);
}
