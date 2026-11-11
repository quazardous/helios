//! The Compare focus, driven as a user drives it, through the
//! accessibility tree: three photos selected in the Library, its Compare
//! button opens them side by side in the Compare place, zoom follows
//! across the photos until the sync is turned off, the arrow keys move
//! the active photo, and the active photo is rated from its marks and
//! from the keys. File > New workspace lists "Compare", and a workspace
//! made from it opens in Compare.
//!
//! The snapshots (`tests/snapshots/compare-*.png`, thresholds in the
//! workspace's `kittest.toml`) are regenerated on the bench box:
//! `UPDATE_SNAPSHOTS=1 cargo test -p helios-gui --test compare_focus`
//! (the box's `gui-snapshots` job with `test=compare_focus`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::{Key, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use helios_gui::library::Entry;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_thumbs::{Key as ThumbKey, ThumbCache};
use serde_json::{Value, json};

const PHOTOS: [&str; 3] = ["a", "b", "c"];

fn names() -> Vec<String> {
    PHOTOS.iter().map(ToString::to_string).collect()
}

fn entries() -> Vec<Entry> {
    PHOTOS
        .iter()
        .enumerate()
        .map(|(n, id)| Entry {
            id: (*id).into(),
            name: format!("IMG_000{}.jpg", n + 1),
            ..Entry::default()
        })
        .collect()
}

/// A folder of its own for each window (the same name each run: the
/// window's title shows it).
fn temp(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("helios-gui-compare-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("photos")).unwrap();
    dir
}

/// A workspace's window with three photos, all selected, drawn headless.
fn library(name: &str, scale: f32) -> Harness<'static, Shell> {
    let dir = temp(name);
    let mut shell = Shell::builder(helios_panels(), Some(dir.join("ui")))
        .workspace(&dir.join("photos"))
        .build();
    shell.load_layout().unwrap();
    shell.set_library(entries());
    shell.library_mut().select_ids(&names());
    let mut h = headless::harness_at(shell, vec2(1280.0, 800.0), scale);
    h.run();
    h
}

fn run(h: &mut Harness<'_, Shell>, command: &str, args: Value) -> Value {
    h.state_mut()
        .run(command, args, Origin::User)
        .unwrap_or_else(|e| panic!("{command}: {e}"))
}

/// The place shown, as the `when` clauses read it.
fn place(h: &Harness<'_, Shell>) -> String {
    h.state().app().handle().context().get("place").to_string()
}

fn rating(h: &Harness<'_, Shell>, id: &str) -> u8 {
    h.state().library().entry(id).unwrap().rating as u8
}

/// Zooms by `factor` with the pointer over the photo named `name`.
fn zoom_over(h: &mut Harness<'_, Shell>, name: &str, factor: f32) {
    h.get_by_role_and_label(Role::Unknown, name).hover();
    h.run();
    h.event(egui::Event::Zoom(factor));
    h.run();
}

#[test]
fn compare_three_photos_sync_their_zoom_and_rate_the_active_one() {
    let mut h = library("scenario", 1.0);
    run(&mut h, "helios.vault.focus", json!(null));
    h.run();
    // The Library's Compare button, as a user clicks it.
    h.get_by_role_and_label(Role::Button, "Compare").click();
    h.run();
    h.run();
    let photos = h.state().comparison().photos().to_vec();
    assert_eq!(photos, names(), "the three selected photos compared");
    assert_eq!(place(&h), "helios.comparison");
    // The first photo is active and selected: the culling keys mark it.
    assert_eq!(h.state().library().selection(), ["a"]);
    // Synchronised by default: zooming one zooms every photo.
    assert!(h.state().comparison().synced);
    zoom_over(&mut h, "IMG_0001.jpg", 2.0);
    let zooms = |h: &Harness<'_, Shell>| {
        let c = h.state().comparison();
        [0, 1, 2].map(|i| c.view(i).zoom)
    };
    assert_eq!(zooms(&h), [2.0, 2.0, 2.0]);
    // Off, through its checkbox: the next zoom stays on its photo.
    h.get_by_role_and_label(Role::CheckBox, "Sync zoom and pan")
        .click();
    h.run();
    assert!(!h.state().comparison().synced);
    zoom_over(&mut h, "IMG_0003.jpg", 3.0);
    let z = zooms(&h);
    assert_eq!(z[2], 3.0, "{z:?}");
    assert_ne!(z[0], 3.0, "{z:?}");
    // On again: every photo shares the view.
    h.get_by_role_and_label(Role::CheckBox, "Sync zoom and pan")
        .click();
    h.run();
    assert!(h.state().comparison().synced);
    // The arrow keys move the active photo, the selection with it.
    run(&mut h, "helios.compare.focus", json!(null));
    h.run();
    h.key_press(Key::ArrowRight);
    h.run();
    h.run();
    assert_eq!(h.state().library().selection(), ["b"]);
    h.key_press(Key::ArrowLeft);
    h.key_press(Key::ArrowLeft);
    h.run();
    h.run();
    assert_eq!(h.state().library().selection(), ["c"], "wraps around");
    h.key_press(Key::ArrowLeft);
    h.run();
    h.run();
    assert_eq!(h.state().library().selection(), ["b"]);
    // Rated from its marks: the stars of the active photo.
    h.get_by_role_and_label(Role::Slider, "Active photo's rating")
        .click();
    h.run();
    let stars = rating(&h, "b");
    assert!((1..=5).contains(&stars), "rated from its stars: {stars}");
    assert_eq!((rating(&h, "a"), rating(&h, "c")), (0, 0));
    // And from the keys, as in the Library.
    run(&mut h, "helios.compare.focus", json!(null));
    h.run();
    h.key_press(Key::Num4);
    h.run();
    assert_eq!(rating(&h, "b"), 4);
    assert_eq!((rating(&h, "a"), rating(&h, "c")), (0, 0));
}

