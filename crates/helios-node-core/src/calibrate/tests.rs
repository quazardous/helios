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
        json!({ "profiles": ["ab"], "camera": null, "dry_run": true }),
        json!({ "images": ["/a.nef"], "profile": "auto", "dry_run": false }),
        json!({ "profile": "ab", "path": "/profiles" }),
        json!({ "path": null, "file": { "kind": "helios.chartprofile" }, "dry_run": true }),
        json!({ "images": ["/a.nef"], "dry_run": true }),
    ];
    let specs = specs();
    assert_eq!(specs.len(), examples.len());
    for (spec, example) in specs.iter().zip(examples) {
        assert!(handles(&spec.id));
        validate(&spec.params, &example).unwrap_or_else(|e| panic!("{}: {e}", spec.id));
    }
}

/// A vault holding the chart picture as an image, and its saved profile.
fn applied_vault(name: &str) -> (Vault<MemoryStore>, PathBuf, ImageId, String) {
    let path = picture(name);
    let vault = vault();
    let image = vault.import_file(&path, 1).unwrap();
    let saved = save(&vault, &path, json!({ "name": "Flash" }), 10);
    let id = saved.answer["profile"]["id"].as_str().unwrap().to_string();
    (vault, path, image, id)
}

fn head(vault: &Vault<MemoryStore>, image: ImageId) -> Option<Value> {
    helios_vault::EditSession::open(vault, image)
        .unwrap()
        .1
        .map(|v| v.edit)
}

