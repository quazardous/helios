//! Contexts and `resolve` per card (`docs/design/control-contexts.md`):
//! the precedence, the class rule (C1), the default detail (C2), the
//! person's layout (C3), the space from the box (C4), never folding a
//! changed value, anti-jump, reachability, and the agent commands.

use std::sync::{Arc, Mutex};

use helios_controls::card::{Card, CardItem};
use helios_controls::catalogue::EXPOSURE;
use helios_controls::context::{
    CardBox, ContextKeys, Density, Detail, Device, ImageFlags, Rule, Space, Task, Tier,
};
use helios_controls::flow::Steps;
use helios_controls::model::{Mode, Representation, Surface};
use helios_controls::resolve::{
    self, CardPlacement, Layout, Layouts, Moment, Source, Values, Visibility,
};
use helios_controls::slot::{Chrome, ChromeKind, Orientation, Size, Slot};
use helios_controls::{Patch, Registry, implicit};
use helios_core::{ParamHint, ParamKind, ParamSpec, ParamWidget};
use helios_shell::commands::{Origin, Registry as Commands};
use helios_values::{Descriptor, Store};
use serde_json::{Value as Json, json};

fn descriptor(spec: ParamSpec, hint: Option<ParamHint>) -> Descriptor {
    Descriptor::from_spec(&spec, hint.as_ref())
}

/// Exposure and implicit controls: a slider, a toggle, a choice, a
/// curve.
fn registry() -> Registry {
    let mut r = Registry::standard();
    let black = descriptor(
        ParamSpec::float("black", "black level correction", -1.0, 1.0, 0.0),
        None,
    );
    let clip = descriptor(
        ParamSpec {
            name: "clip".into(),
            description: "clip highlights".into(),
            kind: ParamKind::Bool { default: false },
        },
        None,
    );
    let method = descriptor(
        ParamSpec {
            name: "method".into(),
            description: "highlight reconstruction method".into(),
            kind: ParamKind::Choice {
                options: vec!["clip".into(), "inpaint".into(), "segments".into()],
                default: "clip".into(),
            },
        },
        None,
    );
    let curve = descriptor(
        ParamSpec {
            name: "curve".into(),
            description: "tone curve".into(),
            kind: ParamKind::Floats {
                count: 8,
                min: 0.0,
                max: 1.0,
                default: vec![0.0, 0.0, 1.0, 1.0],
            },
        },
        Some(ParamHint::new("curve").widget(ParamWidget::Curve)),
    );
    for (path, d) in [
        ("node/exp/black", black),
        ("node/exp/clip", clip),
        ("node/exp/method", method),
        ("node/exp/curve", curve),
    ] {
        r.register(implicit(path, &d)).unwrap();
    }
    r
}

const BLACK: &str = "implicit:node/exp/black";
const CLIP: &str = "implicit:node/exp/clip";
const METHOD: &str = "implicit:node/exp/method";
const CURVE: &str = "implicit:node/exp/curve";

/// A "develop basics" card: exposure (essential), black level and a
/// curve (standard), clip and a raw-only method (expert), with rules
/// over each class of key.
fn card() -> Card {
    Card::new(vec![
        CardItem::Chrome(Chrome::new(ChromeKind::Legend).text("Basics")),
        CardItem::control("exposure", EXPOSURE).tier(Tier::Essential),
        CardItem::control("black", BLACK),
        CardItem::control("curve", CURVE),
        CardItem::control("clip", CLIP).tier(Tier::Expert),
        CardItem::control("method", METHOD).tier(Tier::Expert),
    ])
    .rule(
        Rule::new("ctl.task == 'cull'")
            .fold(&["black", "curve", "clip", "method"])
            .represent(Representation::Simplified)
            .because("Folded while culling"),
    )
    .rule(
        Rule::new("!ctl.image.raw")
            .inapplicable(&["method"])
            .because("Raw only"),
    )
    .rule(
        Rule::new("ctl.space == 'narrow'")
            .fold(&["curve"])
            .because("Too narrow"),
    )
    .rule(
        Rule::new("ctl.tool == 'exposure'")
            .emphasise(&["exposure"])
            .hud(&["exposure"])
            .offer(Mode::Toolbar),
    )
    .rule(Rule::new("ctl.device == 'pen'").grow())
}

