//! The template format: read, checked against a catalogue, built headless
//! (folded and opened composites), written back, and its schemas in step
//! with its types.

use std::sync::Arc;

use egui_pop_model::ids::PanelId;
use egui_pop_model::layout::{Layout, Node};
use egui_pop_model::panels::{Area, PanelSpec};
use helios_core::{Bus, Engine, ModuleRef, Region};
use helios_template::{Bundle, Catalogue, CompositeFile, parse_composite, parse_template, schema};
use serde_json::{Value as Json, json};

/// Two exposures in a row, one exposed parameter setting both.
fn twice(version: u32) -> CompositeFile {
    parse_composite(
        &json!({
            "format": 1,
            "id": "test.twice",
            "version": version,
            "name": "Twice",
            "inputs": [{ "name": "input", "channels": 4 }],
            "outputs": [{ "name": "output", "channels": 4 }],
            "nodes": [
                { "name": "a", "module": "exposure" },
                { "name": "b", "module": "exposure" }
            ],
            "connections": [
                { "from": "@", "port": "input", "to": "a", "input": "input" },
                { "from": "a", "port": "output", "to": "b", "input": "input" }
            ],
            "exits": [{ "from": "b", "port": "output", "output": "output" }],
            "exposed": [
                { "name": "ev", "node": "a", "param": "exposure" },
                { "name": "ev", "node": "b", "param": "exposure" }
            ],
            "panel": { "panel": "test.params", "area": "end_side" }
        })
        .to_string(),
    )
    .unwrap()
}

fn panels() -> Vec<PanelSpec> {
    let id = |s: &str| PanelId::new(s).unwrap();
    vec![
        PanelSpec::new(id("test.image"), "Image", Area::Center).only_in(&[Area::Center]),
        PanelSpec::new(id("test.params"), "Parameters", Area::EndSide),
        PanelSpec::new(id("test.log"), "Log", Area::Bottom),
    ]
}

fn catalogue() -> Catalogue {
    Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .composite(twice(1))
        .panels(panels())
        .commands(["test.draw".to_string(), "test.erase".to_string()])
}

fn template() -> Json {
    json!({
        "format": 1,
        "id": "test.brighter",
        "version": 1,
        "meta": { "name": "Brighter", "scenario": "Make it brighter, twice" },
        "network": {
            "sources": ["source"],
            "nodes": [
                { "name": "folded", "composite": { "id": "test.twice", "version": 1 },
                  "params": { "ev": { "float": 0.5 } } },
                { "name": "opened", "composite": { "id": "test.twice", "version": 1 },
                  "params": { "ev": { "float": 0.5 } }, "open": true }
            ],
            "connections": [
                { "from": "source", "port": "image", "to": "folded", "input": "input" },
                { "from": "source", "port": "image", "to": "opened", "input": "input" }
            ],
            "view": { "node": "opened", "port": "output" },
            "presets": [
                { "id": "more", "name": "More",
                  "values": { "opened": { "ev": { "float": 1.0 } } } }
            ]
        },
        "tools": {
            "toolbox": ["test.draw", "test.erase"],
            "default": "test.draw",
            "keymap": [{ "keys": "D", "command": "test.draw" }]
        }
    })
}

fn layout() -> Layout {
    Layout {
        areas: vec![(Area::Center, Node::tab(PanelId::new("test.image").unwrap()))],
        hidden: vec![PanelId::new("test.log").unwrap()],
        ..Layout::default()
    }
}

fn bundle(t: &Json) -> Bundle {
    Bundle::new(parse_template(&t.to_string()).unwrap(), layout())
}

