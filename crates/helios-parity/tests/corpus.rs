//! Real darktable sidecars from several eras (`tests/darktable-sidecars`,
//! where they come from is in its README), checked against darktable-cli:
//! darktable runs the modules the import keeps, in the import's order; and
//! where Helios has the modules, Helios renders the import as darktable
//! renders the same history with the items Helios does not apply turned
//! off. Skipped, saying so, without darktable-cli or darktable's test
//! images (`HELIOS_TEST_IMAGES`, else a darktable checkout next to
//! Helios's).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, ModuleRef, Value as Param};
use helios_parity::{
    DEMOSAIC_MARGIN, Darktable, Input, Item, align, compare, history_line, inset, linear_ends,
    reduce,
};
use helios_xmp::{Imported, Layouts, Sidecar, import_text};
use serde_json::{Value, json};

/// Each sidecar of the corpus and the darktable test image it is for.
const CORPUS: &[(&str, &str)] = &[
    ("0001-exposure", "mire1.cr2"),
    ("0004-masks", "mire1.cr2"),
    ("0033-blending-modes-uniform", "mire1.cr2"),
    ("0035-multiple-modules", "mire1.cr2"),
    ("0097-sigmoid", "mire1.cr2"),
    ("0151-sigmoid-no-hue-preservation", "mire1.cr2"),
    ("0180-spektrafilm-simple", "hlrecovery.arw"),
];

fn corpus(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/darktable-sidecars")
        .join(format!("{name}.xmp"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn images() -> PathBuf {
    std::env::var_os("HELIOS_TEST_IMAGES").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../darktable/src/tests/integration/images")
        },
        PathBuf::from,
    )
}

/// darktable-cli and the images, or why not.
fn setup() -> Option<(Darktable, PathBuf)> {
    let dt = match Darktable::find() {
        Ok(dt) => dt,
        Err(e) => {
            eprintln!("skipped: {e}");
            return None;
        }
    };
    let dir = images();
    if CORPUS.iter().any(|(_, raw)| !dir.join(raw).is_file()) {
        eprintln!("skipped: no darktable test images in {}", dir.display());
        return None;
    }
    Some((dt, dir))
}

/// Sidecars that use modules only a newer darktable has, and the release
/// they need (darktable 5.6 crashes on spektrafilm's sidecar).
const NEEDS: &[(&str, (u32, u32))] = &[("0180-spektrafilm-simple", (5, 7))];

/// The corpus this darktable can run; the others are named as skipped.
fn runnable(dt: &Darktable) -> Vec<(&'static str, &'static str)> {
    let mut numbers = dt
        .version
        .trim_start_matches("darktable ")
        .split(['.', '+', '~', '-'])
        .map(|n| n.parse::<u32>().unwrap_or(0));
    let version = (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0));
    CORPUS
        .iter()
        .copied()
        .filter(|(name, _)| match NEEDS.iter().find(|(n, _)| n == name) {
            Some((_, needed)) if version < *needed => {
                eprintln!(
                    "skipped {name}: needs darktable {}.{}, found {}",
                    needed.0, needed.1, dt.version
                );
                false
            }
            _ => true,
        })
        .collect()
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("helios-corpus-{}-{name}", std::process::id()))
}

/// darktable's name for a module instance in its pipe debug output.
fn instance_id(operation: &str, instance: u32) -> String {
    match instance {
        0 => operation.to_string(),
        1..=6 => format!("{operation}.{instance}"),
        _ => format!("{operation}.x"),
    }
}

/// Modules darktable runs that no history holds: its pipe's own steps.
const PIPE_STEPS: &[&str] = &["finalscale"];

