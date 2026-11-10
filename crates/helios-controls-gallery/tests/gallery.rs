//! The gallery headless: the exposure card through the managed
//! renderers, each of its views on its own (widths, representations,
//! orientations, variants), the implicit card wide and compact (a curve
//! and a wheel drawn by their editors, then by their value type's rows),
//! its "More (n)" row revealing a part through `controls.reveal`, and the
//! value flow behind a drag.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_pop::egui::accesskit::{Orientation, Role};
use helios_controls::catalogue::EXPOSURE;
use helios_controls::render::Drawn;
use helios_controls::slot::Fallback;
use helios_controls_gallery::{Fit, Gallery, VIEWS, View, set_value};

type Implicit = (Gallery, Vec<(String, Drawn)>);

fn harness(gallery: Gallery) -> Harness<'static, Gallery> {
    Harness::builder()
        .with_size(egui_pop::egui::vec2(1000.0, 360.0))
        .build_ui_state(|ui, g: &mut Gallery| g.card(ui, EXPOSURE), gallery)
}

fn exposure_at_067() -> Gallery {
    let mut g = Gallery::default();
    set_value(&mut g, EXPOSURE, 0.67);
    g
}

#[test]
fn exposure_card() {
    let mut h = harness(exposure_at_067());
    h.run();
    h.get_by_label("Value: +0.67 EV");
    snapshot(&mut h, "exposure_card");
}

/// One view of the exposure control alone, in a window its box's size.
fn view_harness(view: &'static View) -> Harness<'static, Gallery> {
    let width = view.width as f32 / 12.0 * 240.0 + 16.0;
    let height = if view.slot.is_some() { 120.0 } else { 44.0 };
    Harness::builder()
        .with_size(egui_pop::egui::vec2(width, height))
        .build_ui_state(
            move |ui, g: &mut Gallery| g.view(ui, EXPOSURE, view),
            exposure_at_067(),
        )
}

#[test]
fn every_view_of_exposure_draws_its_value_in_its_box() {
    let mut results = egui_kittest::SnapshotResults::new();
    for view in &VIEWS {
        let mut h = view_harness(view);
        h.run();
        let p = h.state().resolve_view(EXPOSURE, view).unwrap();
        let placed = p.get(EXPOSURE).unwrap();
        assert_eq!(
            placed.representation,
            Some(view.representation),
            "{}",
            view.name
        );
        assert_eq!(placed.orientation, Some(view.orientation), "{}", view.name);
        // Named by its label at every size: a narrow slot puts the label
        // in the tooltip, never out of the accessibility tree.
        let node = h.get_by_label("Exposure");
        assert_eq!(node.accesskit_node().role(), Role::Slider, "{}", view.name);
        let v = node.accesskit_node().numeric_value().unwrap();
        assert!((v - 0.67).abs() < 1e-9, "{}: {v}", view.name);
        let vertical = node.accesskit_node().orientation() == Some(Orientation::Vertical);
        assert_eq!(
            vertical,
            view.orientation == helios_controls::Orientation::Vertical,
            "{}",
            view.name
        );
        let rect = placed.rect.unwrap();
        assert!(
            node.rect().width() <= rect.w + 0.5,
            "{}: drawn within its slot",
            view.name
        );
        snapshot_into(&mut h, &format!("exposure-{}", view.name), &mut results);
    }
}

#[test]
fn a_drag_up_on_the_vertical_view_raises_exposure_as_one_undo_step() {
    use egui_pop::egui::{Event, Modifiers, PointerButton, vec2};
    let view = VIEWS.iter().find(|v| v.name == "vertical").unwrap();
    let mut h = view_harness(view);
    h.run();
    h.state_mut().store.clear_history();
    let at = h.get_by_label("Exposure").rect().center();
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(Event::PointerMoved(at));
    h.step();
    h.event(button(at, true));
    h.step();
    for i in 1..=6 {
        h.event(Event::PointerMoved(at - vec2(0.0, 5.0 * i as f32)));
        h.step();
    }
    h.event(button(at - vec2(0.0, 30.0), false));
    h.run();
    assert!(h.state().value(EXPOSURE) > 0.67, "up is more");
    assert_eq!(h.state().store.undo_steps().len(), 1);
}

fn implicit_harness(fit: Fit) -> Harness<'static, Implicit> {
    let width = if fit == Fit::Wide { 500.0 } else { 260.0 };
    Harness::builder()
        .with_size(egui_pop::egui::vec2(width, 420.0))
        .build_ui_state(
            move |ui, (g, drawn): &mut Implicit| {
                *drawn = g.implicit_fit(ui, fit).into_iter().collect();
            },
            (Gallery::default(), Vec::new()),
        )
}

