//! A profile applied to a photo (`docs/design/calibration.md`, sections 6
//! and 7): what `calibrate.apply` writes in an edit, and which profile a
//! photo gets when none is named.
//!
//! - [`with_profile`] sets the `chartprofile` node of an edit to a
//!   profile: its matrix `M_c` and its id copied into the parameters (a
//!   module does no IO), the node turned on. [`without_profile`] turns it
//!   off, its parameters kept.
//! - [`pick`] chooses among a camera's profiles the one whose illuminant
//!   is nearest the photo's as-shot light, within [`MAX_UV_DISTANCE`] in
//!   CIE 1960 `uv`; otherwise none, with the reason.
//! - [`Rule`] is a camera's default, kept in the vault's
//!   `calibration/defaults` namespace under the camera's key.

use helios_core::{Params, Value as Param};
use helios_photo::color::Mat3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The module, and the node it runs in.
pub const MODULE: &str = "chartprofile";

/// How far the photo's light may be from a profile's, in CIE 1960 `uv`.
pub const MAX_UV_DISTANCE: f64 = 0.02;

/// How a camera's default picks among its profiles.
pub const PICK_NEAREST: &str = "nearest";

/// A camera's default: the profiles a photo of it may get, and how one is
/// picked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub profiles: Vec<String>,
    pub pick: String,
}

impl Rule {
    pub fn nearest(profiles: Vec<String>) -> Self {
        Self {
            profiles,
            pick: PICK_NEAREST.into(),
        }
    }
}

/// A raw's as-shot light: the illuminant colour calibration finds from
/// the camera's matrix (`XYZ_TO_CAMERA`, row by row) and its as-shot white
/// balance coefficients, as `channelmixerrgb`'s "as shot in camera".
/// `None` where it finds none.
pub fn as_shot_light(xyz_to_camera: &[f64], white_balance: &[f64]) -> Option<[f64; 2]> {
    helios_module_channelmixerrgb::as_shot_illuminant_xy(xyz_to_camera, white_balance)
        .map(|xy| xy.map(f64::from))
}

/// CIE 1960 `uv` of a chromaticity.
pub fn uv([x, y]: [f64; 2]) -> [f64; 2] {
    let d = -2.0 * x + 12.0 * y + 3.0;
    [4.0 * x / d, 6.0 * y / d]
}

/// The distance between two chromaticities in CIE 1960 `uv`.
pub fn uv_distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (a, b) = (uv(a), uv(b));
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// A profile a photo may get: its id and its illuminant's `xy`.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub xy: [f64; 2],
}

/// The profile picked, and how far its light is from the photo's.
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    pub id: String,
    pub distance: f64,
}

/// Why no profile fits a photo.
#[derive(Clone, Debug, PartialEq)]
pub enum PickError {
    /// The camera has no profile.
    NoProfile,
    /// The nearest profile's light is too far from the photo's.
    TooFar { nearest: String, distance: f64 },
}

/// The candidate whose illuminant is nearest `xy` (the photo's as-shot
/// light), if within [`MAX_UV_DISTANCE`]; ties go to the smaller id, so
/// the same profiles always give the same pick.
///
/// # Errors
/// No candidate, or the nearest too far.
pub fn pick(candidates: &[Candidate], xy: [f64; 2]) -> Result<Picked, PickError> {
    let nearest = candidates
        .iter()
        .map(|c| (uv_distance(c.xy, xy), &c.id))
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .ok_or(PickError::NoProfile)?;
    if nearest.0 <= MAX_UV_DISTANCE {
        Ok(Picked {
            id: nearest.1.clone(),
            distance: nearest.0,
        })
    } else {
        Err(PickError::TooFar {
            nearest: nearest.1.clone(),
            distance: nearest.0,
        })
    }
}

/// The `chartprofile` node's parameters for a profile, as an edit holds
/// them.
pub fn params(id: &str, matrix: &Mat3) -> Value {
    let params = Params::new()
        .with(
            "matrix",
            Param::List(matrix.iter().flatten().copied().collect()),
        )
        .with("profile", Param::Text(id.into()));
    serde_json::to_value(&params).expect("parameters serialise")
}

/// `edit` (an image's edit, `null` for none yet) with its `chartprofile`
/// node set to the profile `id` (matrix `matrix`) and turned on; the other
/// nodes kept as they are.
pub fn with_profile(edit: &Value, id: &str, matrix: &Mat3) -> Value {
    let mut edit = base(edit);
    let entry = json!({ "name": MODULE, "module": MODULE, "params": params(id, matrix) });
    let modules = edit["modules"].as_array_mut().expect("an array");
    match modules.iter_mut().find(|m| m["name"] == MODULE) {
        Some(m) => *m = entry,
        None => modules.push(entry),
    }
    edit
}

