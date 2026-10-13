//! darktable's blend params: how a module's output is mixed with its input
//! (its opacity, blend mode and colour space) and through which mask (the
//! mask mode, the drawn mask's group, the refinement). Read from the
//! layout of `dt_develop_blend_params_t` (`src/develop/blend.h`), version
//! 14, which the flexi masks' version 15 keeps.

use serde::Serialize;

/// The blend params versions read: darktable's current (14) and the flexi
/// masks' (15, the same layout).
pub const VERSIONS: std::ops::RangeInclusive<u32> = 14..=15;

/// The size of the struct, in bytes.
pub const SIZE: usize = 420;

/// `mask_mode` bits (`dt_develop_mask_mode_t`).
pub mod mode {
    pub const ENABLED: u32 = 1;
    pub const MASK: u32 = 1 << 1;
    pub const CONDITIONAL: u32 = 1 << 2;
    pub const RASTER: u32 = 1 << 3;
    /// The flexi masks' tree (pull request darktable-org/darktable#22553).
    pub const FLEXI: u32 = 1 << 4;
}

/// `mask_combine` bits (`dt_develop_mask_combine_mode_t`).
pub mod combine {
    pub const INV: u32 = 0x01;
    pub const INCL: u32 = 0x02;
    pub const MASKS_POS: u32 = 0x04;
}

/// `blend_cst` (`dt_develop_blend_colorspace_t`): scene-referred RGB.
pub const RGB_SCENE: i32 = 4;

/// `blend_mode`: darktable's normal blend (`DEVELOP_BLEND_NORMAL2`), and
/// the flag that swaps input and output.
pub const NORMAL: u32 = 0x18;
pub const REVERSE: u32 = 0x8000_0000;

/// `feathering_guide`: feathering before the blur (else after), guided by
/// the module's output (else its input).
pub mod guide {
    pub const IN_BEFORE_BLUR: u32 = 0x01;
    pub const OUT_BEFORE_BLUR: u32 = 0x02;
    pub const OUT_AFTER_BLUR: u32 = 0x06;
}

/// A module's blend params, as darktable wrote them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Blend {
    pub mask_mode: u32,
    pub blend_cst: i32,
    pub blend_mode: u32,
    pub blend_parameter: f32,
    /// Percent.
    pub opacity: f32,
    pub mask_combine: u32,
    /// The drawn mask's group (a form id), when the mask mode draws.
    pub mask_id: i32,
    pub blendif: u32,
    pub feathering_radius: f32,
    pub feathering_guide: u32,
    pub blur_radius: f32,
    pub contrast: f32,
    pub brightness: f32,
    pub details: f32,
    pub feather_version: u32,
    pub raster_mask_source: String,
    pub raster_mask_instance: i32,
    pub raster_mask_id: i32,
    pub raster_mask_invert: bool,
}

impl Blend {
    /// Reads blend params of `version` (see [`VERSIONS`]).
    pub fn read(version: u32, bytes: &[u8]) -> Result<Blend, String> {
        if !VERSIONS.contains(&version) {
            return Err(format!("blend params v{version}"));
        }
        if bytes.len() != SIZE {
            return Err(format!("blend params of {} bytes, not {SIZE}", bytes.len()));
        }
        let word = |at: usize| -> [u8; 4] { bytes[at..at + 4].try_into().expect("four bytes") };
        let u = |at| u32::from_le_bytes(word(at));
        let i = |at| i32::from_le_bytes(word(at));
        let f = |at| f32::from_le_bytes(word(at));
        let source = &bytes[388..408];
        let end = source.iter().position(|b| *b == 0).unwrap_or(source.len());
        Ok(Blend {
            mask_mode: u(0),
            blend_cst: i(4),
            blend_mode: u(8),
            blend_parameter: f(12),
            opacity: f(16),
            mask_combine: u(20),
            mask_id: i(24),
            blendif: u(28),
            feathering_radius: f(32),
            feathering_guide: u(36),
            blur_radius: f(40),
            contrast: f(44),
            brightness: f(48),
            details: f(52),
            feather_version: u(56),
            raster_mask_source: String::from_utf8_lossy(&source[..end]).into_owned(),
            raster_mask_instance: i(408),
            raster_mask_id: i(412),
            raster_mask_invert: i(416) != 0,
        })
    }

