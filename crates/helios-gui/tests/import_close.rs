//! Closing the import window never holds the interface: whatever work the
//! window left going (a cold folder being glanced, a source's connection
//! waiting, a card coming and going, a source's choices being written to
//! a busy node), the next frame comes within a deadline and the work is
//! cancelled or left to its own thread, never waited for.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_connect::account::{MemoryAccounts, MemorySecrets};
use helios_connect::fake::FakeProvider;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_storage::{Card, Volumes};
use serde_json::{Value, json};

/// How long the frame after the close may take.
const DEADLINE: Duration = Duration::from_millis(500);

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-gui-close-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `n` JPEGs in `folder`, each its own content, without an Exif
/// thumbnail: each glance decodes one whole.
fn photos(folder: &Path, n: usize, width: u32, height: u32) {
    std::fs::create_dir_all(folder).unwrap();
    for i in 0..n {
        let img = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x as usize + i) as u8, (y * 3) as u8, (i * 4) as u8])
        });
        img.save(folder.join(format!("IMG_{i:04}.JPG"))).unwrap();
    }
}

fn until(h: &mut Harness<'_, Shell>, what: &str, done: impl Fn(&mut Harness<'_, Shell>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("never got there: {what}");
}

fn open_on(h: &mut Harness<'_, Shell>, source: &Path) {
    h.state_mut()
        .run(
            "library.importWindow",
            json!({ "source": source }),
            Origin::User,
        )
        .unwrap();
    h.step();
}

/// Clicks the import window's close button and steps the frames after
/// it: each comes within [`DEADLINE`], and the window is closed.
fn close_in_time(h: &mut Harness<'_, Shell>, what: &str) {
    h.get_by_role_and_label(Role::Button, "Close window")
        .click();
    frames_in_time(h, what);
}

/// The frames after a close: each comes within [`DEADLINE`], and the
/// window is closed.
fn frames_in_time(h: &mut Harness<'_, Shell>, what: &str) {
    for frame in 0..3 {
        let at = Instant::now();
        h.step();
        let took = at.elapsed();
        assert!(
            took < DEADLINE,
            "{what}: frame {frame} after the close took {took:?}"
        );
    }
    assert!(!h.state().import_window_open(), "{what}: still open");
}

#[test]
fn closing_while_a_cold_folder_is_glanced_comes_back_at_once_and_stops_the_glances() {
    let dir = temp("cold");
    let folder = dir.join("Shoot");
    photos(&folder, 96, 1600, 1200);
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    open_on(&mut h, &folder);
    until(&mut h, "the glances going", |h| {
        let g = h.state().import_glances();
        g.read + g.cached > 0
    });
    assert!(
        !h.state().import_read(),
        "the folder read through before the close: make it bigger"
    );
    close_in_time(&mut h, "a cold folder glanced");
    // The scans are dropped: their threads see the stop at their next file.
    let g = h.state().import_glances();
    assert_eq!(g.read + g.cached, 0, "a scan left: {g:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn connection(h: &mut Harness<'_, Shell>) -> Value {
    h.state_mut()
        .run("import.connection", Value::Null, Origin::User)
        .unwrap()
}

#[test]
fn closing_while_a_sources_connection_waits_gives_it_up_and_comes_back_at_once() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.add_import_connector(Arc::new(FakeProvider));
    shell.set_import_accounts(
        Arc::new(MemoryAccounts::default()),
        Arc::new(MemorySecrets::default()),
    );
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run("library.importWindow", Value::Null, Origin::User)
        .unwrap();
    until(&mut h, "Add a source…", |h| {
        h.query_by_label("Add a source…").is_some()
    });
    h.get_by_label("Add a source…").click();
    until(&mut h, "the connectors offered", |h| {
        h.query_by_label("Test server").is_some()
    });
    h.get_by_label("Test server").click();
    // The connection's thread waits for the form's answer.
    until(&mut h, "the form", |h| connection(h)["state"] == "form");
    // Closed by the system's close button: the connection's dialog over
    // the window holds the headless window's own close button back.
    let ctx = h.ctx.clone();
    let at = Instant::now();
    h.state_mut().close_import_window(&ctx);
    assert!(at.elapsed() < DEADLINE, "the close took {:?}", at.elapsed());
    frames_in_time(&mut h, "a connection waiting");
    assert!(connection(&mut h).is_null(), "the connection still shown");
}

/// Volumes looked at slowly, as a card being mounted or pulled out can
/// be: each look takes a while on the watch's thread.
#[derive(Default)]
struct SlowVolumes(Mutex<Vec<Card>>);

impl Volumes for SlowVolumes {
    fn volumes(&self) -> Vec<helios_storage::Volume> {
        std::thread::sleep(Duration::from_millis(300));
        self.0.lock().unwrap().volumes()
    }
}

#[test]
fn closing_while_a_card_comes_and_goes_comes_back_at_once() {
    let dir = temp("card");
    let card = Card {
        label: "EOS_DIGITAL".into(),
        root: dir.join("EOS_DIGITAL"),
        capacity: Some(64_000_000_000),
    };
    photos(&card.root.join("DCIM/100CANON"), 24, 1600, 1200);
    let volumes = Arc::new(SlowVolumes::default());
    let mut shell = Shell::new(helios_panels(), None);
    shell.watch_cards(volumes.clone());
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    volumes.0.lock().unwrap().push(card.clone());
    until(&mut h, "the card listed", |h| {
        !h.state().import_cards().is_empty()
    });
    h.state_mut()
        .run("library.importWindow", Value::Null, Origin::User)
        .unwrap();
    until(&mut h, "the card read", |h| {
        let g = h.state().import_glances();
        g.read + g.cached > 0 || h.state().import_read()
    });
    // Pulled out while it is read, and the window closed meanwhile.
    volumes.0.lock().unwrap().clear();
    close_in_time(&mut h, "a card pulled out");
    // Plugged in again with the window closed: frames still in time.
    volumes.0.lock().unwrap().push(card);
    let deadline = Instant::now() + Duration::from_secs(20);
    while h.state().import_cards().is_empty() {
        assert!(Instant::now() < deadline, "the card not seen again");
        let at = Instant::now();
        h.step();
        assert!(at.elapsed() < DEADLINE, "a frame took {:?}", at.elapsed());
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_sources_choices_written_to_a_busy_node_hold_neither_a_frame_nor_the_close() {
    let dir = temp("choices");
    let folder = dir.join("trips");
    photos(&folder, 2, 64, 48);
    // The node's vault held by another writer: each write waits 3 s.
    let written = Arc::new(Mutex::new(None::<String>));
    let into = written.clone();
    let mut shell = Shell::new(helios_panels(), None);
    shell.keep_import_sources_with(
        Box::new(|| Ok(None)),
        helios_gui::shell::write_import_sources_in_background(move |text| {
            std::thread::sleep(Duration::from_secs(3));
            *into.lock().unwrap() = Some(text.to_owned());
            Ok(())
        }),
    );
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    open_on(&mut h, &folder);
    until(&mut h, "the folder read", |h| h.state().import_read());
    h.run_steps(2);
    // A choice changed: saved on the source, through the node.
    h.get_by_label("Keep").click();
    let at = Instant::now();
    h.step();
    assert!(
        at.elapsed() < DEADLINE,
        "the choice's frame took {:?}",
        at.elapsed()
    );
    close_in_time(&mut h, "a source's choices being written");
    // The write lands all the same, off the interface's thread.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(text) = written.lock().unwrap().clone() {
            assert!(text.contains("trips") && text.contains("keep"), "{text}");
            break;
        }
        assert!(Instant::now() < deadline, "the choices never written");
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
