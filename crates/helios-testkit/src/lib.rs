//! Test support for Helios modules and the framework.
//!
//! - [`ramp`] and [`noise`]: synthetic images, deterministic by construction.
//! - [`check_contract`] / [`assert_contract`]: tests generated from a module's
//!   manifest, run on every module the same way (see `docs/module-contract.md`,
//!   "How a module is tested").

use std::sync::Arc;

use helios_core::{
    Bus, Engine, Graph, Image, Manifest, Module, ModuleRef, NodeId, ParamKind, Params, Region,
    Value,
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
        (fx + fy * (c + 1) as f32 / channels as f32) / 2.0
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

/// The region contract tests render: big enough to tile, small enough to be fast.
pub const CONTRACT_REGION: Region = Region::new(0, 0, 37, 23);

/// Runs the contract tests of a module; returns every failure found.
///
/// For the default parameters, and for each float parameter at its minimum
/// and its maximum, it checks that:
/// - every output is finite (no NaN, no infinity);
/// - rendering again with a fresh engine gives the same bits;
/// - rendering tile by tile gives the same bits as one render (the declared
///   overlap is enough).
pub fn check_contract<M: Module>(module: M) -> Result<(), Vec<String>> {
    let module = ModuleRef::new(module);
    let manifest = module.manifest().clone();
    let mut failures = check_manifest(&manifest);
    for (label, params) in parameter_sets(&manifest) {
        if let Err(e) = check_params(&module, &manifest, &params) {
            failures.push(format!("{label}: {e}"));
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
    let name = module.manifest().name;
    if let Err(failures) = check_contract(module) {
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
        }
    }
    sets
}

fn network(
    module: &ModuleRef,
    manifest: &Manifest,
    params: &Params,
) -> Result<(Graph, NodeId), String> {
    let mut g = Graph::new();
    let node = g
        .add_module(&manifest.name, module.clone(), params)
        .map_err(|e| e.to_string())?;
    for (i, port) in manifest.inputs.iter().enumerate() {
        let src = g.add_source(
            &port.name,
            noise(CONTRACT_REGION.grow(16), port.channels(), i as u64 + 1),
        );
        g.connect(src, "image", node, &port.name)
            .map_err(|e| e.to_string())?;
    }
    Ok((g, node))
}

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

fn check_params(module: &ModuleRef, manifest: &Manifest, params: &Params) -> Result<(), String> {
    let (g, node) = network(module, manifest, params)?;
    for port in &manifest.outputs {
        let whole = engine()
            .render(&g, node, &port.name, CONTRACT_REGION)
            .map_err(|e| e.to_string())?;
        if let Some(i) = whole.data().iter().position(|v| !v.is_finite()) {
            return Err(format!(
                "output `{}` has a non-finite value at index {i}",
                port.name
            ));
        }
        let again = engine()
            .render(&g, node, &port.name, CONTRACT_REGION)
            .map_err(|e| e.to_string())?;
        if !same_bits(&whole, &again) {
            return Err(format!(
                "output `{}` differs between two renders",
                port.name
            ));
        }
        let tiled = engine();
        for tile in CONTRACT_REGION.tiles(10) {
            let part = tiled
                .render(&g, node, &port.name, tile)
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
            failures[0].starts_with("d = min 0: output `output` has a non-finite value"),
            "{failures:?}"
        );
    }
}
