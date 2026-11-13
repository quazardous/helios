//! The calibration commands on an in-memory vault, on a chart picture
//! drawn from the chart's reference values.

use helios_calibrate::lab::lab_to_xyz;
use helios_calibrate::{Layout, XRITE_CC24_2014 as CC24, working_to_xyz};
use helios_photo::color::{apply, inverse};
use helios_vault::MemoryStore;

use super::*;

const CORNERS: [[f64; 2]; 4] = [[30.0, 26.0], [170.0, 20.0], [174.0, 108.0], [24.0, 104.0]];

/// A chart picture (16-bit TIFF) in a folder of its own.
fn picture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-calibrate-node-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (200_usize, 130_usize);
    let from_xyz = inverse(&working_to_xyz()).unwrap();
    let values: Vec<[f64; 3]> = CC24
        .patches
        .iter()
        .map(|p| apply(&from_xyz, lab_to_xyz(p.lab)).map(|v| 0.7 * v))
        .collect();
    let data = Layout::new(&CC24, CORNERS)
        .unwrap()
        .draw(w, h, &values, 0.01);
    let image = helios_core::Image::new(helios_core::Region::sized(w as u32, h as u32), 3, data);
    let path = dir.join("chart.tif");
    std::fs::write(
        &path,
        helios_export::encode(&image, &helios_export::Format::Tiff).unwrap(),
    )
    .unwrap();
    path
}

fn vault() -> Vault<MemoryStore> {
    Vault::new(MemoryStore::default())
}

fn save(vault: &Vault<MemoryStore>, path: &Path, extra: Value, now: u64) -> Done {
    let mut params = json!({ "path": path, "corners": CORNERS, "save": true });
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    run(Some(vault), CHART, &params, now).unwrap()
}

