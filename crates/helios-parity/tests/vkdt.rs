//! Parity of the modules ported from vkdt with vkdt itself, through
//! vkdt-cli. Skipped without it.

use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, ModuleRef, Params, Region, Value};
use helios_parity::vkdt::Vkdt;
use helios_parity::{compare, inset};

fn helios(module: ModuleRef, params: Params, input: &Image) -> Image {
    let mut g = Graph::new();
    let src = g.add_source("src", input.clone());
    let n = g.add_module("m", module, &params).unwrap();
    g.connect(src, helios_core::graph::SOURCE_PORT, n, "input")
        .unwrap();
    Engine::new(Arc::new(Bus::new()))
        .render(&g, n, "output", input.region())
        .unwrap()
}

/// A case: vkdt's module name, Helios's port, the parameters, and the
/// border to leave out (vkdt mirrors at the edges, Helios replicates).
type Case = (&'static str, ModuleRef, Vec<(&'static str, f64)>, u32);

fn chart() -> Image {
    Image::from_fn(Region::sized(96, 64), 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        let edge = if (x / 9 + y / 7) % 2 == 0 { 0.6 } else { 0.15 };
        edge + ((x * 7 + y * 3 + i64::from(c) * 11) % 13) as f32 / 100.0
    })
}

#[test]
fn ported_vkdt_modules_match_vkdt() {
    let vkdt = match Vkdt::find() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let input = chart();
    let dir = std::env::temp_dir().join(format!("helios-parity-vkdt-{}", std::process::id()));
    let cases: [Case; 3] = [
        (
            "exposure",
            ModuleRef::new(helios_module_vkdt::VkdtExposure),
            vec![("exposure", 1.5)],
            0,
        ),
        (
            "usm",
            ModuleRef::new(helios_module_vkdt::VkdtUsm),
            vec![("amount", 2.0), ("thrs", 0.2)],
            2,
        ),
        (
            "usm",
            ModuleRef::new(helios_module_vkdt::VkdtUsm),
            vec![("amount", 0.8), ("thrs", 0.05)],
            2,
        ),
    ];
    for (i, (name, module, values, margin)) in cases.into_iter().enumerate() {
        let params = values
            .iter()
            .fold(Params::new(), |p, (k, v)| p.with(k, Value::Float(*v)));
        let ours = helios(module, params, &input);
        let cfg: Vec<(&str, String)> = values.iter().map(|(k, v)| (*k, v.to_string())).collect();
        let theirs = vkdt
            .render(&input, name, &cfg, &dir.join(i.to_string()))
            .unwrap();
        let d = compare(&inset(&ours, margin), &inset(&theirs, margin)).unwrap();
        eprintln!("vkdt {name} {values:?}: {d:?}");
        assert!(d.passes(), "{name} {values:?}: {d:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