/// A two-column panel, every key at its default.
fn keys() -> ContextKeys {
    ContextKeys::new(CardBox::new(Surface::Panel, 24, 240))
}

fn run(keys: &ContextKeys) -> CardPlacement {
    run_with(keys, &Layouts::default(), &Values::new())
}

fn run_with(keys: &ContextKeys, layouts: &Layouts, values: &Values) -> CardPlacement {
    let p = resolve::resolve(
        &registry(),
        &card(),
        keys,
        layouts,
        values,
        &Moment::default(),
        None,
    )
    .unwrap();
    resolve::reachable(&p).unwrap();
    p
}

fn shown(p: &CardPlacement) -> Vec<&str> {
    p.controls()
        .filter(|c| c.visibility.is_shown())
        .map(|c| c.key.as_str())
        .collect()
}

fn reason<'a>(p: &'a CardPlacement, key: &str) -> &'a resolve::Reason {
    p.get(key).unwrap().visibility.reason().unwrap()
}

/// Where every item went: what a fast key must not change.
fn geometry(p: &CardPlacement) -> Vec<(String, bool, Option<egui_gridflow::GridRect>)> {
    p.items
        .iter()
        .map(|i| (i.key.clone(), i.visibility.is_shown(), i.position))
        .collect()
}

#[test]
fn c2_the_default_detail_is_standard_and_folds_the_rare_parts() {
    assert_eq!(Detail::default(), Detail::Standard);
    let p = run(&keys());
    assert_eq!(shown(&p), ["exposure", "black", "curve"]);
    let more = p.more.as_ref().unwrap();
    assert_eq!(more.count, 2);
    assert_eq!(more.title(), "More (2)");
    assert_eq!(reason(&p, "clip").source, Source::Detail);
    assert_eq!(reason(&p, "clip").text, "Expert");
    // Folded parts have no slot; shown ones have one.
    assert!(p.get("clip").unwrap().position.is_none());
    assert!(p.get("black").unwrap().slot.is_some());

    let mut k = keys();
    k.detail = Detail::Essential;
    assert_eq!(shown(&run(&k)), ["exposure"]);
    k.detail = Detail::Expert;
    let all = run(&k);
    assert_eq!(shown(&all).len(), 5);
    assert!(all.more.is_none());
}

#[test]
fn precedence_declaration_rules_profile_person_moment() {
    // 2 over 1: a rule folds what the tier shows, and sets the
    // representation.
    let mut cull = keys();
    cull.task = Task::Cull;
    let p = run(&cull);
    assert_eq!(shown(&p), ["exposure"]);
    let r = reason(&p, "black");
    assert_eq!(
        (r.source, r.text.as_str(), r.when.as_deref()),
        (
            Source::Group,
            "Folded while culling",
            Some("ctl.task == 'cull'")
        )
    );
    assert_eq!(
        p.get("exposure").unwrap().representation,
        Some(Representation::Simplified)
    );

    // 3 over 2: the profile shows black while culling.
    let profile = Layout::default().pin_shown("black");
    let layouts = Layouts {
        profile: vec![profile],
        person: vec![],
    };
    let p = run_with(&cull, &layouts, &Values::new());
    assert!(p.get("black").unwrap().visibility.is_shown());
    // The rule it overrode is kept as a note, for agents.
    assert_eq!(p.get("black").unwrap().reasons[0].source, Source::Group);

    // 4 over 3: the person hides it again.
    let layouts = Layouts {
        profile: vec![Layout::default().pin_shown("black")],
        person: vec![Layout::default().hide("black")],
    };
    let p = run_with(&cull, &layouts, &Values::new());
    assert_eq!(reason(&p, "black").source, Source::Layout);
    assert_eq!(reason(&p, "black").text, "Hidden in your layout");

    // A per-task layout applies in its task only, after the general one.
    let layouts = Layouts::default();
    let layouts = Layouts {
        person: vec![
            Layout::default().hide("curve"),
            Layout::for_task(Task::Develop).pin_shown("curve"),
        ],
        ..layouts
    };
    assert!(
        run_with(&keys(), &layouts, &Values::new())
            .get("curve")
            .unwrap()
            .visibility
            .is_shown()
    );
    let mut retouch = keys();
    retouch.task = Task::Retouch;
    assert!(
        !run_with(&retouch, &layouts, &Values::new())
            .get("curve")
            .unwrap()
            .visibility
            .is_shown()
    );

    // 5 over 4: the moment reveals what the person hid.
    let hidden = Layouts::person(Layout::default().hide("black"));
    let mut moment = Moment::default();
    moment.revealed.insert("black".into());
    let p = resolve::resolve(
        &registry(),
        &card(),
        &keys(),
        &hidden,
        &Values::new(),
        &moment,
        None,
    )
    .unwrap();
    let black = p.get("black").unwrap();
    assert!(black.visibility.is_shown());
    assert!(black.reasons.iter().any(|r| r.source == Source::Revealed));
    resolve::reachable(&p).unwrap();
}

