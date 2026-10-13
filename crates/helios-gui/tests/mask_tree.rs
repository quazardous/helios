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
fn the_tree_shows_the_module_s_mask_with_icons_and_operators() {
    let h = harness();
    assert_eq!(
        rows(&h),
        [
            "▣ mask — maximum",
            "  ▣ inner — minimum",
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
    assert!(rows(&h).contains(&"  ○ spot · 50 %".to_string()));
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
    click_row(&mut h, "mask — maximum");
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
    click_row(&mut h, "spot · 50 %");
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
        rows(&h).contains(&"  ○ Sun · 50 %".to_string()),
        "{:?}",
        rows(&h)
    );
    assert_eq!(steps(&h), ["opacity of spot", "rename spot to Sun"]);
    // The node keeps its name: the viewport and the store still find it.
    assert_eq!(text(&h, SHAPE), "spot");
    run(&mut h, "param.undo", json!(null));
    assert!(rows(&h).contains(&"  ○ spot · 50 %".to_string()));
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
    let (from, to) = (row(&h, "oval · 25 %"), row(&h, "spot"));
    drag(&mut h, from.center(), to.center_top() + vec2(0.0, 3.0));
    assert_eq!(wired(&h, "mask"), ["inner", "oval", "spot"]);
    assert_eq!(&floats(&h, "node/mask/opacity")[..3], [1.0, 0.25, 1.0]);
    assert_eq!(
        &rows(&h)[3..],
        ["  ◎ oval · 25 %", "  ○ spot"],
        "the tree shows the network's order"
    );
    // Into another group: the spot joins the inner group.
    let (from, inner) = (row(&h, "spot"), row(&h, "inner — minimum"));
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
    let (oval, root) = (row(&h, "oval"), row(&h, "mask — maximum"));
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

#[test]
fn a_row_s_menu_acts_on_that_row_whatever_is_selected() {
    let mut h = harness();
    click_row(&mut h, "spot");
    assert_eq!(cursor(&h).as_deref(), Some("spot"));
    // The oval's menu inverts the oval, the spot staying selected.
    let at = row(&h, "oval").center();
    right_click(&mut h, at);
    h.get_by_label("Masks: invert the selected member (or `member`), or back")
        .click();
    for _ in 0..3 {
        h.step();
    }
    assert!(
        rows(&h).contains(&"  ◎ oval · inverted".to_string()),
        "{:?}",
        rows(&h)
    );
    assert!(rows(&h).contains(&"  ○ spot".to_string()));
    assert_eq!(cursor(&h).as_deref(), Some("spot"));
    // Its rename starts on the oval, from its name.
    let at = row(&h, "oval · inverted").center();
    right_click(&mut h, at);
    h.get_by_label(
        "Masks: name the selected member (or `member`) `label`; without it, rename in the Masks panel",
    )
    .click();
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
