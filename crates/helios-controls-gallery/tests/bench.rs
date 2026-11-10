//! The bench headless: a control dragged from the shelf onto the bench,
//! reordered and removed in the layout mode, a value changed through the
//! flow and undone, the `controls.bench.*` commands, and a composed bench
//! at two widths.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::egui::accesskit::Role;
use egui_pop::egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use helios_controls_gallery::bench_ui::BenchPage;
use serde_json::json;

fn harness(page: BenchPage) -> Harness<'static, BenchPage> {
    Harness::builder()
        .with_size(vec2(1100.0, 900.0))
        .build_ui_state(|ui, p: &mut BenchPage| p.ui(ui), page)
}

/// A pointer drag from `from` to `to`, in small moves, released there.
fn drag(h: &mut Harness<'_, BenchPage>, from: Pos2, to: Pos2) {
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(button(from, true));
    h.step();
    for i in 1..=10 {
        let t = i as f32 / 10.0;
        h.event(Event::PointerMoved(from + (to - from) * t));
        h.step();
    }
    h.event(button(to, false));
    h.step();
    h.event(Event::PointerGone);
    h.run();
}

/// The middle of the bench, from its width handle.
fn bench_middle(h: &Harness<'_, BenchPage>) -> Pos2 {
    let handle = h
        .get_by_role(egui_pop::egui::accesskit::Role::Splitter)
        .rect();
    Pos2::new(
        handle.left() - h.state().bench.width / 2.0,
        handle.top() + 20.0,
    )
}