#[test]
fn the_person_s_order_leaves_chrome_in_place() {
    let layouts = Layouts::person(Layout::default().order(&["curve", "exposure"]));
    let p = run_with(&keys(), &layouts, &Values::new());
    // Reading order: the curve first, then exposure, then black.
    let rank = |k: &str| p.get(k).unwrap().visual.unwrap();
    assert!(rank("curve") < rank("exposure"), "{p:#?}");
    assert!(rank("exposure") < rank("black"));
    assert_eq!(p.items[0].kind, helios_controls::card::ItemKind::Chrome);
    assert_eq!(p.items[0].position.unwrap().y, 0, "the legend stays first");
}

#[test]
fn c1_the_image_key_marks_what_does_not_apply() {
    let mut k = keys();
    k.detail = Detail::Expert;
    k.image = ImageFlags {
        raw: false,
        ..ImageFlags::default()
    };
    let p = run(&k);
    let method = p.get("method").unwrap();
    assert!(matches!(
        method.visibility,
        Visibility::NotApplicable { .. }
    ));
    assert_eq!(reason(&p, "method").text, "Raw only");
    // Named in "More", not counted.
    let more = p.more.as_ref().unwrap();
    assert_eq!(more.count, 0);
    assert!(more.names("method"));
}

#[test]
fn c1_fast_keys_never_fold_shrink_or_move() {
    let still = run(&keys());
    let mut k = keys();
    k.tool = "exposure".into();
    let tool = run(&k);
    assert_eq!(geometry(&tool), geometry(&still));
    let exp = tool.get("exposure").unwrap();
    assert!(exp.emphasised && exp.chip, "emphasised, its chip offered");
    assert_eq!(tool.offer, Some(Mode::Toolbar));
    for device in [Device::Pen, Device::Touch, Device::Midi, Device::Keys] {
        let mut k = keys();
        k.device = device;
        let p = run(&k);
        assert_eq!(geometry(&p), geometry(&still), "{device:?}");
        assert_eq!(
            p.controls().all(|c| c.grown),
            device == Device::Pen,
            "{device:?}"
        );
    }
    // A card whose rule folds by a fast key is refused.
    let bad = card().rule(Rule::new("ctl.tool == 'crop'").fold(&["black"]));
    assert!(
        resolve::resolve(
            &registry(),
            &bad,
            &keys(),
            &Layouts::default(),
            &Values::new(),
            &Moment::default(),
            None
        )
        .is_err()
    );
    // And so is a case declaring one.
    let case = helios_controls::model::Case::default()
        .part("black", BLACK)
        .rule(Rule::new("ctl.device == 'touch'").fold(&["black"]));
    let c = helios_controls::catalogue::exposure().case(case);
    assert!(c.check().is_err());
}