/// `edit` with its `chartprofile` node turned off (bypassed, its
/// parameters kept); the same edit when it has none on.
pub fn without_profile(edit: &Value) -> Value {
    let mut edit = base(edit);
    for m in edit["modules"].as_array_mut().into_iter().flatten() {
        if m["name"] == MODULE {
            m["bypassed"] = Value::Bool(true);
        }
    }
    edit
}

/// The profile an edit's `chartprofile` node holds, when it is on.
pub fn applied(edit: &Value) -> Option<String> {
    edit["modules"]
        .as_array()?
        .iter()
        .find(|m| m["name"] == MODULE && m["bypassed"] != true)
        .and_then(|m| m["params"]["profile"]["text"].as_str())
        .filter(|id| !id.is_empty())
        .map(String::from)
}

fn base(edit: &Value) -> Value {
    let mut edit = if edit.is_object() {
        edit.clone()
    } else {
        json!({})
    };
    if !edit["modules"].is_array() {
        edit["modules"] = json!([]);
    }
    edit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, xy: [f64; 2]) -> Candidate {
        Candidate { id: id.into(), xy }
    }

    /// D65, D50 and a 3200 K tungsten light.
    const D65: [f64; 2] = [0.3127, 0.3290];
    const D50: [f64; 2] = [0.3457, 0.3585];
    const TUNGSTEN: [f64; 2] = [0.4234, 0.3990];

    #[test]
    fn the_nearest_illuminant_is_picked() {
        let profiles = [
            candidate("daylight", D65),
            candidate("studio", D50),
            candidate("tungsten", TUNGSTEN),
        ];
        // A shot a little warmer than D50.
        let picked = pick(&profiles, [0.350, 0.360]).unwrap();
        assert_eq!(picked.id, "studio");
        assert!(picked.distance < 0.005, "{picked:?}");
        assert_eq!(pick(&profiles, [0.42, 0.40]).unwrap().id, "tungsten");
        assert_eq!(pick(&profiles, D65).unwrap().id, "daylight");
        // Order does not matter.
        let mut reversed = profiles.to_vec();
        reversed.reverse();
        assert_eq!(pick(&reversed, [0.350, 0.360]).unwrap().id, "studio");
    }

    #[test]
    fn a_light_too_far_or_no_profile_is_refused_with_its_reason() {
        assert_eq!(pick(&[], D65), Err(PickError::NoProfile));
        // Tungsten against a daylight profile only: far beyond 0.02.
        match pick(&[candidate("daylight", D65)], TUNGSTEN) {
            Err(PickError::TooFar { nearest, distance }) => {
                assert_eq!(nearest, "daylight");
                assert!(distance > MAX_UV_DISTANCE, "{distance}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn ties_go_to_the_smaller_id() {
        let profiles = [candidate("b", D50), candidate("a", D50)];
        assert_eq!(pick(&profiles, D50).unwrap().id, "a");
    }

    #[test]
    fn an_edit_gets_the_profile_then_loses_it_with_its_other_nodes_kept() {
        let edit = json!({ "modules": [
            { "name": "exposure", "module": "exposure", "params": { "exposure": { "float": 1.0 } } }
        ] });
        let m: Mat3 = [[1.1, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.9]];
        let on = with_profile(&edit, "p1", &m);
        assert_eq!(applied(&on).as_deref(), Some("p1"));
        assert_eq!(on["modules"][0], edit["modules"][0]);
        let entry = &on["modules"][1];
        assert_eq!(entry["module"], MODULE);
        assert_eq!(entry["params"]["matrix"]["list"][0], 1.1);
        // Another profile replaces it; none turns it off, values kept.
        let other = with_profile(&on, "p2", &m);
        assert_eq!(other["modules"].as_array().unwrap().len(), 2);
        assert_eq!(applied(&other).as_deref(), Some("p2"));
        let off = without_profile(&other);
        assert_eq!(applied(&off), None);
        assert_eq!(off["modules"][1]["params"]["profile"]["text"], "p2");
        // A photo never edited.
        let first = with_profile(&Value::Null, "p1", &m);
        assert_eq!(first["modules"].as_array().unwrap().len(), 1);
        // The parameters are the module's.
        let params: Params = serde_json::from_value(entry["params"].clone()).unwrap();
        assert_eq!(params.get("profile"), Some(&Param::Text("p1".into())));
    }
}