fn order(h: &Harness<'_, BenchPage>) -> Vec<String> {
    h.state()
        .bench
        .composition
        .keys()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn page_with(commands: &[serde_json::Value]) -> BenchPage {
    let mut page = BenchPage::default();
    for c in commands {
        page.bench
            .run("controls.bench.add", c)
            .expect("the control is on the shelf");
    }
    page
}

#[test]
fn a_drag_from_the_shelf_adds_the_control_where_it_is_dropped() {
    let mut h = harness(BenchPage::default());
    h.run();
    let from = h.get_by_label("Shelf: Exposure").rect().center();
    let to = bench_middle(&h);
    drag(&mut h, from, to);
    assert_eq!(order(&h), ["exposure"]);
    // A toggle dropped at the left of the exposure goes before it.
    let from = h.get_by_label("Shelf: Preserve colours").rect().center();
    let exposure = h.get_by_role_and_label(Role::Slider, "Exposure").rect();
    drag(&mut h, from, exposure.left_center() + vec2(4.0, 0.0));
    assert_eq!(order(&h), ["toggle", "exposure"]);
    assert!(
        h.query_by_role_and_label(Role::CheckBox, "Preserve colours")
            .is_some(),
        "drawn live"
    );
}

#[test]
fn arranging_reorders_by_dragging_and_removes() {
    let mut h = harness(page_with(&[
        json!({"control": "exposure"}),
        json!({"control": "number"}),
        json!({"control": "toggle"}),
    ]));
    h.run();
    h.get_by_label("Arrange").click();
    h.run();
    let from = h.get_by_label("Move toggle").rect().center();
    let to = h.get_by_label("Move exposure").rect().left_center() + vec2(6.0, 0.0);
    drag(&mut h, from, to);
    assert_eq!(order(&h), ["toggle", "exposure", "number"]);
    h.get_by_label("Remove exposure").click();
    h.run();
    assert_eq!(order(&h), ["toggle", "number"]);
    assert!(h.query_by_label("Move exposure").is_none());
}

#[test]
fn arranging_groups_selected_controls_into_a_case() {
    let mut h = harness(page_with(&[
        json!({"control": "exposure"}),
        json!({"control": "number"}),
        json!({"control": "toggle"}),
    ]));
    h.run();
    h.get_by_label("Arrange").click();
    h.run();
    h.get_by_label("Move number").click();
    h.run();
    h.get_by_label("Move toggle").click();
    h.run();
    h.get_by_label("Group into case").click();
    h.run();
    let c = &h.state().bench.composition;
    assert_eq!(c.entries.len(), 2);
    let case = c.case("case-1").expect("the case");
    assert_eq!(case.parts.len(), 2);
    assert!(case.separator);
    assert!(
        h.query_by_label("Move case-1").is_some(),
        "its legend moves it"
    );
}

#[test]
fn a_value_changes_through_the_flow_and_undoes() {
    let mut h = harness(page_with(&[json!({"control": "exposure"})]));
    h.run();
    h.get_by_role_and_label(Role::Slider, "Exposure")
        .set_value_accesskit(0.7);
    h.run();
    assert_eq!(h.state().bench.readout("exposure"), "+0.70 EV");
    assert!(h.query_by_label("exposure: +0.70 EV").is_some(), "read out");
    assert_eq!(h.state().bench.store.undo_steps().len(), 1);
    h.get_by_label("Undo").click();
    h.run();
    assert_eq!(h.state().bench.readout("exposure"), "0.00 EV");
    assert!(h.query_by_label("exposure: 0.00 EV").is_some());
    h.get_by_label("Redo").click();
    h.run();
    assert_eq!(h.state().bench.readout("exposure"), "+0.70 EV");
}

#[test]
fn the_width_handle_resizes_the_bench_and_the_placements_follow() {
    // Alone, the slider grows across the columns the bench gives it.
    let mut h = harness(page_with(&[json!({"control": "exposure"})]));
    h.run();
    let before = h
        .get_by_role_and_label(Role::Slider, "Exposure")
        .rect()
        .width();
    let handle = h
        .get_by_role(egui_pop::egui::accesskit::Role::Splitter)
        .rect()
        .center();
    drag(&mut h, handle, handle - vec2(240.0, 0.0));
    assert!(
        (h.state().bench.width - 240.0).abs() < 1.0,
        "{}",
        h.state().bench.width
    );
    let after = h
        .get_by_role_and_label(Role::Slider, "Exposure")
        .rect()
        .width();
    assert!(after < before, "{after} < {before}");
}

#[test]
fn the_bench_commands_build_and_list_a_bench_through_the_gallery() {
    let mut g = helios_controls_gallery::Gallery::default();
    let run = |g: &mut helios_controls_gallery::Gallery, name: &str, args| {
        g.run_command(name, &args).unwrap()
    };
    run(&mut g, "controls.bench.add", json!({"control": "exposure"}));
    run(&mut g, "controls.bench.add", json!({"control": "curve"}));
    run(
        &mut g,
        "controls.bench.add",
        json!({"control": "choice", "at": 0}),
    );
    run(
        &mut g,
        "controls.bench.move",
        json!({"key": "curve", "to": 0}),
    );
    let list = run(&mut g, "controls.bench.list", json!({}));
    assert_eq!(list["order"], json!(["curve", "choice", "exposure"]));
    assert_eq!(list["controls"][0]["slot"], "1x6", "a curve's block");
    run(&mut g, "controls.bench.remove", json!({"key": "choice"}));
    let list = run(&mut g, "controls.bench.list", json!({}));
    assert_eq!(list["order"], json!(["curve", "exposure"]));
    run(&mut g, "controls.bench.clear", json!({}));
    let list = run(&mut g, "controls.bench.list", json!({}));
    assert_eq!(list["order"], json!([]));
    // The other controls commands still reach the gallery's store.
    assert!(g.run_command("controls.catalogue", &json!({})).is_ok());
}

/// A bench of a case and loose controls.
fn composed(width: f32) -> BenchPage {
    let mut page = page_with(&[
        json!({"control": "exposure"}),
        json!({"control": "number"}),
        json!({"control": "toggle"}),
        json!({"control": "choice"}),
        json!({"control": "whole"}),
        json!({"control": "colour"}),
    ]);
    let b = &mut page.bench;
    b.run(
        "controls.bench.case",
        &json!({"keys": ["number", "toggle", "choice"], "legend": "Mix", "help": "How the result is blended"}),
    )
    .unwrap();
    b.set_width(width);
    page
}

#[test]
fn a_composed_bench_at_two_widths() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (width, name) in [(240.0, "bench-narrow"), (480.0, "bench-wide")] {
        let mut h = Harness::builder()
            .with_size(vec2(width + 24.0, 400.0))
            .build_ui_state(|ui, p: &mut BenchPage| p.bench_zone(ui), composed(width));
        h.run();
        let list = h.state_mut().bench.list().unwrap();
        for c in list["controls"].as_array().unwrap() {
            assert_eq!(c["shown"], true, "{name}: {} shown", c["key"]);
        }
        assert!(h.query_by_label("Mix").is_some(), "{name}: the legend");
        snapshot_into(&mut h, name, &mut results);
    }
}

