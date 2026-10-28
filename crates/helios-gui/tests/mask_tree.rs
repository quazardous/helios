//! The Masks panel: the selected module's mask network as a tree, kept
//! live by edits from anywhere (the viewport, agents, undo), its
//! selection two-way with the viewport's, and edited in place: rename
//! (F2), reorder (drag and drop, vetoed when the network cannot take
//! it), invert, opacity, delete.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use helios_core::{Graph, Image, ModuleRef, NodeId, Params, Region, Value};
use helios_gui::mask_tree::{self, label_path};
use helios_gui::masks::{MASK, SHAPE};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

const W: u32 = 600;
const H: u32 = 400;

fn module(g: &mut Graph, name: &str, m: ModuleRef, p: &Params) -> NodeId {
    g.add_module(name, m, p).unwrap()
}

/// An exposure through a mask: the group `mask` folds a nested group
/// `inner` (a circle `dot`), a circle `spot` and an ellipse `oval`.
fn graph() -> (Graph, NodeId) {
    use helios_module_masks::{MaskBlend, MaskCircle, MaskCombine, MaskEllipse};
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, |x, _, c| match c {
            0..=2 => 0.1 + 0.4 * x as f32 / W as f32,
            _ => 1.0,
        }),
    );
    let exp = module(
        &mut g,
        "exposure",
        ModuleRef::new(helios_module_exposure::Exposure),
        &Params::new().with("exposure", Value::Float(1.0)),
    );
    g.connect(src, "image", exp, "input").unwrap();
    let mask = module(&mut g, "mask", ModuleRef::new(MaskCombine), &Params::new());
    let inner = module(
        &mut g,
        "inner",
        ModuleRef::new(MaskCombine),
        &Params::new().with("operator", Value::Choice("minimum".into())),
    );
    let at = |x: f64| {
        Params::new()
            .with("x", Value::Float(x))
            .with("y", Value::Float(200.0))
    };
    let circle = |x: f64| at(x).with("radius", Value::Float(60.0));
    let dot = module(&mut g, "dot", ModuleRef::new(MaskCircle), &circle(150.0));
    let spot = module(&mut g, "spot", ModuleRef::new(MaskCircle), &circle(300.0));
    let oval = module(
        &mut g,
        "oval",
        ModuleRef::new(MaskEllipse),
        &at(450.0)
            .with("radius_x", Value::Float(80.0))
            .with("radius_y", Value::Float(40.0)),
    );
    g.connect(dot, "mask", inner, "in0").unwrap();
    for (i, n) in [inner, spot, oval].into_iter().enumerate() {
        g.connect(n, "mask", mask, &format!("in{i}")).unwrap();
    }
    let blend = module(&mut g, "blend", ModuleRef::new(MaskBlend), &Params::new());
    g.connect(src, "image", blend, "input").unwrap();
    g.connect(exp, "output", blend, "processed").unwrap();
    g.connect(mask, "mask", blend, "mask").unwrap();
    (g, blend)
}

fn harness() -> Harness<'static, Shell> {
    let (g, blend) = graph();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, blend, "output").unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.step();
    run(&mut h, "helios.masks.focus", json!(null));
    h
}

fn run(h: &mut Harness<'_, Shell>, command: &str, args: serde_json::Value) {
    h.state_mut().run(command, args, Origin::User).unwrap();
    for _ in 0..3 {
        h.step();
    }
}

fn text(h: &Harness<'_, Shell>, path: &str) -> String {
    match h.state().values.lock().unwrap().get(path).map(|e| &e.value) {
        Some(Value::Text(t) | Value::Choice(t)) => t.clone(),
        v => panic!("{path}: {v:?}"),
    }
}

/// The rows of the tree as the view shows them, indented by depth: the
/// icon, then the label.
fn rows(h: &Harness<'_, Shell>) -> Vec<String> {
    let t = h.state().mask_tree();
    let (model, net) = (t.state().model(), t.network());
    model
        .rows_now()
        .iter()
        .filter_map(|r| {
            let k = r.key()?;
            Some(format!(
                "{}{} {}",
                "  ".repeat(r.depth()),
                net.items.get(k)?.icon(),
                model.node(k)?.label
            ))
        })
        .collect()
}

fn cursor(h: &Harness<'_, Shell>) -> Option<String> {
    h.state().mask_tree().state().selection().cursor().cloned()
}

/// The members of a group, in port order, as the graph holds them.
fn wired(h: &Harness<'_, Shell>, group: &str) -> Vec<String> {
    let v = h.state().viewport();
    let g = v.graph().unwrap();
    g.input_nodes(g.find(group).unwrap())
        .unwrap()
        .into_iter()
        .flatten()
        .map(|id| g.name(id).unwrap().to_string())
        .collect()
}

fn floats(h: &Harness<'_, Shell>, path: &str) -> Vec<f64> {
    let store = h.state().values.lock().unwrap();
    store.get(path).unwrap().value.as_list().unwrap().to_vec()
}

fn steps(h: &Harness<'_, Shell>) -> Vec<String> {
    let store = h.state().values.lock().unwrap();
    store.undo_steps().iter().map(|s| s.label.clone()).collect()
}

/// A row of the tree, by its label (the Parameters panel may show the
/// same name).
fn row(h: &Harness<'_, Shell>, label: &str) -> egui::Rect {
    let panel = h.state().target_rect(mask_tree::PANEL).unwrap();
    let rows: Vec<egui::Rect> = h
        .query_all_by_label(label)
        .map(|n| n.rect())
        .filter(|r| panel.contains(r.center()))
        .collect();
    match rows[..] {
        [r] => r,
        _ => panic!("{} rows labelled `{label}`", rows.len()),
    }
}

