//! The bench's model over Helios's kit: the shelf, moves and cases, the
//! commands, the value flow, the box, the sample's folds and reasons,
//! saving and loading, the modes, rules and nudges.

use std::collections::BTreeMap;

use egui_hud::CardPlacement;
use egui_hud::card::CardGroup;
use egui_hud::flow::Cause;
use egui_hud::model::Surface;
use egui_hud::render;
use egui_hud::resolve::{Layouts, Visibility};
use egui_hud::slot::ChromeKind;
use egui_hud::value::Value;
use helios_controls_gallery::bench::*;
use serde_json::{Value as Json, json};

fn bench_of(names: &[&str]) -> Bench {
    let mut b = standard();
    for n in names {
        b.add(n, None).unwrap();
    }
    b
}

#[test]
fn the_shelf_lists_the_catalogue_by_purpose_then_each_value_type() {
    let b = standard();
    let groups = b.shelf.groups("");
    assert_eq!(groups[0].0, "Tone");
    let tone: Vec<&str> = groups[0].1.iter().map(|e| e.name.as_str()).collect();
    assert!(tone.contains(&"exposure"), "{tone:?}");
    assert!(tone.contains(&"black-level"), "{tone:?}");
    let implicit = &groups.last().unwrap().1;
    let names: Vec<&str> = implicit.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "number", "whole", "toggle", "choice", "curve", "colour", "text"
        ]
    );
    for e in &b.shelf.entries {
        assert!(
            b.registry.get(&e.control).is_some(),
            "{} registered",
            e.name
        );
    }
    let found = b.shelf.groups("BOOL");
    assert!(found.is_empty(), "no entry says bool");
    let found = b.shelf.groups("toggle");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1.len(), 1);
}

#[test]
fn adding_twice_gives_two_keys_each_with_its_value() {
    let mut b = bench_of(&["exposure", "exposure"]);
    assert_eq!(b.composition.keys(), ["exposure", "exposure-2"]);
    assert_eq!(b.value("exposure-2"), Some(Value::Float(0.0)));
    assert!(b.add("nothing", None).is_err());
    let (p, _) = b.resolve().unwrap();
    assert!(p.get("exposure-2").unwrap().visibility.is_shown());
}

#[test]
fn moves_land_where_asked_inside_and_out_of_cases() {
    let mut b = bench_of(&["exposure", "number", "toggle", "choice"]);
    b.composition.place("choice", &Spot::top(0)).unwrap();
    assert_eq!(
        b.composition.keys(),
        ["choice", "exposure", "number", "toggle"]
    );
    // Dropped before `toggle` (index 3): lands at 2 once taken out.
    let at = b.composition.landing("choice", &Spot::top(3));
    b.composition.place("choice", &at).unwrap();
    assert_eq!(
        b.composition.keys(),
        ["exposure", "number", "choice", "toggle"]
    );
    let case = b
        .group(&["number".into(), "toggle".into()], "Basics")
        .unwrap();
    assert_eq!(b.composition.entries.len(), 3);
    assert_eq!(b.composition.find("toggle"), Some(Spot::in_case(&case, 1)));
    b.composition
        .place("exposure", &Spot::in_case(&case, 0))
        .unwrap();
    assert_eq!(
        b.composition.keys(),
        ["exposure", "number", "toggle", "choice"]
    );
    assert!(
        b.composition
            .place(&case, &Spot::in_case(&case, 0))
            .is_err()
    );
    assert!(b.composition.holds(&case), "a refused move keeps the case");
    b.composition.ungroup(&case).unwrap();
    assert_eq!(b.composition.entries.len(), 4);
}

#[test]
fn a_case_is_a_group_of_its_chrome_and_parts() {
    let mut b = bench_of(&["exposure", "number", "toggle"]);
    let case = b.group(&["number".into()], "Mix").unwrap();
    b.composition.case_mut(&case).unwrap().help = Some("How much".into());
    let (card, origins) = b.composition.card();
    assert_eq!(
        card.items.len(),
        6,
        "control, legend, help, part, separator, control"
    );
    assert_eq!(
        origins[1],
        Origin::Chrome {
            case: case.clone(),
            kind: ChromeKind::Legend
        }
    );
    // The legend heads the group; the separator follows it.
    assert_eq!(
        card.groups,
        [CardGroup {
            key: case.clone(),
            items: 1..4,
            header: true
        }]
    );
    let (p, _) = b.resolve().unwrap();
    assert!(p.items.iter().all(|i| i.visibility.is_shown()));
    // In order, one card: exposure, then the case's band (its legend
    // above its part), then the toggle below it.
    let rect = |k: &str| p.get(k).unwrap().rect.unwrap();
    let band = p
        .layout
        .as_ref()
        .unwrap()
        .group(&case)
        .unwrap()
        .rect
        .unwrap();
    let legend = p.items[1].rect.unwrap();
    assert!(rect("exposure").y + rect("exposure").h <= band.y);
    assert_eq!(legend.y, band.y);
    assert!(legend.y < rect("number").y && rect("number").y < band.y + band.h);
    assert!(rect("toggle").y >= band.y + band.h);
    // Removing the last part takes the case away.
    b.remove("number").unwrap();
    assert!(!b.composition.holds(&case));
}