    /// The opacity as a fraction, clamped as darktable clamps it.
    pub fn opacity(&self) -> f64 {
        (f64::from(self.opacity) / 100.0).clamp(0.0, 1.0)
    }

    /// Whether the drawn mask is inverted before its opacity: darktable
    /// inverts on either bit (`MASKS_POS` for a drawn mask alone, `INV`
    /// otherwise).
    pub fn inverted(&self) -> bool {
        self.mask_combine & (combine::INV | combine::MASKS_POS) != 0
    }

    /// Writes the params back, as darktable lays them out (for tests and
    /// fixtures).
    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SIZE);
        out.extend(self.mask_mode.to_le_bytes());
        out.extend(self.blend_cst.to_le_bytes());
        out.extend(self.blend_mode.to_le_bytes());
        out.extend(self.blend_parameter.to_le_bytes());
        out.extend(self.opacity.to_le_bytes());
        out.extend(self.mask_combine.to_le_bytes());
        out.extend(self.mask_id.to_le_bytes());
        out.extend(self.blendif.to_le_bytes());
        out.extend(self.feathering_radius.to_le_bytes());
        out.extend(self.feathering_guide.to_le_bytes());
        out.extend(self.blur_radius.to_le_bytes());
        out.extend(self.contrast.to_le_bytes());
        out.extend(self.brightness.to_le_bytes());
        out.extend(self.details.to_le_bytes());
        out.extend(self.feather_version.to_le_bytes());
        out.extend([0u8; 8]);
        // blendif parameters: each channel's span open (0, 0, 1, 1), and
        // the boost factors at 0, as darktable's defaults.
        for _ in 0..16 {
            for v in [0.0f32, 0.0, 1.0, 1.0] {
                out.extend(v.to_le_bytes());
            }
        }
        out.extend([0u8; 64]);
        let mut source = [0u8; 20];
        let name = self.raster_mask_source.as_bytes();
        source[..name.len().min(19)].copy_from_slice(&name[..name.len().min(19)]);
        out.extend(source);
        out.extend(self.raster_mask_instance.to_le_bytes());
        out.extend(self.raster_mask_id.to_le_bytes());
        out.extend(i32::from(self.raster_mask_invert).to_le_bytes());
        out
    }

    /// darktable's defaults for a module blending in scene-referred RGB,
    /// with a drawn mask of group `mask_id`.
    pub fn drawn(mask_id: i32) -> Blend {
        Blend {
            mask_mode: mode::ENABLED | mode::MASK,
            blend_cst: RGB_SCENE,
            blend_mode: NORMAL,
            blend_parameter: 0.0,
            opacity: 100.0,
            mask_combine: combine::INCL,
            mask_id,
            blendif: 0,
            feathering_radius: 0.0,
            feathering_guide: 0x05,
            blur_radius: 0.0,
            contrast: 0.0,
            brightness: 0.0,
            details: 0.0,
            feather_version: 1,
            raster_mask_source: String::new(),
            raster_mask_instance: 0,
            raster_mask_id: 0,
            raster_mask_invert: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_params_round_trip_at_their_offsets() {
        let mut b = Blend::drawn(1_234_567);
        b.opacity = 75.0;
        b.mask_combine = combine::MASKS_POS | combine::INCL;
        b.blur_radius = 3.5;
        b.raster_mask_source = "exposure".into();
        let bytes = b.write();
        assert_eq!(bytes.len(), SIZE);
        assert_eq!(&bytes[24..28], &1_234_567i32.to_le_bytes());
        assert_eq!(Blend::read(14, &bytes).unwrap(), b);
        assert!((b.opacity() - 0.75).abs() < 1e-9);
        assert!(b.inverted());
        assert!(Blend::read(13, &bytes).is_err());
        assert!(Blend::read(14, &bytes[..400]).is_err());
    }
}