fn button(h: &mut Harness<'_, Shell>, pos: Pos2, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

fn click(h: &mut Harness<'_, Shell>, at: Pos2) {
    h.hover_at(at);
    h.step();
    button(h, at, true);
    button(h, at, false);
    for _ in 0..3 {
        h.step();
    }
}

fn click_row(h: &mut Harness<'_, Shell>, label: &str) {
    let at = row(h, label).center();
    click(h, at);
}

/// Drags from `from` to `to` in steps and lets go there.
fn drag(h: &mut Harness<'_, Shell>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.step();
    button(h, from, true);
    h.step();
    for i in 1..=8 {
        h.hover_at(from + (to - from) * (i as f32 / 8.0));
        h.step();
    }
    button(h, to, false);
    for _ in 0..3 {
        h.step();
    }
}

#[test]
fn an_ai_object_member_shows_with_its_icon_and_its_question() {
    use helios_module_masks::{MaskObject, object};
    let (mut g, blend) = graph();
    let sky = module(&mut g, "sky", ModuleRef::new(MaskObject), &Params::new());
    g.connect(g.find("source").unwrap(), "image", sky, "input")
        .unwrap();
    g.connect(sky, "mask", g.find("mask").unwrap(), "in3")
        .unwrap();
    let answer = object::answer(2, 1, &[255, 0], Region::sized(W, H), 0.8).unwrap();
    g.answer(sky, &answer).unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, blend, "output").unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.step();
    run(&mut h, "helios.masks.focus", json!(null));
    assert_eq!(rows(&h).last().unwrap(), "  ✨ sky");
    // Its question is in the Parameters panel's values; its answer is not.
    let store = h.state().values.lock().unwrap();
    for p in ["kind", "prompt", "resolver"] {
        assert!(store.get(&format!("node/sky/{p}")).is_some(), "{p}");
    }
    assert!(store.get("node/sky/mask").is_none());
}

#[test]
fn a_brush_member_shows_with_its_icon() {
    use helios_module_masks::MaskBrush;
    let (mut g, blend) = graph();
    let stroke = module(&mut g, "stroke", ModuleRef::new(MaskBrush), &Params::new());
    g.connect(stroke, "mask", g.find("mask").unwrap(), "in3")
        .unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, blend, "output").unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.step();
    run(&mut h, "helios.masks.focus", json!(null));
    assert_eq!(rows(&h).last().unwrap(), "  ✏ stroke");
}

#[test]
fn the_tree_shows_the_module_s_mask_with_icons_and_operators() {
    let h = harness();
    assert_eq!(
        rows(&h),
        [
            "▣ mask — union",
            "  ▣ inner — intersection",
            "    ○ dot",
            "  ○ spot",
            "  ◎ oval",
        ]
    );
    // The view's own nodes, by the network's names.
    let net = h.state().mask_tree().network();
    assert_eq!(net.root.as_deref(), Some("mask"));
    assert_eq!(net.blend.as_deref(), Some("blend"));
}

#[test]
fn group_operators_are_named_by_their_effect_with_a_preview() {
    let mut h = harness();
    // Each operator by what it does to the members, the row's glyph
    // saying it too.
    for (operator, effect) in [
        ("maximum", "union"),
        ("screen", "soft union"),
        ("minimum", "intersection"),
        ("product", "multiply"),
        ("sum", "add"),
        ("difference", "subtract from the first"),
        ("exclusion", "exclude"),
    ] {
        h.state()
            .values
            .lock()
            .unwrap()
            .set(helios_values::Set::new(
                "node/mask/operator",
                helios_values::Change::To(Value::Choice(operator.into())),
                Origin::User,
            ))
            .unwrap();
        for _ in 0..3 {
            h.step();
        }
        assert_eq!(rows(&h)[0], format!("▣ mask — {effect}"));
        assert_eq!(
            mask_tree::operator_effect(operator).english,
            effect,
            "{operator}"
        );
        let glyph =
            format!("{effect}: how the members fold, in port order (the operator {operator})");
        assert!(in_panel(&h, &glyph), "{glyph}");
    }
    // The nested group keeps its own.
    assert_eq!(rows(&h)[1], "  ▣ inner — intersection");
}

#[test]
fn a_member_s_opacity_and_inversion_read_on_its_row() {
    let mut h = harness();
    // The group's per-port values, said on each member's row.
    run(
        &mut h,
        "mask.member.opacity",
        json!({ "member": "spot", "opacity": 0.3 }),
    );
    run(&mut h, "mask.member.invert", json!({ "member": "spot" }));
    run(
        &mut h,
        "mask.member.opacity",
        json!({ "member": "oval", "opacity": 0.0 }),
    );
    // The root's own, on its row.
    run(
        &mut h,
        "mask.member.opacity",
        json!({ "member": "mask", "opacity": 0.5 }),
    );
    assert_eq!(
        rows(&h),
        [
            "▣ mask — union · opacity 50 %",
            "  ▣ inner — intersection",
            "    ○ dot",
            "  ○ spot · inverted · opacity 30 %",
            "  ◎ oval · hidden",
        ]
    );
    // In the interface's language.
    let app = h.state().app().handle().clone();
    egui_pop::i18n::plugin::set_language(&app, "fr").unwrap();
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(
        rows(&h),
        [
            "▣ mask — union · opacité 50 %",
            "  ▣ inner — intersection",
            "    ○ dot",
            "  ○ spot · inversé · opacité 30 %",
            "  ◎ oval · masqué",
        ]
    );
}

/// A label shown in the Parameters panel.
fn in_params(h: &Harness<'_, Shell>, label: &str) -> bool {
    let panel = h.state().target_rect("helios.params").unwrap();
    h.query_all_by_label(label)
        .any(|n| panel.contains(n.rect().center()))
}

