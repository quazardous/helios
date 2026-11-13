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
//! table this Helios cannot render, an id that is not a hash. Formats 1
//! (no residual table) and 2 are read. The profile keeps the
//! id the file names, so one shared back to its library is the same
//! profile.

use helios_core::Message;
use serde_json::{Value, json};

use crate::profile::{Profile, READ_FORMATS, camera_key};

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
#[allow(
    clippy::too_many_lines,
    reason = "every check of the file, in the order the refusals list them"
)]
pub fn read(file: &Value) -> Result<(String, Profile), Vec<Message>> {
    if file["kind"] != KIND {
        return Err(vec![Message::new(
            "calibrate-import-not-profile",
            "the file is not a Helios calibration profile",
        )]);
    }
    let doc = &file["profile"];
    match doc["format"].as_u64() {
        Some(f) if READ_FORMATS.iter().any(|&r| u64::from(r) == f) => {}
        found => {
            return Err(vec![
                Message::new(
                    "calibrate-import-format",
                    "the profile's format is { $found }; this Helios reads formats { $expected }",
                )
                .with(
                    "found",
                    found.map_or_else(|| "missing".to_string(), |f| f.to_string()),
                )
                .with("expected", READ_FORMATS.map(|f| f.to_string()).join(", ")),
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
    if profile
        .lut
        .as_ref()
        .is_some_and(|t| profile.format < 2 || !t.is_valid())
    {
        reasons.push(Message::new(
            "calibrate-import-table",
            "the profile's residual table is not one this Helios can apply",
        ));
    }
    // The id the file names is kept, so a profile shared back to the
    // library it came from is the same profile. Its hash cannot be checked
    // against the content: a number read back from JSON text may differ
    // from the one written in its last bit.
    let id = match file["id"].as_str() {
        None => profile.id(),
        Some(named) if named.len() == 64 && named.bytes().all(|b| b.is_ascii_hexdigit()) => {
            named.to_ascii_lowercase()
        }
        Some(_) => {
            reasons.push(Message::new(
                "calibrate-import-id",
                "the profile's id is not a 64-digit hexadecimal hash",
            ));
            String::new()
        }
    };
    if reasons.is_empty() {
        Ok((id, profile))
    } else {
        Err(reasons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::{HUES, HueSatTable, SATURATIONS, VALUES};
    use crate::profile::{PROFILE_FORMAT, ProfileCamera, ProfileMeasured, ProfileReport};
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
                residual: None,
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
            read(v).unwrap_err().into_iter().map(|m| m.key).collect()
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
        file["id"] = json!("not a hash");
        assert_eq!(
            keys(&file),
            [
                "calibrate-import-camera-key",
                "calibrate-import-matrix",
                "calibrate-import-illuminant",
                "calibrate-import-id",
            ]
        );
        let mut file = write(&with_table());
        file["profile"]["lut"]["nodes"][0] = json!([0.0, 9.0, 1.0]);
        assert_eq!(keys(&file), ["calibrate-import-table"]);
        let mut file = write(&with_table());
        file["profile"]["format"] = json!(1);
        assert_eq!(keys(&file), ["calibrate-import-table"]);
        // Renamed by hand: the name is not part of the id.
        let mut file = write(&profile());
        file["profile"]["name"] = json!("Renamed");
        assert_eq!(read(&file).unwrap().1.name, "Renamed");
    }

    /// The profile with a residual table: a gentle hue turn on coloured
    /// nodes, varying with hue.
    fn with_table() -> Profile {
        let mut table = HueSatTable::identity([0.55, 0.6, 0.58]);
        for v in 0..VALUES {
            for h in 0..HUES {
                for s in 1..SATURATIONS {
                    let i = (v * HUES + h) * SATURATIONS + s;
                    table.nodes[i] = [
                        3.0 * (h as f64 / 7.0).sin() + 0.1,
                        1.0 + 0.013 * s as f64,
                        1.0,
                    ];
                }
            }
        }
        Profile {
            lut: Some(table),
            ..profile()
        }
    }

    #[test]
    fn a_profile_with_its_table_round_trips_through_its_file() {
        let p = with_table();
        let text = serde_json::to_string_pretty(&write(&p)).unwrap();
        let (id, back) = read(&serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(id, p.id());
        let (a, b) = (p.lut.as_ref().unwrap(), back.lut.as_ref().unwrap());
        assert_eq!(a.nodes.len(), b.nodes.len());
        for (x, y) in a.to_param().iter().zip(b.to_param()) {
            assert!((x - y).abs() <= 1e-15 * x.abs().max(1.0), "{x} {y}");
        }
        assert_eq!(Profile { lut: None, ..back }, Profile { lut: None, ..p });
    }

    #[test]
    fn a_format_1_profile_still_reads() {
        // A profile shared before residual tables: format 1, `lut` null,
        // no residual check in its report.
        let mut file = write(&profile());
        file["profile"]["format"] = json!(1);
        file["profile"]["report"]
            .as_object_mut()
            .unwrap()
            .remove("residual");
        let (_, back) = read(&file).unwrap();
        assert_eq!(back.format, 1);
        assert_eq!(back.lut, None);
        assert_eq!(back.report.residual, None);
    }
}
