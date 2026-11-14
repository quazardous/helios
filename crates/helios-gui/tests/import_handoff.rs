//! The import window hands its import to the node (an import by rule,
//! answered at once with a promise) and may close while it runs; the main
//! window's status bar follows the promise: a spinner, the count of tasks
//! and "Importing 120/412", gone once it settles; a failure leaves a
//! warning badge until it is clicked, which opens the Activity focus on
//! the failed import. Cancel on the import's line stops it; Import
//! again goes through a promise too.
//!
//! The node is the desktop's (`VaultService` in process) with a stand-in
//! for the import's work, held until the test lets it end, so every state
//! of the promise is seen.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::core::indicator;
use helios_core::jobs::Jobs;
use helios_gui::{Shell, headless, helios_panels};
use helios_inspect::client::Client;
use helios_node_core::endpoint::VaultService;
use helios_node_core::promises::Promises;
use helios_shell::commands::Origin;
use helios_storage::Card as Volume;
use helios_vault::{MemoryStore, Vault};
use serde_json::{Value, json};

fn temp(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("helios-gui-handoff-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// How the stand-in import ends, once the test says so.
#[derive(Clone, Debug)]
enum End {
    Done,
    Fail(&'static str),
}

/// The stand-in import's controls: the requests it got, how it ends.
#[derive(Default)]
struct Gate {
    requests: Mutex<Vec<Value>>,
    end: Mutex<Option<End>>,
    cancelled: AtomicBool,
}

/// The desktop's node on a library in memory, whose library folder is
/// `dir/library`, its imports run by the stand-in `gate` controls.
fn node(dir: &Path, gate: &Arc<Gate>) -> Arc<Client> {
    let bus = Arc::new(helios_core::Bus::new());
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let library = dir.join("library");
    std::fs::create_dir_all(&library).unwrap();
    helios_library::import::set_library_root(&vault, &library).unwrap();
    let jobs = Jobs::new(bus.clone());
    let gate = gate.clone();
    let promises = Promises::new(vault.clone(), jobs.clone()).kind("import", true, move |handle| {
        gate.requests.lock().unwrap().push(handle.request().clone());
        handle.progress(120, Some(412), Some("IMG_0120.JPG"));
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if handle.cancel_asked() {
                gate.cancelled.store(true, Ordering::SeqCst);
                handle.cancelled();
                return;
            }
            match gate.end.lock().unwrap().clone() {
                Some(End::Done) => return handle.done(json!({ "report": { "added": 412 } })),
                Some(End::Fail(why)) => return handle.failed(why, true),
                None if Instant::now() > deadline => return handle.failed("timed out", false),
                None => {}
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let service = VaultService::new(vault, bus, jobs, "test").with_promises(Arc::new(promises));
    Arc::new(Client::in_process(Arc::new(service)).with_origin("user"))
}

/// A shell on `node` watching one card, `EOS_DIGITAL` under `dir` with
/// one photo, the import window open on it once it is read through.
fn card_window(dir: &Path, node: Arc<Client>) -> Harness<'static, Shell> {
    let card = dir.join("EOS_DIGITAL");
    let dcim = card.join("DCIM/100CANON");
    std::fs::create_dir_all(&dcim).unwrap();
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../helios-plugin/tests/fixtures/rated-photos/a.jpg"),
        dcim.join("IMG_0001.JPG"),
    )
    .unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.follow_activity(node);
    shell.watch_cards(Arc::new(vec![Volume {
        label: "EOS_DIGITAL".into(),
        root: card,
        capacity: Some(64_000_000_000),
    }]));
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    until(&mut h, "the card listed", |h| {
        !h.state().import_cards().is_empty()
    });
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::I);
    h.step();
    until(&mut h, "the card read", |h| h.state().import_read());
    h.run_steps(3);
    h
}

/// Steps frames until `done` holds, for ten seconds at most.
fn until(h: &mut Harness<'_, Shell>, what: &str, done: impl Fn(&Harness<'_, Shell>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("never got there: {what}");
}

fn shown(h: &Harness<'_, Shell>, id: &str) -> Option<indicator::Record> {
    indicator::find(&h.ctx, id).filter(|r| r.shown)
}

/// Clicks indicator `id` of the status bar.
fn click(h: &mut Harness<'_, Shell>, id: &str) {
    let [x, y, w, hh] = shown(h, id).unwrap_or_else(|| panic!("no {id}")).rect;
    let centre = egui::pos2(x + w / 2.0, y + hh / 2.0);
    h.hover_at(centre);
    h.step();
    h.drag_at(centre);
    h.step();
    h.drop_at(centre);
    h.step();
}

/// Presses Import, and waits for the stand-in import to start.
fn import(h: &mut Harness<'_, Shell>, gate: &Gate) {
    h.get_by_label("Import 1").click();
    h.run_steps(3);
    until(h, "the import started on the node", |_| {
        !gate.requests.lock().unwrap().is_empty()
    });
}

#[test]
fn import_hands_off_to_a_promise_the_window_may_close_and_the_status_bar_follows_it() {
    let dir = temp("handoff");
    let gate = Arc::new(Gate::default());
    let mut h = card_window(&dir, node(&dir, &gate));
    // Nothing runs: no indicator.
    assert!(shown(&h, helios_gui::activity::MARK).is_none());
    import(&mut h, &gate);
    // Handed to the node, not queued for the host: answered at once.
    assert!(h.state_mut().take_imports().is_empty());
    let request = gate.requests.lock().unwrap()[0].clone();
    let photo = dir.join("EOS_DIGITAL/DCIM/100CANON/IMG_0001.JPG");
    assert_eq!(request["rule"], json!({ "items": [photo] }), "{request}");
    assert_eq!(request["copy"], json!(true), "{request}");
    assert_eq!(request["darktable"], json!(true), "{request}");
    assert_eq!(request["origin"], json!("user"), "{request}");
    // The window stays, the import's line in it.
    assert!(h.state().import_window_open());
    until(&mut h, "the import's line", |h| {
        h.query_by_label("120 of 412").is_some()
    });
    // The status bar: the count of tasks and the main one's progress.
    until(&mut h, "the indicator", |h| {
        h.query_by_label("Importing 120/412").is_some()
    });
    let running = shown(&h, helios_gui::activity::MARK).unwrap();
    assert_eq!(running.value, Some(1.0));
    // The window closes; the import goes on, still in the status bar.
    let ctx = h.ctx.clone();
    h.state_mut().close_import_window(&ctx);
    h.run_steps(3);
    assert!(!h.state().import_window_open());
    assert!(h.query_by_label("Importing 120/412").is_some());
    // It ends: the indicator goes.
    *gate.end.lock().unwrap() = Some(End::Done);
    until(&mut h, "the indicator gone", |h| {
        shown(h, helios_gui::activity::MARK).is_none()
            && h.query_by_label("Importing 120/412").is_none()
    });
    assert!(shown(&h, helios_gui::activity::FAILED_MARK).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_on_the_import_line_stops_it() {
    let dir = temp("cancel");
    let gate = Arc::new(Gate::default());
    let mut h = card_window(&dir, node(&dir, &gate));
    import(&mut h, &gate);
    until(&mut h, "the import's line", |h| {
        h.query_by_label("120 of 412").is_some()
    });
    // The line's Cancel, above the window's own.
    let line_cancel = h
        .get_all_by_label("Cancel")
        .min_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y))
        .unwrap();
    line_cancel.click();
    until(&mut h, "the import cancelled", |_| {
        gate.cancelled.load(Ordering::SeqCst)
    });
    until(&mut h, "its outcome", |h| {
        h.query_by_label("Cancelled").is_some()
    });
    until(&mut h, "the indicator gone", |h| {
        shown(h, helios_gui::activity::MARK).is_none()
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_import_leaves_a_badge_until_it_is_clicked() {
    let dir = temp("failed");
    let gate = Arc::new(Gate::default());
    let mut h = card_window(&dir, node(&dir, &gate));
    import(&mut h, &gate);
    let ctx = h.ctx.clone();
    h.state_mut().close_import_window(&ctx);
    h.run_steps(3);
    *gate.end.lock().unwrap() = Some(End::Fail("the disk is full"));
    until(&mut h, "the failure's badge", |h| {
        shown(h, helios_gui::activity::FAILED_MARK).is_some()
    });
    assert!(shown(&h, helios_gui::activity::MARK).is_none());
    let badge = shown(&h, helios_gui::activity::FAILED_MARK).unwrap();
    assert_eq!(badge.value, Some(1.0));
    // A click opens the Activity focus on the failed import, with its
    // reason and Retry; the badge goes.
    click(&mut h, helios_gui::activity::FAILED_MARK);
    until(&mut h, "the Activity focus", |h| {
        h.state().activity_focus_state()["open"] == true
    });
    let failed = h.state().activity_focus_state()["selected"].clone();
    assert!(failed.is_string(), "the failure selected: {failed}");
    until(&mut h, "the outcome", |h| {
        h.query_all_by_label("Failed: the disk is full").count() > 0
    });
    assert!(h.query_by_label("Retry").is_some());
    until(&mut h, "the badge gone", |h| {
        shown(h, helios_gui::activity::FAILED_MARK).is_none()
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_again_goes_through_a_promise() {
    let dir = temp("again");
    let gate = Arc::new(Gate::default());
    let node = node(&dir, &gate);
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    // The history holds one import of `photos`.
    let record = helios_library::import_history::Record {
        id: "import-1".into(),
        sources: vec![photos.clone()],
        choices: json!({ "copy": false, "duplicates": "skip" }),
        origin: "user".into(),
        ..Default::default()
    };
    let reader: helios_gui::shell::ImportHistoryReader = Arc::new(move || Ok(vec![record.clone()]));
    let mut shell = Shell::new(helios_panels(), None);
    shell.follow_activity(node);
    shell.set_import_history(reader);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run_steps(3);
    // The command runs at the coming frame.
    h.state_mut()
        .run(
            "import.history.again",
            json!({ "id": "import-1" }),
            Origin::User,
        )
        .unwrap();
    until(&mut h, "the import started on the node", |_| {
        !gate.requests.lock().unwrap().is_empty()
    });
    assert!(h.state_mut().take_imports().is_empty(), "not queued");
    let request = gate.requests.lock().unwrap()[0].clone();
    assert_eq!(request["rule"], json!({ "items": [photos] }), "{request}");
    until(&mut h, "the indicator", |h| {
        h.query_by_label("Importing 120/412").is_some()
    });
    *gate.end.lock().unwrap() = Some(End::Done);
    until(&mut h, "the indicator gone", |h| {
        shown(h, helios_gui::activity::MARK).is_none()
    });
    let _ = std::fs::remove_dir_all(&dir);
}