fn set(h: &mut Harness<'_, Shell>, path: &str, value: Value) {
    h.state()
        .values
        .lock()
        .unwrap()
        .set(helios_values::Set::new(
            path,
            helios_values::Change::To(value),
            Origin::User,
        ))
        .unwrap();
    for _ in 0..3 {
        h.step();
    }
}

#[test]
fn a_group_s_parameters_say_its_operator_and_its_members() {
    use egui::accesskit::Role;

    let mut h = harness();
    run(&mut h, "helios.params.focus", json!(null));
    // The spot at 30 %, the oval hidden and inverted.
    let mut opacity = vec![1.0; 8];
    (opacity[1], opacity[2]) = (0.3, 0.0);
    set(&mut h, "node/mask/opacity", Value::List(opacity));
    let mut invert = vec![0; 8];
    invert[2] = 1;
    set(&mut h, "node/mask/invert", Value::Ints(invert));
    click_row(&mut h, "mask — union");
    assert_eq!(text(&h, "view/graph/selection"), "mask");
    // A line per wired port, the member and its value in words; no
    // bare count of values.
    for line in [
        "inner: opacity 100 %",
        "spot: opacity 30 %",
        "oval: hidden",
        "inner: not inverted",
        "spot: not inverted",
        "oval: inverted",
    ] {
        assert!(in_params(&h, line), "{line}");
    }
    assert!(!in_params(&h, "8 values"));
    // The operator by its effect, with its preview.
    let params = h.state().target_rect("helios.params").unwrap();
    let combo = h
        .query_all(by().role(Role::ComboBox))
        .find(|n| params.contains(n.rect().center()))
        .expect("the operator's list");
    assert_eq!(combo.value().as_deref(), Some("union"));
    assert!(in_params(
        &h,
        "union: how the members fold, in port order (the operator maximum)"
    ));
    // Picked by its effect, the operator changes.
    let at = combo.rect().center();
    click(&mut h, at);
    // Each choice has its preview beside it.
    assert!(
        h.query_by_label("exclude: how the members fold, in port order (the operator exclusion)")
            .is_some()
    );
    h.get_by_label("intersection").click();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(text(&h, "node/mask/operator"), "minimum");
    assert_eq!(rows(&h)[0], "▣ mask — intersection");
}

#[test]
fn a_group_s_member_lines_follow_its_label_and_the_language() {
    let mut h = harness();
    run(&mut h, "helios.params.focus", json!(null));
    run(
        &mut h,
        "mask.member.rename",
        json!({ "member": "spot", "label": "Sun" }),
    );
    click_row(&mut h, "mask — union");
    assert!(in_params(&h, "Sun: opacity 100 %"));
    assert!(!in_params(&h, "spot: opacity 100 %"));
    // In French: the words and the operator's effect.
    let app = h.state().app().handle().clone();
    egui_pop::i18n::plugin::set_language(&app, "fr").unwrap();
    for _ in 0..3 {
        h.step();
    }
    assert!(in_params(&h, "Sun : opacité 100 %"), "French line");
    assert!(in_params(&h, "Sun : non inversé"), "French inversion");
}

#[test]
fn a_viewport_edit_an_agent_and_undo_show_in_the_tree() {
    let mut h = harness();
    // A shape added on the viewport (its key, the image focused) joins
    // the mask the viewport edits.
    run(&mut h, "mask.select", json!({ "mask": "mask" }));
    let c = h.state().target_rect("viewport.display").unwrap().center();
    click(&mut h, c + vec2(0.0, 150.0));
    h.key_press(Key::G);
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(text(&h, SHAPE), "gradient");
    assert_eq!(rows(&h).last().unwrap(), "  ◑ gradient");
    assert_eq!(cursor(&h).as_deref(), Some("gradient"), "and is selected");
    // An agent halves the spot's opacity and inverts the oval: one step
    // each, both shown.
    let agent = Origin::Agent("a1".into());
    h.state_mut()
        .run(
            "mask.member.opacity",
            json!({ "member": "spot", "opacity": 0.5 }),
            agent.clone(),
        )
        .unwrap();
    h.state_mut()
        .run("mask.member.invert", json!({ "member": "oval" }), agent)
        .unwrap();
    for _ in 0..3 {
        h.step();
    }
    assert!(rows(&h).contains(&"  ○ spot · opacity 50 %".to_string()));
    assert!(rows(&h).contains(&"  ◎ oval · inverted".to_string()));
    assert_eq!(steps(&h), ["opacity of spot", "invert oval"]);
    // Undo takes them back, and the tree follows.
    run(&mut h, "param.undo", json!(null));
    run(&mut h, "param.undo", json!(null));
    assert!(rows(&h).contains(&"  ○ spot".to_string()));
    assert!(rows(&h).contains(&"  ◎ oval".to_string()));
}

#[test]
fn selection_is_two_way_between_the_tree_and_the_viewport() {
    let mut h = harness();
    // A click on a row selects the shape on the viewport: the mask it
    // edits is the shape's group, its handles are the shape's.
    click_row(&mut h, "dot");
    assert_eq!(text(&h, SHAPE), "dot");
    assert_eq!(text(&h, MASK), "inner");
    assert_eq!(text(&h, "view/graph/selection"), "dot");
    assert_eq!(h.state().viewport().masks().overlays()[0].id, "dot");
    // A group selected: the viewport edits it, no shape.
    click_row(&mut h, "mask — union");
    assert_eq!(text(&h, MASK), "mask");
    assert_eq!(text(&h, SHAPE), "");
    // The viewport selects (its click, an agent): the tree follows.
    run(&mut h, "mask.select", json!({ "shape": "oval" }));
    assert_eq!(cursor(&h).as_deref(), Some("oval"));
    // A click on the spot's top edge on the viewport, clear of the oval.
    let m = h.state().viewport().state().mapping().unwrap();
    click(&mut h, m.image_to_screen([300.0, 140.0]));
    assert_eq!(text(&h, SHAPE), "spot");
    assert_eq!(cursor(&h).as_deref(), Some("spot"));
}

