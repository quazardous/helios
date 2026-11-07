//! Orientation, as darktable's `flip` module (`src/iop/flip.c`): quarter
//! turns and mirrors, set by default from the raw's EXIF orientation.
//!
//! Its one parameter is darktable's orientation value, so a darktable
//! edit's `flip` carries over as it is: -1 (darktable's `ORIENTATION_NULL`,
//! the default) follows the EXIF tag of the camera's description; 0 to 7
//! are three bits, applied as darktable applies them: flip vertically (1),
//! flip horizontally (2), then swap x and y (4). Rotate 90 degrees
//! clockwise is 5, counter-clockwise 6, 180 degrees 3, a mirror 2. A value
//! of 0 or more is the user's and beats the EXIF tag.
//!
//! The step is geometric ([`Needs::geometry`]): it maps the plane about
//! the origin, without knowing the image's size, so a turned image's
//! extent starts off the origin. Pixel `(x, y)` of the input lands on
//! `(x', y')` with `x' = -1 - x` when flipped horizontally (`y` the same
//! vertically), then the two swapped; a region asked of the output maps
//! back through the inverse. Rotated 90 degrees clockwise, a `W x H`
//! image at the origin covers `(-H, 0)` to `(0, W)`.

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamKind, ParamSpec, Params,
    PortSpec, Region, Role,
};
use helios_photo::camera::fields;

/// darktable's orientation bits (`dt_image_orientation_t`).
pub const FLIP_Y: u8 = 1;
pub const FLIP_X: u8 = 2;
pub const SWAP_XY: u8 = 4;

/// darktable's named orientations.
pub const AUTO: i64 = -1;
pub const NONE: u8 = 0;
pub const MIRROR: u8 = FLIP_X;
pub const ROTATE_180: u8 = FLIP_X | FLIP_Y;
pub const ROTATE_CW: u8 = FLIP_Y | SWAP_XY;
pub const ROTATE_CCW: u8 = FLIP_X | SWAP_XY;

/// The orientation bits darktable gives an EXIF orientation tag
/// (`dt_image_orientation_to_flip_bits`); none for a value out of range.
pub fn from_exif(tag: i64) -> u8 {
    match tag {
        2 => FLIP_X,
        3 => ROTATE_180,
        4 => FLIP_Y,
        5 => SWAP_XY,
        6 => ROTATE_CW,
        7 => FLIP_X | FLIP_Y | SWAP_XY,
        8 => ROTATE_CCW,
        _ => NONE,
    }
}

/// A plane map: darktable's orientation bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Orientation(pub u8);

impl Orientation {
    fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }

    /// Where an input pixel lands.
    pub fn forward(self, x: i64, y: i64) -> (i64, i64) {
        let x = if self.has(FLIP_X) { -1 - x } else { x };
        let y = if self.has(FLIP_Y) { -1 - y } else { y };
        if self.has(SWAP_XY) { (y, x) } else { (x, y) }
    }

    /// The input pixel an output pixel comes from.
    pub fn backward(self, x: i64, y: i64) -> (i64, i64) {
        let (x, y) = if self.has(SWAP_XY) { (y, x) } else { (x, y) };
        let x = if self.has(FLIP_X) { -1 - x } else { x };
        let y = if self.has(FLIP_Y) { -1 - y } else { y };
        (x, y)
    }

    /// The region an input region covers once mapped.
    pub fn forward_region(self, r: Region) -> Region {
        let x = if self.has(FLIP_X) { -r.right() } else { r.x };
        let y = if self.has(FLIP_Y) { -r.bottom() } else { r.y };
        if self.has(SWAP_XY) {
            Region::new(y, x, r.height, r.width)
        } else {
            Region::new(x, y, r.width, r.height)
        }
    }

    /// The input region an output region comes from.
    pub fn backward_region(self, r: Region) -> Region {
        let r = if self.has(SWAP_XY) {
            Region::new(r.y, r.x, r.height, r.width)
        } else {
            r
        };
        let x = if self.has(FLIP_X) { -r.right() } else { r.x };
        let y = if self.has(FLIP_Y) { -r.bottom() } else { r.y };
        Region::new(x, y, r.width, r.height)
    }
}

pub struct Flip;

impl Module for Flip {
    type Plan = Orientation;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "flip".into(),
            version: 1,
            description: "Turn or mirror the image: the camera's orientation, or the user's".into(),
            params: vec![ParamSpec {
                name: "orientation".into(),
                description: "darktable's orientation: -1 from the camera (EXIF), 0 none, \
                              2 mirror, 1 flip vertically, 3 rotate 180 degrees, \
                              5 rotate 90 degrees clockwise, 6 counter-clockwise, \
                              4 transpose, 7 transverse"
                    .into(),
                kind: ParamKind::Int {
                    min: AUTO,
                    max: 7,
                    default: AUTO,
                },
            }],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![fields::ORIENTATION.into()],
            needs: Needs {
                geometry: true,
                ..Needs::default()
            },
            kind: ModuleKind::Process,
            family: Family::Geometry,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<Self::Plan, String> {
        let asked = params
            .get("orientation")
            .and_then(helios_core::Value::as_i64)
            .unwrap_or(AUTO);
        Ok(Orientation(match asked {
            // A picture has no camera description: kept as it is.
            AUTO => from_exif(
                context
                    .get(fields::ORIENTATION)
                    .and_then(helios_core::Value::as_i64)
                    .unwrap_or(1),
            ),
            o => u8::try_from(o)
                .ok()
                .filter(|o| *o <= 7)
                .ok_or_else(|| format!("orientation {o} is not darktable's (-1 to 7)"))?,
        }))
    }

    fn output_extent(&self, plan: &Self::Plan, input: Region) -> Region {
        plan.forward_region(input)
    }

    fn input_region(&self, plan: &Self::Plan, output: Region, _overlap: u32) -> Region {
        plan.backward_region(output)
    }

    fn process(
        &self,
        plan: &Self::Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        if plan.0 == NONE {
            return Ok(vec![input.sample(output)]);
        }
        let channels = input.channels();
        let image = Image::from_fn(output, channels, |x, y, c| {
            let (sx, sy) = plan.backward(x, y);
            input.pixel(sx, sy)[c as usize]
        });
        Ok(vec![image])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

    #[test]
    fn the_maps_invert_each_other() {
        let r = Region::new(-3, 5, 7, 2);
        for o in ALL.map(Orientation) {
            assert_eq!(o.backward_region(o.forward_region(r)), r, "{o:?}");
            for (x, y) in [(0, 0), (6, 1), (-3, 5)] {
                assert_eq!(o.backward(o.forward(x, y).0, o.forward(x, y).1), (x, y));
                let (fx, fy) = o.forward(x, y);
                assert!(o.forward_region(Region::new(x, y, 1, 1)) == Region::new(fx, fy, 1, 1));
            }
        }
    }

    #[test]
    fn exif_tags_give_darktables_bits() {
        assert_eq!(from_exif(1), NONE);
        assert_eq!(from_exif(6), ROTATE_CW);
        assert_eq!(from_exif(8), ROTATE_CCW);
        assert_eq!(from_exif(3), ROTATE_180);
        assert_eq!(from_exif(0), NONE);
    }
}
