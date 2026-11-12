//! The desktop's `calibrate.chart` command on a chart picture: the same
//! dry run as the node's request, reached through the shell.

use egui::vec2;
use helios_calibrate::lab::lab_to_xyz;
use helios_calibrate::{Layout, XRITE_CC24_2014 as CHART, working_to_xyz};
use helios_gui::{Shell, calibrate, headless, helios_panels};
use helios_photo::color::{apply, inverse};
use helios_shell::commands::Origin;
use serde_json::json;

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

    let mut shell = Shell::new(helios_panels(), None);
    calibrate::register(&mut shell).unwrap();
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
    assert!(refused.unwrap_err().contains("dry run"));
    let _ = std::fs::remove_dir_all(&dir);
}
