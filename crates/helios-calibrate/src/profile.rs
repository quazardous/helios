//! A calibration profile: what a chart's dry run found, kept in the vault
//! (`docs/design/calibration.md`, section 6).
//!
//! A [`Profile`] is made from a [`ChartAnswer`] and what was measured (the
//! camera, the shot's content id, the raw-front parameters of its edit).
//! It carries the format's version ([`PROFILE_FORMAT`]), as a fitted look
//! carries its fitter's: a later solver writes a new format, never a new
//! meaning under the old one.
//!
//! Its id is the BLAKE3 hash (hex) of its canonical JSON without its name
//! and creation time: the same measurement saved twice is one profile, as
//! an image is one by its content; two shots of one camera under one light
//! are two profiles, both kept. Same inputs, same profile, byte for byte.

use helios_photo::CameraInfo;
use helios_photo::color::Mat3;
use serde::{Deserialize, Serialize};

use crate::look::HueSatTable;
use crate::request::{ChartAnswer, IlluminantAnswer, ResidualAnswer, SummaryAnswer};

/// The profile document's format, the one written.
///
/// 2: the residual table (`lut`, a hue/saturation table) and its check
/// in the report (`report.residual`).
pub const PROFILE_FORMAT: u32 = 2;

/// The formats this Helios reads: format 1 is format 2 without a residual
/// table, and reads and applies as it did.
pub const READ_FORMATS: [u32; 2] = [1, PROFILE_FORMAT];

/// The camera a profile belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileCamera {
    pub make: String,
    pub model: String,
    /// The make and model normalised ([`camera_key`]): what profiles are
    /// grouped and looked up by. `picture` for a decoded picture, which
    /// names no camera.
    pub key: String,
}

/// Where and how the chart was measured.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileMeasured {
    /// `camera` (a raw's white-balanced camera RGB) or `picture` (a
    /// decoded picture's working RGB: approximate).
    pub space: String,
    /// The chart shot's content id.
    pub image: String,
    /// The edit version whose raw-front parameters the raw was measured
    /// with; `None` when it was measured with the modules' defaults.
    pub edit: Option<String>,
    /// The handles, in the chart's corner order, in the measured pixels.
    pub corners: [[f64; 2]; 4],
    /// Patches left out by name.
    pub excluded: Vec<String>,
}

/// How the profile fitted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileReport {
    /// darktable's word for the mean Delta E after.
    pub quality: String,
    pub exposure_ev: f64,
    /// Delta E 2000 through the standard matrix.
    pub before: SummaryAnswer,
    /// Delta E 2000 through the profile's matrix.
    pub after: SummaryAnswer,
    /// The residual table's check (format 2): kept or dropped, the
    /// held-out means without and with it; the table itself is `lut`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual: Option<ResidualAnswer>,
}

/// A calibration profile, as the vault keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub format: u32,
    pub name: String,
    pub camera: ProfileCamera,
    pub illuminant: IlluminantAnswer,
    /// The chart's id.
    pub chart: String,
    pub measured: ProfileMeasured,
    /// The camera's `XYZ_TO_CAMERA` the raw was measured with (`M_s` comes
    /// from it); `None` for a picture.
    pub base_matrix: Option<[f64; 9]>,
    /// `M_c`: camera RGB to the working space.
    pub matrix: Mat3,
    /// `P`: the correction after the standard matrix.
    pub correction: Mat3,
    /// The residual table after the matrix, when the fit kept it: a
    /// hue/saturation table in DNG's layout ([`HueSatTable`]); `None` for
    /// a matrix-only profile (and every format 1 profile).
    pub lut: Option<HueSatTable>,
    pub report: ProfileReport,
    /// Seconds since the epoch.
    pub created: u64,
}

/// What a profile is made of besides the answer.
#[derive(Clone, Debug)]
pub struct ProfileSource<'a> {
    /// The raw's camera; `None` for a picture.
    pub camera: Option<&'a CameraInfo>,
    /// The chart shot's content id.
    pub image: &'a str,
    /// The edit version measured with, if any.
    pub edit: Option<String>,
    /// Patches left out by name.
    pub excluded: &'a [String],
}

/// A camera's key: make and model in lower case, spaces collapsed, the
/// make not repeated when the model starts with it (`Nikon`, `Z 6` and
/// `NIKON`, `Nikon Z 6` are both `nikon z 6`).
pub fn camera_key(make: &str, model: &str) -> String {
    let words = |s: &str| {
        s.split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
    };
    let (make, model) = (words(make), words(model));
    let mut key = make.clone();
    let repeated = !make.is_empty() && model.starts_with(&make);
    key.extend(
        model
            .into_iter()
            .skip(if repeated { make.len() } else { 0 }),
    );
    key.join(" ")
}

impl Profile {
    /// The profile of `answer`, named `name`, made at `created`.
    pub fn new(answer: &ChartAnswer, source: &ProfileSource<'_>, name: &str, created: u64) -> Self {
        let camera = match source.camera {
            Some(c) => ProfileCamera {
                make: c.maker.clone(),
                model: c.model.clone(),
                key: camera_key(&c.maker, &c.model),
            },
            None => ProfileCamera {
                make: String::new(),
                model: String::new(),
                key: "picture".into(),
            },
        };
        Profile {
            format: PROFILE_FORMAT,
            name: name.into(),
            camera,
            illuminant: answer.illuminant.clone(),
            chart: answer.chart.clone(),
            measured: ProfileMeasured {
                space: if source.camera.is_some() {
                    "camera"
                } else {
                    "picture"
                }
                .into(),
                image: source.image.into(),
                edit: source.edit.clone(),
                corners: answer.corners,
                excluded: source.excluded.to_vec(),
            },
            base_matrix: source.camera.map(|c| c.xyz_to_camera),
            matrix: answer.camera_matrix,
            correction: answer.matrix,
            lut: answer.residual.as_ref().and_then(|r| r.table.clone()),
            report: ProfileReport {
                quality: answer.quality.clone(),
                exposure_ev: answer.exposure_ev,
                before: answer.before.clone(),
                after: answer.after.clone(),
                residual: answer.residual.as_ref().map(|r| ResidualAnswer {
                    table: None,
                    ..r.clone()
                }),
            },
            created,
        }
    }

    /// The profile's id: BLAKE3 (hex) of its JSON without its name and
    /// creation time.
    pub fn id(&self) -> String {
        let mut keyed = self.clone();
        keyed.name.clear();
        keyed.created = 0;
        let bytes = serde_json::to_vec(&keyed).expect("a profile serialises");
        blake3::hash(&bytes).to_hex().to_string()
    }

    /// A default name: the camera and the light (`Nikon Z 6, 3200 K`).
    pub fn default_name(&self) -> String {
        let camera = [self.camera.make.as_str(), self.camera.model.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        let light = format!("{:.0} K", self.illuminant.cct);
        if camera.is_empty() {
            light
        } else {
            format!("{camera}, {light}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_camera_key_ignores_case_spacing_and_a_repeated_make() {
        assert_eq!(camera_key("Nikon", "Z 6"), "nikon z 6");
        assert_eq!(camera_key("NIKON", "Nikon  Z 6"), "nikon z 6");
        assert_eq!(camera_key("", "X100V"), "x100v");
    }
}