#[test]
fn compare_needs_two_known_photos_and_leaves_with_its_place() {
    let mut h = library("needs-two", 1.0);
    h.state().library_mut().select_ids(&["a".to_string()]);
    // Refused (its error goes to the notes): one photo, an unknown one.
    for args in [json!(null), json!({ "images": ["a", "zz"] })] {
        run(&mut h, "library.compare", args);
        h.run();
        assert!(h.state().comparison().photos().is_empty());
        assert_ne!(place(&h), "helios.comparison");
    }
    // By ids, as an agent asks; the first is active.
    run(&mut h, "library.compare", json!({ "images": ["c", "a"] }));
    h.run();
    assert_eq!(h.state().comparison().photos(), ["c", "a"]);
    assert_eq!(h.state().library().selection(), ["c"]);
    // Leaving the place leaves the photos: the Compare tab compares the
    // selection again.
    run(&mut h, "view.library", json!(null));
    h.run();
    assert!(h.state().comparison().photos().is_empty());
}

#[test]
fn new_workspace_offers_compare_and_a_compare_workspace_opens_in_it() {
    let mut h = library("template", 1.0);
    run(&mut h, "workspace.new", json!(null));
    h.run();
    let picker = run(&mut h, "dialog.current", json!(null));
    let option = picker["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["value"] == json!("helios.compare"))
        .unwrap_or_else(|| panic!("Compare is offered: {picker}"));
    assert_eq!(option["label"], json!("Compare"));
    // The template's library says the workspace opens in Compare.
    let mut h = library("template-opened", 1.0);
    let compare = helios_gui::templates::find("helios.compare").unwrap();
    h.state_mut()
        .apply_template_library(compare.template.library.as_ref());
    h.run();
    h.run();
    assert_eq!(place(&h), "helios.comparison");
    // No photos chosen: the selection's three are compared.
    assert!(h.state().comparison().photos().is_empty());
    let layout = h.state().layout();
    assert_eq!(
        layout.area_of(&egui_pop::model::ids::PanelId::new("helios.compare").unwrap()),
        Some(egui_pop::model::panels::Area::Center)
    );
}

/// Three photos of distinct colours, their large thumbnails cached.
fn shown_in_colour(h: &mut Harness<'static, Shell>) -> helios_thumbs::Loader {
    let dir = std::env::temp_dir().join(format!("helios-gui-compare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = Arc::new(ThumbCache::open(&dir.join("thumbs"), 1 << 30).unwrap());
    for (n, id) in PHOTOS.iter().enumerate() {
        let (w, hgt) = (helios_thumbs::LARGE, 683);
        let shade = [[170, 90, 60], [70, 140, 90], [60, 100, 170]][n];
        let pixels = (0..w * hgt)
            .flat_map(|p| {
                let x = (p % w) * 60 / w;
                shade.map(|c: u32| (c + x) as u8)
            })
            .collect();
        let large = helios_thumbs::Rgb8 {
            width: w,
            height: hgt,
            pixels,
        };
        cache
            .put(&ThumbKey::preview(*id), helios_thumbs::LARGE, &large)
            .unwrap();
    }
    let loader = helios_gui::thumbnails::start(h.state(), Some(cache), &[], |_| {});
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        h.step();
        if PHOTOS
            .iter()
            .all(|id| h.state().library().large_thumbnail(id).is_some())
        {
            break;
        }
        assert!(Instant::now() < deadline, "the large entries within 30 s");
        std::thread::sleep(Duration::from_millis(20));
    }
    h.run();
    loader
}

#[test]
fn compare_snapshots() {
    let mut results = egui_kittest::SnapshotResults::new();
    for scale in [1.0, 2.0] {
        let mut h = library(&format!("shots-{scale}"), scale);
        run(&mut h, "library.compare", json!(null));
        h.run();
        let loader = shown_in_colour(&mut h);
        // The second photo active, rated, picked.
        run(&mut h, "compare.next", json!(null));
        run(&mut h, "vault.rate.3", json!(null));
        run(&mut h, "vault.flag.pick", json!(null));
        h.run();
        let name = format!("compare-three-pop-dark-{}x", scale as u32);
        results.add(h.try_snapshot(&name));
        if scale == 1.0 {
            // Zoomed, synchronised: every photo shows the same detail.
            zoom_over(&mut h, "IMG_0001.jpg", 2.5);
            results.add(h.try_snapshot("compare-zoomed-pop-dark-1x"));
        }
        drop(loader);
    }
}
