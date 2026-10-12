//! Test support for Helios modules and the framework.
//!
//! - [`ramp`] and [`noise`]: synthetic images, deterministic by construction.
//! - [`check_contract`] / [`assert_contract`]: tests generated from a module's
//!   manifest, run on every module the same way (see `docs/module-contract.md`,
//!   "How a module is tested").

use std::sync::Arc;

use helios_core::{
    Bus, Context, Engine, Error, Graph, Image, Manifest, Module, ModuleRef, NodeId, ParamKind,
    Params, Region, Value,
};

/// A smooth image: each channel grows along x and y, values in `[0, 1]`.
pub fn ramp(region: Region, channels: u32) -> Image {
    let (w, h) = (
        region.width.max(2) as f32 - 1.0,
        region.height.max(2) as f32 - 1.0,
    );
    Image::from_fn(region, channels, |x, y, c| {
        let fx = (x - region.x) as f32 / w;
        let fy = (y - region.y) as f32 / h;
        f32::midpoint(fx, fy * (c + 1) as f32 / channels as f32)
    })
}

/// Uniform noise in `[0, 1]`, the same for the same seed, region and channels.
pub fn noise(region: Region, channels: u32, seed: u64) -> Image {
    Image::from_fn(region, channels, |x, y, c| {
        let mut z = seed
            ^ (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
            ^ u64::from(c);
        // splitmix64 finalizer
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    })
}

/// The smallest region contract tests render: big enough to tile, small
/// enough to be fast. A module reading far around each pixel gets a larger
/// one, at least four times its reach on each axis, so that its tiles
/// really are tiles (see [`contract_region`]).
pub const CONTRACT_REGION: Region = Region::new(0, 0, 37, 23);

/// The largest side a contract region grows to (four times a reach of 32).
const MAX_CONTRACT_SIDE: u32 = 128;

/// The tile side contract tests cut regions into.
const CONTRACT_TILE: u32 = 16;

/// Runs the contract tests of a module; returns every failure found.
///
/// For the default parameters, for each float parameter at its minimum and
/// its maximum, and for every float parameter at its minimum, then at its
/// maximum, together (a combination the module refuses with an error when
/// it prepares is skipped), at full resolution and at half of it, it checks
/// that:
/// - every output is finite (no NaN, no infinity);
/// - rendering again with a fresh engine gives the same bits;
/// - rendering tile by tile gives the same bits as one render (the declared
///   overlap is enough). The region is at least four times what the module
///   reads around each pixel, and tiles are 16 pixels (or a quarter of a
///   larger region), so a tile never reads the whole region.
///
/// Then, with the default parameters, it feeds adversarial inputs
/// ([`ADVERSARIAL`]: negative values, zeros, huge values, NaN, and all of
/// them mixed): the module must not panic, its outputs keep their shape,
/// tiles still give the whole render, and inputs without NaN give finite
/// outputs. A module may refuse an input with an error instead.
pub fn check_contract<M: Module>(module: M) -> Result<(), Vec<String>> {
    check_contract_in(module, &Context::new())
}

/// [`check_contract`] for a module that reads context fields (a camera's
/// description): every render is made with `context`.
pub fn check_contract_in<M: Module>(module: M, context: &Context) -> Result<(), Vec<String>> {
    let module = ModuleRef::new(module);
    let manifest = module.manifest().clone();
    let mut failures = check_manifest(&manifest);
    for (label, params) in parameter_sets(&manifest) {
        // Extremes combined may be refused, plainly: no curve fits every
        // sigmoid target at its maximum.
        if label.starts_with("every float")
            && matches!(
                module.reads(&params, context, 1.0, CONTRACT_REGION),
                Err(Error::Module { .. })
            )
        {
            continue;
        }
        if let Err(e) = check_params(&module, &manifest, &params, context) {
            failures.push(format!("{label}: {e}"));
        }
    }
    for (label, fill) in ADVERSARIAL {
        if let Err(e) = check_adversarial(&module, &manifest, context, *fill) {
            failures.push(format!("{label} inputs: {e}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

/// [`check_contract`], panicking with the list of failures.
pub fn assert_contract<M: Module>(module: M) {
    assert_contract_in(module, &Context::new());
}

/// [`check_contract_in`], panicking with the list of failures.
pub fn assert_contract_in<M: Module>(module: M, context: &Context) {
    let name = module.manifest().name;
    if let Err(failures) = check_contract_in(module, context) {
        panic!(
            "module `{name}` fails its contract:\n- {}",
            failures.join("\n- ")
        );
    }
}

fn check_manifest(m: &Manifest) -> Vec<String> {
    let mut failures = Vec::new();
    if m.outputs.is_empty() {
        failures.push("no output port".into());
    }
    let mut names: Vec<_> = m.params.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    if names.windows(2).any(|w| w[0] == w[1]) {
        failures.push("duplicate parameter names".into());
    }
    if let Err(e) = m.resolve(&m.defaults()) {
        failures.push(format!("defaults do not validate: {e}"));
    }
    failures
}

fn parameter_sets(m: &Manifest) -> Vec<(String, Params)> {
    let mut sets = vec![("defaults".to_string(), Params::new())];
    let (mut all_min, mut all_max) = (Params::new(), Params::new());
    let mut floats = 0;
    for p in &m.params {
        if let ParamKind::Float { min, max, .. } = p.kind {
            sets.push((
                format!("{} = min {min}", p.name),
                Params::new().with(&p.name, Value::Float(min)),
            ));
            sets.push((
                format!("{} = max {max}", p.name),
                Params::new().with(&p.name, Value::Float(max)),
            ));
            all_min = all_min.with(&p.name, Value::Float(min));
            all_max = all_max.with(&p.name, Value::Float(max));
            floats += 1;
        }
    }
    // Extremes combined: every float at its minimum, then at its maximum.
    if floats > 1 {
        sets.push(("every float at its minimum".into(), all_min));
        sets.push(("every float at its maximum".into(), all_max));
    }
    sets
}

/// How an adversarial input changes a noise value `v` (in `[0, 1]`) at
/// position `i` (a running index over pixels and channels).
pub type Fill = fn(f32, u64) -> f32;

/// The adversarial inputs [`check_contract`] feeds, by name.
pub const ADVERSARIAL: &[(&str, Fill)] = &[
    ("negative", |v, _| -0.5 - v),
    ("zero", |_, _| 0.0),
    ("huge", |v, _| 1.0e6 * (1.0 + v)),
    ("NaN", |v, i| if i % 7 == 0 { f32::NAN } else { v }),
    ("mixed", |v, i| match i % 5 {
        0 => -0.5 - v,
        1 => 0.0,
        2 => 1.0e6 * (1.0 + v),
        3 => f32::NAN,
        _ => v,
    }),
];

/// Whether an adversarial input holds NaN (its outputs may then hold NaN).
fn holds_nan(fill: Fill) -> bool {
    (0..35).any(|i| fill(0.5, i).is_nan())
}

/// The region to test a module over with `params`: [`CONTRACT_REGION`],
/// grown to four times what the module reads around each pixel on each
/// axis, and the tile side to cut it into. A module reading more than 32
/// pixels around (a blur at its largest radius) keeps [`CONTRACT_REGION`]:
/// a region four times its reach would take minutes in a debug build, and
/// smaller parameters test its tiling.
fn contract_region(module: &ModuleRef, params: &Params, context: &Context) -> (Region, u32) {
    let r = CONTRACT_REGION;
    let (rx, ry) = module
        .reads(params, context, 1.0, r)
        .map_or((0, 0), |need| {
            (
                (r.x - need.x).max(need.right() - r.right()).max(0),
                (r.y - need.y).max(need.bottom() - r.bottom()).max(0),
            )
        });
    let side = |min: u32, reach: i64| match u32::try_from(4 * reach) {
        Ok(side) if side <= MAX_CONTRACT_SIDE => side.max(min),
        _ => min,
    };
    let region = Region::sized(side(r.width, rx), side(r.height, ry));
    let tile = CONTRACT_TILE.max(region.width.max(region.height).div_ceil(4));
    (region, tile)
}

fn network(
    module: &ModuleRef,
    manifest: &Manifest,
    params: &Params,
    context: &Context,
    region: Region,
    fill: Option<Fill>,
) -> Result<(Graph, NodeId), String> {
    let mut g = Graph::new();
    let node = g
        .add_module(&manifest.name, module.clone(), params)
        .map_err(|e| e.to_string())?;
    g.set_context(node, context.clone())
        .map_err(|e| e.to_string())?;
    for (i, port) in manifest.inputs.iter().enumerate() {
        let channels = port
            .channels()
            .ok_or_else(|| format!("input `{}` is not an image the engine runs yet", port.name))?;
        let mut image = noise(region.grow(16), channels, i as u64 + 1);
        if let Some(fill) = fill {
            let data: Vec<f32> = image
                .data()
                .iter()
                .zip(0u64..)
                .map(|(v, i)| fill(*v, i))
                .collect();
            image = Image::new(image.region(), channels, data);
        }
        let src = g.add_source(&port.name, image);
        g.connect(src, "image", node, &port.name)
            .map_err(|e| e.to_string())?;
    }
    Ok((g, node))
}

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

fn check_params(
    module: &ModuleRef,
    manifest: &Manifest,
    params: &Params,
    context: &Context,
) -> Result<(), String> {
    let (region, tile) = contract_region(module, params, context);
    let (g, node) = network(module, manifest, params, context, region, None)?;
    for scale in [1.0, 0.5] {
        check_at(&g, node, manifest, scale, region, tile)
            .map_err(|e| format!("at scale {scale}: {e}"))?;
    }
    Ok(())
}

/// Feeds an adversarial input to a module with its default parameters.
fn check_adversarial(
    module: &ModuleRef,
    manifest: &Manifest,
    context: &Context,
    fill: Fill,
) -> Result<(), String> {
    let params = Params::new();
    let (region, tile) = contract_region(module, &params, context);
    let (g, node) = network(module, manifest, &params, context, region, Some(fill))?;
    for port in &manifest.outputs {
        let render = |r: Region| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine().render(&g, node, &port.name, r)
            }))
            .map_err(|_| format!("output `{}`: the module panicked", port.name))
        };
        // Refusing the input with an error is allowed.
        let Ok(whole) = render(region)? else {
            continue;
        };
        if !holds_nan(fill)
            && let Some(i) = whole.data().iter().position(|v| !v.is_finite())
        {
            return Err(format!(
                "output `{}` has a non-finite value at index {i}",
                port.name
            ));
        }
        for t in region.tiles(tile) {
            let part = render(t)?.map_err(|e| e.to_string())?;
            if !same_bits(&part, &whole.sample(t)) {
                return Err(format!(
                    "output `{}`: tile {t} differs from the whole render",
                    port.name
                ));
            }
        }
    }
    Ok(())
}

fn check_at(
    g: &Graph,
    node: NodeId,
    manifest: &Manifest,
    scale: f64,
    full: Region,
    tile: u32,
) -> Result<(), String> {
    // The region and tiles at the render's scale.
    let region = Region::sized(
        ((f64::from(full.width) * scale).ceil() as u32).max(1),
        ((f64::from(full.height) * scale).ceil() as u32).max(1),
    );
    let tile = ((f64::from(tile) * scale).ceil() as u32).max(8);
    let tiles = region.tiles(tile);
    if tiles.len() < 2 {
        return Err(format!("{region} in tiles of {tile} is not tiled"));
    }
    for port in &manifest.outputs {
        let whole = engine()
            .render_at(g, node, &port.name, region, scale)
            .map_err(|e| e.to_string())?;
        if let Some(i) = whole.data().iter().position(|v| !v.is_finite()) {
            return Err(format!(
                "output `{}` has a non-finite value at index {i}",
                port.name
            ));
        }
        let again = engine()
            .render_at(g, node, &port.name, region, scale)
            .map_err(|e| e.to_string())?;
        if !same_bits(&whole, &again) {
            return Err(format!(
                "output `{}` differs between two renders",
                port.name
            ));
        }
        let tiled = engine();
        for tile in &tiles {
            let tile = *tile;
            let part = tiled
                .render_at(g, node, &port.name, tile, scale)
                .map_err(|e| e.to_string())?;
            if !same_bits(&part, &whole.sample(tile)) {
                return Err(format!(
                    "output `{}`: tile {tile} differs from the whole render",
                    port.name
                ));
            }
        }
    }
    Ok(())
}

fn same_bits(a: &Image, b: &Image) -> bool {
    a.region() == b.region()
        && a.data()
            .iter()
            .zip(b.data())
            .all(|(x, y)| x.to_bits() == y.to_bits())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_images_are_deterministic_and_in_range() {
        let r = Region::sized(8, 4);
        assert_eq!(noise(r, 3, 7).content_hash(), noise(r, 3, 7).content_hash());
        assert_ne!(noise(r, 3, 7).content_hash(), noise(r, 3, 8).content_hash());
        for img in [noise(r, 3, 7), ramp(r, 4)] {
            assert!(img.data().iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use helios_core::{Context, Family, ModuleKind, Needs, ParamSpec, PortSpec, Role};

    /// Divides by `d`: fine by default, infinite at `d = 0`, its declared minimum.
    struct Divide;

    impl Module for Divide {
        type Plan = f32;

        fn manifest(&self) -> Manifest {
            Manifest {
                name: "divide".into(),
                version: 1,
                description: String::new(),
                params: vec![ParamSpec::float("d", "divisor", 0.0, 4.0, 1.0)],
                inputs: vec![PortSpec::image("input", 1)],
                outputs: vec![PortSpec::image("output", 1)],
                context: vec![],
                needs: Needs::default(),
                kind: ModuleKind::Process,
                family: Family::Other,
                role: Role::General,
            }
        }

        fn prepare(&self, params: &Params, _: &Context) -> Result<f32, String> {
            Ok(params.float("d") as f32)
        }

        fn process(
            &self,
            d: &f32,
            inputs: &[&Image],
            output: Region,
        ) -> Result<Vec<Image>, String> {
            Ok(vec![Image::new(
                output,
                1,
                inputs[0].data().iter().map(|v| (v + 1.0) / d).collect(),
            )])
        }
    }

    #[test]
    fn the_contract_catches_non_finite_outputs_at_range_ends() {
        let failures = check_contract(Divide).unwrap_err();
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(
            failures[0]
                .starts_with("d = min 0: at scale 1: output `output` has a non-finite value"),
            "{failures:?}"
        );
    }

    /// Averages `reads` pixels on each side along x but declares
    /// `declares`; overflows on huge values and panics on NaN.
    struct Fragile {
        reads: i64,
        declares: u32,
    }

    impl Module for Fragile {
        type Plan = ();

        fn manifest(&self) -> Manifest {
            Manifest {
                name: "fragile".into(),
                version: 1,
                description: String::new(),
                params: vec![],
                inputs: vec![PortSpec::image("input", 1)],
                outputs: vec![PortSpec::image("output", 1)],
                context: vec![],
                needs: Needs {
                    overlap: self.declares,
                    ..Needs::default()
                },
                kind: ModuleKind::Process,
                family: Family::Other,
                role: Role::General,
            }
        }

        fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
            Ok(())
        }

        fn process(
            &self,
            (): &(),
            inputs: &[&Image],
            output: Region,
        ) -> Result<Vec<Image>, String> {
            let input = inputs[0];
            let r = input.region();
            Ok(vec![Image::from_fn(output, 1, |x, y, _| {
                let mut sum = 0.0f32;
                for d in -self.reads..=self.reads {
                    let v = input.pixel((x + d).clamp(r.x, r.right() - 1), y)[0];
                    assert!(!v.is_nan(), "NaN input");
                    let square = v * v;
                    sum += square * square * square * square;
                }
                sum
            })])
        }
    }

    #[test]
    fn adversarial_inputs_catch_overflows_and_panics() {
        let failures = check_contract(Fragile {
            reads: 1,
            declares: 1,
        })
        .unwrap_err();
        let has = |p: &str| failures.iter().any(|f| f.starts_with(p));
        assert!(
            has("huge inputs: output `output` has a non-finite value"),
            "{failures:?}"
        );
        assert!(
            has("NaN inputs: output `output`: the module panicked"),
            "{failures:?}"
        );
        assert!(has("mixed inputs:"), "{failures:?}");
        assert!(!has("negative") && !has("zero") && !has("defaults"));
    }

    #[test]
    fn a_reach_larger_than_declared_is_caught_by_real_tiles() {
        // Reads 9 pixels around but declares 8: only a tile edge inside
        // the region shows it.
        let failures = check_contract(Fragile {
            reads: 9,
            declares: 8,
        })
        .unwrap_err();
        assert!(
            failures
                .iter()
                .any(|f| f.starts_with("defaults: at scale 1: output `output`: tile")),
            "{failures:?}"
        );
        let (region, tile) = contract_region(
            &ModuleRef::new(Fragile {
                reads: 9,
                declares: 8,
            }),
            &Params::new(),
            &Context::new(),
        );
        assert_eq!((region, tile), (Region::sized(37, 32), 16));
    }
}