#[test]
fn a_template_is_checked_and_built_headless() {
    let b = bundle(&template());
    let report = b.validate(&catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    assert!(report.updates.is_empty());
    // The composite's hint docks the panel the layout did not show.
    assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
    let built = b.build(&catalogue()).unwrap();
    assert_eq!(
        built.layout.area_of(&PanelId::new("test.params").unwrap()),
        Some(Area::EndSide)
    );
    // Folded: one node; opened: its inner nodes, by path.
    let names: Vec<&str> = built.nodes.keys().map(String::as_str).collect();
    assert_eq!(names, ["folded", "opened.a", "opened.b", "source"]);
    assert_eq!(built.view.0, built.nodes["opened.b"]);
    // The exposed value reached both inner nodes of the opened one.
    for inner in ["opened.a", "opened.b"] {
        let params = built.graph.params(built.nodes[inner]).unwrap().unwrap();
        assert_eq!(params.float("exposure"), 0.5);
    }
    // Folded and opened render the same pixels.
    let engine = Engine::new(Arc::new(Bus::new()));
    let region = Region::sized(8, 8);
    let folded = engine
        .render(&built.graph, built.nodes["folded"], "output", region)
        .unwrap();
    let opened = engine
        .render(&built.graph, built.view.0, &built.view.1, region)
        .unwrap();
    assert_eq!(folded.to_vec(), opened.to_vec());
    // A preset's values go over the nodes' own.
    let more = b
        .build_with(&catalogue(), &helios_template::placeholder(), Some("more"))
        .unwrap();
    let params = more.graph.params(more.nodes["opened.a"]).unwrap().unwrap();
    assert_eq!(params.float("exposure"), 1.0);
}

#[test]
fn unknown_modules_composites_panels_and_commands_are_refused() {
    let mut t = template();
    t["network"]["nodes"][0] = json!({ "name": "folded", "module": "mystery" });
    t["network"]["nodes"][1]["composite"]["version"] = json!(7);
    t["tools"]["toolbox"][1] = json!("test.nothing");
    t["tools"]["default"] = json!("test.elsewhere");
    let mut b = bundle(&t);
    b.layout
        .areas
        .push((Area::Bottom, Node::tab(PanelId::new("test.image").unwrap())));
    b.layout.hidden.push(PanelId::new("test.ghost").unwrap());
    let report = b.validate(&catalogue());
    let has = |s: &str| report.problems.iter().any(|p| p.contains(s));
    assert!(has("unknown module `mystery`"), "{:?}", report.problems);
    assert!(
        has("unknown composite `test.twice@7`"),
        "{:?}",
        report.problems
    );
    assert!(
        has("unknown command `test.nothing`"),
        "{:?}",
        report.problems
    );
    assert!(
        has("`test.elsewhere` is not in the toolbox"),
        "{:?}",
        report.problems
    );
    assert!(has("unknown panel `test.ghost`"), "{:?}", report.problems);
    assert!(has("Image stays out of"), "{:?}", report.problems);
    assert!(b.build(&catalogue()).is_err());
}

#[test]
fn a_newer_composite_is_offered_never_taken_silently() {
    let catalogue = catalogue().composite(twice(2));
    let mut b = bundle(&template());
    let report = b.validate(&catalogue);
    assert!(report.is_ok(), "{:?}", report.problems);
    assert_eq!(report.updates.len(), 2);
    assert_eq!(report.updates[0].available, 2);
    // Built as pinned.
    let built = b.build(&catalogue).unwrap();
    let folded = built
        .graph
        .manifest(built.nodes["folded"])
        .unwrap()
        .unwrap();
    assert_eq!(folded.version, 1);
    // Taken when asked.
    let moved = b.upgrade_composites(&catalogue);
    assert_eq!(moved.len(), 2);
    let built = b.build(&catalogue).unwrap();
    let folded = built
        .graph
        .manifest(built.nodes["folded"])
        .unwrap()
        .unwrap();
    assert_eq!(folded.version, 2);
    assert!(b.validate(&catalogue).updates.is_empty());
}

#[test]
fn a_newer_format_and_unknown_fields_are_refused() {
    let mut t = template();
    t["format"] = json!(2);
    let e = parse_template(&t.to_string()).unwrap_err();
    assert!(e.contains("newer than this Helios reads"), "{e}");
    let mut t = template();
    t["network"]["nodes"][0]["colour"] = json!("red");
    let e = parse_template(&t.to_string()).unwrap_err();
    assert!(e.contains("colour"), "{e}");
    // The schema refuses it as well.
    let problems = schema::check(&schema::template(), &t);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("unknown field `colour`")),
        "{problems:?}"
    );
}

#[test]
fn the_schemas_describe_what_the_types_write() {
    let b = bundle(&template());
    let written = serde_json::to_value(&b.template).unwrap();
    assert_eq!(
        schema::check(&schema::template(), &written),
        Vec::<String>::new()
    );
    let written = serde_json::to_value(twice(1)).unwrap();
    assert_eq!(
        schema::check(&schema::composite(), &written),
        Vec::<String>::new()
    );
    // A node naming both a module and a composite fits neither option.
    let mut t = template();
    t["network"]["nodes"][0]["module"] = json!("exposure");
    assert!(!schema::check(&schema::template(), &t).is_empty());
}

#[test]
fn a_bundle_is_written_and_read_back_with_its_composites_and_texts() {
    let dir = std::env::temp_dir().join(format!("helios-template-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut b = bundle(&template());
    b.texts.push(helios_template::Text {
        locale: "fr".into(),
        name: "template.ftl".into(),
        source: "test-brighter-name = Plus clair\n".into(),
    });
    b.bundle_composites(&catalogue()).unwrap();
    assert_eq!(b.composites, [twice(1)]);
    b.save(&dir).unwrap();
    assert!(
        dir.join("composites/test.twice@1.helios-composite.json")
            .is_file()
    );
    let read = Bundle::load(&dir).unwrap();
    assert_eq!(read, b);
    // Its bundled composite builds it without the catalogue's.
    let bare = Catalogue::new().module(ModuleRef::new(helios_module_exposure::Exposure));
    assert!(read.build(&bare).is_ok());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_template_writes_only_its_own_text() {
    let mut b = bundle(&template());
    b.texts.push(helios_template::Text {
        locale: "fr".into(),
        name: "template.ftl".into(),
        source: "test-brighter-name = Plus clair\ngui-open-workspace-verb = Pirate\n".into(),
    });
    let report = b.validate(&catalogue());
    assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
    assert!(report.problems[0].contains("gui-open-workspace-verb"));
}