#[test]
fn darktable_runs_what_the_import_keeps_in_the_same_order() {
    let Some((dt, dir)) = setup() else { return };
    eprintln!("against {}", dt.version);
    let layouts = Layouts::built_in();
    let cases = runnable(&dt);
    let runs: Vec<(String, Vec<String>, Vec<String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .iter()
            .map(|&(name, raw)| {
                let (dt, dir, layouts) = (&dt, &dir, &layouts);
                scope.spawn(move || {
                    let xmp = corpus(name);
                    let (sidecar, _) = import_text(&xmp, layouts).unwrap();
                    // Every item in force and enabled, applied or kept.
                    let ours: Vec<String> = sidecar
                        .in_pipe_order()
                        .items
                        .iter()
                        .filter(|i| i.enabled)
                        .map(|i| instance_id(&i.operation, i.multi_priority))
                        .collect();
                    let work = scratch(&format!("order-{name}"));
                    let theirs = dt
                        .processed(&dir.join(raw), &xmp, &work)
                        .unwrap_or_else(|e| panic!("{name}: {e}"));
                    let _ = std::fs::remove_dir_all(&work);
                    (name.to_string(), ours, theirs)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (name, ours, theirs) in runs {
        eprintln!("{name}\n  helios:    {ours:?}\n  darktable: {theirs:?}");
        // Nothing darktable runs is missing from the import.
        let missing: Vec<&String> = theirs
            .iter()
            .filter(|m| !ours.contains(m) && !PIPE_STEPS.contains(&m.as_str()))
            .collect();
        assert!(missing.is_empty(), "{name}: not imported {missing:?}");
        // What both run, they run in the same order.
        let ours_run: Vec<&String> = ours.iter().filter(|m| theirs.contains(m)).collect();
        let theirs_ours: Vec<&String> = theirs.iter().filter(|m| ours.contains(m)).collect();
        assert_eq!(ours_run, theirs_ours, "{name}");
    }
}

/// A history item's attributes as an `rdf:li`.
fn li(attributes: &serde_json::Map<String, Value>) -> String {
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let attrs: Vec<String> = attributes
        .iter()
        .map(|(k, v)| format!(r#"{k}="{}""#, escape(v.as_str().unwrap_or_default())))
        .collect();
    format!("<rdf:li {}/>", attrs.join(" "))
}

/// The sidecar's history as darktable should render it to match Helios's
/// import: the items Helios does not apply turned off, the undone ones
/// left out, then what Helios runs in place of darktable's choices: its
/// colorin into linear Rec.2020 and a linear Rec.2020 output, RCD (Helios
/// does not have darktable's other Bayer methods), and highlights' clip
/// method (the only one Helios has; the import reports any other). The
/// items darktable adds itself when it reads the file are left to it.
fn reference(sidecar: &Sidecar, imported: &Imported, layouts: &Layouts) -> String {
    let mut lines = Vec::new();
    for item in imported.darktable["history"].as_array().unwrap() {
        if item["inserted"] == true || item["status"] == "undone" {
            continue;
        }
        let mut attributes = item["attributes"].as_object().unwrap().clone();
        attributes.insert("darktable:num".into(), json!(lines.len().to_string()));
        if item["status"] == "not_applied" {
            attributes.insert("darktable:enabled".into(), json!("0"));
        }
        lines.push(li(&attributes));
    }
    let (colorin, colorout) = linear_ends(Input::Camera);
    let mut extra = vec![
        colorin,
        colorout,
        Item::new(
            "demosaic",
            json!({"demosaicing_method": "DT_IOP_DEMOSAIC_RCD"}),
        ),
    ];
    if let Some(h) = imported.nodes.iter().find(|n| n.module == "highlights") {
        let clip = match h.params.get("clip") {
            Some(Param::Float(c)) => *c,
            _ => 1.0,
        };
        extra.push(Item::new(
            "highlights",
            json!({"mode": "DT_IOP_HIGHLIGHTS_CLIP", "clip": clip}),
        ));
    }
    for item in &extra {
        lines.push(history_line(lines.len(), item, layouts).unwrap());
    }
    let mut description = format!(
        r#"darktable:xmp_version="{}" darktable:auto_presets_applied="1" darktable:history_end="{}""#,
        sidecar.xmp_version.unwrap_or(0),
        lines.len()
    );
    if let Some(v) = sidecar.iop_order_version {
        let _ = write!(description, r#" darktable:iop_order_version="{v}""#);
    }
    if let Some(list) = &sidecar.iop_order_list {
        let _ = write!(description, r#" darktable:iop_order_list="{list}""#);
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/" {description}>
   <darktable:history>
    <rdf:Seq>
     {}
    </rdf:Seq>
   </darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#,
        lines.join("\n     ")
    )
}

/// A Helios module by the name the import gives it.
fn module(name: &str) -> ModuleRef {
    match name {
        "rawprepare" => ModuleRef::new(helios_module_rawprepare::RawPrepare),
        "whitebalance" => ModuleRef::new(helios_module_whitebalance::WhiteBalance),
        "highlights" => ModuleRef::new(helios_module_highlights::Highlights),
        "demosaic" => ModuleRef::new(helios_module_demosaic::Demosaic),
        "colorin" => ModuleRef::new(helios_module_colorin::ColorIn),
        "exposure" => ModuleRef::new(helios_module_exposure::Exposure),
        "sigmoid" => ModuleRef::new(helios_module_sigmoid::Sigmoid),
        other => panic!("the import gave a module Helios has not: {other}"),
    }
}

/// The imported nodes chained after the raw, over the whole image.
fn helios(raw: &Path, imported: &Imported) -> Image {
    let loaded = helios_load::load(raw).unwrap();
    let camera = loaded.camera.unwrap().to_context();
    let mut g = Graph::new();
    let mut prev = g.add_source("raw", loaded.image.clone());
    let mut port = helios_core::graph::SOURCE_PORT.to_string();
    for node in &imported.nodes {
        let module = module(&node.module);
        let n = g
            .add_module(&node.name, module.clone(), &node.params)
            .unwrap();
        g.set_context(n, camera.clone()).unwrap();
        g.connect(prev, &port, n, &module.manifest().inputs[0].name)
            .unwrap();
        prev = n;
        port.clone_from(&module.manifest().outputs[0].name);
    }
    Engine::new(Arc::new(Bus::new()))
        .render(&g, prev, &port, loaded.image.region())
        .unwrap()
}

#[test]
fn where_helios_has_the_modules_it_renders_the_import_as_darktable_does() {
    let Some((dt, dir)) = setup() else { return };
    let layouts = dt.layouts();
    for (name, raw) in runnable(&dt) {
        let raw = dir.join(raw);
        let (sidecar, imported) = import_text(&corpus(name), &Layouts::built_in()).unwrap();
        let xmp = reference(&sidecar, &imported, &layouts);
        let work = scratch(&format!("render-{name}"));
        let theirs = dt
            .render_raw_sidecar(&raw, &xmp, &work)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let _ = std::fs::remove_dir_all(&work);
        let ours = helios(&raw, &imported);
        let (dx, dy, ours, theirs) = align(&ours, &theirs, 24);
        let (ours, theirs) = (
            inset(&ours, DEMOSAIC_MARGIN),
            inset(&theirs, DEMOSAIC_MARGIN),
        );
        let full = compare(&ours, &theirs).unwrap();
        let coarse = compare(&reduce(&ours, 8), &reduce(&theirs, 8)).unwrap();
        let nodes: Vec<&str> = imported.nodes.iter().map(|n| n.name.as_str()).collect();
        eprintln!("{name}: {nodes:?}, shift ({dx}, {dy}), full {full:?}, 1/8 {coarse:?}");
        assert!(full.passes_as_a_port(), "{name}: {full:?}");
    }
}
