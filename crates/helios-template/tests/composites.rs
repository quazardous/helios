//! The composite files Helios ships (`templates/composites/`): each one
//! fits the schema, is named after its id and version, builds as a macro,
//! and a template referencing it builds and renders.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui_pop_model::layout::Layout;
use helios_core::{Bus, Engine, ModuleRef, Region};
use helios_template::{Bundle, Catalogue, CompositeFile, CompositeRef, parse_composite, schema};
use serde_json::json;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates/composites")
}

fn files() -> Vec<(PathBuf, String)> {
    let mut v: Vec<(PathBuf, String)> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".helios-composite.json"))
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            (p, text)
        })
        .collect();
    v.sort();
    v
}

/// The modules the shipped composites use.
fn catalogue() -> Catalogue {
    use helios_module_masks as m;
    let mut c = Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(m::MaskCombine))
        .module(ModuleRef::new(m::MaskFeather))
        .module(ModuleRef::new(m::MaskBlur))
        .module(ModuleRef::new(m::MaskLevels))
        .module(ModuleRef::new(m::MaskBlend));
    for (_, text) in files() {
        c.add_composite(parse_composite(&text).unwrap());
    }
    c
}

#[test]
fn every_shipped_composite_fits_its_schema_and_builds() {
    let files = files();
    assert!(!files.is_empty());
    let catalogue = catalogue();
    for (path, text) in files {
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let problems = schema::check(&schema::composite(), &json);
        assert!(problems.is_empty(), "{}: {problems:?}", path.display());
        let file: CompositeFile = parse_composite(&text).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            file.file_name(),
            "named after its id and version"
        );
        let built = catalogue.build_composite(&file.reference(), 0).unwrap();
        assert_eq!(built.network().nodes().len(), file.nodes.len());
    }
}

#[test]
fn the_masked_adjustment_exposes_its_group_and_hints_the_masks_panel() {
    let catalogue = catalogue();
    let r = CompositeRef {
        id: "helios.masked-adjustment".into(),
        version: 1,
    };
    let file = catalogue.get_composite(&r).unwrap();
    assert_eq!(file.panel.as_ref().unwrap().panel, "helios.masks");
    let module = ModuleRef::composite(catalogue.build_composite(&r, 0).unwrap());
    let names: Vec<&str> = module
        .manifest()
        .params
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, ["operator", "feather", "opacity"]);
}

#[test]
fn a_template_using_it_builds_and_renders_the_adjustment_through_an_empty_mask() {
    let template = helios_template::parse_template(
        &json!({
            "format": 1,
            "id": "test.masked",
            "version": 1,
            "meta": { "name": "Masked", "scenario": "An exposure through a mask" },
            "network": {
                "sources": ["source"],
                "nodes": [
                    { "name": "exposure", "module": "exposure",
                      "params": { "exposure": { "float": 1.0 } } },
                    { "name": "folded",
                      "composite": { "id": "helios.masked-adjustment", "version": 1 } },
                    { "name": "retouch",
                      "composite": { "id": "helios.masked-adjustment", "version": 1 },
                      "open": true }
                ],
                "connections": [
                    { "from": "source", "port": "image", "to": "exposure", "input": "input" },
                    { "from": "source", "port": "image", "to": "folded", "input": "input" },
                    { "from": "exposure", "port": "output", "to": "folded", "input": "processed" },
                    { "from": "source", "port": "image", "to": "retouch", "input": "input" },
                    { "from": "exposure", "port": "output", "to": "retouch", "input": "processed" }
                ],
                "view": { "node": "retouch", "port": "output" }
            }
        })
        .to_string(),
    )
    .unwrap();
    let bundle = Bundle::new(template, Layout::default());
    let report = bundle.validate(&catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    let built = bundle.build(&catalogue()).unwrap();
    // Opened: the group, its feather and its blend, which the Masks panel
    // and the viewport edit.
    for inner in ["retouch.mask", "retouch.feather", "retouch.blend"] {
        assert!(built.nodes.contains_key(inner), "{inner}");
    }
    assert_eq!(built.view.0, built.nodes["retouch.blend"]);
    // The masks panel docked for the composite's hint.
    let masks = egui_pop_model::ids::PanelId::new("helios.masks").unwrap();
    assert_eq!(
        built.layout.area_of(&masks),
        Some(egui_pop_model::panels::Area::StartSide)
    );
    // A group with no member is 1: the adjustment applies fully.
    let engine = Engine::new(Arc::new(Bus::new()));
    let region = Region::sized(8, 8);
    let exposed = engine
        .render(&built.graph, built.nodes["exposure"], "output", region)
        .unwrap()
        .to_vec();
    for node in ["folded", "retouch.blend"] {
        let out = engine
            .render(&built.graph, built.nodes[node], "output", region)
            .unwrap()
            .to_vec();
        for (a, b) in out.iter().zip(&exposed) {
            assert!((a - b).abs() < 1e-4, "{node}: {a} against {b}");
        }
    }
}

/// The masked adjustment with an interface: its opacity renamed, labelled,
/// narrowed and grouped, a soft range and a default of its own.
fn with_interface(exposed: serde_json::Value) -> String {
    let mut file: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir().join("helios.masked-adjustment@1.helios-composite.json"))
            .unwrap(),
    )
    .unwrap();
    file["id"] = json!("test.look");
    file["exposed"] = exposed;
    file.to_string()
}

