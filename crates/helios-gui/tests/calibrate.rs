//! The desktop's calibration commands on a chart picture, reached through
//! the shell and run by the desktop's node: the dry run, a profile saved,
//! listed, deleted and brought back by the library's undo, each command
//! heard on the node's bus with who ran it.

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
    let _ = std::fs::remove_dir_all(&dir);
}