#[test]
fn f2_renames_in_place_as_one_undo_step() {
    let mut h = harness();
    run(
        &mut h,
        "mask.member.opacity",
        json!({ "member": "spot", "opacity": 0.5 }),
    );
    click_row(&mut h, "spot · opacity 50 %");
    h.key_press(Key::F2);
    for _ in 0..3 {
        h.step();
    }
    // The field starts from the name, not from the row's decorations.
    let field = h.get(by().label_contains("Rename"));
    assert_eq!(field.value().as_deref(), Some("spot"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.step();
    h.get(by().label_contains("Rename")).type_text("Sun");
    h.step();
    h.key_press(Key::Enter);
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(text(&h, &label_path("spot")), "Sun");
    // Its decorations come back around the new name.
    assert!(
        rows(&h).contains(&"  ○ Sun · opacity 50 %".to_string()),
        "{:?}",
        rows(&h)
    );
    assert_eq!(steps(&h), ["opacity of spot", "rename spot to Sun"]);
    // The node keeps its name: the viewport and the store still find it.
    assert_eq!(text(&h, SHAPE), "spot");
    run(&mut h, "param.undo", json!(null));
    assert!(rows(&h).contains(&"  ○ spot · opacity 50 %".to_string()));
    // Agents name members too.
    run(
        &mut h,
        "mask.member.rename",
        json!({ "member": "oval", "label": "Sky" }),
    );
    assert!(rows(&h).contains(&"  ◎ Sky".to_string()));
}

#[test]
fn a_drop_reorders_members_and_their_values_follow() {
    let mut h = harness();
    run(
        &mut h,
        "mask.member.opacity",
        json!({ "member": "oval", "opacity": 0.25 }),
    );
    assert_eq!(wired(&h, "mask"), ["inner", "spot", "oval"]);
    // The oval dropped before the spot: wired before it, its opacity
    // with it.
    let (from, to) = (row(&h, "oval · opacity 25 %"), row(&h, "spot"));
    drag(&mut h, from.center(), to.center_top() + vec2(0.0, 3.0));
    assert_eq!(wired(&h, "mask"), ["inner", "oval", "spot"]);
    assert_eq!(&floats(&h, "node/mask/opacity")[..3], [1.0, 0.25, 1.0]);
    assert_eq!(
        &rows(&h)[3..],
        ["  ◎ oval · opacity 25 %", "  ○ spot"],
        "the tree shows the network's order"
    );
    // Into another group: the spot joins the inner group.
    let (from, inner) = (row(&h, "spot"), row(&h, "inner — intersection"));
    drag(&mut h, from.center(), inner.center());
    assert_eq!(wired(&h, "inner"), ["dot", "spot"]);
    assert_eq!(wired(&h, "mask"), ["inner", "oval"]);
    assert_eq!(rows(&h)[3], "    ○ spot");
    // The rewiring is no undo step: only the opacity is.
    assert_eq!(steps(&h), ["opacity of oval"]);
}

#[test]
fn a_drop_the_network_cannot_take_is_vetoed() {
    let mut h = harness();
    let before = rows(&h);
    // Nothing goes beside the module's mask.
    let (oval, root) = (row(&h, "oval"), row(&h, "mask — union"));
    drag(&mut h, oval.center(), root.center_top() + vec2(0.0, 2.0));
    assert_eq!(rows(&h), before);
    assert_eq!(wired(&h, "mask"), ["inner", "spot", "oval"]);
    let notices: Vec<String> = h.state().notices().iter().map(|n| n.text.clone()).collect();
    assert!(
        notices.iter().any(|n| n.contains("beside the module")),
        "{notices:?}"
    );
    // A shape takes no members.
    let net = h.state().mask_tree().network();
    let full = mask_tree::drop_place(
        &net,
        &["spot".to_string()],
        "dot",
        egui_pop::views::tree::Position::Inside,
    );
    let refused = full.unwrap_err();
    assert_eq!(refused.key, "gui-masks-not-a-group");
    assert!(refused.english.contains("only a group takes members"));
}

#[test]
fn delete_takes_a_member_out_of_the_mask() {
    let mut h = harness();
    click_row(&mut h, "spot");
    assert_eq!(text(&h, SHAPE), "spot");
    h.key_press(Key::Delete);
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(wired(&h, "mask"), ["inner", "oval"]);
    assert!(
        !rows(&h).iter().any(|r| r.contains("spot")),
        "{:?}",
        rows(&h)
    );
    // The viewport no longer draws it; the selection went to its group.
    assert!(
        !h.state()
            .viewport()
            .masks()
            .members()
            .iter()
            .any(|m| m.node == "spot")
    );
    assert_eq!(text(&h, SHAPE), "");
    assert_eq!(cursor(&h).as_deref(), Some("mask"));
    // A parametric member added from the context command reads the
    // masked module's input.
    run(&mut h, "mask.add.parametric", json!(null));
    assert!(
        rows(&h).contains(&"  ☰ parametric".to_string()),
        "{:?}",
        rows(&h)
    );
    assert_eq!(wired(&h, "parametric"), ["source"]);
}

/// Right-clicks at `at`.
fn right_click(h: &mut Harness<'_, Shell>, at: Pos2) {
    h.hover_at(at);
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    for _ in 0..3 {
        h.step();
    }
}

/// A menu as its titles: `-` a separator, `label >` a submenu.
fn titles(items: &serde_json::Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| match i["kind"].as_str().unwrap() {
            "separator" => "-".into(),
            "submenu" => format!("{} >", i["label"].as_str().unwrap()),
            _ => i["title"].as_str().unwrap().to_string(),
        })
        .collect()
}

#[test]
fn a_row_s_menu_says_its_commands_in_short() {
    let mut h = harness();
    let items = h.state_mut().menu_items("helios.masks.row");
    assert_eq!(
        titles(&items),
        [
            "Rename…",
            "Invert",
            "Opacity off or full",
            "Lock or unlock",
            "Take out of the mask",
            "-",
            "Add >"
        ]
    );
    assert_eq!(items[0]["id"], "mask.member.rename");
    assert_eq!(items[4]["id"], "mask.member.delete");
    assert_eq!(
        titles(&items[6]["items"]),
        ["Circle", "Ellipse", "Gradient", "Path", "Parametric"]
    );
    assert_eq!(items[6]["items"][4]["id"], "mask.add.parametric");
}

#[test]
fn a_row_s_menu_acts_on_that_row_whatever_is_selected() {
    let mut h = harness();
    click_row(&mut h, "spot");
    assert_eq!(cursor(&h).as_deref(), Some("spot"));
    // The oval's menu turns the oval's opacity off, the spot staying
    // selected.
    let at = row(&h, "oval").center();
    right_click(&mut h, at);
    h.get_by_label("Opacity off or full").click();
    for _ in 0..3 {
        h.step();
    }
    assert!(
        rows(&h).contains(&"  ◎ oval · hidden".to_string()),
        "{:?}",
        rows(&h)
    );
    assert!(rows(&h).contains(&"  ○ spot".to_string()));
    assert_eq!(cursor(&h).as_deref(), Some("spot"));
    // Its rename starts on the oval, from its name.
    let at = row(&h, "oval · hidden").center();
    right_click(&mut h, at);
    h.get_by_label("Rename…").click();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(
        h.state().mask_tree().state().renaming().map(String::as_str),
        Some("oval")
    );
    let field = h.get(by().label_contains("Rename"));
    assert_eq!(field.value().as_deref(), Some("oval"));
}

#[test]
fn a_row_s_eye_and_lock_act_on_that_row() {
    let mut h = harness();
    click_row(&mut h, "dot");
    // The spot's eye turns its opacity off, the dot staying selected.
    h.get_by_label("Hide spot: its opacity off").click();
    for _ in 0..3 {
        h.step();
    }
    assert!(
        rows(&h).contains(&"  ○ spot · hidden".to_string()),
        "{:?}",
        rows(&h)
    );
    assert_eq!(cursor(&h).as_deref(), Some("dot"));
    h.get_by_label("Show spot: its opacity full").click();
    for _ in 0..3 {
        h.step();
    }
    assert!(rows(&h).contains(&"  ○ spot".to_string()));
    // The oval's lock: one undo step; the oval no longer moves, nor
    // leaves the mask.
    h.get_by_label("Lock oval against moves and edits").click();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(
        steps(&h),
        ["opacity of spot", "opacity of spot", "lock oval"]
    );
    assert_eq!(cursor(&h).as_deref(), Some("dot"));
    let (oval, spot) = (row(&h, "oval"), row(&h, "spot"));
    drag(&mut h, oval.center(), spot.center_top() + vec2(0.0, 3.0));
    assert_eq!(wired(&h, "mask"), ["inner", "spot", "oval"]);
    let notices: Vec<String> = h.state().notices().iter().map(|n| n.text.clone()).collect();
    assert!(
        notices
            .iter()
            .any(|n| n.contains("oval") && n.contains("is locked")),
        "{notices:?}"
    );
    // Commands are queued: a refusal is a notice.
    let locked = |h: &Harness<'_, Shell>| {
        h.state()
            .notices()
            .iter()
            .filter(|n| n.text.contains("locked"))
            .count()
    };
    let before = locked(&h);
    run(&mut h, "mask.member.delete", json!({ "member": "oval" }));
    assert_eq!(wired(&h, "mask"), ["inner", "spot", "oval"]);
    assert_eq!(locked(&h), before + 1);
    // Its shape shows no handles on the viewport, and agents cannot
    // drag them.
    run(
        &mut h,
        "mask.select",
        json!({ "mask": "mask", "shape": "oval" }),
    );
    let overlays = h.state().viewport().masks().overlays().to_vec();
    assert_eq!(overlays.len(), 1);
    assert_eq!(overlays[0].id, "oval");
    assert!(overlays[0].handles.is_empty());
    run(
        &mut h,
        "mask.handle.move",
        json!({ "shape": "oval", "handle": "centre", "x": 10.0, "y": 10.0 }),
    );
    assert_eq!(locked(&h), before + 2);
    assert!(!steps(&h).iter().any(|s| s.starts_with("drag oval")));
    // A locked group keeps its members: nothing joins the inner group.
    run(
        &mut h,
        "mask.member.lock",
        json!({ "member": "inner", "locked": true }),
    );
    let (spot, inner) = (row(&h, "spot"), row(&h, "inner — intersection"));
    drag(&mut h, spot.center(), inner.center());
    assert_eq!(wired(&h, "inner"), ["dot"]);
    // Unlocked, the oval moves again.
    run(&mut h, "mask.member.lock", json!({ "member": "oval" }));
    let (oval, spot) = (row(&h, "oval"), row(&h, "spot"));
    drag(&mut h, oval.center(), spot.center_top() + vec2(0.0, 3.0));
    assert_eq!(wired(&h, "mask"), ["inner", "oval", "spot"]);
}

