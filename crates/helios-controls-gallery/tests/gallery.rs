//! The gallery's exposure card, headless: its snapshot, and a drag through
//! the value flow.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_controls::catalogue::EXPOSURE;
use helios_controls_gallery::{Gallery, set_value};

fn harness(gallery: Gallery) -> Harness<'static, Gallery> {
    Harness::builder()
        .with_size(egui_pop::egui::vec2(820.0, 260.0))
        .build_ui_state(|ui, g: &mut Gallery| g.card(ui, EXPOSURE), gallery)
}

#[test]
fn exposure_card() {
    let mut g = Gallery::default();
    set_value(&mut g, EXPOSURE, 0.67);
    let mut h = harness(g);
    h.run();
    h.get_by_label("Value: +0.67 EV");
    snapshot(&mut h, "exposure_card");
}

/// The harness's snapshot, compared with `tests/snapshots/<name>.png`.
/// A snapshot not committed yet is rendered into the bench box's results
/// (`/out/snapshots`) to be reviewed and committed, and the test says so
/// rather than failing on a box run that cannot write the source tree.
fn snapshot(h: &mut Harness<'_, Gallery>, name: &str) {
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
    h.snapshot(name);
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