#[test]
fn c3_the_layout_wins_but_never_past_the_physical_limits() {
    // Pinned shown while culling: the rule does not touch it.
    let mut cull = keys();
    cull.task = Task::Cull;
    let layouts = Layouts::person(
        Layout::default()
            .pin_shown("curve")
            .represent("exposure", Representation::Compact),
    );
    let p = run_with(&cull, &layouts, &Values::new());
    assert!(p.get("curve").unwrap().visibility.is_shown());
    // The person's representation beats the rule's simplified.
    assert_eq!(
        p.get("exposure").unwrap().representation,
        Some(Representation::Compact)
    );

    // A slot below the slider's minimum: its smaller form, with a reason.
    let third = Slot::new(Size::THIRD, Size::ONE);
    let p = run_with(
        &keys(),
        &Layouts::person(Layout::default().slot("exposure", third)),
        &Values::new(),
    );
    let exp = p.get("exposure").unwrap();
    assert_eq!(exp.representation, Some(Representation::Simplified));
    assert!(
        exp.reasons
            .iter()
            .any(|r| r.source == Source::Fallback && r.text.contains("below its minimum")),
        "{exp:#?}"
    );
    assert_eq!(exp.slot.unwrap().units, third);

    // A wider slot within its policy: taken, and its span counted.
    let p = run_with(
        &keys(),
        &Layouts::person(Layout::default().slot("black", Slot::wide(2))),
        &Values::new(),
    );
    assert_eq!(p.get("black").unwrap().slot.unwrap().span, 2);

    // An orientation it does not support: refused, with a reason.
    let mut k = keys();
    k.detail = Detail::Expert;
    let p = run_with(
        &k,
        &Layouts::person(Layout::default().orient("method", Orientation::Vertical)),
        &Values::new(),
    );
    let m = p.get("method").unwrap();
    assert_eq!(m.orientation, Some(Orientation::Horizontal));
    assert!(m.reasons.iter().any(|r| r.text.contains("does not turn")));

    // A representation it does not have: the next smaller one.
    let p = run_with(
        &k,
        &Layouts::person(Layout::default().represent("clip", Representation::Compact)),
        &Values::new(),
    );
    let clip = p.get("clip").unwrap();
    assert_eq!(clip.representation, Some(Representation::Simplified));
    assert!(
        clip.reasons
            .iter()
            .any(|r| r.text.starts_with("No compact"))
    );
}

#[test]
fn c4_the_space_comes_from_the_box_with_hysteresis() {
    let mut k = ContextKeys::new(CardBox::new(Surface::Panel, 12, 240));
    assert_eq!(k.space, Space::Narrow);
    let p = run(&k);
    assert_eq!(reason(&p, "curve").text, "Too narrow");
    // A resize just past the threshold keeps it narrow.
    k.set_box(CardBox::new(Surface::Panel, 20, 240));
    assert_eq!(k.space, Space::Narrow);
    assert!(!run(&k).get("curve").unwrap().visibility.is_shown());
    // Half a unit past, it turns regular.
    k.set_box(CardBox::new(Surface::Panel, 24, 240));
    assert_eq!(k.space, Space::Regular);
    assert!(run(&k).get("curve").unwrap().visibility.is_shown());
    // The density scales the unit: the same points hold fewer columns
    // at touch density.
    let comfortable = CardBox::from_points(Surface::Panel, 300.0, 600.0, Density::Comfortable);
    let touch = CardBox::from_points(Surface::Panel, 300.0, 600.0, Density::Touch);
    assert!(touch.width < comfortable.width);
}

#[test]
fn a_changed_value_is_never_folded() {
    let mut values = Values::new();
    values.insert("clip".into(), json!(true));
    // Expert, folded by its tier, but changed: shown.
    let p = run_with(&keys(), &Layouts::default(), &values);
    let clip = p.get("clip").unwrap();
    assert!(clip.visibility.is_shown() && clip.changed);
    assert_eq!(
        clip.reasons.iter().map(|r| r.source).collect::<Vec<_>>(),
        [Source::Detail, Source::Changed]
    );
    // Not by a rule, not by the person's layout either.
    let mut cull = keys();
    cull.task = Task::Cull;
    let hidden = Layouts::person(Layout::default().hide("clip"));
    assert!(
        run_with(&cull, &hidden, &values)
            .get("clip")
            .unwrap()
            .visibility
            .is_shown()
    );
    // S5: a raw-only part changed, then a picture: it stays, with its
    // reason.
    let mut values = Values::new();
    values.insert("method".into(), json!("inpaint"));
    let mut picture = keys();
    picture.image.raw = false;
    let p = run_with(&picture, &Layouts::default(), &values);
    let m = p.get("method").unwrap();
    assert!(m.visibility.is_shown());
    assert!(m.reasons.iter().any(|r| r.text == "Raw only"));
    // Its default value does not count as changed.
    let mut values = Values::new();
    values.insert("method".into(), json!("clip"));
    assert!(
        !run_with(&keys(), &Layouts::default(), &values)
            .get("method")
            .unwrap()
            .visibility
            .is_shown()
    );
}

