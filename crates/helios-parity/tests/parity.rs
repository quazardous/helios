//! Parity of Helios's modules with darktable's, through darktable-cli.
//! Without darktable-cli these tests are skipped, saying so.

use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, ModuleRef, Params, Region, Value};
use helios_parity::{Darktable, Item, compare, delta_e_2000, read_pfm, write_pfm};
use serde_json::json;

#[test]
fn delta_e_2000_matches_published_pairs() {
    // Sharma, Wu and Dalal (2005), test data pairs 1, 7, 17 and 25.
    for (a, b, expected) in [
        ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
        ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
        ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
        (
            [60.2574, -34.0099, 36.2677],
            [60.4626, -34.1751, 39.4387],
            1.2644,
        ),
    ] {
        let d = delta_e_2000(a, b);
        assert!(
            (d - expected).abs() < 1e-4,
            "{a:?} {b:?}: {d}, expected {expected}"
        );
    }
}

/// A linear Rec.2020 test image: ramps past 1, a grey band, saturated
/// patches and a few negative values.
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

#[test]
fn pfm_round_trips() {
    let dir = std::env::temp_dir().join(format!("helios-pfm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let image = chart();
    write_pfm(&image, &dir.join("a.pfm")).unwrap();
    let back = read_pfm(&dir.join("a.pfm")).unwrap();
    assert_eq!(compare(&image, &back).unwrap().max_abs, 0.0);
    std::fs::remove_dir_all(&dir).unwrap();
}

fn helios_exposure(input: &Image, exposure: f64, black: f64) -> Image {
    let module = ModuleRef::new(helios_module_exposure::Exposure);
    let mut g = Graph::new();
    let src = g.add_source("src", input.clone());
    let params = Params::new()
        .with("exposure", Value::Float(exposure))
        .with("black", Value::Float(black));
    let n = g.add_module("exposure", module, &params).unwrap();
    g.connect(src, helios_core::graph::SOURCE_PORT, n, "input")
        .unwrap();
    Engine::new(Arc::new(Bus::new()))
        .render(&g, n, "output", input.region())
        .unwrap()
}

#[test]
fn exposure_matches_darktable() {
    let dt = match Darktable::find() {
        Ok(dt) => dt,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    eprintln!("against {}", dt.version);
    let layouts = dt.layouts();
    let input = chart();
    let dir = std::env::temp_dir().join(format!("helios-parity-{}", std::process::id()));
    let cases = [(0.0, 0.0), (1.0, 0.01), (-2.0, 0.05), (3.5, -0.02)];
    // Each darktable-cli run has a directory and a library of its own, and
    // most of its time is darktable starting: the four run at once.
    let renders: Vec<Image> = std::thread::scope(|scope| {
        let runs: Vec<_> = cases
            .iter()
            .map(|&(exposure, black)| {
                let (dt, input, layouts, dir) = (&dt, &input, &layouts, &dir);
                scope.spawn(move || {
                    dt.render(
                        input,
                        &[Item::new(
                            "exposure",
                            json!({"exposure": exposure, "black": black, "compensate_hilite_pres": false}),
                        )],
                        layouts,
                        &dir.join(format!("{exposure}_{black}")),
                    )
                    .unwrap()
                })
            })
            .collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    });
    for (&(exposure, black), theirs) in cases.iter().zip(renders) {
        if exposure != 0.0 {
            // darktable applied the history: an ignored module would pass vacuously.
            let moved = compare(&input, &theirs).unwrap();
            assert!(moved.max_abs > 0.1, "darktable left the image unchanged");
        }
        let ours = helios_exposure(&input, exposure, black);
        let d = compare(&ours, &theirs).unwrap();
        eprintln!("exposure {exposure}, black {black}: {d:?}");
        assert!(d.passes(), "exposure {exposure}, black {black}: {d:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Helios's exposure modules of an imported edit, chained in its order.
fn helios_imported_exposures(input: &Image, nodes: &[&helios_xmp::Node]) -> Image {
    let module = ModuleRef::new(helios_module_exposure::Exposure);
    let mut g = Graph::new();
    let mut prev = g.add_source("src", input.clone());
    let mut port = helios_core::graph::SOURCE_PORT;
    for (i, node) in nodes.iter().enumerate() {
        let n = g
            .add_module(&format!("exposure {i}"), module.clone(), &node.params)
            .unwrap();
        g.connect(prev, port, n, "input").unwrap();
        (prev, port) = (n, "output");
    }
    Engine::new(Arc::new(Bus::new()))
        .render(&g, prev, port, input.region())
        .unwrap()
}

/// Two exposure instances edited in one order and run in the other, as the
/// sidecar's `iop_order_list` says: the imported edit renders as darktable
/// does, and the history order would not.
#[test]
fn an_imported_edit_runs_in_darktables_pipe_order() {
    let dt = match Darktable::find() {
        Ok(dt) => dt,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let layouts = dt.layouts();
    let items = [
        Item::new(
            "exposure",
            json!({"exposure": 1.0, "black": 0.1, "compensate_hilite_pres": false}),
        ),
        Item::new(
            "exposure",
            json!({"exposure": 0.5, "black": -0.05, "compensate_hilite_pres": false}),
        )
        .instance(1),
    ];
    // darktable's v5.0 order with the second instance before the first.
    let list: Vec<String> = helios_xmp::iop_order::Preset::V50
        .table()
        .iter()
        .flat_map(|(_, op)| {
            if *op == "exposure" {
                vec!["exposure,1".to_string(), "exposure,0".to_string()]
            } else {
                vec![format!("{op},0")]
            }
        })
        .collect();
    let xmp = helios_parity::sidecar(&items, helios_parity::Input::LinearRec2020, &layouts)
        .unwrap()
        .replace(
            "darktable:history_end=",
            &format!(
                r#"darktable:iop_order_version="0" darktable:iop_order_list="{}" darktable:history_end="#,
                list.join(",")
            ),
        );
    let input = chart();
    let dir = std::env::temp_dir().join(format!("helios-parity-order-{}", std::process::id()));
    let theirs = dt.render_sidecar(&input, &xmp, &dir).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    let imported = helios_xmp::import(
        &helios_xmp::Sidecar::parse(&xmp).unwrap(),
        &helios_xmp::Layouts::built_in(),
    );
    let exposures: Vec<&helios_xmp::Node> = imported
        .nodes
        .iter()
        .filter(|n| n.module == "exposure")
        .collect();
    assert_eq!(exposures.len(), 2, "{:#?}", imported.report);
    assert_eq!(
        exposures[0].multi_name, "1",
        "the list runs instance 1 first"
    );
    let ours = helios_imported_exposures(&input, &exposures);
    let d = compare(&ours, &theirs).unwrap();
    eprintln!("pipe order: {d:?}");
    assert!(d.passes(), "{d:?}");
    // The history order computes another image: the case tells them apart.
    let history = helios_imported_exposures(&input, &[exposures[1], exposures[0]]);
    let wrong = compare(&history, &theirs).unwrap();
    eprintln!("history order: {wrong:?}");
    assert!(!wrong.passes(), "{wrong:?}");
}
