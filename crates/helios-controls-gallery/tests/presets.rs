//! The gallery's "Helios UI" page headless: every `helios_ui` preset in
//! its states, dark and light, as a snapshot, and its clicks reported.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_pop::core::motion;
use egui_pop::egui;
use egui_pop::theme::{self, ThemeSpec};
use helios_controls_gallery::presets::PresetsPage;
use helios_ui::{Action, Empty, Search};

fn harness(theme_name: &str) -> Harness<'static, PresetsPage> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(900.0, 1700.0))
        .build_ui_state(
            |ui, page: &mut PresetsPage| {
                page.content(ui);
            },
            PresetsPage::default(),
        );
    // A given clock: no animation asks for frames (set once: setting it
    // asks for a frame).
    let _clock = motion::freeze(&h.ctx, 0.0);
    theme::install(&h.ctx, &ThemeSpec::new(theme_name));
    h.run();
    h
}

#[test]
fn the_page_shows_every_preset() {
    let h = harness("pop-dark");
    for search in Search::ALL {
        h.get_by_label(search.label());
    }
    for case in Empty::ALL {
        h.get_by_label(case.title());
    }
    h.get_by_label("Retry");
    h.get_by_label("Album: Trip");
    // The presets of the Activity window: a choice named by what it
    // chooses, a details grid, bars and spinners named after their job,
    // row actions.
    assert_eq!(
        h.get_by_label("Kind shown").accesskit_node().role(),
        egui::accesskit::Role::ComboBox
    );
    h.get_by_label("Started");
    h.get_by_label("12 Oct 2026, 18:42");
    for name in ["Import in a row", "Preview in a row", "Export wide"] {
        h.get_all_by_label(name).next().expect(name);
    }
    for kind in Action::ALL {
        h.get_by_label(&format!("{} in a row", kind.name()));
    }
    h.get_by_label("18:43:00  failed: IMG_0025.CR3, disk full");
    // The late values, one per state.
    for name in ["Photos", "Free space", "Last backup", "Archive size"] {
        h.get_by_role_and_label(egui::accesskit::Role::Status, name);
    }
}

#[test]
fn a_failed_late_value_reports_its_retry() {
    let mut h = harness("pop-dark");
    h.get_by_label("Retry Archive size").click();
    h.run();
    assert_eq!(h.state().last.as_deref(), Some("value Archive size retry"));
}

#[test]
fn a_row_action_is_reported() {
    let mut h = harness("pop-dark");
    h.get_by_label("danger in a row").click();
    h.run();
    assert_eq!(h.state().last.as_deref(), Some("row action danger"));
}

#[test]
fn a_click_on_a_preset_is_reported() {
    let mut h = harness("pop-dark");
    h.get_by_label("Import photos…").click();
    h.run();
    assert_eq!(h.state().last.as_deref(), Some("empty no-photos"));
}

#[test]
fn presets_page_snapshots() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (theme_name, name) in [("pop-dark", "presets-dark"), ("pop-light", "presets-light")] {
        let mut h = harness(theme_name);
        snapshot_into(&mut h, name, &mut results);
    }
}

/// A snapshot, or, when none is committed yet and the box gives an
/// output folder, the new image written there to be committed.
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
        let image = h.render().expect("the page renders");
        let dir = out.join("snapshots");
        std::fs::create_dir_all(&dir).unwrap();
        image.save(dir.join(format!("{name}.png"))).unwrap();
        eprintln!("new snapshot {name}: written to /out/snapshots, to commit");
        return;
    }
    results.add(h.try_snapshot(name));
}
