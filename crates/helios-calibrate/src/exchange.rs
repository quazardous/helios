//! A calibration profile as a file, to share it between libraries
//! (`docs/design/calibration.md`, section 6, "Sharing").
//!
//! The file, `<name>.helios-chartprofile.json`, is the profile document
//! the vault keeps, wrapped with what says what it is:
//!
//! ```jsonc
//! { "kind": "helios.chartprofile", "id": "<the profile's id>", "profile": { /* the document */ } }
//! ```
//!
//! It holds no local path: the chart shot is named by its content id
//! only. [`read`] checks a file before it is added, and refuses it with
//! every reason it finds, each keyed for translation: not a profile, a
//! format this Helios does not read, a camera key that does not match
//! the make and model, a matrix that is not finite or cannot be
//! inverted, an illuminant outside the chromaticity diagram, a residual
//! LUT (not taken yet), content that does not match its id.

use helios_core::Message;
use serde_json::{Value, json};

use crate::profile::{PROFILE_FORMAT, Profile, camera_key};

/// What the file says it is.
pub const KIND: &str = "helios.chartprofile";

/// The file's name ends with it.
pub const SUFFIX: &str = ".helios-chartprofile.json";

/// The file's content for `profile`.
pub fn write(profile: &Profile) -> Value {
    json!({ "kind": KIND, "id": profile.id(), "profile": profile })
}

/// The file's name for `profile`: its name with the characters a file
/// system may refuse replaced, then [`SUFFIX`].
pub fn file_name(profile: &Profile) -> String {
    let stem: String = profile
        .name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let stem = stem.trim().trim_start_matches('.');
    let stem = if stem.is_empty() { "profile" } else { stem };
    format!("{stem}{SUFFIX}")
}

fn finite(values: impl IntoIterator<Item = f64>) -> bool {
    values.into_iter().all(f64::is_finite)
}