#[test]
fn a_saved_profile_is_listed_and_read_back_with_the_same_values() {
    let path = picture("save");
    let vault = vault();
    let done = save(
        &vault,
        &path,
        json!({ "name": "Studio flash" }),
        1_700_000_000,
    );
    let answer = &done.answer;
    assert_eq!(answer["dry_run"], false, "{answer}");
    let id = answer["profile"]["id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 64);
    assert_eq!(done.changes.documents.len(), 1);

    let listed = run(Some(&vault), LIST, &json!({}), 0).unwrap().answer;
    let profiles = listed["profiles"].as_array().unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0]["id"], id.as_str());
    assert_eq!(profiles[0]["name"], "Studio flash");
    assert_eq!(profiles[0]["camera"]["key"], "picture");

    let got = run(Some(&vault), GET, &json!({ "profile": id }), 0)
        .unwrap()
        .answer;
    let profile = &got["profile"];
    assert_eq!(profile["format"], helios_calibrate::PROFILE_FORMAT);
    assert_eq!(profile["matrix"], answer["camera_matrix"]);
    assert_eq!(profile["correction"], answer["matrix"]);
    assert_eq!(profile["illuminant"], answer["illuminant"]);
    assert_eq!(profile["report"]["after"], answer["after"]);
    assert_eq!(profile["measured"]["image"], answer["image"]["hash"]);
    assert_eq!(profile["measured"]["space"], "picture");
    assert_eq!(profile["created"], 1_700_000_000);
    // The document read back is the profile, and its id its own.
    let back: Profile = serde_json::from_value(profile.clone()).unwrap();
    assert_eq!(back.id(), id);

    // A camera filter that names another camera lists none.
    let none = run(Some(&vault), LIST, &json!({ "camera": "Nikon Z 6" }), 0)
        .unwrap()
        .answer;
    assert_eq!(none["profiles"], json!([]));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_deleted_profile_comes_back_with_undo() {
    let path = picture("delete");
    let vault = vault();
    let saved = save(&vault, &path, json!({}), 10);
    let id = saved.answer["profile"]["id"].as_str().unwrap().to_string();
    let deleted = run(Some(&vault), DELETE, &json!({ "profile": id }), 0).unwrap();
    assert_eq!(deleted.answer["deleted"]["id"], id.as_str());
    assert!(vault.calibration_profiles().unwrap().is_empty());
    let refused = run(Some(&vault), GET, &json!({ "profile": id }), 0).unwrap_err();
    assert_eq!(refused.key, "calibrate-no-profile");

    vault.revert(&deleted.changes).unwrap();
    assert!(vault.calibration_profile(&id).unwrap().is_some());
    // The save undone, then redone.
    vault.revert(&saved.changes).unwrap();
    assert!(vault.calibration_profiles().unwrap().is_empty());
    vault.replay(&saved.changes).unwrap();
    assert!(vault.calibration_profile(&id).unwrap().is_some());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn one_measurement_is_one_profile_and_another_shot_under_the_light_a_second() {
    let path = picture("twice");
    let vault = vault();
    let first = save(&vault, &path, json!({ "name": "A" }), 10);
    let again = save(&vault, &path, json!({ "name": "A" }), 10);
    assert_eq!(first.answer["profile"]["id"], again.answer["profile"]["id"]);
    assert!(
        again.changes.is_empty(),
        "the same profile again changes nothing"
    );
    // Saved again later under another name: still one profile, renamed.
    let renamed = save(&vault, &path, json!({ "name": "B" }), 20);
    assert_eq!(
        first.answer["profile"]["id"],
        renamed.answer["profile"]["id"]
    );
    assert_eq!(vault.calibration_profiles().unwrap().len(), 1);
    // Another measurement of the same camera and light: both kept.
    let other = save(&vault, &path, json!({ "exclude": ["B3"] }), 30);
    assert_ne!(first.answer["profile"]["id"], other.answer["profile"]["id"]);
    assert_eq!(vault.calibration_profiles().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_save_needs_a_vault_and_a_dry_run_writes_nothing() {
    let path = picture("dry");
    let params = json!({ "path": path, "corners": CORNERS, "chart": null });
    let dry = run(None::<&Vault<MemoryStore>>, CHART, &params, 0).unwrap();
    assert_eq!(dry.answer["dry_run"], true);
    assert!(dry.answer.get("profile").is_none());
    let vault = vault();
    run(Some(&vault), CHART, &params, 0).unwrap();
    assert!(vault.calibration_profiles().unwrap().is_empty());

    let mut saving = params.clone();
    saving["save"] = json!(true);
    let refused = run(None::<&Vault<MemoryStore>>, CHART, &saving, 0).unwrap_err();
    assert_eq!(refused.key, "calibrate-needs-vault");
    let mut writing = params;
    writing["dry_run"] = json!(false);
    let refused = run(Some(&vault), CHART, &writing, 0).unwrap_err();
    assert_eq!(refused.key, "calibrate-save-needed");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn the_same_shot_saves_the_same_profile_byte_for_byte() {
    let path = picture("same");
    let (a, b) = (vault(), vault());
    let id = save(&a, &path, json!({ "name": "X" }), 5).answer["profile"]["id"].clone();
    save(&b, &path, json!({ "name": "X" }), 5);
    let doc = |v: &Vault<MemoryStore>| {
        serde_json::to_string(&v.calibration_profile(id.as_str().unwrap()).unwrap()).unwrap()
    };
    assert_eq!(doc(&a), doc(&b));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn every_command_has_a_schema_its_example_fits() {
    use crate::commands::validate;
    let examples = [
        json!({ "path": "/chart.tif", "image": null, "corners": [[0, 0], [1, 0], [1, 1], [0, 1]],
                "chart": null, "exclude": ["B3"], "dry_run": true, "save": true, "name": "Flash" }),
        json!({ "camera": "nikon z 6" }),
        json!({ "profile": "ab" }),
        json!({ "profile": "ab" }),
    ];
    let specs = specs();
    assert_eq!(specs.len(), examples.len());
    for (spec, example) in specs.iter().zip(examples) {
        assert!(handles(&spec.id));
        validate(&spec.params, &example).unwrap_or_else(|e| panic!("{}: {e}", spec.id));
    }
}
