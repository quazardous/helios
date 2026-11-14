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
        Some(Arc::new(client_on(&vault, &bus))),
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

/// Frames until `done` holds of the shell's values: what the user asks
/// of the node runs off the frame. Within a minute: a deadline, not a
/// guess at how long it takes.
fn until(h: &mut egui_kittest::Harness<'_, Shell>, done: impl Fn(&helios_values::Store) -> bool) {
    let end = std::time::Instant::now() + std::time::Duration::from_mins(1);
    while !done(&h.state().values().lock().unwrap()) {
        assert!(std::time::Instant::now() < end, "the node answers");
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.run();
    }
    h.run();
}

/// A profile document of camera "Test Bayer" under light `xy`, its
/// matrix bent by `bend`.
/// A client of a node owning `vault`, as the desktop's.
fn client_of<S: helios_vault::Store + 'static>(
    vault: &Arc<helios_vault::Vault<S>>,
) -> helios_inspect::client::Client {
    client_on(vault, &Arc::new(helios_core::Bus::new()))
}

/// A client of a node owning `vault`, its runs heard on `bus`.
fn client_on<S: helios_vault::Store + 'static>(
    vault: &Arc<helios_vault::Vault<S>>,
    bus: &Arc<helios_core::Bus>,
) -> helios_inspect::client::Client {
    let bus = bus.clone();
    let service = helios_node_core::endpoint::VaultService::new(
        vault.clone(),
        bus.clone(),
        helios_core::jobs::Jobs::new(bus),
        "test",
    );
    helios_inspect::client::Client::in_process(Arc::new(service)).with_origin("user")
}

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
    calibrate::register(&mut shell, Some(Arc::new(client_of(&vault))), history).unwrap();
    // As an image of the library opens: the header is offered its camera's
    // profiles (no default rule: nothing is applied).
    let image = helios_vault::ImageId::from_hex(&"cd".repeat(32)).unwrap();
    assert!(calibrate::on_open(&client_of(&vault), image, &g, shell.values()).is_empty());
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
        let before = h.state().values().lock().unwrap().undo_steps().len();
        h.state_mut()
            .run(
                calibrate::CHOOSE,
                json!({ "profile": profile }),
                Origin::User,
            )
            .unwrap();
        // Read through the node off the frame: one step once written.
        until(h, |s| s.undo_steps().len() > before);
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
        Some(Arc::new(client_of(&vault))),
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