#[test]
fn the_commands_build_a_bench_and_list_it() {
    let mut b = standard();
    let k = b
        .run("controls.bench.add", &json!({"control": "exposure"}))
        .unwrap();
    assert_eq!(k["key"], "exposure");
    b.run("controls.bench.add", &json!({"control": "toggle", "at": 0}))
        .unwrap();
    b.run(
        "controls.bench.add",
        &json!({"control": "implicit:shelf/number"}),
    )
    .unwrap();
    b.run("controls.bench.move", &json!({"key": "number", "to": 0}))
        .unwrap();
    let case = b
        .run(
            "controls.bench.case",
            &json!({"keys": ["toggle", "exposure"], "legend": "Light"}),
        )
        .unwrap();
    let list = b.run("controls.bench.list", &json!({})).unwrap();
    assert_eq!(list["order"], json!(["number", "toggle", "exposure"]));
    assert_eq!(list["cases"][0]["key"], case["key"]);
    assert_eq!(list["controls"][2]["readout"], "0.00 EV");
    assert!(list["controls"][2]["slot"].is_string(), "placed in a slot");
    b.run("controls.bench.remove", &json!({"key": "number"}))
        .unwrap();
    assert!(
        b.run("controls.bench.remove", &json!({"key": "number"}))
            .is_err()
    );
    let out = b.run("controls.bench.clear", &json!({})).unwrap();
    assert_eq!(out["removed"], 2);
    assert!(b.composition.entries.is_empty());
    assert!(b.run("controls.bench.nothing", &json!({})).is_err());
}

#[test]
fn a_change_goes_through_the_flow_and_undoes() {
    let mut b = bench_of(&["exposure", "toggle"]);
    let change = |to, done| render::Change {
        to,
        cause: Cause::Drag,
        done,
        cancelled: false,
    };
    b.apply(vec![
        render::Edit::Number {
            key: "exposure".into(),
            change: change(0.5, false),
        },
        render::Edit::Number {
            key: "exposure".into(),
            change: change(0.7, true),
        },
        render::Edit::Set {
            key: "toggle".into(),
            to: Value::Bool(false),
            done: true,
        },
    ]);
    assert_eq!(b.readout("exposure"), "+0.70 EV");
    assert_eq!(b.readout("toggle"), "off");
    assert_eq!(b.store.undo_steps().len(), 2);
    assert!(b.undo());
    assert_eq!(b.readout("toggle"), "on");
    assert!(b.undo());
    assert_eq!(b.readout("exposure"), "0.00 EV");
    assert!(b.redo());
    assert_eq!(b.readout("exposure"), "+0.70 EV");
}

#[test]
fn the_box_follows_the_width_with_hysteresis() {
    // Alone, the slider grows across the columns it is given.
    let mut b = bench_of(&["exposure"]);
    b.set_width(240.0);
    let (narrow, _) = b.resolve().unwrap();
    b.set_width(480.0);
    let (wide, _) = b.resolve().unwrap();
    let rect = |p: &CardPlacement, k: &str| p.get(k).unwrap().rect.unwrap();
    assert!(rect(&wide, "exposure").w > rect(&narrow, "exposure").w);
    b.set_width(5000.0);
    assert_eq!(b.width, WIDTH_RANGE.1);
}

#[test]
fn the_sample_folds_by_tier_rule_and_relevance_and_says_why() {
    let mut b = standard();
    b.load_sample();
    assert_eq!(
        b.composition.keys(),
        [
            "exposure", "black", "contrast", "feather", "angle", "profile"
        ]
    );
    // At the standard detail, the expert profile folds.
    let (p, _) = b.resolve().unwrap();
    let why = b.explain(&p);
    let profile = why.iter().find(|e| e.key == "profile").unwrap();
    assert_eq!(profile.visibility, "folded");
    assert_eq!(profile.decided_by, "the detail level, against its tier");
    let exposure = why.iter().find(|e| e.key == "exposure").unwrap();
    assert_eq!(exposure.visibility, "shown");
    assert!(exposure.slot.is_some() && exposure.representation.is_some());
    // Retouching folds contrast and black level by the card's rule.
    b.set_context(&json!({"task": "retouch"})).unwrap();
    let (p, _) = b.resolve().unwrap();
    let contrast = explain(p.get("contrast").unwrap(), 2.0);
    assert_eq!(contrast.visibility, "folded");
    assert_eq!(contrast.decided_by, "a rule of the card");
    assert!(
        contrast.why.contains("ctl.task == 'retouch'"),
        "{}",
        contrast.why
    );
    // A picture: the black level does not apply.
    b.set_context(
        &json!({"task": "develop", "image": {"raw": false, "edited": false, "masked": false}}),
    )
    .unwrap();
    let (p, _) = b.resolve().unwrap();
    assert_eq!(
        explain(p.get("black").unwrap(), 2.0).visibility,
        "not applicable"
    );
    egui_hud::resolve::reachable(&p).unwrap();
    // Every slow-key combination keeps every control reachable.
    let (card, _) = b.composition.card();
    egui_hud::resolve::reachable_everywhere(
        &b.registry,
        &card,
        Surface::Panel,
        &Layouts::person(b.layout.clone()),
        &BTreeMap::new(),
    )
    .unwrap();
}