fn resolve_in(keys: &ContextKeys, moment: &Moment, previous: &CardPlacement) -> CardPlacement {
    resolve::resolve(
        &registry(),
        &card(),
        keys,
        &Layouts::default(),
        &Values::new(),
        moment,
        Some(previous),
    )
    .unwrap()
}

#[test]
fn anti_jump_nothing_moves_under_the_hand() {
    let develop = run(&keys());
    let mut cull = keys();
    cull.task = Task::Cull;
    // The pointer is in the panel: the task switch waits.
    let hand = Moment {
        pointer_in_panel: true,
        ..Moment::default()
    };
    let held = resolve_in(&cull, &hand, &develop);
    assert!(held.pending);
    assert_eq!(geometry(&held), geometry(&develop));
    let black = held.get("black").unwrap();
    assert!(black.deferred && black.visibility.is_shown());
    assert!(black.reasons.iter().any(|r| r.source == Source::Deferred));
    resolve::reachable(&held).unwrap();
    // The hand leaves: it applies.
    let applied = resolve_in(&cull, &Moment::default(), &held);
    assert!(!applied.pending);
    assert!(!applied.get("black").unwrap().visibility.is_shown());

    // The person's own reveal applies at once with the pointer in the
    // panel...
    let reveal = Moment {
        pointer_in_panel: true,
        revealed: ["clip".to_owned()].into(),
        ..Moment::default()
    };
    let opened = resolve_in(&keys(), &reveal, &develop);
    assert!(!opened.pending);
    assert!(opened.get("clip").unwrap().visibility.is_shown());
    // ...but not while a part is handled: that part never moves.
    let handling = Moment {
        gesture_open: true,
        handled: Some("exposure".into()),
        revealed: ["clip".to_owned()].into(),
        ..Moment::default()
    };
    let still = resolve_in(&keys(), &handling, &develop);
    assert_eq!(geometry(&still), geometry(&develop));
    assert_eq!(
        still.get("exposure").unwrap().position,
        develop.get("exposure").unwrap().position
    );
    // A fast change still applies while frozen: it moves nothing.
    let mut tool = keys();
    tool.tool = "exposure".into();
    let emph = resolve_in(&tool, &hand, &develop);
    assert!(!emph.pending);
    assert!(emph.get("exposure").unwrap().emphasised);
}

#[test]
fn reachable_for_every_slow_key_combination() {
    let r = registry();
    let c = card();
    for surface in [Surface::Panel, Surface::Dock, Surface::Toolbar] {
        resolve::reachable_everywhere(&r, &c, surface, &Layouts::default(), &Values::new())
            .unwrap();
    }
    // With a layout hiding everything and changed values.
    let hide_all = Layouts::person(
        ["exposure", "black", "curve", "clip", "method"]
            .into_iter()
            .fold(Layout::default(), Layout::hide),
    );
    let mut values = Values::new();
    values.insert("black".into(), json!(0.1));
    resolve::reachable_everywhere(&r, &c, Surface::Panel, &hide_all, &values).unwrap();
    // A box too small for anything: overflowed parts are folded, counted.
    let mut k = ContextKeys::new(CardBox::new(Surface::Toolbar, 2, 12));
    k.detail = Detail::Expert;
    let p = run(&k);
    assert!(
        p.controls()
            .any(|c| matches!(&c.visibility, Visibility::Folded { reason } if reason.source == Source::Size)),
        "{p:#?}"
    );
}

#[test]
fn reachable_catches_a_lost_part() {
    let mut p = run(&keys());
    resolve::reachable(&p).unwrap();
    let mut lost = p.clone();
    lost.more = None;
    assert!(resolve::reachable(&lost).is_err(), "folded but not counted");
    // A shown part with no slot is not reachable either.
    let black = p.items.iter_mut().find(|i| i.key == "black").unwrap();
    black.position = None;
    assert!(resolve::reachable(&p).is_err());
}

#[test]
fn search_finds_folded_parts() {
    let p = run(&keys());
    let found = resolve::find(&p, "CLIP");
    assert_eq!(found.len(), 1);
    assert!(!found[0].visibility.is_shown());
    // By label too ("clip highlights" is the implicit control's label).
    assert!(!resolve::find(&p, "black").is_empty());
    assert_eq!(resolve::find(&p, "").len(), 5);
}