/// The profile a file holds, and its id, once checked.
///
/// # Errors
/// Every reason the file is refused for, each keyed for translation.
pub fn read(file: &Value) -> Result<(String, Profile), Vec<Message>> {
    if file["kind"] != KIND {
        return Err(vec![Message::new(
            "calibrate-import-not-profile",
            "the file is not a Helios calibration profile",
        )]);
    }
    let doc = &file["profile"];
    match doc["format"].as_u64() {
        Some(f) if f == u64::from(PROFILE_FORMAT) => {}
        found => {
            return Err(vec![
                Message::new(
                    "calibrate-import-format",
                    "the profile's format is { $found }; this Helios reads format { $expected }",
                )
                .with(
                    "found",
                    found.map_or_else(|| "missing".to_string(), |f| f.to_string()),
                )
                .with("expected", PROFILE_FORMAT.to_string()),
            ]);
        }
    }
    let profile: Profile = serde_json::from_value(doc.clone()).map_err(|e| {
        vec![
            Message::new(
                "calibrate-import-unreadable",
                "the profile cannot be read: { $error }",
            )
            .with("error", e.to_string()),
        ]
    })?;
    let mut reasons = Vec::new();
    let c = &profile.camera;
    let key_fits = if c.key == "picture" {
        c.make.is_empty() && c.model.is_empty()
    } else {
        !c.key.is_empty() && c.key == camera_key(&c.make, &c.model)
    };
    if !key_fits {
        reasons.push(
            Message::new(
                "calibrate-import-camera-key",
                "the camera key “{ $key }” does not match the make and model “{ $make } { $model }”",
            )
            .with("key", c.key.clone())
            .with("make", c.make.clone())
            .with("model", c.model.clone()),
        );
    }
    let matrix = profile.matrix.iter().flatten().copied();
    let invertible = helios_photo::color::inverse(&profile.matrix).is_some();
    if !finite(matrix) || !invertible {
        reasons.push(Message::new(
            "calibrate-import-matrix",
            "the profile's matrix is not a finite matrix that can be inverted",
        ));
    }
    if !finite(profile.correction.iter().flatten().copied())
        || !profile
            .base_matrix
            .is_none_or(|m| finite(m.iter().copied()))
    {
        reasons.push(Message::new(
            "calibrate-import-numbers",
            "the profile's correction or camera matrix holds numbers that are not finite",
        ));
    }
    let [x, y] = profile.illuminant.xy;
    if !(x > 0.0 && y > 0.0 && x + y < 1.0) {
        reasons.push(
            Message::new(
                "calibrate-import-illuminant",
                "the light's chromaticity { $x }, { $y } is outside the diagram",
            )
            .with("x", x.to_string())
            .with("y", y.to_string()),
        );
    }
    if profile.lut.as_ref().is_some_and(|l| !l.is_null()) {
        reasons.push(Message::new(
            "calibrate-import-lut",
            "the profile carries a residual LUT, which this Helios does not take yet",
        ));
    }
    let id = profile.id();
    if let Some(named) = file["id"].as_str()
        && named != id
    {
        reasons.push(Message::new(
            "calibrate-import-id",
            "the profile's content does not match its id: it was changed or damaged",
        ));
    }
    if reasons.is_empty() {
        Ok((id, profile))
    } else {
        Err(reasons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{ProfileCamera, ProfileMeasured, ProfileReport};
    use crate::request::{IlluminantAnswer, SummaryAnswer};

    fn profile() -> Profile {
        let summary = SummaryAnswer {
            mean: 1.0,
            max: 2.0,
            worst: vec!["A1".into()],
        };
        Profile {
            format: PROFILE_FORMAT,
            name: "Studio: flash/2".into(),
            camera: ProfileCamera {
                make: "Nikon".into(),
                model: "Z 6".into(),
                key: "nikon z 6".into(),
            },
            illuminant: IlluminantAnswer {
                xy: [0.3127, 0.329],
                cct: 6504.0,
                kind: "d".into(),
            },
            chart: "xrite.cc24.2014".into(),
            measured: ProfileMeasured {
                space: "camera".into(),
                image: "ab".repeat(32),
                edit: None,
                corners: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                excluded: Vec::new(),
            },
            base_matrix: Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
            matrix: [[1.1, -0.1, 0.0], [0.0, 1.0, 0.0], [0.0, 0.05, 0.95]],
            correction: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            lut: None,
            report: ProfileReport {
                quality: "good".into(),
                exposure_ev: 0.1,
                before: summary.clone(),
                after: summary,
            },
            created: 1_700_000_000,
        }
    }

    #[test]
    fn a_written_profile_reads_back_the_same_through_its_text() {
        let p = profile();
        let text = serde_json::to_string_pretty(&write(&p)).unwrap();
        let (id, back) = read(&serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(back, p);
        assert_eq!(id, p.id());
        assert_eq!(file_name(&p), "Studio_ flash_2.helios-chartprofile.json");
    }

    #[test]
    fn a_bad_file_is_refused_with_every_reason() {
        let keys = |v: &Value| -> Vec<String> {
            read(v)
                .unwrap_err()
                .into_iter()
                .map(|m| m.key.to_string())
                .collect()
        };
        assert_eq!(
            keys(&json!({ "kind": "something" })),
            ["calibrate-import-not-profile"]
        );
        let mut file = write(&profile());
        file["profile"]["format"] = json!(99);
        assert_eq!(keys(&file), ["calibrate-import-format"]);
        let mut file = write(&profile());
        file["profile"]["matrix"] = json!("no");
        assert_eq!(keys(&file), ["calibrate-import-unreadable"]);
        let mut file = write(&profile());
        file["profile"]["camera"]["key"] = json!("canon eos r5");
        file["profile"]["matrix"] = json!([[1, 0, 0], [0, 1, 0], [0, 0, 0]]);
        file["profile"]["illuminant"]["xy"] = json!([0.8, 0.4]);
        assert_eq!(
            keys(&file),
            [
                "calibrate-import-camera-key",
                "calibrate-import-matrix",
                "calibrate-import-illuminant",
                "calibrate-import-id",
            ]
        );
        let mut file = write(&profile());
        file["profile"]["lut"] = json!({ "size": 9 });
        assert!(keys(&file).contains(&"calibrate-import-lut".to_string()));
        // Renamed by hand: the name is not part of the id.
        let mut file = write(&profile());
        file["profile"]["name"] = json!("Renamed");
        assert_eq!(read(&file).unwrap().1.name, "Renamed");
    }
}