#[test]
fn a_bench_saves_and_loads_with_its_context_and_layout() {
    let mut b = standard();
    b.load_sample();
    b.run(
        "controls.bench.part",
        &json!({"key": "exposure", "variant": "scrub", "representation": "compact"}),
    )
    .unwrap();
    b.run(
        "controls.bench.part",
        &json!({"key": "angle", "visibility": "hidden"}),
    )
    .unwrap();
    b.run(
        "controls.bench.mode",
        &json!({"mode": "dock", "width": 340}),
    )
    .unwrap();
    b.set_context(&json!({"task": "retouch", "detail": "expert", "density": "compact"}))
        .unwrap();
    let saved = b.run("controls.bench.save", &json!({})).unwrap()["bench"].clone();
    assert_eq!(saved["format"], FORMAT);
    assert_eq!(
        saved["settings"][LAYOUT_KEY]["parts"]["angle"]["visibility"],
        "hidden"
    );
    assert_eq!(saved["settings"][LAYOUT_KEY]["order"][0], "exposure");
    assert_eq!(saved["context"]["task"], "retouch");

    let mut other = standard();
    other.add("toggle", None).unwrap();
    other
        .run("controls.bench.load", &json!({"bench": saved}))
        .unwrap();
    assert_eq!(other.composition, b.composition);
    assert_eq!(other.mode, BenchMode::Dock);
    assert!((other.width - 340.0).abs() < 0.5);
    let k = other.keys();
    assert_eq!(k.task, egui_hud::context::Task::Retouch);
    assert_eq!(k.density, egui_hud::context::Density::Compact);
    assert_eq!(k.card_box.surface, Surface::Dock);
    let (p, _) = other.resolve().unwrap();
    assert!(matches!(
        p.get("angle").unwrap().visibility,
        Visibility::Folded { .. }
    ));
    assert_eq!(other.value("feather"), Some(Value::Float(20.0)));
    assert_eq!(other.save(), b.save());

    // Refused: the bench stays as it was.
    let mut bad = saved_of(&b);
    bad["composition"]["entries"][0]["control"] = json!("nothing");
    assert!(
        other
            .run("controls.bench.load", &json!({"bench": bad}))
            .is_err()
    );
    let mut bad = saved_of(&b);
    bad["format"] = json!("other");
    assert!(
        other
            .run("controls.bench.load", &json!({"bench": bad}))
            .is_err()
    );
    let mut bad = saved_of(&b);
    bad["context"]["task"] = json!("sleep");
    assert!(
        other
            .run("controls.bench.load", &json!({"bench": bad}))
            .is_err()
    );
    assert_eq!(other.composition, b.composition);
}

fn saved_of(b: &Bench) -> Json {
    serde_json::to_value(b.save()).unwrap()
}

#[test]
fn the_modes_lay_out_on_their_surface_and_the_hud_shows_chips() {
    let mut b = standard();
    b.load_sample();
    for mode in [
        BenchMode::Panel,
        BenchMode::Dock,
        BenchMode::Toolbar,
        BenchMode::Float,
    ] {
        b.set_mode(mode);
        let (p, _) = b.resolve().unwrap();
        assert_eq!(p.keys.card_box.surface, mode.surface());
        egui_hud::resolve::reachable(&p).unwrap();
    }
    b.set_mode(BenchMode::Hud);
    assert!(b.resolve().is_err(), "the HUD has no slots");
    let hud = b.hud();
    assert!(hud.iter().find(|h| h.key == "exposure").unwrap().chip);
    assert!(!hud.iter().find(|h| h.key == "feather").unwrap().chip);
    let list = b.list().unwrap();
    assert_eq!(list["mode"], "hud");
    assert_eq!(list["controls"][0]["hud"]["chip"], true);
}

#[test]
fn rules_and_parts_are_checked() {
    let mut b = standard();
    b.load_sample();
    assert!(
        b.run(
            "controls.bench.rules",
            &json!({"rules": [{"when": "ctl.tool == 'crop'", "effect": {"fold": ["feather"]}}]}),
        )
        .is_err(),
        "a fast key never folds"
    );
    assert_eq!(b.composition.rules.len(), 3);
    assert!(
        b.run(
            "controls.bench.part",
            &json!({"key": "feather", "variant": "nope"})
        )
        .is_err()
    );
    // A rule naming a control taken off the bench still checks.
    b.remove("black").unwrap();
    assert!(b.resolve().is_ok());
}

#[test]
fn a_nudge_is_one_undo_step() {
    let mut b = standard();
    b.load_sample();
    assert_eq!(
        b.nudge("feather", 3, "midi").unwrap(),
        "committed: one undo step"
    );
    assert_eq!(b.readout("feather"), "23 px");
    assert_eq!(b.store.undo_steps().len(), 1);
    assert!(b.nudge("profile", 1, "midi").is_err(), "not a number");
}