#[test]
fn keys_and_placements_read_as_data() {
    let k = keys();
    let r = k.report();
    assert_eq!(r["task"], "develop");
    assert_eq!(r["detail"], "standard");
    assert_eq!(r["box"]["surface"], "panel");
    let p = run(&k);
    let j = serde_json::to_value(&p).unwrap();
    let clip = j["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["key"] == "clip")
        .unwrap();
    assert_eq!(clip["visibility"]["state"], "folded");
    assert_eq!(clip["visibility"]["reason"]["source"], "detail");
    assert_eq!(j["more"]["count"], 2);
    // A layout round-trips, as a setting.
    let l = Layout::for_task(Task::Cull)
        .pin_shown("black")
        .slot("black", Slot::wide(2))
        .orient("exposure", Orientation::Vertical)
        .position("curve", 0, 12);
    let back: Layout = serde_json::from_value(serde_json::to_value(&l).unwrap()).unwrap();
    assert_eq!(back, l);
}

// The agent commands, on a store with an exposure node.

fn store() -> Store {
    let mut s = Store::new();
    let mut d = descriptor(
        ParamSpec::float("exposure", "exposure adjustment, in EV", -18.0, 18.0, 0.0),
        Some(ParamHint::new("exposure").soft(-3.0, 4.0).unit("EV")),
    );
    d.module = Some("exposure".into());
    s.declare("node/exp/exposure", d);
    let mut b = descriptor(
        ParamSpec::float("black", "black level correction", -1.0, 1.0, 0.0),
        None,
    );
    b.module = Some("exposure".into());
    s.declare("node/exp/black", b);
    s
}

#[test]
fn context_find_reveal_and_describe_commands() {
    let mut reg = Registry::standard();
    assert!(
        reg.patch([Patch::new("exposure", "exposure", EXPOSURE, "value")])
            .is_empty()
    );
    let mut cmds = Commands::new();
    helios_controls::commands::register(
        &mut cmds,
        Arc::new(Mutex::new(store())),
        Arc::new(reg),
        Arc::new(Mutex::new(Steps::new())),
    )
    .unwrap();
    let agent = Origin::Agent("t".into());
    let cx = helios_shell::when::Context::new();
    let mut run = |id: &str, args: Json| cmds.run(id, args, agent.clone(), &cx);

    let c = run("controls.context", json!({})).unwrap();
    assert_eq!(c["keys"]["task"], "develop");
    assert_eq!(c["keys"]["detail"], "standard");
    assert!(run("controls.context", json!({"space": "wide"})).is_err());
    let c = run(
        "controls.context",
        json!({"detail": "essential", "box": {"surface": "panel", "width": 60, "height": 240}}),
    )
    .unwrap();
    assert_eq!(c["keys"]["space"], "wide");

    // Essential: the node's standard parts fold; search finds them.
    let found = run("controls.find", json!({"node": "exp", "query": "black"})).unwrap();
    assert_eq!(found[0]["key"], "black");
    assert_eq!(found[0]["visibility"]["state"], "folded");
    assert_eq!(found[0]["visibility"]["reason"]["source"], "detail");

    let r = run("controls.reveal", json!({"node": "exp", "key": "black"})).unwrap();
    assert_eq!(r["placement"]["visibility"]["state"], "shown");
    assert!(run("controls.reveal", json!({"node": "exp", "key": "nope"})).is_err());

    let d = run(
        "controls.describe",
        json!({"id": "exposure", "node": "exp"}),
    )
    .unwrap();
    assert_eq!(d["id"], "exposure");
    assert_eq!(d["placement"][0]["key"], "exposure");
    assert_eq!(d["placement"][0]["visibility"]["state"], "folded");
    assert_eq!(d["more"]["count"], 1);

    // A task switch ends the reveal.
    run("controls.context", json!({"task": "cull"})).unwrap();
    let found = run("controls.find", json!({"node": "exp", "query": "black"})).unwrap();
    assert_eq!(found[0]["visibility"]["state"], "folded");

    let all = run("controls.reveal_all", json!({"node": "exp"})).unwrap();
    assert!(all["more"].is_null(), "{all}");
    assert!(
        all["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["visibility"]["state"] == "shown")
    );
}
