//! The desktop's calibration commands on a chart picture, reached through
//! the shell and run by the desktop's node: the dry run, a profile saved,
//! listed, deleted and brought back by the library's undo, then applied
//! to the picture and taken back by the undo, each command heard on the
//! node's bus with who ran it.

use egui::vec2;
use helios_calibrate::lab::lab_to_xyz;
use helios_calibrate::{Layout, XRITE_CC24_2014 as CHART, working_to_xyz};
use helios_gui::{Shell, calibrate, headless, helios_panels};
use helios_photo::color::{apply, inverse};
use helios_shell::commands::Origin;
use helios_vault::{MemoryStore, Vault};
use serde_json::json;
use std::sync::Arc;

const CORNERS: [[f64; 2]; 4] = [[30.0, 26.0], [170.0, 20.0], [174.0, 108.0], [24.0, 104.0]];

#[test]
fn the_shell_answers_a_chart_dry_run() {
    let dir = std::env::temp_dir().join(format!("helios-calibrate-cmd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (200_usize, 130_usize);
    let from_xyz = inverse(&working_to_xyz()).unwrap();
    let values: Vec<[f64; 3]> = CHART
        .patches
        .iter()
        .map(|p| apply(&from_xyz, lab_to_xyz(p.lab)).map(|v| 0.7 * v))
        .collect();
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

    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let bus = Arc::new(helios_core::Bus::new());
    let heard = bus.subscribe();
    let mut shell = Shell::new(helios_panels(), None);
    // As the desktop wires it: undo and redo move the vault's documents.
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
        Some(calibrate::Node {
            vault: vault.clone(),
            bus: bus.clone(),
        }),
        history.clone(),
    )
    .unwrap();
    let mut harness = headless::harness(shell, vec2(800.0, 600.0));
    harness.run();
    let shell = harness.state_mut();
    let answer = shell
        .run(
            calibrate::CHART_COMMAND,
            json!({ "path": picture, "corners": CORNERS }),
            Origin::Agent("test".into()),
        )
        .unwrap();
    assert_eq!(answer["dry_run"], true, "{answer}");
    assert_eq!(answer["patches"].as_array().unwrap().len(), 24);
    assert_eq!(answer["orientation"]["quarter_turns"], 0);
    assert!(
        answer["after"]["mean"].as_f64().unwrap() < 1.0,
        "{}",
        answer["after"]
    );

    let refused = shell.run(
        calibrate::CHART_COMMAND,
        json!({ "path": picture, "corners": CORNERS, "dry_run": false }),
        Origin::Agent("test".into()),
    );
    assert!(refused.unwrap_err().contains("save"));
    assert!(vault.calibration_profiles().unwrap().is_empty());

    // Saved by the person at the desktop: through the node, heard on its
    // bus as theirs, one step of the library's history.
    let saved = shell
        .run(
            calibrate::CHART_COMMAND,
            json!({ "path": picture, "corners": CORNERS, "save": true, "name": "Daylight" }),
            Origin::User,
        )
        .unwrap();
    assert_eq!(saved["dry_run"], false, "{saved}");
    let id = saved["profile"]["id"].as_str().unwrap().to_string();
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
    assert_eq!(
        ran.last(),
        Some(&("calibrate.chart".to_string(), "user".to_string())),
        "{ran:?}"
    );
    assert_eq!(history.line().undo.len(), 1);
    let listed = shell
        .run(
            "calibrate.profiles.list",
            json!({}),
            Origin::Agent("test".into()),
        )
        .unwrap();
    assert_eq!(listed["profiles"][0]["id"], id.as_str());
    assert_eq!(listed["profiles"][0]["name"], "Daylight");

    shell
        .run(
            "calibrate.profiles.delete",
            json!({ "profile": id }),
            Origin::User,
        )
        .unwrap();
    assert!(vault.calibration_profiles().unwrap().is_empty());
    assert_eq!(history.line().undo.len(), 2);
    assert_eq!(
        history.line().undo[1].label.text,
        "Delete the calibration profile “Daylight”"
    );
    // Ctrl+Z brings the deleted profile back.
    let request = egui_pop::api::history::Request::new(helios_gui::library_history::LIBRARY);
    let egui_pop::api::history::Answer::Done(Ok(_)) =
        history.undo(&request, &egui_pop::model::commands::Origin::User)
    else {
        panic!("undone");
    };
    assert!(vault.calibration_profile(&id).unwrap().is_some());

    // Applied to the chart picture by the person at the desktop: through
    // the node, heard on its bus, the edit's `chartprofile` node on; one
    // step of the library's history that Ctrl+Z takes back.
    let image = vault.import_file(&picture, 1).unwrap();
    let head = || {
        helios_vault::EditSession::open(&vault, image)
            .unwrap()
            .1
            .map(|v| v.edit)
    };
    let applied = shell
        .run(
            "calibrate.apply",
            json!({ "images": [image.hex()], "profile": id }),
            Origin::User,
        )
        .unwrap();
    assert_eq!(applied["images"][0]["profile"], id.as_str(), "{applied}");
    assert_eq!(
        heard
            .try_iter()
            .filter(|e| e.kind == "control.command")
            .map(|e| (e.payload["id"].clone(), e.payload["origin"].clone()))
            .last(),
        Some((json!("calibrate.apply"), json!("user")))
    );
    let edit = head().unwrap();
    assert_eq!(
        helios_calibrate::apply::applied(&edit).as_deref(),
        Some(id.as_str())
    );
    let steps = history.line().undo.len();
    assert_eq!(
        history.line().undo[steps - 1].label.text,
        "Apply the calibration profile “Daylight”"
    );
    let egui_pop::api::history::Answer::Done(Ok(_)) =
        history.undo(&request, &egui_pop::model::commands::Origin::User)
    else {
        panic!("undone");
    };
    assert_eq!(head(), None, "the apply undone: no chart profile");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A profile document of camera "Test Bayer" under light `xy`, its
/// matrix bent by `bend`.
fn profile(name: &str, xy: [f64; 2], bend: f64) -> serde_json::Value {
    let summary = json!({ "mean": 1.0, "max": 2.0, "worst": [] });
    json!({
        "format": helios_calibrate::PROFILE_FORMAT,
        "name": name,
        "camera": { "make": "Test", "model": "Bayer", "key": "test bayer" },
        "illuminant": { "xy": xy, "cct": 5000.0, "kind": "d" },
        "chart": "xrite.cc24.2014",
        "measured": { "space": "camera", "image": "ab", "edit": null,
                      "corners": CORNERS, "excluded": [] },
        "base_matrix": null,
        "matrix": [[1.0 + bend, -bend, 0.0], [0.0, 1.0, 0.0], [0.0, bend, 1.0 - bend]],
        "correction": [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        "lut": null,
        "report": { "quality": "good", "exposure_ev": 0.0, "before": summary, "after": summary },
        "created": 0
    })
}

#[test]
fn the_module_header_lists_the_camera_s_profiles_and_a_switch_is_one_edit_step() {
    use egui_kittest::kittest::Queryable;
    use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};

    let camera = helios_photo::CameraInfo {
        maker: "Test".into(),
        model: "Bayer".into(),
        iso: 100.0,
        exposure_bias: 0.0,
        exposure_time: 0.01,
        aperture: 4.0,
        focal_length: 50.0,
        black: [0.0; 4],
        white: 1.0,
        white_balance: [2.0, 1.0, 1.5, 1.0],
        xyz_to_camera: [0.9, -0.3, -0.1, -0.4, 1.2, 0.2, -0.05, 0.15, 0.6],
        cfa: helios_photo::Cfa::parse("RGGB").unwrap(),
        orientation: 1,
    };
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(32, 24), 4, |_, _, _| 0.2),
    );
    let node = g
        .add_module(
            "chartprofile",
            ModuleRef::new(helios_module_chartprofile::ChartProfile),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", node, "input").unwrap();
    g.set_context(node, camera.to_context()).unwrap();
    g.set_bypass(node, true).unwrap();

    let vault = Arc::new(Vault::new(MemoryStore::default()));
    for (id, name, xy, bend) in [
        ("daylight", "Daylight", [0.3127, 0.329], 0.05),
        ("tungsten", "Tungsten", [0.4476, 0.4074], 0.1),
    ] {
        vault
            .set_calibration_profile(id, Some(&profile(name, xy, bend)))
            .unwrap();
    }
    let other = {
        let mut p = profile("Other body", [0.3127, 0.329], 0.0);
        p["camera"] = json!({ "make": "Other", "model": "Body", "key": "other body" });
        p
    };
    vault
        .set_calibration_profile("other", Some(&other))
        .unwrap();

    let mut shell = Shell::new(helios_panels(), None);
    let history = shell.library_history();
    calibrate::register(
        &mut shell,
        Some(calibrate::Node {
            vault: vault.clone(),
            bus: Arc::new(helios_core::Bus::new()),
        }),
        history,
    )
    .unwrap();
    // As an image of the library opens: the header is offered its camera's
    // profiles (no default rule: nothing is applied).
    let image = helios_vault::ImageId::from_hex(&"cd".repeat(32)).unwrap();
    assert!(calibrate::on_open(&vault, image, &g, shell.values()).is_none());
    shell.set_view_source(g, node, "output").unwrap();
    let offer = calibrate::read_offer(&shell.values().lock().unwrap()).unwrap();
    let ids: Vec<&str> = offer["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["daylight", "tungsten"], "the camera's only");
    assert_eq!(offer["camera"], "test bayer");

    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.state_mut()
        .run("helios.params.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Calibration profile");

    let read = |h: &egui_kittest::Harness<'_, Shell>, path: &str| {
        h.state()
            .values()
            .lock()
            .unwrap()
            .get(path)
            .unwrap()
            .value
            .clone()
    };
    let choose = |h: &mut egui_kittest::Harness<'_, Shell>, profile: &str| {
        h.state_mut()
            .run(
                calibrate::CHOOSE,
                json!({ "profile": profile }),
                Origin::User,
            )
            .unwrap();
    };
    choose(&mut h, "daylight");
    assert_eq!(read(&h, "bypass/chartprofile"), Value::Bool(false));
    assert_eq!(
        read(&h, "node/chartprofile/profile"),
        Value::Text("daylight".into())
    );
    choose(&mut h, "tungsten");
    let Value::List(m) = read(&h, "node/chartprofile/matrix") else {
        panic!("a matrix")
    };
    assert!((m[0] - 1.1).abs() < 1e-12, "{m:?}");
    // Each switch is one step of the edit: undone, the previous profile.
    let undo = |h: &egui_kittest::Harness<'_, Shell>| {
        h.state()
            .values()
            .lock()
            .unwrap()
            .undo(&Origin::User)
            .unwrap();
    };
    undo(&h);
    assert_eq!(
        read(&h, "node/chartprofile/profile"),
        Value::Text("daylight".into())
    );
    undo(&h);
    assert_eq!(read(&h, "bypass/chartprofile"), Value::Bool(true));
    // Another camera's profile is refused; none turns it off.
    assert!(
        h.state_mut()
            .run(
                calibrate::CHOOSE,
                json!({ "profile": "other" }),
                Origin::User
            )
            .is_err()
    );
    choose(&mut h, "tungsten");
    choose(&mut h, "none");
    assert_eq!(read(&h, "bypass/chartprofile"), Value::Bool(true));
    h.run();
}

#[test]
fn the_shell_fits_a_camera_look_through_the_node_and_says_why_not() {
    let dir = std::env::temp_dir().join(format!("helios-look-cmd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let image = helios_core::Image::new(helios_core::Region::sized(4, 4), 3, vec![0.5; 48]);
    let picture = dir.join("picture.tif");
    std::fs::write(
        &picture,
        helios_export::encode(&image, &helios_export::Format::Tiff).unwrap(),
    )
    .unwrap();
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let id = vault.import_file(&picture, 1).unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    let history = shell.library_history();
    calibrate::register(
        &mut shell,
        Some(calibrate::Node {
            vault: vault.clone(),
            bus: Arc::new(helios_core::Bus::new()),
        }),
        history.clone(),
    )
    .unwrap();
    // A picture has no sensor data to fit: answered per image, in words,
    // and nothing is written.
    let answer = shell
        .run(
            "look.camera.fit",
            json!({ "images": [id.hex()] }),
            Origin::Agent("test".into()),
        )
        .unwrap();
    assert_eq!(answer["fitted"], 0, "{answer}");
    assert_eq!(answer["images"][0]["refused"]["gate"], "not-a-raw");
    assert!(history.line().undo.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