#[test]
fn an_interface_in_the_file_reaches_the_macro_and_round_trips() {
    let text = with_interface(json!([
        { "name": "strength", "node": "blend", "param": "opacity",
          "label": "Strength", "description": "How much of the adjustment",
          "group": "Look", "min": 0.2, "max": 0.8, "soft": [0.3, 0.7],
          "default": { "float": 0.5 } },
        { "name": "feather", "node": "feather", "param": "radius",
          "unit": "px", "group": "Edges" }
    ]));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let problems = schema::check(&schema::composite(), &json);
    assert!(problems.is_empty(), "{problems:?}");
    let file = parse_composite(&text).unwrap();
    // Written back, read again: the same file.
    let again = parse_composite(&serde_json::to_string(&file).unwrap()).unwrap();
    assert_eq!(again, file);
    let mut catalogue = catalogue();
    catalogue.add_composite(file.clone());
    let built = catalogue.build_composite(&file.reference(), 0).unwrap();
    let module = ModuleRef::composite(built);
    let strength = &module.manifest().params[0];
    assert_eq!(strength.name, "strength");
    assert_eq!(strength.description, "How much of the adjustment");
    assert_eq!(
        strength.kind,
        helios_core::ParamKind::Float {
            min: 0.2,
            max: 0.8,
            default: 0.5
        }
    );
    let hints = module.param_hints();
    let h = hints.iter().find(|h| h.name == "strength").unwrap();
    assert_eq!(h.label.as_deref(), Some("Strength"));
    assert_eq!(h.group.as_deref(), Some("Look"));
    assert_eq!(h.soft, Some((0.3, 0.7)));
    // The inner hint's unit and display scale, kept.
    assert_eq!(h.unit.as_deref(), Some("%"));
    assert_eq!(h.display_scale, Some(100.0));
    let f = hints.iter().find(|h| h.name == "feather").unwrap();
    assert_eq!(f.unit.as_deref(), Some("px"));
    assert_eq!(f.group.as_deref(), Some("Edges"));
}

#[test]
fn an_interface_that_widens_the_inner_range_is_refused_by_entry() {
    let text = with_interface(json!([
        { "name": "strength", "node": "blend", "param": "opacity", "max": 2.0 }
    ]));
    let file = parse_composite(&text).unwrap();
    let mut catalogue = catalogue();
    catalogue.add_composite(file.clone());
    let Err(e) = catalogue.build_composite(&file.reference(), 0) else {
        panic!("a wider range was accepted");
    };
    assert!(e.contains("exposed[0]") && e.contains("not within"), "{e}");
    // An unknown field is refused, so an older Helios refuses a newer
    // interface rather than dropping it.
    let text = with_interface(json!([
        { "name": "strength", "node": "blend", "param": "opacity", "colour": "red" }
    ]));
    let e = parse_composite(&text).unwrap_err();
    assert!(e.contains("colour"), "{e}");
}