#[test]
fn applying_a_profile_turns_on_the_chart_profile_and_undo_takes_it_back() {
    let (vault, path, image, id) = applied_vault("apply");
    let params = json!({ "images": [image.hex()], "profile": id });
    let dry = run(
        Some(&vault),
        APPLY,
        &json!({ "images": [path], "profile": id, "dry_run": true }),
        20,
    )
    .unwrap();
    assert!(dry.changes.is_empty());
    assert_eq!(head(&vault, image), None, "a dry run writes nothing");
    assert_eq!(
        dry.answer["images"][0]["params"]["profile"]["text"],
        id.as_str()
    );

    let done = run_as(Some(&vault), APPLY, &params, "agent:test", 20).unwrap();
    assert_eq!(done.changes.images.len(), 1, "one step");
    let edit = head(&vault, image).unwrap();
    assert_eq!(apply::applied(&edit).as_deref(), Some(id.as_str()));
    // The profile's matrix, copied into the parameters.
    let profile: Profile =
        serde_json::from_value(vault.calibration_profile(&id).unwrap().unwrap()).unwrap();
    let entry = &edit["modules"][0];
    assert_eq!(entry["module"], apply::MODULE);
    let matrix: Vec<f64> = entry["params"]["matrix"]["list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(
        matrix,
        profile.matrix.iter().flatten().copied().collect::<Vec<_>>()
    );
    let versions = vault.versions(&image).unwrap();
    assert_eq!(versions[0].origin, "agent:test");

    // Undo: the version and so the module are gone; redo brings them back.
    vault.revert(&done.changes).unwrap();
    assert_eq!(head(&vault, image), None);
    vault.replay(&done.changes).unwrap();
    assert_eq!(
        apply::applied(&head(&vault, image).unwrap()).as_deref(),
        Some(id.as_str())
    );

    // `none` turns it off, its values kept.
    let off = run(
        Some(&vault),
        APPLY,
        &json!({ "images": [image.hex()], "profile": "none" }),
        30,
    )
    .unwrap();
    assert_eq!(off.changes.images.len(), 1);
    let edit = head(&vault, image).unwrap();
    assert_eq!(apply::applied(&edit), None);
    assert_eq!(edit["modules"][0]["params"]["profile"]["text"], id.as_str());
    vault.revert(&off.changes).unwrap();
    assert!(apply::applied(&head(&vault, image).unwrap()).is_some());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// The saved profile, as one of camera `camera` under light `xy`.
fn as_camera(vault: &Vault<MemoryStore>, from: &str, id: &str, camera: &str, xy: [f64; 2]) {
    let mut doc = vault.calibration_profile(from).unwrap().unwrap();
    doc["camera"]["key"] = json!(camera);
    doc["illuminant"]["xy"] = json!(xy);
    vault.set_calibration_profile(id, Some(&doc)).unwrap();
}

const D65: [f64; 2] = [0.3127, 0.3290];
const D50: [f64; 2] = [0.3457, 0.3585];
const TUNGSTEN: [f64; 2] = [0.4234, 0.3990];
const FOUR_K: [f64; 2] = [0.3805, 0.3768];

#[test]
fn auto_picks_the_camera_s_profile_nearest_the_shot_s_light() {
    let (vault, path, image, id) = applied_vault("auto");
    as_camera(&vault, &id, "daylight", "nikon z 6", D65);
    as_camera(&vault, &id, "studio", "nikon z 6", D50);
    as_camera(&vault, &id, "tungsten", "nikon z 6", TUNGSTEN);
    as_camera(&vault, &id, "other-body", "canon eos r5", [0.35, 0.36]);
    let shot = |light: Option<[f64; 2]>, camera: &'static str| {
        move |_: &Path| {
            Ok(Photo {
                camera: camera.into(),
                light,
            })
        }
    };
    let params = json!({ "images": [image.hex()], "profile": "auto" });
    let picked = |light, camera| {
        apply_with(&vault, &params, "user", 40, &shot(light, camera)).map(|d| {
            d.answer["images"][0]["profile"]
                .as_str()
                .unwrap()
                .to_string()
        })
    };
    // A shot a little warmer than D50: the studio profile, not the other
    // camera's nearer one.
    assert_eq!(picked(Some([0.350, 0.360]), "nikon z 6").unwrap(), "studio");
    assert_eq!(picked(Some([0.42, 0.40]), "nikon z 6").unwrap(), "tungsten");
    assert_eq!(
        apply::applied(&head(&vault, image).unwrap()).as_deref(),
        Some("tungsten")
    );

    // The camera's default narrows the choice.
    let set = run(
        Some(&vault),
        SET_DEFAULT,
        &json!({ "profiles": ["daylight", "tungsten"] }),
        0,
    )
    .unwrap();
    assert_eq!(set.answer["camera"], "nikon z 6");
    assert_eq!(set.answer["after"]["pick"], "nearest");
    assert_eq!(set.changes.documents.len(), 1);
    // A 4000 K shot: 0.019 from the studio profile in uv, beyond 0.02 from
    // the two the default keeps.
    let refused = picked(Some(FOUR_K), "nikon z 6").unwrap_err();
    assert_eq!(refused.key, "calibrate-auto-too-far", "{}", refused.english);
    assert_eq!(picked(Some(D65), "nikon z 6").unwrap(), "daylight");
    // Cleared (undone), every profile of the camera is a candidate again.
    vault.revert(&set.changes).unwrap();
    assert_eq!(picked(Some(FOUR_K), "nikon z 6").unwrap(), "studio");
    let mixed = run(
        Some(&vault),
        SET_DEFAULT,
        &json!({ "profiles": ["daylight", "other-body"] }),
        0,
    )
    .unwrap_err();
    assert_eq!(mixed.key, "calibrate-default-cameras");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn auto_refuses_with_its_reason_and_writes_nothing() {
    let (vault, path, image, id) = applied_vault("refuse");
    as_camera(&vault, &id, "daylight", "nikon z 6", D65);
    let params = json!({ "images": [image.hex()], "profile": "auto" });
    let refused = |light: Option<[f64; 2]>, camera: &'static str| {
        apply_with(&vault, &params, "user", 40, &move |_: &Path| {
            Ok(Photo {
                camera: camera.into(),
                light,
            })
        })
        .unwrap_err()
    };
    let none = refused(Some(D65), "fujifilm x-t5");
    assert_eq!(none.key, "calibrate-auto-no-profile");
    assert!(none.english.contains("fujifilm x-t5"), "{}", none.english);
    let far = refused(Some(TUNGSTEN), "nikon z 6");
    assert_eq!(far.key, "calibrate-auto-too-far");
    assert_eq!(refused(None, "nikon z 6").key, "calibrate-auto-no-light");
    assert_eq!(head(&vault, image), None, "nothing written");
    // A profile of another camera is refused too.
    let other = run(
        Some(&vault),
        APPLY,
        &json!({ "images": [image.hex()], "profile": "daylight" }),
        0,
    )
    .unwrap_err();
    assert_eq!(other.key, "calibrate-other-camera");
    // Read from its file, a picture has no as-shot light to pick by; its
    // own profile, named, fits it.
    let picture = run(
        Some(&vault),
        APPLY,
        &json!({ "images": [image.hex()], "profile": "auto" }),
        0,
    )
    .unwrap_err();
    assert_eq!(picture.key, "calibrate-auto-no-light");
    run(
        Some(&vault),
        APPLY,
        &json!({ "images": [image.hex()], "profile": id }),
        0,
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn an_exported_profile_imports_into_another_library_as_the_same_profile() {
    let (vault, path, _, id) = applied_vault("exchange");
    let dir = path.parent().unwrap();
    let out = run(
        Some(&vault),
        EXPORT,
        &json!({ "profile": id, "path": dir }),
        0,
    )
    .unwrap();
    assert!(out.changes.is_empty());
    let file = PathBuf::from(out.answer["path"].as_str().unwrap());
    assert_eq!(
        file.file_name().unwrap().to_str().unwrap(),
        "Flash.helios-chartprofile.json"
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains(dir.to_str().unwrap()), "no local path");

    let other = self::vault();
    let dry = run(
        Some(&other),
        IMPORT,
        &json!({ "path": file, "dry_run": true }),
        0,
    )
    .unwrap();
    assert!(dry.changes.is_empty());
    assert!(other.calibration_profiles().unwrap().is_empty());
    let done = run(Some(&other), IMPORT, &json!({ "path": file }), 0).unwrap();
    assert_eq!(done.answer["profile"]["id"], id.as_str());
    assert_eq!(done.answer["existed"], false);
    // The same profile, its numbers as JSON text gives them back.
    let read = |v: &Vault<MemoryStore>| -> Profile {
        serde_json::from_value(v.calibration_profile(&id).unwrap().unwrap()).unwrap()
    };
    let (theirs, ours) = (read(&other), read(&vault));
    assert_eq!(
        (&theirs.name, &theirs.camera, &theirs.measured),
        (&ours.name, &ours.camera, &ours.measured)
    );
    for (a, b) in theirs
        .matrix
        .iter()
        .flatten()
        .zip(ours.matrix.iter().flatten())
    {
        assert!((a - b).abs() <= 1e-12 * b.abs().max(1.0), "{a} against {b}");
    }
    // Undone like a save.
    other.revert(&done.changes).unwrap();
    assert!(other.calibration_profile(&id).unwrap().is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_bad_profile_file_is_refused_with_its_reasons_and_nothing_added() {
    let (vault, path, _, id) = applied_vault("bad-file");
    let mut file = run(Some(&vault), EXPORT, &json!({ "profile": id }), 0)
        .unwrap()
        .answer["file"]
        .clone();
    file["profile"]["illuminant"]["xy"] = json!([0.9, 0.5]);
    let other = self::vault();
    let refused = run(Some(&other), IMPORT, &json!({ "file": file }), 0).unwrap_err();
    assert_eq!(refused.key, "calibrate-import-refused");
    assert!(
        refused.english.contains("outside the diagram"),
        "{}",
        refused.english
    );
    let junk = path.parent().unwrap().join("junk.json");
    std::fs::write(&junk, "not json").unwrap();
    let unreadable = run(Some(&other), IMPORT, &json!({ "path": junk }), 0).unwrap_err();
    assert_eq!(unreadable.key, "calibrate-import-unreadable-file");
    assert!(other.calibration_profiles().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_raw_opened_the_first_time_gets_its_camera_s_default_once() {
    let (vault, path, image, id) = applied_vault("on-open");
    as_camera(&vault, &id, "daylight", "nikon z 6", D65);
    as_camera(&vault, &id, "tungsten", "nikon z 6", TUNGSTEN);
    let shot = Photo {
        camera: "nikon z 6".into(),
        light: Some([0.3130, 0.3295]),
    };
    // No default rule: nothing happens.
    assert!(matches!(
        default_on_open(&vault, image, &shot, "user", 50).unwrap(),
        OnOpen::NoDefault
    ));
    assert_eq!(head(&vault, image), None);
    run(
        Some(&vault),
        SET_DEFAULT,
        &json!({ "profiles": ["daylight", "tungsten"] }),
        0,
    )
    .unwrap();
    let OnOpen::Applied {
        profile, changes, ..
    } = default_on_open(&vault, image, &shot, "user", 50).unwrap()
    else {
        panic!("the default applies")
    };
    assert_eq!(profile, "daylight");
    assert_eq!(changes.images.len(), 1);
    assert_eq!(
        apply::applied(&head(&vault, image).unwrap()).as_deref(),
        Some("daylight")
    );
    // Opened again: the edit is the user's now, left alone.
    let tungsten = Photo {
        light: Some(TUNGSTEN),
        ..shot.clone()
    };
    assert!(matches!(
        default_on_open(&vault, image, &tungsten, "user", 60).unwrap(),
        OnOpen::Edited
    ));
    assert_eq!(vault.versions(&image).unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_default_that_picks_none_leaves_the_photo_untouched_with_its_reason() {
    let (vault, path, image, id) = applied_vault("on-open-refused");
    as_camera(&vault, &id, "daylight", "nikon z 6", D65);
    run(
        Some(&vault),
        SET_DEFAULT,
        &json!({ "profiles": ["daylight"] }),
        0,
    )
    .unwrap();
    let under = |light| Photo {
        camera: "nikon z 6".into(),
        light,
    };
    for (light, key) in [
        (Some(TUNGSTEN), "calibrate-auto-too-far"),
        (None, "calibrate-auto-no-light"),
    ] {
        match default_on_open(&vault, image, &under(light), "user", 50).unwrap() {
            OnOpen::Refused(m) => assert_eq!(m.key, key, "{}", m.english),
            other => panic!("refused, not {other:?}"),
        }
    }
    assert_eq!(head(&vault, image), None, "untouched");
    // A picture never gets one.
    let picture = Photo {
        camera: "picture".into(),
        light: None,
    };
    assert!(matches!(
        default_on_open(&vault, image, &picture, "user", 50).unwrap(),
        OnOpen::NoDefault
    ));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