/// The network of [`graph`] with refinements: a blur between the module's
/// mask and its blend, a feather between the spot and its group.
fn refined() -> (Graph, NodeId) {
    use helios_module_masks::{MaskBlur, MaskFeather};
    let (mut g, blend) = graph();
    let (mask, spot) = (g.find("mask").unwrap(), g.find("spot").unwrap());
    let blur = module(&mut g, "soft", ModuleRef::new(MaskBlur), &Params::new());
    g.disconnect(blend, "mask").unwrap();
    g.connect(mask, "mask", blur, "mask").unwrap();
    g.connect(blur, "mask", blend, "mask").unwrap();
    let feather = module(&mut g, "edge", ModuleRef::new(MaskFeather), &Params::new());
    g.disconnect(mask, "in1").unwrap();
    g.connect(spot, "mask", feather, "mask").unwrap();
    g.connect(feather, "mask", mask, "in1").unwrap();
    (g, blend)
}

fn harness_of((g, blend): (Graph, NodeId)) -> Harness<'static, Shell> {
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, blend, "output").unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.step();
    run(&mut h, "helios.masks.focus", json!(null));
    h
}

/// The mask nodes the engine renders for the blend's mask: everything
/// upstream of its `mask` port that is a mask module, by name and module.
fn rendered_mask(h: &Harness<'_, Shell>) -> Vec<(String, String)> {
    let shown = h.state().viewport().shown().unwrap();
    let g = &shown.graph;
    let module = |id: NodeId| g.manifest(id).unwrap().map(|m| m.name).unwrap_or_default();
    let blend = g.find("blend").unwrap();
    let mut todo = vec![g.input_nodes(blend).unwrap()[2].unwrap()];
    let mut out = Vec::new();
    while let Some(id) = todo.pop() {
        let m = module(id);
        if !m.starts_with("mask_") || out.iter().any(|(n, _)| n == g.name(id).unwrap()) {
            continue;
        }
        out.push((g.name(id).unwrap().to_string(), m));
        todo.extend(g.input_nodes(id).unwrap().into_iter().flatten());
    }
    out.sort();
    out
}

