//! What the camera recorded, as context fields modules can declare.
//!
//! A module that needs camera data lists the fields it reads in its
//! manifest's `context` (for instance [`fields::EXPOSURE_BIAS`]); the
//! framework hands it only those (see `helios_core::Context`).

use helios_core::{Context, Value};
use serde::{Deserialize, Serialize};

use crate::cfa::Cfa;

/// The names of the context fields, one constant each.
pub mod fields {
    pub const MAKER: &str = "photo.maker";
    pub const MODEL: &str = "photo.model";
    pub const ISO: &str = "photo.iso";
    /// Exposure compensation dialed on the camera, in EV.
    pub const EXPOSURE_BIAS: &str = "photo.exposure_bias";
    pub const EXPOSURE_TIME: &str = "photo.exposure_time";
    pub const APERTURE: &str = "photo.aperture";
    pub const FOCAL_LENGTH: &str = "photo.focal_length";
    /// Black level per position of the 2 x 2 pattern.
    pub const BLACK: &str = "photo.black";
    pub const WHITE: &str = "photo.white";
    /// As-shot white balance multipliers (R, G, B, G2).
    pub const WHITE_BALANCE: &str = "photo.white_balance";
    /// The camera's matrix from CIE XYZ (D65) to camera RGB, row by row.
    pub const XYZ_TO_CAMERA: &str = "photo.xyz_to_camera";
    pub const CFA: &str = "photo.cfa";

    /// Every field, for modules that need the whole description.
    pub const ALL: &[&str] = &[
        MAKER,
        MODEL,
        ISO,
        EXPOSURE_BIAS,
        EXPOSURE_TIME,
        APERTURE,
        FOCAL_LENGTH,
        BLACK,
        WHITE,
        WHITE_BALANCE,
        XYZ_TO_CAMERA,
        CFA,
    ];
}

/// The description of a raw image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraInfo {
    pub maker: String,
    pub model: String,
    pub iso: f64,
    pub exposure_bias: f64,
    /// Seconds.
    pub exposure_time: f64,
    /// f-number.
    pub aperture: f64,
    /// Millimetres.
    pub focal_length: f64,
    pub black: [f64; 4],
    pub white: f64,
    pub white_balance: [f64; 4],
    pub xyz_to_camera: [f64; 9],
    pub cfa: Cfa,
}

impl CameraInfo {
    /// The description as context fields.
    pub fn to_context(&self) -> Context {
        Context::new()
            .with(fields::MAKER, Value::Choice(self.maker.clone()))
            .with(fields::MODEL, Value::Choice(self.model.clone()))
            .with(fields::ISO, Value::Float(self.iso))
            .with(fields::EXPOSURE_BIAS, Value::Float(self.exposure_bias))
            .with(fields::EXPOSURE_TIME, Value::Float(self.exposure_time))
            .with(fields::APERTURE, Value::Float(self.aperture))
            .with(fields::FOCAL_LENGTH, Value::Float(self.focal_length))
            .with(fields::BLACK, Value::List(self.black.to_vec()))
            .with(fields::WHITE, Value::Float(self.white))
            .with(
                fields::WHITE_BALANCE,
                Value::List(self.white_balance.to_vec()),
            )
            .with(
                fields::XYZ_TO_CAMERA,
                Value::List(self.xyz_to_camera.to_vec()),
            )
            .with(fields::CFA, Value::Choice(self.cfa.name()))
    }

    /// Reads the description back; names the first field missing or malformed.
    pub fn from_context(context: &Context) -> Result<Self, String> {
        let get = |name: &str| {
            context
                .get(name)
                .ok_or_else(|| format!("context field `{name}` missing"))
        };
        let float = |name: &str| {
            get(name)?
                .as_f64()
                .ok_or_else(|| format!("context field `{name}` is not a number"))
        };
        let text = |name: &str| {
            get(name)?
                .as_choice()
                .map(str::to_string)
                .ok_or_else(|| format!("context field `{name}` is not text"))
        };
        fn fixed<const N: usize>(name: &str, value: Option<&[f64]>) -> Result<[f64; N], String> {
            value
                .and_then(|v| <[f64; N]>::try_from(v).ok())
                .ok_or_else(|| format!("context field `{name}` is not a list of {N} numbers"))
        }
        Ok(Self {
            maker: text(fields::MAKER)?,
            model: text(fields::MODEL)?,
            iso: float(fields::ISO)?,
            exposure_bias: float(fields::EXPOSURE_BIAS)?,
            exposure_time: float(fields::EXPOSURE_TIME)?,
            aperture: float(fields::APERTURE)?,
            focal_length: float(fields::FOCAL_LENGTH)?,
            black: fixed(fields::BLACK, get(fields::BLACK)?.as_list())?,
            white: float(fields::WHITE)?,
            white_balance: fixed(fields::WHITE_BALANCE, get(fields::WHITE_BALANCE)?.as_list())?,
            xyz_to_camera: fixed(fields::XYZ_TO_CAMERA, get(fields::XYZ_TO_CAMERA)?.as_list())?,
            cfa: Cfa::parse(&text(fields::CFA)?)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sample() -> CameraInfo {
        CameraInfo {
            maker: "Nikon".into(),
            model: "Z 8".into(),
            iso: 400.0,
            exposure_bias: -0.7,
            exposure_time: 1.0 / 250.0,
            aperture: 5.6,
            focal_length: 50.0,
            black: [1008.0; 4],
            white: 15520.0,
            white_balance: [2.1, 1.0, 1.4, 1.0],
            xyz_to_camera: [0.9, -0.3, -0.1, -0.4, 1.2, 0.2, -0.05, 0.15, 0.6],
            cfa: Cfa::parse("RGGB").unwrap(),
        }
    }

    #[test]
    fn camera_info_round_trips_through_context() {
        let info = sample();
        assert_eq!(CameraInfo::from_context(&info.to_context()).unwrap(), info);
    }

    #[test]
    fn a_missing_or_malformed_field_is_named() {
        let partial = Context::new().with(fields::MAKER, Value::Choice("Nikon".into()));
        assert_eq!(
            CameraInfo::from_context(&partial).unwrap_err(),
            "context field `photo.model` missing"
        );
        let bad = sample()
            .to_context()
            .with(fields::BLACK, Value::List(vec![1.0]));
        assert_eq!(
            CameraInfo::from_context(&bad).unwrap_err(),
            "context field `photo.black` is not a list of 4 numbers"
        );
    }

    #[test]
    fn every_field_is_listed() {
        let context = sample().to_context();
        for field in fields::ALL {
            assert!(context.get(field).is_some(), "{field}");
        }
    }
}
