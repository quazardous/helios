//! Parity of Helios's raw chain with darktable's, on darktable's own
//! integration images (`HELIOS_TEST_IMAGES`, else a darktable checkout next
//! to Helios's). Skipped without darktable-cli or the images.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, ModuleRef, Params};
use helios_parity::{Darktable, Item, align, compare, reduce};
use serde_json::json;

fn images() -> PathBuf {
    std::env::var_os("HELIOS_TEST_IMAGES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../darktable/src/tests/integration/images")
        })
}

/// rawprepare → white balance as shot → highlights clip → demosaic →
/// colorin (standard matrix, linear Rec.2020), over the whole image.
fn helios(raw: &Path) -> Image {
    let loaded = helios_load::load(raw).unwrap();
    let camera = loaded.camera.unwrap().to_context();
    let mut g = Graph::new();
    let mut prev = g.add_source("raw", loaded.image.clone());
    let mut port = helios_core::graph::SOURCE_PORT.to_string();
    let chain: [(&str, ModuleRef); 5] = [
        (
            "rawprepare",
            ModuleRef::new(helios_module_rawprepare::RawPrepare),
        ),
        (
            "whitebalance",
            ModuleRef::new(helios_module_whitebalance::WhiteBalance),
        ),
        (
            "highlights",
            ModuleRef::new(helios_module_highlights::Highlights),
        ),
        ("demosaic", ModuleRef::new(helios_module_demosaic::Demosaic)),
        ("colorin", ModuleRef::new(helios_module_colorin::ColorIn)),
    ];
    for (name, module) in chain {
        let n = g.add_module(name, module.clone(), &Params::new()).unwrap();
        g.set_context(n, camera.clone()).unwrap();
        g.connect(prev, &port, n, &module.manifest().inputs[0].name)
            .unwrap();
        prev = n;
        port = module.manifest().outputs[0].name.clone();
    }
    Engine::new(Arc::new(Bus::new()))
        .render(&g, prev, &port, loaded.image.region())
        .unwrap()
}

fn darktable(dt: &Darktable, raw: &Path, method: &str) -> Image {
    let dir = std::env::temp_dir().join(format!(
        "helios-parity-raw-{}-{}",
        std::process::id(),
        raw.file_stem().unwrap().to_string_lossy()
    ));
    let items = [
        Item::new(
            "highlights",
            json!({"mode": "DT_IOP_HIGHLIGHTS_CLIP", "clip": 1.0}),
        ),
        Item::new("demosaic", json!({"demosaicing_method": method})),
    ];
    let out = dt.render_raw(raw, &items, &dt.layouts(), &dir).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// Reports the difference at full size and at 1/8 (colour alone, detail
/// averaged out). It asserts nothing yet: Helios's demosaic is bilinear,
/// darktable's RCD and Markesteijn, and closing that gap is the work this
/// report measures.
#[test]
fn raw_chain_against_darktable_is_reported() {
    let Ok(dt) = Darktable::find() else {
        eprintln!("skipped: no darktable-cli");
        return;
    };
    for (file, method) in [
        ("mire1.cr2", "DT_IOP_DEMOSAIC_RCD"),
        ("mire1-xtrans.raf", "DT_IOP_DEMOSAIC_MARKESTEIJN"),
    ] {
        let raw = images().join(file);
        if !raw.is_file() {
            eprintln!("skipped: no {}", raw.display());
            continue;
        }
        let ours = helios(&raw);
        let theirs = darktable(&dt, &raw, method);
        eprintln!(
            "{file}: helios {} ({} ch), darktable {}",
            ours.region(),
            ours.channels(),
            theirs.region()
        );
        let (dx, dy, ours, theirs) = align(&ours, &theirs, 24);
        // HELIOS_PARITY_DUMP=dir keeps both aligned renders, to study them.
        if let Some(dir) = std::env::var_os("HELIOS_PARITY_DUMP") {
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            helios_parity::write_pfm(&ours, &dir.join(format!("{file}.helios.pfm"))).unwrap();
            helios_parity::write_pfm(&theirs, &dir.join(format!("{file}.darktable.pfm"))).unwrap();
        }
        let full = compare(&ours, &theirs).unwrap();
        let coarse = compare(&reduce(&ours, 8), &reduce(&theirs, 8)).unwrap();
        eprintln!("{file} {method}: shift ({dx}, {dy}), full {full:?}, 1/8 {coarse:?}");
    }
}

#[test]
#[ignore = "diagnostic: prints the camera description Helios reads"]
fn print_camera() {
    for file in ["mire1.cr2", "mire1-xtrans.raf"] {
        let raw = images().join(file);
        if let Ok(l) = helios_load::load(&raw) {
            eprintln!("{file}: {:?}", l.camera.unwrap().to_context());
        }
    }
}