#[test]
fn the_panel_lists_every_mask_node_the_pipeline_renders() {
    let mut h = harness_of(refined());
    let check = |h: &Harness<'_, Shell>| {
        let net = h.state().mask_tree().network();
        // Groups, members and refinements: a row each, the same nodes.
        let mut listed: Vec<String> = net.items.values().map(|i| i.node.clone()).collect();
        listed.sort();
        let rendered: Vec<String> = rendered_mask(h).into_iter().map(|(n, _)| n).collect();
        assert_eq!(listed, rendered);
        assert_eq!(rows(h).len(), net.items.len(), "{:?}", rows(h));
    };
    check(&h);
    // Refinements are rows under what they refine: the root's blur after
    // its members, the spot's feather under the spot.
    assert_eq!(
        rows(&h),
        [
            "▣ mask — union",
            "  ▣ inner — intersection",
            "    ○ dot",
            "  ○ spot",
            "    〰 edge — feather",
            "  ◎ oval",
            "  ≈ soft — blur",
        ]
    );
    // Still equal after the network changes: a member moved, one taken
    // out, one added.
    run(
        &mut h,
        "mask.member.move",
        json!({ "member": "oval", "group": "inner" }),
    );
    check(&h);
    run(&mut h, "mask.member.delete", json!({ "member": "spot" }));
    check(&h);
    run(&mut h, "mask.add.parametric", json!(null));
    check(&h);
}

#[test]
fn a_refinement_row_selected_shows_its_parameters() {
    let mut h = harness_of(refined());
    run(&mut h, "helios.params.focus", json!(null));
    click_row(&mut h, "edge — feather");
    // The Parameters panel shows the feather; the viewport edits the
    // mask the spot is in, no shape selected.
    assert_eq!(text(&h, "view/graph/selection"), "edge");
    assert_eq!(cursor(&h).as_deref(), Some("edge"));
    assert_eq!(text(&h, MASK), "mask");
    assert_eq!(text(&h, SHAPE), "");
    let params = h.state().target_rect("helios.params").unwrap();
    assert!(
        h.query_all_by_label("edge")
            .any(|n| params.contains(n.rect().center())),
        "the Parameters panel shows the feather"
    );
    assert!(
        h.state()
            .values
            .lock()
            .unwrap()
            .get("node/edge/radius")
            .is_some()
    );
    // The root's blur, likewise.
    click_row(&mut h, "soft — blur");
    assert_eq!(text(&h, "view/graph/selection"), "soft");
    assert_eq!(text(&h, MASK), "mask");
    assert!(
        h.query_all_by_label("soft")
            .any(|n| params.contains(n.rect().center()))
    );
    // A refinement is neither inverted, hidden nor moved: refused, said.
    run(&mut h, "mask.member.invert", json!({ "member": "soft" }));
    let notices: Vec<String> = h.state().notices().iter().map(|n| n.text.clone()).collect();
    assert!(
        notices
            .iter()
            .any(|n| n.contains("soft") && n.contains("refines")),
        "{notices:?}"
    );
    let net = h.state().mask_tree().network();
    let refused = mask_tree::drop_place(
        &net,
        &["edge".to_string()],
        "oval",
        egui_pop::views::tree::Position::Before,
    )
    .unwrap_err();
    assert_eq!(refused.key, "gui-masks-refinement-stays");
    assert!(steps(&h).is_empty(), "{:?}", steps(&h));
}