/// A shell showing a raw's chain in short, as the camera look's tests
/// open it: the chart profile (bypassed) with the camera's context, the
/// look's two halves, then sigmoid; the photo a picture of the library
/// in `name`'s folder. Answers the shell, the library's history and the
/// folder.
fn look_shell(
    name: &str,
) -> (
    Shell,
    helios_gui::library_history::LibraryHistory,
    std::path::PathBuf,
) {
    use helios_core::{Graph, Image, ModuleRef, Params, Region};

    let dir = std::env::temp_dir().join(format!("helios-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let picture = dir.join("picture.tif");
    let small = Image::new(Region::sized(4, 4), 3, vec![0.5; 48]);
    std::fs::write(
        &picture,
        helios_export::encode(&small, &helios_export::Format::Tiff).unwrap(),
    )
    .unwrap();
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let image = vault.import_file(&picture, 1).unwrap();

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
    let chart = g
        .add_module(
            "chartprofile",
            ModuleRef::new(helios_module_chartprofile::ChartProfile),
            &Params::new(),
        )
        .unwrap();
    g.set_context(chart, camera.to_context()).unwrap();
    g.set_bypass(chart, true).unwrap();
    let colour = g
        .add_module(
            "cameracolour",
            ModuleRef::new(helios_module_cameracolour::CameraColour),
            &Params::new(),
        )
        .unwrap();
    g.set_bypass(colour, true).unwrap();
    let tone = g
        .add_module(
            "cameratone",
            ModuleRef::new(helios_module_cameratone::CameraTone),
            &Params::new(),
        )
        .unwrap();
    g.set_bypass(tone, true).unwrap();
    let sigmoid = g
        .add_module(
            "sigmoid",
            ModuleRef::new(helios_module_sigmoid::Sigmoid),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", chart, "input").unwrap();
    g.connect(chart, "output", colour, "input").unwrap();
    g.connect(colour, "output", tone, "input").unwrap();
    g.connect(tone, "output", sigmoid, "input").unwrap();

    let mut shell = Shell::new(helios_panels(), None);
    let history = shell.library_history();
    calibrate::register(
        &mut shell,
        Some(Arc::new(client_of(&vault))),
        history.clone(),
    )
    .unwrap();
    assert!(calibrate::on_open(&client_of(&vault), image, &g, shell.values()).is_empty());
    shell.set_view_source(g, sigmoid, "output").unwrap();

    (shell, history, dir)
}

#[test]
fn the_camera_look_header_fits_says_why_not_and_its_switch_is_one_edit_step() {
    use egui_kittest::kittest::Queryable;
    use helios_core::Value;

    let (shell, history, dir) = look_shell("look-header");
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.state_mut()
        .run("helios.params.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
    h.get_by_label("Not fitted yet: the look starts from the camera's own JPEG");
    // Nothing fitted: the switch waits for a fit.
    assert!(
        h.state_mut()
            .run(
                calibrate::LOOK_CHOOSE,
                json!({ "look": "on" }),
                Origin::User
            )
            .is_err()
    );

    // Fitting a picture is refused by its gate, in words, in the header;
    // nothing is written, in the edit or the library.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Fit to the camera's JPEG")
        .click();
    h.run();
    // The fit runs through the node off the frame: its refusal comes at a
    // coming frame.
    until(&mut h, |s| !calibrate::read_refusal(s).is_empty());
    h.get_by_label("Not fitted: the photo is not a raw: it has no sensor data to fit");
    assert!(history.line().undo.is_empty());

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
    assert_eq!(read(&h, "bypass/cameratone"), Value::Bool(true));

    // A look fitted (its numbers set as a fit sets them): the header says
    // how well, and its switch turns both halves on in sigmoid's place,
    // one step of the edit, then off again.
    {
        let values = h.state().values().clone();
        let mut s = values.lock().unwrap();
        for (path, value) in [
            (
                "node/cameratone/tone",
                Value::List(vec![0.0, 0.0, 1.0, 1.0]),
            ),
            ("node/cameratone/held_out", Value::Float(2.4)),
            ("node/cameratone/baseline", Value::Float(9.8)),
        ] {
            s.set(helios_values::Set::new(
                path,
                helios_values::Change::To(value),
                Origin::User,
            ))
            .unwrap();
        }
    }
    calibrate::set_refusal(h.state().values(), "");
    h.run();
    h.get_by_label("Fitted to the camera's JPEG: Delta E 2.4 (default 9.8)");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Refit");
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Camera look")
        .click();
    h.run();
    h.run();
    assert_eq!(read(&h, "bypass/cameratone"), Value::Bool(false));
    assert_eq!(read(&h, "bypass/cameracolour"), Value::Bool(false));
    assert_eq!(read(&h, "bypass/sigmoid"), Value::Bool(true));
    // One step: undone, the look is off and sigmoid back.
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    assert_eq!(read(&h, "bypass/cameratone"), Value::Bool(true));
    assert_eq!(read(&h, "bypass/cameracolour"), Value::Bool(true));
    assert_eq!(read(&h, "bypass/sigmoid"), Value::Bool(false));
    // With a chart profile on, the colour half stays off and says so.
    h.state()
        .values()
        .lock()
        .unwrap()
        .set(helios_values::Set::new(
            "bypass/chartprofile",
            helios_values::Change::To(Value::Bool(false)),
            Origin::User,
        ))
        .unwrap();
    h.state_mut()
        .run(
            calibrate::LOOK_CHOOSE,
            json!({ "look": "on" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    assert_eq!(read(&h, "bypass/cameratone"), Value::Bool(false));
    assert_eq!(read(&h, "bypass/cameracolour"), Value::Bool(true));
    h.get_by_label("Colour half off: the chart profile rules the colours");
    h.state_mut()
        .run(
            calibrate::LOOK_CHOOSE,
            json!({ "look": "off" }),
            Origin::User,
        )
        .unwrap();
    assert_eq!(read(&h, "bypass/sigmoid"), Value::Bool(false));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_camera_look_header_saves_and_clears_the_camera_s_default() {
    use egui_kittest::kittest::Queryable;
    use helios_core::Value;

    let (shell, history, dir) = look_shell("look-default");
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.state_mut()
        .run("helios.params.focus", json!(null), Origin::User)
        .unwrap();
    // A look fitted (its numbers set as a fit sets them), turned on.
    {
        let values = h.state().values().clone();
        let mut s = values.lock().unwrap();
        for (path, value) in [
            (
                "node/cameratone/tone",
                Value::List(vec![0.0, 0.0, 1.0, 1.0]),
            ),
            ("node/cameratone/preview", Value::Text("fitted-to".into())),
            ("node/cameratone/held_out", Value::Float(2.4)),
            ("node/cameratone/baseline", Value::Float(9.8)),
        ] {
            s.set(helios_values::Set::new(
                path,
                helios_values::Change::To(value),
                Origin::User,
            ))
            .unwrap();
        }
    }
    h.state_mut()
        .run(
            calibrate::LOOK_CHOOSE,
            json!({ "look": "on" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    let offered = |h: &egui_kittest::Harness<'_, Shell>| {
        calibrate::read_offer(&h.state().values().lock().unwrap()).unwrap()["look_default"].clone()
    };
    assert_eq!(offered(&h), serde_json::Value::Null);

    // Saved as the camera's default: one step of the library, and the
    // header says the open photo's look is it.
    h.get_by_role_and_label(
        egui::accesskit::Role::Button,
        "Save as the camera's default",
    )
    .click();
    h.run();
    h.run();
    assert_eq!(offered(&h)["preview"], "fitted-to");
    assert_eq!(history.line().undo.len(), 1);
    h.get_by_label("The default look of Test Bayer: its raws are imported with it");

    // Cleared: another step, and the header says no more.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Clear the default")
        .click();
    h.run();
    h.run();
    assert_eq!(offered(&h), serde_json::Value::Null);
    assert_eq!(history.line().undo.len(), 2);
    assert!(
        h.query_by_label("The default look of Test Bayer: its raws are imported with it")
            .is_none()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_camera_look_preset_puts_the_camera_s_default_look_on_in_one_step() {
    use egui_kittest::kittest::Queryable;
    use helios_core::Value;

    let (shell, _history, dir) = look_shell("look-preset");
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.state_mut()
        .run("helios.params.focus", json!(null), Origin::User)
        .unwrap();
    let set = |h: &egui_kittest::Harness<'_, Shell>, writes: Vec<(&str, Value)>| {
        let values = h.state().values().clone();
        let mut s = values.lock().unwrap();
        for (path, value) in writes {
            s.set(helios_values::Set::new(
                path,
                helios_values::Change::To(value),
                Origin::User,
            ))
            .unwrap();
        }
    };
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
    // A look fitted, kept as the camera's default.
    set(
        &h,
        vec![
            (
                "node/cameratone/tone",
                Value::List(vec![0.0, 0.0, 1.0, 1.0]),
            ),
            ("node/cameratone/preview", Value::Text("fitted-to".into())),
        ],
    );
    h.run();
    h.get_by_role_and_label(
        egui::accesskit::Role::Button,
        "Save as the camera's default",
    )
    .click();
    h.run();
    h.run();
    // The photo's own look changed and turned off since.
    set(
        &h,
        vec![
            (
                "node/cameratone/tone",
                Value::List(vec![0.0, 0.0, 1.0, 0.8]),
            ),
            ("node/cameratone/preview", Value::Text("another".into())),
        ],
    );
    h.state_mut()
        .run(
            calibrate::LOOK_CHOOSE,
            json!({ "look": "off" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    // Presets, then "Camera look": the camera's default, turned on.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Presets")
        .click();
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::MenuItem, "Camera look")
        .click();
    h.run();
    // Read through the node off the frame.
    until(&mut h, |s| {
        s.get("node/cameratone/preview")
            .is_some_and(|e| e.value == Value::Text("fitted-to".into()))
    });
    assert_eq!(
        read(&h, "node/cameratone/preview"),
        Value::Text("fitted-to".into())
    );
    assert_eq!(
        read(&h, "node/cameratone/tone"),
        Value::List(vec![0.0, 0.0, 1.0, 1.0])
    );
    assert_eq!(read(&h, "bypass/cameratone"), Value::Bool(false));
    assert_eq!(read(&h, "bypass/sigmoid"), Value::Bool(true));
    // One step of the edit: undone, the photo's own look is back, off.
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    assert_eq!(
        read(&h, "node/cameratone/preview"),
        Value::Text("another".into())
    );
    assert_eq!(read(&h, "bypass/cameratone"), Value::Bool(true));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_camera_look_tone_amount_is_one_edit_step() {
    use helios_core::Value;

    let (shell, _history, dir) = look_shell("look-amount");
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let read = |h: &egui_kittest::Harness<'_, Shell>| {
        h.state()
            .values()
            .lock()
            .unwrap()
            .get("node/cameratone/amount")
            .unwrap()
            .value
            .clone()
    };
    // The tone half's amount: all of the fit to start with.
    assert_eq!(read(&h), Value::Float(1.0));
    {
        let values = h.state().values().clone();
        let mut s = values.lock().unwrap();
        s.set(helios_values::Set::new(
            "node/cameratone/amount",
            helios_values::Change::To(Value::Float(0.5)),
            Origin::User,
        ))
        .unwrap();
    }
    h.run();
    assert_eq!(read(&h), Value::Float(0.5));
    // One step of the edit: undone, all of the fit again.
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    h.run();
    assert_eq!(read(&h), Value::Float(1.0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_camera_look_compare_shows_the_camera_jpeg_left_of_the_split() {
    use egui_kittest::kittest::Queryable;
    use helios_core::{Image, Region, Value};

    let (shell, history, dir) = look_shell("look-compare");
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.state_mut()
        .run("helios.params.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
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

    // The photo is a picture, with no camera JPEG: the switch turns the
    // compare off again and says why, in words; nothing is an edit step.
    h.get_by_role_and_label(
        egui::accesskit::Role::CheckBox,
        "Compare with the camera's JPEG",
    )
    .click();
    h.run();
    h.run();
    h.get_by_label("No camera JPEG to compare: the photo is not a raw");
    assert_eq!(read(&h, calibrate::LOOK_COMPARE), Value::Bool(false));
    assert_eq!(read(&h, "view/viewport/compare"), Value::Bool(false));
    assert!(history.line().undo.is_empty());
    assert!(
        h.state().values().lock().unwrap().undo_steps().is_empty(),
        "the compare is a view, not a step of the edit"
    );

    // A camera JPEG smaller than the render (as embedded previews are) is
    // scaled to it, so the split lines the two up; "before" shows it.
    let after = h.state().viewport().shown().unwrap().extent;
    let jpeg = Image::from_fn(Region::sized(16, 12), 3, |x, _, c| {
        if c == 0 { x as f32 / 16.0 } else { 0.25 }
    });
    h.state().viewport_mut().show_camera_jpeg(&jpeg).unwrap();
    h.run();
    let before = h.state().viewport().before_shown().unwrap();
    assert_eq!(before.extent, after);
    let node = before
        .graph
        .find(helios_gui::viewport::CAMERA_JPEG)
        .unwrap();
    let shown = before.graph.source_image(node).unwrap().unwrap();
    assert_eq!(shown.region(), after);
    assert_eq!(shown.channels(), 4);
    // The right edge of the render shows the JPEG's right edge, opaque.
    let right = shown.pixel(after.right() - 1, after.y);
    assert!((right[0] - 15.0 / 16.0).abs() < 1e-6, "{right:?}");
    assert_eq!(right[3], 1.0);
    let _ = std::fs::remove_dir_all(&dir);
}
