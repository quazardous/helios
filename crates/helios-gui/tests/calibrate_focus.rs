//! The Calibration focus on a synthetic chart picture: opened by its
//! command, its corner handles dragged on the picture re-solve the chart,
//! the report shows, "Save the profile" keeps a profile through the node
//! (heard on its bus as the person's, one undoable step of the library's
//! history), the profile list's actions work, and with real viewports the
//! focus opens in an OS window of its own.

use egui::{Event, Modifiers, PointerButton, Pos2, ViewportId, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::api::windows::WindowId;
use helios_calibrate::lab::lab_to_xyz;
use helios_calibrate::{Layout, XRITE_CC24_2014 as CHART, working_to_xyz};
use helios_gui::shell::{CALIBRATE_COMMAND, CALIBRATE_WINDOW};
use helios_gui::{Shell, calibrate, headless, helios_panels};
use helios_photo::color::{apply, inverse};
use helios_shell::commands::Origin;
use helios_vault::{MemoryStore, Vault};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Where the chart's corner patches are in the picture.
const CORNERS: [[f64; 2]; 4] = [[60.0, 52.0], [340.0, 40.0], [348.0, 216.0], [48.0, 208.0]];
const SIZE: (usize, usize) = (400, 260);

/// A chart picture written as a TIFF in a folder of its own.
fn chart_picture(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("helios-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let from_xyz = inverse(&working_to_xyz()).unwrap();
    let values: Vec<[f64; 3]> = CHART
        .patches
        .iter()
        .map(|p| apply(&from_xyz, lab_to_xyz(p.lab)).map(|v| 0.7 * v))
        .collect();
    let (w, h) = SIZE;
    let data = Layout::new(&CHART, CORNERS)
        .unwrap()
        .draw(w, h, &values, 0.01);
    let image = helios_core::Image::new(helios_core::Region::sized(w as u32, h as u32), 3, data);
    let picture = dir.join("chart.tif");
    std::fs::write(
        &picture,
        helios_export::encode(&image, &helios_export::Format::Tiff).unwrap(),
    )
    .unwrap();
    (dir, picture)
}

struct Setup {
    vault: Arc<Vault<MemoryStore>>,
    bus: Arc<helios_core::Bus>,
    history: helios_gui::library_history::LibraryHistory,
}

/// A shell with the calibration commands run by a node over `vault`, as
/// the desktop wires them.
fn shell() -> (Shell, Setup) {
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let bus = Arc::new(helios_core::Bus::new());
    let mut shell = Shell::new(helios_panels(), None);
    let history = shell.library_history();
    {
        let vault = vault.clone();
        history.set_apply(Arc::new(move |changes, forward| {
            if forward {
                vault.replay(changes)
            } else {
                vault.revert(changes)
            }
        }));
    }
    calibrate::register(
        &mut shell,
        Some(Arc::new(
            helios_inspect::client::Client::in_process(Arc::new(
                helios_node_core::endpoint::VaultService::new(
                    vault.clone(),
                    bus.clone(),
                    helios_core::jobs::Jobs::new(bus.clone()),
                    "test",
                ),
            ))
            .with_origin("user"),
        )),
        history.clone(),
    )
    .unwrap();
    (
        shell,
        Setup {
            vault,
            bus,
            history,
        },
    )
}

/// A text field by its name (its label is drawn beside it too).
fn field(name: &str) -> egui_kittest::kittest::By<'_> {
    egui_kittest::kittest::By::new()
        .role(egui::accesskit::Role::TextInput)
        .label(name)
}

fn state(h: &Harness<'_, Shell>) -> Value {
    h.state().calibration_state()
}

/// A drag of the pointer from `from` to `to`, in steps.
fn drag(h: &mut Harness<'_, Shell>, from: Pos2, to: Pos2) {
    let button = |pressed| Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.input_mut().events.push(Event::PointerMoved(from));
    h.run();
    h.input_mut().events.push(button(true));
    h.run();
    for k in 1..=6_u8 {
        let t = f32::from(k) / 6.0;
        h.input_mut()
            .events
            .push(Event::PointerMoved(from + (to - from) * t));
        h.run();
    }
    h.input_mut().events.push(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
    h.run_steps(2);
}

#[test]
fn the_calibration_focus_solves_saves_and_lists_profiles() {
    let (dir, picture) = chart_picture("calibrate-focus");
    let (shell, setup) = shell();
    let heard = setup.bus.subscribe();
    let mut h = headless::harness(shell, vec2(1400.0, 900.0));
    h.run();

    // Opened on the shot, the handles on a centred rectangle; the dry run
    // answers through the node.
    h.state_mut()
        .run(CALIBRATE_COMMAND, json!({ "path": picture }), Origin::User)
        .unwrap();
    h.run_steps(3);
    let s = state(&h);
    assert_eq!(s["open"], true);
    assert_eq!(s["corners"][0], json!([80.0, 52.0]), "{s}");
    let first = s["report"]["after"]["mean"].as_f64().expect("a report");
    assert!(first > 1.0, "corners off the patches: {first}");

    // Each handle dragged onto its patch's centre: the solve follows.
    for (i, target) in CORNERS.iter().enumerate() {
        let name = format!("Chart corner {}", i + 1);
        let from = h.get_by_label(&name).rect().center();
        let at = state(&h)["corners"][i].clone();
        let (x, y) = (at[0].as_f64().unwrap(), at[1].as_f64().unwrap());
        // Points per image pixel, from two handles' places.
        let (a, b) = (
            h.get_by_label("Chart corner 1").rect().center(),
            h.get_by_label("Chart corner 2").rect().center(),
        );
        let c = state(&h)["corners"].clone();
        let scale = f64::from(b.x - a.x) / (c[1][0].as_f64().unwrap() - c[0][0].as_f64().unwrap());
        let to = from
            + vec2(
                ((target[0] - x) * scale) as f32,
                ((target[1] - y) * scale) as f32,
            );
        drag(&mut h, from, to);
        let c = &state(&h)["corners"][i];
        assert!(
            (c[0].as_f64().unwrap() - target[0]).abs() < 2.0
                && (c[1].as_f64().unwrap() - target[1]).abs() < 2.0,
            "corner {i}: {c} for {target:?}"
        );
    }
    let s = state(&h);
    let after = s["report"]["after"]["mean"].as_f64().expect("a report");
    assert!(after < 1.0, "on the patches: Delta E {after}, was {first}");
    assert_eq!(s["report"]["patches"].as_array().unwrap().len(), 24);
    // The report shows in the window.
    assert!(
        h.query_all_by_label_contains("Delta E after: mean")
            .next()
            .is_some()
    );

    // Named and saved by the person: through the node, heard as theirs,
    // one step of the library's history; listed at once.
    h.query(field("Profile name")).unwrap().focus();
    h.run();
    h.query(field("Profile name")).unwrap().type_text("Studio");
    h.run();
    h.get_by_label("Save the profile").click();
    h.run_steps(3);
    let profiles = setup.vault.calibration_profiles().unwrap();
    assert_eq!(profiles.len(), 1, "{}", state(&h));
    let (id, doc) = &profiles[0];
    assert_eq!(doc["name"], "Studio");
    let ran: Vec<(String, String)> = heard
        .try_iter()
        .filter(|e| e.kind == "control.command")
        .map(|e| {
            (
                e.payload["id"].as_str().unwrap().to_string(),
                e.payload["origin"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(
        ran.contains(&("calibrate.chart".to_string(), "user".to_string())),
        "{ran:?}"
    );
    assert_eq!(setup.history.line().undo.len(), 1);
    assert_eq!(
        setup.history.line().undo[0].label.text,
        "Save the calibration profile “Studio”"
    );
    assert_eq!(state(&h)["profiles"][0]["id"], id.as_str());

    // The list's actions: the camera's default, an export, a delete.
    h.get_by_label("Set as default: Studio").click();
    h.run_steps(3);
    let camera = doc["camera"]["key"].as_str().unwrap().to_string();
    assert_eq!(state(&h)["defaults"][&camera], json!([id]), "{}", state(&h));
    h.get_by_label("Export: Studio").click();
    h.run_steps(3);
    let exported: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".helios-chartprofile.json"))
        .collect();
    assert_eq!(exported.len(), 1, "{exported:?}");
    h.get_by_label("Delete: Studio").click();
    h.run_steps(3);
    assert!(setup.vault.calibration_profiles().unwrap().is_empty());
    assert_eq!(state(&h)["profiles"], json!([]));
    let undo = setup.history.line().undo;
    assert_eq!(
        undo.last().unwrap().label.text,
        "Delete the calibration profile “Studio”"
    );

    // Imported back from its file.
    h.query(field("Profile file to import")).unwrap().focus();
    h.run();
    h.query(field("Profile file to import"))
        .unwrap()
        .type_text(&dir.join(&exported[0]).display().to_string());
    h.run();
    h.get_by_label("Import the profile").click();
    h.run_steps(3);
    assert_eq!(setup.vault.calibration_profiles().unwrap().len(), 1);
    assert_eq!(state(&h)["profiles"][0]["name"], "Studio");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The windows the main window's last pass declared, as eframe would
/// open them.
fn declared(h: &Harness<'static, Shell>) -> Vec<ViewportId> {
    h.output()
        .viewport_output
        .iter()
        .filter(|(v, o)| **v != ViewportId::ROOT && o.viewport_ui_cb.is_some())
        .map(|(v, _)| *v)
        .collect()
}

#[test]
fn the_calibration_focus_opens_in_its_own_window() {
    let (dir, picture) = chart_picture("calibrate-window");
    let (shell, _setup) = shell();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.state_mut().draw_frame(true);
    h.run();
    assert!(declared(&h).is_empty(), "no window but the main one");
    h.state_mut()
        .run(
            CALIBRATE_COMMAND,
            json!({ "path": picture, "corners": CORNERS }),
            Origin::User,
        )
        .unwrap();
    h.run_steps(3);
    let windows = declared(&h);
    assert_eq!(windows.len(), 1, "the focus's own window: {windows:?}");
    let app = h.state().app().handle().clone();
    assert_eq!(
        app.window_viewport(&WindowId::new(CALIBRATE_WINDOW)),
        Some(windows[0])
    );
    // The dry run ran at the main window's pass, on the corners given.
    let s = h.state().calibration_state();
    assert!(s["report"]["after"]["mean"].as_f64().unwrap() < 1.0, "{s}");
    // Closed with its command's window: the focus closes, the main stays.
    app.close_window(&WindowId::new(CALIBRATE_WINDOW));
    h.run_steps(3);
    assert!(declared(&h).is_empty());
    assert_eq!(h.state().calibration_state()["open"], false);
    let _ = std::fs::remove_dir_all(&dir);
}