#[test]
fn a_member_wired_to_two_ports_shows_once_per_port() {
    let (mut g, blend) = graph();
    let (spot, mask) = (g.find("spot").unwrap(), g.find("mask").unwrap());
    g.connect(spot, "mask", mask, "in3").unwrap();
    let mut h = harness_of((g, blend));
    assert_eq!(
        rows(&h),
        [
            "▣ mask — union",
            "  ▣ inner — intersection",
            "    ○ dot",
            "  ○ spot",
            "  ◎ oval",
            "  ○ spot",
        ]
    );
    // Each row is its own port: the second one's opacity.
    run(
        &mut h,
        "mask.member.opacity",
        json!({ "member": "spot@mask#3", "opacity": 0.5 }),
    );
    assert_eq!(&floats(&h, "node/mask/opacity")[..4], [1.0, 1.0, 1.0, 0.5]);
    assert_eq!(rows(&h)[3], "  ○ spot");
    assert_eq!(rows(&h)[5], "  ○ spot · opacity 50 %");
    // Either row selects the spot on the viewport, and stays selected.
    click_row(&mut h, "spot · opacity 50 %");
    assert_eq!(text(&h, SHAPE), "spot");
    assert_eq!(cursor(&h).as_deref(), Some("spot@mask#3"));
}

/// The exposure through a mask with no member yet, as `--image` opens.
fn empty() -> (Graph, NodeId) {
    use helios_module_masks::{MaskBlend, MaskCombine};
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, |_, _, c| match c {
            0..=2 => 0.2,
            _ => 1.0,
        }),
    );
    let exp = module(
        &mut g,
        "exposure",
        ModuleRef::new(helios_module_exposure::Exposure),
        &Params::new(),
    );
    g.connect(src, "image", exp, "input").unwrap();
    let mask = module(&mut g, "mask", ModuleRef::new(MaskCombine), &Params::new());
    let blend = module(&mut g, "blend", ModuleRef::new(MaskBlend), &Params::new());
    g.connect(src, "image", blend, "input").unwrap();
    g.connect(exp, "output", blend, "processed").unwrap();
    g.connect(mask, "mask", blend, "mask").unwrap();
    (g, blend)
}

fn in_panel(h: &Harness<'_, Shell>, label: &str) -> bool {
    let panel = h.state().target_rect(mask_tree::PANEL).unwrap();
    h.query_all_by_label(label)
        .any(|n| panel.contains(n.rect().center()))
}

#[test]
fn an_empty_mask_says_the_module_applies_everywhere() {
    const EMPTY: &str = "The mask is empty: exposure applies everywhere. \
                         Add a shape or a parametric member to limit it.";
    let mut h = harness_of(empty());
    assert_eq!(rows(&h), ["▣ mask — union"]);
    assert!(in_panel(&h, "Mask of exposure"));
    assert!(in_panel(&h, EMPTY));
    // A member limits it: the message goes.
    run(&mut h, "mask.add.circle", json!(null));
    assert_eq!(rows(&h).len(), 2, "{:?}", rows(&h));
    assert!(!in_panel(&h, EMPTY));
    // Taken out again, the mask is empty again.
    run(&mut h, "mask.member.delete", json!({ "member": "circle" }));
    assert!(in_panel(&h, EMPTY));
    // The panel says whose mask it shows on the full network too.
    let h = harness();
    assert!(in_panel(&h, "Mask of exposure"));
    assert!(
        !h.query_all_by_label_contains("The mask is empty")
            .any(|_| true)
    );
}

fn dialog(h: &mut Harness<'_, Shell>) -> serde_json::Value {
    h.state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap()
}