fn drawn(h: &Harness<'_, Implicit>, key: &str) -> Drawn {
    h.state()
        .1
        .iter()
        .find(|(k, _)| k == key)
        .map_or_else(|| panic!("`{key}` not drawn"), |(_, d)| d.clone())
}

#[test]
fn the_implicit_card_draws_its_curve_and_wheel_wide_and_falls_back_compact() {
    let mut results = egui_kittest::SnapshotResults::new();
    let mut h = implicit_harness(Fit::Wide);
    h.run();
    assert_eq!(drawn(&h, "curve"), Drawn::Integrated);
    assert_eq!(drawn(&h, "lift"), Drawn::Integrated);
    assert_eq!(drawn(&h, "amount"), Drawn::Managed);
    assert!(h.query_by_label("Preserve colours").is_some());
    snapshot_into(&mut h, "implicit_card_wide", &mut results);

    let mut h = implicit_harness(Fit::Compact);
    h.run();
    assert_eq!(drawn(&h, "curve"), Drawn::Fallback(Fallback::ValueType));
    assert_eq!(drawn(&h, "lift"), Drawn::Fallback(Fallback::ValueType));
    // The curve's points as numbered rows, the wheel's channels.
    assert!(h.query_by_label("Curve 1 x").is_some());
    assert!(h.query_by_label("Lift Luminance").is_some());
    snapshot_into(&mut h, "implicit_card_compact", &mut results);
}

#[test]
fn the_more_row_counts_the_hidden_part_and_reveals_it_through_controls_reveal() {
    let mut h = implicit_harness(Fit::Wide);
    h.run();
    assert!(
        !h.state().1.iter().any(|(k, _)| k == "notes"),
        "hidden by the layout"
    );
    h.get_by_label("More, 1").click();
    h.run();
    h.get_by_label("Show Notes").click();
    h.run();
    h.run();
    assert!(
        h.state()
            .0
            .contexts()
            .moments
            .get("implicit")
            .is_some_and(|m| m.revealed.contains("notes")),
        "controls.reveal ran"
    );
    assert!(
        h.query_by_label("More, 1").is_none(),
        "nothing left under More"
    );
    assert_eq!(drawn(&h, "notes"), Drawn::Managed);
}

#[test]
fn a_toggle_on_the_implicit_card_is_written_to_its_parameter() {
    let mut h = implicit_harness(Fit::Wide);
    h.run();
    h.get_by_label("Preserve colours").click();
    h.run();
    assert_eq!(
        h.state().0.implicit_value("preserve"),
        Some(helios_core::Value::Bool(false))
    );
}

/// The harness's snapshot, compared with `tests/snapshots/<name>.png`.
/// A snapshot not committed yet is rendered into the bench box's results
/// (`/out/snapshots`) to be reviewed and committed, and the test says so
/// rather than failing on a box run that cannot write the source tree.
fn snapshot<S>(h: &mut Harness<'_, S>, name: &str) {
    let mut results = egui_kittest::SnapshotResults::new();
    snapshot_into(h, name, &mut results);
}

/// [`snapshot`] into a test's results, for a test with several harnesses.
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
        let image = h.render().expect("the card renders");
        let dir = out.join("snapshots");
        std::fs::create_dir_all(&dir).unwrap();
        image.save(dir.join(format!("{name}.png"))).unwrap();
        eprintln!("new snapshot {name}: written to /out/snapshots, to commit");
        return;
    }
    results.add(h.try_snapshot(name));
}

#[test]
fn a_reset_on_the_card_is_one_undo_step() {
    let mut g = Gallery::default();
    set_value(&mut g, EXPOSURE, 1.0);
    g.store.clear_history();
    let mut h = harness(g);
    h.run();
    let control = h.state().registry.get(EXPOSURE).cloned().unwrap();
    h.state_mut().apply(
        &control,
        &[
            helios_controls::render::Change {
                to: 0.5,
                cause: helios_controls::flow::Cause::Scrub,
                done: false,
                cancelled: false,
            },
            helios_controls::render::Change {
                to: 0.25,
                cause: helios_controls::flow::Cause::Scrub,
                done: true,
                cancelled: false,
            },
        ],
    );
    assert_eq!(h.state().value(EXPOSURE), 0.25);
    assert_eq!(h.state().store.undo_steps().len(), 1);
    h.state_mut().apply(
        &control,
        &[
            helios_controls::render::Change {
                to: 2.0,
                cause: helios_controls::flow::Cause::Scrub,
                done: false,
                cancelled: false,
            },
            helios_controls::render::Change {
                to: 0.25,
                cause: helios_controls::flow::Cause::Scrub,
                done: true,
                cancelled: true,
            },
        ],
    );
    assert_eq!(h.state().value(EXPOSURE), 0.25, "a cancel puts it back");
    assert_eq!(h.state().store.undo_steps().len(), 1, "and leaves no step");
}