#[test]
fn controls_context_and_the_context_bar_share_the_keys() {
    let mut g = helios_controls_gallery::Gallery::default();
    g.run_command("controls.bench.sample", &json!({})).unwrap();
    g.run_command(
        "controls.context",
        &json!({"task": "retouch", "detail": "expert"}),
    )
    .unwrap();
    let list = g.run_command("controls.bench.list", &json!({})).unwrap();
    assert_eq!(list["context"]["task"], "retouch");
    let black = &list["controls"][1];
    assert_eq!(black["key"], "black");
    assert_eq!(black["explained"]["visibility"], "folded");
    // The bench's box is what `controls.context` reports.
    let cx = g.run_command("controls.context", &json!({})).unwrap();
    assert_eq!(cx["keys"]["box"], list["context"]["box"]);

    // The bar sets the shared keys too: a click on its density.
    let mut h = harness(BenchPage::new(
        helios_controls_gallery::bench::Bench::with_contexts(
            helios_controls::HeliosRegistry::standard(),
            g.bench.bench.contexts(),
        ),
    ));
    h.run();
    h.get_by_label("raw").click();
    h.run();
    assert!(!g.contexts().keys.image.raw, "unticked through the bar");
}

/// The scenarios' bench in a retouch context, its black level changed on
/// a picture: shown (changed), contrast folded by a rule, the profile by
/// its tier.
fn explained_page() -> BenchPage {
    let mut b = helios_controls_gallery::bench::Bench::default();
    b.load_sample();
    b.nudge("black", 20, "pointer").unwrap();
    b.set_context(
        &json!({"task": "retouch", "image": {"raw": false, "edited": true, "masked": false}}),
    )
    .unwrap();
    BenchPage::new(b)
}

#[test]
fn snapshots_of_the_context_bar_and_the_reason_pane() {
    let mut results = egui_kittest::SnapshotResults::new();
    let mut h = Harness::builder()
        .with_size(vec2(760.0, 90.0))
        .build_ui_state(|ui, p: &mut BenchPage| p.context_bar(ui), explained_page());
    h.run();
    assert!(h.query_by_role_and_label(Role::ComboBox, "Task").is_some());
    assert!(h.query_by_label_contains("Space: regular").is_some());
    snapshot_into(&mut h, "bench-context-bar", &mut results);

    let mut h = Harness::builder()
        .with_size(vec2(260.0, 720.0))
        .build_ui_state(
            |ui, p: &mut BenchPage| {
                p.last = p.bench.resolve().ok().map(|r| r.0);
                p.reasons(ui);
            },
            explained_page(),
        );
    h.run();
    assert!(h.query_by_label("Contrast · folded").is_some());
    assert!(h.query_by_label("Black level · shown").is_some());
    snapshot_into(&mut h, "bench-reasons", &mut results);
    results.unwrap();
}

#[test]
fn the_mode_switch_shows_the_hud_as_chips() {
    let mut page = BenchPage::default();
    page.bench.load_sample();
    let mut h = harness(page);
    h.run();
    h.get_by_label("HUD").click();
    h.run();
    assert!(h.query_by_label("HUD chip: Exposure 0.00 EV").is_some());
    assert!(
        h.query_by_role_and_label(Role::Slider, "Exposure")
            .is_none()
    );
    h.get_by_label("Toolbar").click();
    h.run();
    assert_eq!(
        h.state().bench.keys().card_box.surface,
        helios_controls::Surface::Toolbar
    );
    assert!(
        h.query_by_role_and_label(Role::Slider, "Exposure")
            .is_some()
    );
}

/// The harness's snapshot, compared with `tests/snapshots/<name>.png`;
/// a new one is written to the bench box's results (as in `gallery.rs`).
fn snapshot_into<S>(
    h: &mut Harness<'_, S>,
    name: &str,
    results: &mut egui_kittest::SnapshotResults,
) {
    let committed = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.png"));
    let out = std::path::Path::new("/out");
    if !committed.exists() && std::env::var_os("UPDATE_SNAPSHOTS").is_none() && out.is_dir() {
        let image = h.render().expect("the bench renders");
        let dir = out.join("snapshots");
        std::fs::create_dir_all(&dir).unwrap();
        image.save(dir.join(format!("{name}.png"))).unwrap();
        eprintln!("new snapshot {name}: written to /out/snapshots, to commit");
        return;
    }
    results.add(h.try_snapshot(name));
}
