//! Parity of the modules ported from vkdt with vkdt itself, through
//! vkdt-cli. Skipped without it.

use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, ModuleRef, Params, Region, Value};
use helios_parity::vkdt::Vkdt;
use helios_parity::{DARKTABLE, compare, inset};

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
        assert!(d.within(&DARKTABLE), "{name} {values:?}: {d:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A larger chart for `contrast`, whose sides divide by 64: vkdt halves
/// odd sides rounding up, Helios's blurs halve exactly, and the two agree
/// only where every level divides by two.
fn contrast_chart() -> Image {
    Image::from_fn(Region::sized(256, 192), 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        let edge = if (x / 23 + y / 17) % 2 == 0 {
            0.55
        } else {
            0.12
        };
        let ramp = (x as f32 / 256.0) * 0.2;
        edge + ramp + ((x * 7 + y * 3 + i64::from(c) * 11) % 13) as f32 / 120.0
    })
}

/// vkdt's `contrast`, a multi-kernel module: Helios expands it into the
/// same network of kernels vkdt builds (a guided filter's kernels, its
/// blurs a cascade at lower resolutions, then `combine`). vkdt mirrors at
/// the edges and Helios replicates, and the blurs carry the difference
/// inwards: the border left out is the blurs' reach.
#[test]
fn ported_vkdt_contrast_matches_vkdt() {
    let vkdt = match Vkdt::find() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    // vkdt's contrast reads half floats: both read the same rounded pixels.
    let input = helios_parity::vkdt::to_f16(&contrast_chart());
    let dir = std::env::temp_dir().join(format!(
        "helios-parity-vkdt-contrast-{}",
        std::process::id()
    ));
    // radius 0.01: 2.56 pixels, a separable pass then one level; 0.03:
    // 7.68 pixels, two levels; edges and thrs raised: edges kept.
    //
    // vkdt's guided filter divides by `var + eps`, and next to an edge the
    // Catmull-Rom upsampling makes the variance overshoot below zero (to
    // -0.0014 on this chart at radius 0.01): where it crosses `-eps` the
    // model is singular in vkdt too, and the few pixels around depend on
    // rounding (vkdt keeps its buffers in half floats). At radius 0.01 and
    // the default edges (eps 9e-7), a spot by an edge differs by up to
    // 10 Delta E; edges 1.5 (eps 0.00225) keeps the variance above `-eps`.
    let cases: [(Vec<(&str, f64)>, u32); 3] = [
        (vec![("radius", 0.01), ("edges", 1.5), ("detail", 2.0)], 16),
        (vec![("radius", 0.03), ("detail", 2.0)], 32),
        (
            vec![
                ("radius", 0.03),
                ("edges", 0.5),
                ("detail", 1.5),
                ("thrs", 0.003),
            ],
            32,
        ),
    ];
    for (i, (values, margin)) in cases.into_iter().enumerate() {
        let params = values
            .iter()
            .fold(Params::new(), |p, (k, v)| p.with(k, Value::Float(*v)));
        let ours = helios(
            ModuleRef::composite(helios_module_vkdt::VkdtContrast),
            params,
            &input,
        );
        let cfg: Vec<(&str, String)> = values.iter().map(|(k, v)| (*k, v.to_string())).collect();
        let theirs = vkdt
            .render_f16(&input, "contrast", &cfg, &dir.join(i.to_string()))
            .unwrap();
        let whole = compare(&ours, &theirs).unwrap();
        let d = compare(&inset(&ours, margin), &inset(&theirs, margin)).unwrap();
        eprintln!("vkdt contrast {values:?}: inside {d:?}, whole image {whole:?}");
        assert!(d.within(&DARKTABLE), "contrast {values:?}: {d:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// One vkdt module (its name, and Helios's port) rendered by both and
/// compared on the whole image (point operations: no border). `cfg` gives the same
/// parameters as vkdt's cfg writes them; `f16` feeds vkdt half floats, for
/// a module reading `rgba:f16` (the input must be rounded already).
fn point_case(
    vkdt: &Vkdt,
    (name, module): (&str, ModuleRef),
    params: Params,
    cfg: &[(&str, String)],
    input: &Image,
    f16: bool,
    dir: &std::path::Path,
) {
    let ours = helios(module, params, input);
    let theirs = if f16 {
        vkdt.render_f16(input, name, cfg, dir)
    } else {
        vkdt.render(input, name, cfg, dir)
    }
    .unwrap();
    let d = compare(&ours, &theirs).unwrap();
    eprintln!("vkdt {name} {cfg:?}: {d:?}");
    assert!(d.within(&DARKTABLE), "{name} {cfg:?}: {d:?}");
}

/// A list parameter as vkdt's cfg writes it: values joined by colons.
fn cfg_list(values: &[f64]) -> String {
    values
        .iter()
        .map(f64::to_string)
        .collect::<Vec<_>>()
        .join(":")
}

#[test]
fn ported_vkdt_grade_matches_vkdt() {
    let vkdt = match Vkdt::find() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let input = chart();
    let dir = std::env::temp_dir().join(format!("helios-parity-vkdt-grade-{}", std::process::id()));
    let lift = [0.05, -0.02, 0.1, 0.01];
    let gamma = [1.2, 0.9, 1.0, 0.05];
    let gain = [1.1, 1.0, 0.8, 0.1];
    let offset = [0.0, 0.01, -0.02, 0.0];
    for (i, mode) in [0, 1].into_iter().enumerate() {
        let params = Params::new()
            .with("lift", Value::List(lift.to_vec()))
            .with("gamma", Value::List(gamma.to_vec()))
            .with("gain", Value::List(gain.to_vec()))
            .with("offset", Value::List(offset.to_vec()))
            .with(
                "mode",
                Value::Choice(if mode == 0 { "classic" } else { "zoned_log" }.into()),
            )
            .with("sh_pivot", Value::Float(0.35))
            .with("hi_pivot", Value::Float(0.6));
        let cfg = [
            ("lift", cfg_list(&lift)),
            ("gamma", cfg_list(&gamma)),
            ("gain", cfg_list(&gain)),
            ("offset", cfg_list(&offset)),
            ("mode", mode.to_string()),
            ("sh_pivot", "0.35".to_string()),
            ("hi_pivot", "0.6".to_string()),
        ];
        point_case(
            &vkdt,
            ("grade", ModuleRef::new(helios_module_vkdt::VkdtGrade)),
            params,
            &cfg,
            &input,
            false,
            &dir.join(i.to_string()),
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// vkdt's filmcurv writes `rgba:f16`: the difference to Helios's f32 is
/// the half floats' rounding, well within the thresholds.
#[test]
fn ported_vkdt_filmcurv_matches_vkdt() {
    let vkdt = match Vkdt::find() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    // Scene-referred: the chart's light spread up to about 2.
    let chart = chart();
    let input = Image::new(
        chart.region(),
        4,
        chart.data().iter().map(|v| v * 2.5).collect(),
    );
    let dir = std::env::temp_dir().join(format!(
        "helios-parity-vkdt-filmcurv-{}",
        std::process::id()
    ));
    // vkdt's colour modes: 1 per channel, 3 hsl.
    let cases = [
        (3.0, 1.2, 0.0, "hsl", 3),
        (1.5, 0.8, 0.01, "hsl", 3),
        (5.0, 1.6, 0.005, "per_channel", 1),
    ];
    for (i, (light, contrast, bias, colour, mode)) in cases.into_iter().enumerate() {
        let params = Params::new()
            .with("light", Value::Float(light))
            .with("contrast", Value::Float(contrast))
            .with("bias", Value::Float(bias))
            .with("colour", Value::Choice(colour.into()));
        let cfg = [
            ("light", light.to_string()),
            ("contrast", contrast.to_string()),
            ("bias", bias.to_string()),
            ("colour", mode.to_string()),
        ];
        point_case(
            &vkdt,
            ("filmcurv", ModuleRef::new(helios_module_vkdt::VkdtFilmcurv)),
            params,
            &cfg,
            &input,
            false,
            &dir.join(i.to_string()),
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