#[test]
fn taking_out_a_group_that_holds_members_is_asked_first() {
    let mut h = harness();
    // Delete on the inner group, which holds the dot: asked, nothing
    // taken out meanwhile.
    click_row(&mut h, "inner — intersection");
    h.key_press(Key::Delete);
    for _ in 0..3 {
        h.step();
    }
    let asked = dialog(&mut h);
    assert_eq!(asked["kind"], json!("confirm"), "{asked}");
    assert_eq!(asked["verb"], json!("Take out"));
    assert_eq!(wired(&h, "mask"), ["inner", "spot", "oval"]);
    // Cancelled: still there.
    h.state_mut()
        .run("dialog.answer", json!({ "answer": "cancel" }), Origin::User)
        .unwrap();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(wired(&h, "mask"), ["inner", "spot", "oval"]);
    // The row's menu asks too; answered yes, the group goes.
    let at = row(&h, "inner — intersection").center();
    right_click(&mut h, at);
    h.get_by_label("Take out of the mask").click();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(wired(&h, "mask"), ["inner", "spot", "oval"]);
    h.state_mut()
        .run(
            "dialog.answer",
            json!({ "answer": "primary" }),
            Origin::User,
        )
        .unwrap();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(wired(&h, "mask"), ["spot", "oval"]);
    // A shape goes unasked, and so does a group for an agent.
    run(&mut h, "mask.member.delete", json!({ "member": "spot" }));
    assert_eq!(wired(&h, "mask"), ["oval"]);
    assert_eq!(dialog(&mut h), json!(null));
    let mut h = harness();
    h.state_mut()
        .run(
            "mask.member.delete",
            json!({ "member": "inner" }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(wired(&h, "mask"), ["spot", "oval"]);
}

fn float(h: &Harness<'_, Shell>, path: &str) -> f64 {
    h.state()
        .values
        .lock()
        .unwrap()
        .get(path)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

#[test]
fn a_right_click_never_takes_anything_out() {
    let mut h = harness();
    let radius = float(&h, "node/spot/radius");
    // On a row: its menu opens, nothing goes.
    for label in ["spot", "inner — intersection", "dot"] {
        let at = row(&h, label).center();
        right_click(&mut h, at);
        h.key_press(Key::Escape);
        for _ in 0..3 {
            h.step();
        }
    }
    // On a shape's edge on the viewport, selected or not.
    run(
        &mut h,
        "mask.select",
        json!({ "mask": "mask", "shape": "spot" }),
    );
    let m = h.state().viewport().state().mapping().unwrap();
    for at in [[300.0, 140.0], [450.0, 160.0]] {
        right_click(&mut h, m.image_to_screen(at));
        h.key_press(Key::Escape);
        for _ in 0..3 {
            h.step();
        }
    }
    assert_eq!(wired(&h, "mask"), ["inner", "spot", "oval"]);
    assert_eq!(wired(&h, "inner"), ["dot"]);
    assert_eq!(rows(&h).len(), 5, "{:?}", rows(&h));
    assert_eq!(float(&h, "node/spot/radius"), radius);
    assert!(steps(&h).is_empty(), "{:?}", steps(&h));
}

#[test]
fn adding_members_keeps_every_element_of_the_mask() {
    let mut h = harness();
    run(&mut h, "mask.select", json!({ "mask": "mask" }));
    // Two parametric members and a shape join the drawn ones: nothing is
    // discarded, nothing asked.
    run(&mut h, "mask.add.parametric", json!(null));
    run(&mut h, "mask.select", json!({ "mask": "mask" }));
    run(&mut h, "mask.add.parametric", json!(null));
    run(&mut h, "mask.select", json!({ "mask": "mask" }));
    run(&mut h, "mask.add.circle", json!(null));
    assert_eq!(
        wired(&h, "mask"),
        [
            "inner",
            "spot",
            "oval",
            "parametric",
            "parametric2",
            "circle"
        ]
    );
    assert_eq!(wired(&h, "inner"), ["dot"]);
    assert_eq!(float(&h, "node/spot/radius"), 60.0);
    assert_eq!(dialog(&mut h), json!(null));
}

/// Steps frames until the viewport's tiles are all on screen and more
/// than `after` were rendered, waiting for the render threads.
fn rendered_after(h: &mut Harness<'_, Shell>, after: u64) {
    use helios_gui::engine_source::Stats;
    for _ in 0..40 {
        h.step();
        let stats = h.state().viewport_stats();
        assert!(
            stats.wait_idle(std::time::Duration::from_mins(1)),
            "the render thread never answered: {stats:?}"
        );
        h.step();
        if h.state().viewport().state().tiles_complete() && Stats::get(&stats.tiles) > after {
            return;
        }
    }
    panic!("the viewport never rendered again");
}

/// The red of the viewport's picture at an image pixel, the palettes
/// hidden (Tab) so that none lies over it.
fn red_at(h: &mut Harness<'_, Shell>, at: [f64; 2]) -> u8 {
    egui_pop::api::palettes::set_hidden(h.state().app().handle(), true);
    let shot = headless::snapshot(h, "viewport.display").unwrap();
    let r = h.state().target_rect("viewport.display").unwrap();
    let p = h
        .state()
        .viewport()
        .state()
        .mapping()
        .unwrap()
        .image_to_screen(at);
    let ppp = h.state().pixels_per_point();
    shot.get_pixel(
        ((p.x - r.min.x) * ppp) as u32,
        ((p.y - r.min.y) * ppp) as u32,
    )
    .0[0]
}

#[test]
fn a_structure_change_renders_at_once() {
    use helios_gui::engine_source::Stats;
    let mut h = harness();
    let tiles = |h: &Harness<'_, Shell>| Stats::get(&h.state().viewport_stats().tiles);
    rendered_after(&mut h, 0);
    // Inside the spot, the exposure applies.
    let lit = red_at(&mut h, [300.0, 200.0]);
    // The spot moved into the inner group (the minimum with the dot,
    // which it does not meet): no value nudged, the image follows.
    let before = tiles(&h);
    run(
        &mut h,
        "mask.member.move",
        json!({ "member": "spot", "group": "inner" }),
    );
    rendered_after(&mut h, before);
    let moved = red_at(&mut h, [300.0, 200.0]);
    assert!(moved + 20 < lit, "{lit} then {moved}");
    // The oval taken out: its area goes back to the module's input.
    let lit = red_at(&mut h, [450.0, 200.0]);
    let before = tiles(&h);
    run(&mut h, "mask.member.delete", json!({ "member": "oval" }));
    rendered_after(&mut h, before);
    let out = red_at(&mut h, [450.0, 200.0]);
    assert!(out + 20 < lit, "{lit} then {out}");
    assert!(steps(&h).is_empty(), "{:?}", steps(&h));
}

#[test]
fn the_panel_follows_a_new_network_and_undo() {
    let mut h = harness();
    click_row(&mut h, "dot");
    assert_eq!(text(&h, MASK), "inner");
    // Another network replaces this one (another image, an edit opened
    // at another step): the panel shows it, nothing of the old one stays
    // selected or drawn.
    let (g, blend) = empty();
    h.state_mut().set_view_source(g, blend, "output").unwrap();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(rows(&h), ["▣ mask — union"]);
    assert_eq!(cursor(&h), None);
    assert!(h.state().viewport().masks().overlays().is_empty());
    assert!(in_panel(&h, "Mask of exposure"));
    // Back to the first one: its tree again, the dot selected as it was.
    let (g, blend) = graph();
    h.state_mut().set_view_source(g, blend, "output").unwrap();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(rows(&h).len(), 5, "{:?}", rows(&h));
    assert_eq!(cursor(&h).as_deref(), Some("dot"));
    // An undo of a selected member's edit keeps the panel on it.
    run(
        &mut h,
        "mask.member.opacity",
        json!({ "member": "dot", "opacity": 0.5 }),
    );
    assert!(rows(&h).contains(&"    ○ dot · opacity 50 %".to_string()));
    run(&mut h, "param.undo", json!(null));
    assert!(rows(&h).contains(&"    ○ dot".to_string()));
    assert_eq!(cursor(&h).as_deref(), Some("dot"));
    assert_eq!(text(&h, SHAPE), "dot");
}
