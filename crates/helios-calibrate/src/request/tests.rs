//! `calibrate.chart`'s dry run on a decoded picture, through its
//! serialised arguments and answer.

use super::*;
use crate::run::tests::{CHART, CORNERS, H, W, adaptation_from, draw, lit};
use crate::solver::Orientation;
use helios_module_channelmixerrgb::daylight_xy;

/// The chart under 6500 K as a decoded picture would hold it: linear
/// working RGB at 80 % exposure (the white patch's blue would reach the
/// clip level at full exposure), four channels (alpha 1).
fn picture() -> Vec<f32> {
    let values: Vec<[f64; 3]> = lit(&adaptation_from(daylight_xy(6500.0)))
        .into_iter()
        .map(|v| v.map(|c| 0.8 * c))
        .collect();
    let rgb = draw(&values);
    rgb.chunks(3)
        .flat_map(|p| [p[0], p[1], p[2], 1.0])
        .collect()
}

fn args(json: serde_json::Value) -> ChartArgs {
    serde_json::from_value(json).unwrap()
}

#[test]
fn arguments_take_their_defaults() {
    let parsed = args(serde_json::json!({ "corners": CORNERS }));
    assert_eq!(parsed, ChartArgs::new(CORNERS));
    assert_eq!(parsed.chart, "xrite.cc24.2014");
    assert!(parsed.dry_run);
    assert!(parsed.exclude.is_empty());
}

#[test]
fn a_picture_answers_its_report() {
    let data = picture();
    let handles = Orientation::ALL[3].handles(CORNERS);
    let given = args(serde_json::json!({ "corners": handles }));
    let answer = calibrate_picture(&given, &data, W, H, 4).unwrap();
    assert!(answer.dry_run);
    assert_eq!(answer.chart, "xrite.cc24.2014");
    assert_eq!(
        answer.orientation,
        OrientationAnswer {
            quarter_turns: 3,
            mirrored: false
        }
    );
    assert_eq!(answer.corners, CORNERS);
    assert_eq!(answer.illuminant.kind, "d");
    assert!(
        (answer.illuminant.cct - 6500.0).abs() < 50.0,
        "{:?}",
        answer.illuminant
    );
    assert_eq!(answer.patches.len(), CHART.len());
    assert!(
        answer
            .patches
            .iter()
            .all(|p| p.status == "kept" && p.flags.is_empty()),
        "{:?}",
        answer.patches
    );
    assert_eq!(answer.patches[0].name, CHART.patches[0].name);
    assert!(answer.after.max < 5e-3, "{:?}", answer.after);
    assert_eq!(answer.after.worst.len(), 3);
    assert_eq!(answer.quality, "very good");
    // A light far from D50 may show as a cast; nothing else.
    assert!(
        answer
            .warnings
            .iter()
            .all(|w| matches!(w, ChartWarning::NeutralCast { .. })),
        "{:?}",
        answer.warnings
    );
    for (row, identity) in answer.camera_matrix.iter().zip(&IDENTITY) {
        for (v, i) in row.iter().zip(identity) {
            assert!((v - i).abs() < 1e-4, "{:?}", answer.camera_matrix);
        }
    }

    // The answer goes through JSON unchanged.
    let json = serde_json::to_value(&answer).unwrap();
    assert_eq!(json["patches"][0]["status"], "kept");
    assert_eq!(json["orientation"]["quarter_turns"], 3);
    let back: ChartAnswer = serde_json::from_value(json).unwrap();
    assert_eq!(back, answer);
}

#[test]
fn excluded_and_missing_patches_say_so() {
    let data = picture();
    let given = args(serde_json::json!({ "corners": CORNERS, "exclude": ["B3"] }));
    let answer = calibrate_picture(&given, &data, W, H, 4).unwrap();
    let b3 = answer.patches.iter().find(|p| p.name == "B3").unwrap();
    assert_eq!(b3.status, "excluded");
    assert!(!answer.after.worst.contains(&"B3".to_string()));

    // The image cut through the chart's last column: its patches are
    // missing, the rest still solves.
    let width = 520;
    let cut: Vec<f32> = data
        .chunks(W * 4)
        .flat_map(|row| row[..width * 4].to_vec())
        .collect();
    let answer = calibrate_picture(&ChartArgs::new(CORNERS), &cut, width, H, 4).unwrap();
    let missing: Vec<&str> = answer
        .patches
        .iter()
        .filter(|p| p.status == "missing")
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(missing.len(), CHART.rows, "{missing:?}");
}

#[test]
fn refusals_are_named() {
    let data = picture();
    let mut save = ChartArgs::new(CORNERS);
    save.dry_run = false;
    assert_eq!(
        calibrate_picture(&save, &data, W, H, 4),
        Err(ChartError::NotDryRun)
    );
    let mut other = ChartArgs::new(CORNERS);
    other.chart = "spyder.48".into();
    assert_eq!(
        calibrate_picture(&other, &data, W, H, 4),
        Err(ChartError::UnknownChart("spyder.48".into()))
    );
    let mut unknown = ChartArgs::new(CORNERS);
    unknown.exclude = vec!["Z9".into()];
    assert_eq!(
        calibrate_picture(&unknown, &data, W, H, 4),
        Err(ChartError::UnknownPatch("Z9".into()))
    );
    assert_eq!(
        calibrate_picture(&ChartArgs::new(CORNERS), &data[..8], W, H, 4),
        Err(ChartError::BadImage)
    );
    let flat = vec![0.2_f32; W * H * 3];
    let far = [
        [1e5, 1e5],
        [1e5 + 400.0, 1e5],
        [1e5 + 400.0, 1e5 + 300.0],
        [1e5, 1e5 + 300.0],
    ];
    let unsolved = calibrate_picture(&ChartArgs::new(far), &flat, W, H, 3).unwrap_err();
    assert!(matches!(unsolved, ChartError::Unsolved(_)), "{unsolved:?}");
    assert!(unsolved.to_string().contains("could not be measured"));
}
