//! Exporting photos from the desktop: File > Export… (Ctrl+Shift+E), the
//! Library's context menu and the palette ask for the settings; the
//! export runs as a batch job with an export per image, in the jobs
//! centre and the Queue panel of the node exported; an image that fails
//! goes to the Log panel; the end is a toast offering the folder;
//! cancelling the batch cancels its exports; the "Batch export" template
//! opens on the picks with the export ready.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::api::jobs::JOBS;
use egui_pop::model::jobs::JobState;
use helios_core::{Bus, Describe as _, Image, Region};
use helios_export::Format;
use helios_gui::dialogs::Level;
use helios_gui::export::Exporter;
use helios_gui::persist::Persist;
use helios_gui::queue::ItemState;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use helios_vault::MemoryStore;
use serde_json::{Value, json};

const LONG: Duration = Duration::from_mins(1);

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-gui-export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A small picture written as a PNG file.
fn picture(path: &Path, shade: f32) {
    let image = Image::from_fn(Region::sized(48, 32), 4, |x, y, c| {
        if c == 3 {
            1.0
        } else {
            (shade + (x + y) as f32 / 200.0).min(1.0)
        }
    });
    let bytes = helios_export::encode(&image, &Format::Png { bits: 8 }).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn run(h: &mut Harness<'_, Shell>, id: &str, args: Value) -> Value {
    let out = h
        .state_mut()
        .run(id, args, Origin::User)
        .unwrap_or_else(|e| panic!("{id}: {e}"));
    for _ in 0..3 {
        h.run();
    }
    out
}

fn dialog(h: &mut Harness<'_, Shell>) -> Value {
    h.state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap()
}

fn field_keys(form: &Value) -> Vec<String> {
    form["fields"]
        .as_array()
        .unwrap_or_else(|| panic!("not a form: {form}"))
        .iter()
        .map(|f| f["key"].as_str().unwrap().to_string())
        .collect()
}

/// A library of the pictures named (`broken` ones are not pictures), the
/// shell showing it, and an exporter over it.
struct Desk {
    h: Harness<'static, Shell>,
    exporter: Exporter<MemoryStore>,
}

fn desk(dir: &Path, names: &[&str], workers: usize) -> Desk {
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    let persist = Persist::new(MemoryStore::default(), Arc::new(Bus::new()));
    for (i, name) in names.iter().enumerate() {
        let path = photos.join(name);
        if name.starts_with("broken") {
            std::fs::write(&path, b"not a picture").unwrap();
        } else {
            picture(&path, i as f32 / 10.0);
        }
        persist.vault().import_file(&path, 1).unwrap();
    }
    let mut shell = Shell::new(helios_panels(), None);
    persist.fill_library(&mut shell).unwrap();
    // On the viewport's engine, as the desktop does.
    let exporter = Exporter::with_engine(
        persist.vault().clone(),
        shell.jobs(),
        shell.export_sink(),
        || {},
        workers,
        shell.viewport().engine().clone(),
    );
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    Desk { h, exporter }
}

impl Desk {
    /// Runs the exports the shell asked for, as the window's host does.
    fn run_exports(&mut self) -> Vec<String> {
        let bundle = self
            .h
            .state()
            .workspace_bundle()
            .unwrap_or_else(helios_template::workspace::blank);
        let asks = self.h.state_mut().take_exports();
        asks.iter()
            .map(|a| self.exporter.run(&bundle, a).unwrap())
            .collect()
    }

    /// Until the exports ended and the shell heard of it.
    fn finish(&mut self) {
        assert!(self.exporter.wait(LONG), "the exports end");
        for _ in 0..5 {
            self.h.run();
        }
    }
}

#[test]
fn file_export_is_in_the_file_menu_on_ctrl_shift_e_and_in_the_library_s_menu() {
    let dir = temp("menu");
    let mut d = desk(&dir, &["a.png", "b.png"], 1);
    let items =
        d.h.state_mut()
            .menu_items(egui_pop::api::app::MENU_FILE.as_str());
    let export = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "library.export")
        .unwrap_or_else(|| panic!("no File > Export…: {items}"))
        .clone();
    assert_eq!(export["title"], json!("Export…"), "{export}");
    assert!(export["keys"].is_string(), "{export}");
    // In the palette's list: a command like any other.
    assert!(d.h.state().specs().iter().any(|s| s.id == "library.export"));

    // Ctrl+Shift+E asks: which photos, the format and its settings, the
    // folder, the names and what a name taken does.
    d.h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::E);
    for _ in 0..3 {
        d.h.run();
    }
    let form = dialog(&mut d.h);
    assert_eq!(form["command"], json!("library.export"), "{form}");
    assert_eq!(
        field_keys(&form),
        [
            "scope",
            "format",
            "quality",
            "bits",
            "long_edge",
            "space",
            "folder",
            "pattern",
            "clash"
        ]
    );
    let out = dir.join("out");
    run(
        &mut d.h,
        "dialog.answer",
        json!({ "values": {
            "scope": "shown", "format": "png", "bits": "16", "long_edge": 24,
            "space": "display-p3", "folder": out, "pattern": "{stem}-web", "clash": "number",
        } }),
    );
    let asks = d.h.state_mut().take_exports();
    assert_eq!(asks.len(), 1);
    let ask = &asks[0];
    assert_eq!(ask.images.len(), 2, "every image shown");
    assert_eq!(ask.settings.output.format, Format::Png { bits: 16 });
    assert_eq!(ask.settings.output.long_edge, Some(24));
    assert_eq!(ask.settings.folder, out);
    assert_eq!(ask.settings.pattern, "{stem}-web");

    // The Library's context menu offers it, on its images.
    run(&mut d.h, "helios.vault.focus", json!(null));
    d.h.query_all_by_label("Images")
        .next()
        .expect("the library's grid")
        .click_secondary();
    for _ in 0..3 {
        d.h.run();
    }
    assert!(
        d.h.query_all_by_label_contains("Export…").count() > 0,
        "Export… in the library's menu"
    );
    drop(d);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_export_is_a_batch_with_an_export_per_image_and_a_failure_in_the_log() {
    let dir = temp("batch");
    let mut d = desk(&dir, &["a.png", "broken.png", "c.png"], 2);
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    // `c.jpg` is taken: numbered.
    std::fs::write(out.join("c.jpg"), b"there").unwrap();
    run(
        &mut d.h,
        "library.export",
        json!({ "scope": "shown", "format": "jpeg", "quality": 80, "folder": out }),
    );
    let ids = d.run_exports();
    assert_eq!(ids.len(), 1, "one batch for the three images");
    d.finish();

    // The files: the names decided when asked, JPEG.
    for name in ["a.jpg", "c-2.jpg"] {
        let bytes = std::fs::read(out.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(&bytes[..2], [0xFF, 0xD8], "{name} is a JPEG");
    }
    assert!(!out.join("broken.jpg").exists());

    // The jobs centre: the batch, failed for its one image, its exports
    // under it.
    let app = d.h.state().app().handle().clone();
    let centre = app.service(&JOBS).expect("the jobs plugin");
    let batch = centre.job(&ids[0]).expect("the batch in the jobs centre");
    assert_eq!(batch.title, "Export 3 photos");
    assert_eq!(batch.kind, "batch");
    assert_eq!(batch.state, JobState::Failed);
    assert_eq!(batch.errors.len(), 1);
    assert_eq!(batch.errors[0].item, "broken.png");
    let children: Vec<_> = centre
        .all()
        .into_iter()
        .filter(|j| j.parent.as_deref() == Some(ids[0].as_str()))
        .collect();
    assert_eq!(children.len(), 3);

    // The Queue panel of the node exported (selected for it): an item
    // per image.
    run(&mut d.h, "helios.queue.focus", json!(null));
    assert_eq!(d.h.state().queue_panel().node(), Some("blend"));
    let states: Vec<ItemState> =
        d.h.state()
            .queue_panel()
            .queue()
            .map(|q| q.items.iter().map(|i| i.state).collect())
            .unwrap_or_default();
    let done = states.iter().filter(|s| **s == ItemState::Done).count();
    let failed = states.iter().filter(|s| **s == ItemState::Failed).count();
    assert_eq!((done, failed), (2, 1), "{states:?}");

    // The failure in the Log panel with its reason; the end said, with
    // the folder offered.
    let log = d.h.state().log().entries().to_vec();
    let failure = log
        .iter()
        .find(|e| e.message.key == "gui-export-image-failed")
        .expect("the failure in the Log panel");
    assert_eq!(failure.level, Level::Error);
    assert!(failure.message.english.contains("broken.png"));
    let end =
        d.h.state()
            .notices()
            .iter()
            .rev()
            .find(|n| n.key == "gui-export-ended-failed")
            .cloned()
            .expect("the end said");
    assert!(end.text.contains("2 photos exported"), "{}", end.text);
    assert_eq!(
        end.action.as_ref().map(|a| a.1.as_str()),
        Some(helios_gui::shell::OPEN_FOLDER)
    );
    drop(d);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancelling_the_batch_cancels_its_exports() {
    let dir = temp("cancel");
    let mut d = desk(&dir, &["a.png", "b.png", "c.png"], 1);
    let out = dir.join("out");
    // Held: nothing starts before the cancel.
    d.exporter.pause();
    run(
        &mut d.h,
        "library.export",
        json!({ "scope": "shown", "format": "tiff", "folder": out }),
    );
    let ids = d.run_exports();
    d.h.run();
    // As the jobs centre's Cancel does (`job.cancel`).
    d.h.state().jobs().cancel(&ids[0]).unwrap();
    d.h.run();
    d.exporter.resume();
    d.finish();
    let app = d.h.state().app().handle().clone();
    let centre = app.service(&JOBS).unwrap();
    assert_eq!(centre.job(&ids[0]).unwrap().state, JobState::Cancelled);
    let children: Vec<_> = centre
        .all()
        .into_iter()
        .filter(|j| j.parent.as_deref() == Some(ids[0].as_str()))
        .collect();
    assert_eq!(children.len(), 3);
    assert!(children.iter().all(|c| c.state == JobState::Cancelled));
    let written = std::fs::read_dir(&out).map_or(0, Iterator::count);
    assert_eq!(written, 0, "nothing written");
    assert!(
        d.h.state()
            .notices()
            .iter()
            .any(|n| n.key == "gui-export-cancelled"),
        "the cancel said"
    );
    drop(d);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_batch_export_template_opens_on_the_picks_with_the_export_ready() {
    let root = temp("template");
    let config = root.join("config");
    let shell = Shell::new(helios_panels(), None);
    let catalogue = templates::catalogue(&shell.template_panels(), shell.specs());
    drop(shell);
    let created = templates::create_workspace(
        &templates::find("helios.batch-export").unwrap(),
        &catalogue,
        &root.join("work"),
        &config,
    )
    .unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&created.workspace)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &created.workspace).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    // The library on the picks.
    let flags = h.state().library().filter().flags.clone();
    assert_eq!(flags, [egui_pop::views::library::Flag::Pick]);
    // The export ready: the dialog starts from the workspace's settings,
    // which "Remember" offers to keep.
    let settings = h.state().export_settings();
    assert_eq!(settings.folder.as_deref(), Some("~/Pictures/Helios export"));
    run(
        &mut h,
        "library.export",
        json!({ "images": ["ab".repeat(32)] }),
    );
    let form = dialog(&mut h);
    let keys = field_keys(&form);
    assert!(keys.contains(&"remember".to_string()), "{keys:?}");
    assert!(!keys.contains(&"scope".to_string()), "the images are given");
    let folder = form["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == "folder")
        .unwrap()
        .clone();
    assert_eq!(
        folder["value"]["text"],
        json!("~/Pictures/Helios export"),
        "{folder}"
    );
    // Remembered: the workspace keeps the new settings.
    let out = root.join("out");
    run(
        &mut h,
        "dialog.answer",
        json!({ "values": { "format": "png", "folder": out, "remember": true } }),
    );
    let kept = h.state().export_settings();
    assert_eq!(kept.format, "png");
    assert_eq!(kept.folder.as_deref(), out.to_str());
    let asks = h.state_mut().take_exports();
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0].images, ["ab".repeat(32)]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn exporting_the_image_open_reads_no_source_again_and_stays_within_the_budget() {
    let dir = temp("shared-engine");
    let mut d = desk(&dir, &["a.png"], 1);
    let engine = d.h.state().viewport().engine().clone();
    assert!(Arc::ptr_eq(&engine, d.exporter.engine()), "one engine");
    let bundle =
        d.h.state()
            .workspace_bundle()
            .unwrap_or_else(helios_template::workspace::blank);
    // The darkroom shows the image whole, on the desktop's engine.
    let (graph, node) = templates::open_file(&dir.join("photos").join("a.png"), &bundle).unwrap();
    let extent = graph.extent(node).unwrap().unwrap();
    engine.render(&graph, node, "output", extent).unwrap();
    let budget = 1 << 20;
    engine.set_budget(Some(budget));
    let before = engine.counters();
    let out = dir.join("out");
    run(
        &mut d.h,
        "library.export",
        json!({ "scope": "shown", "format": "tiff", "folder": out }),
    );
    let ids = d.run_exports();
    assert_eq!(ids.len(), 1);
    d.finish();
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1, "exported");
    let since = engine.counters().since(&before);
    assert_eq!(since.source_reads, 0, "the source is read again: {since:?}");
    assert_eq!(since.computed, 0, "computed again: {since:?}");
    let held = engine.describe()["cache"]["bytes"].as_u64().unwrap();
    assert!(held <= budget as u64, "{held} bytes over {budget}");
}
