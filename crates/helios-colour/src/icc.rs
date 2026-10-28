//! ICC profiles for the output spaces.
//!
//! Version 4 profiles come from moxcms, with Helios's own colorants (the
//! Bradford-adapted columns of the matrix the pixels are encoded with) and
//! the creation date fixed, so that the same export gives the same bytes.
//! moxcms writes version 4 only: the version 2 profiles that JPEG files
//! carry for older readers are written here, a plain matrix/TRC display
//! profile.

use moxcms::{ColorProfile, LocalizableString, Matrix3d, ProfileText, Xyzd};

use crate::{ADOBE_RGB_GAMMA, ICC_D50, Mat3, OutputSpace, d65_to_d50};

/// The creation date every Helios profile carries (2026-01-01 00:00:00),
/// in the header's dateTimeNumber layout: six big-endian u16.
const CREATION_DATE: [u16; 6] = [2026, 1, 1, 0, 0, 0];

/// Where the header keeps the creation date.
const DATE_AT: usize = 24;

const COPYRIGHT: &str = "No copyright, use freely";

/// Entries of a v2 tone curve table: enough for 16-bit output to
/// interpolate within a code value.
const TABLE_SIZE: usize = 1024;

fn xyzd([x, y, z]: [f64; 3]) -> Xyzd {
    Xyzd { x, y, z }
}

fn column(m: &Mat3, c: usize) -> [f64; 3] {
    [m[0][c], m[1][c], m[2][c]]
}

fn description(space: OutputSpace) -> String {
    format!("Helios {}", space.name())
}

pub(crate) fn v4(space: OutputSpace) -> Vec<u8> {
    let mut profile = match space {
        OutputSpace::Srgb => ColorProfile::new_srgb(),
        OutputSpace::DisplayP3 => ColorProfile::new_display_p3(),
        OutputSpace::AdobeRgb => ColorProfile::new_adobe_rgb(),
        OutputSpace::Rec2020 => ColorProfile::new_bt2020(),
    };
    // moxcms's built-ins bring the right curves (sRGB and BT.2020 as
    // parametric curves, Adobe RGB as gamma 563/256); the colorants and the
    // white are Helios's, so the profile says exactly what the pixels are.
    let colorants = space.to_xyz_d50();
    profile.red_colorant = xyzd(column(&colorants, 0));
    profile.green_colorant = xyzd(column(&colorants, 1));
    profile.blue_colorant = xyzd(column(&colorants, 2));
    profile.white_point = xyzd(ICC_D50);
    // ICC.1 v4: a display profile's media white is the PCS white, and the
    // adaptation from the real white goes into the chad tag.
    profile.media_white_point = Some(xyzd(ICC_D50));
    profile.chromatic_adaptation = Some(Matrix3d { v: d65_to_d50() });
    // The cICP tag in a profile is recent (ICC v4.4): PNG carries the code
    // points in its own chunk, the profile stays a plain v4.0 matrix/TRC.
    profile.cicp = None;
    let text = |s: String| {
        Some(ProfileText::Localizable(vec![LocalizableString::new(
            "en".to_string(),
            "US".to_string(),
            s,
        )]))
    };
    profile.description = text(description(space));
    profile.copyright = text(COPYRIGHT.to_string());
    let mut bytes = profile
        .encode()
        .expect("a matrix/TRC profile always encodes");
    for (i, v) in CREATION_DATE.iter().enumerate() {
        bytes[DATE_AT + 2 * i..DATE_AT + 2 * i + 2].copy_from_slice(&v.to_be_bytes());
    }
    bytes
}

fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag([x, y, z]: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    for v in [x, y, z] {
        t.extend(s15f16(v));
    }
    t
}

/// `textDescriptionType` (v2 only): an ASCII string, no Unicode or
/// `ScriptCode` variant.
fn desc_tag(text: &str) -> Vec<u8> {
    let mut t = b"desc\0\0\0\0".to_vec();
    t.extend((text.len() as u32 + 1).to_be_bytes());
    t.extend(text.as_bytes());
    t.push(0);
    // Unicode language code and count, ScriptCode code and count, then the
    // 67 bytes of the ScriptCode string.
    t.extend([0; 4 + 4 + 2 + 1 + 67]);
    t
}

fn text_tag(text: &str) -> Vec<u8> {
    let mut t = b"text\0\0\0\0".to_vec();
    t.extend(text.as_bytes());
    t.push(0);
    t
}

/// curveType: a single gamma when the curve is one, a table otherwise.
fn curve_tag(space: OutputSpace) -> Vec<u8> {
    let mut t = b"curv\0\0\0\0".to_vec();
    if space == OutputSpace::AdobeRgb {
        t.extend(1u32.to_be_bytes());
        // u8Fixed8Number: 563/256 is exact.
        t.extend(((ADOBE_RGB_GAMMA * 256.0).round() as u16).to_be_bytes());
    } else {
        t.extend((TABLE_SIZE as u32).to_be_bytes());
        for i in 0..TABLE_SIZE {
            let linear = space.decode(i as f64 / (TABLE_SIZE - 1) as f64);
            t.extend(((linear.clamp(0.0, 1.0) * 65535.0).round() as u16).to_be_bytes());
        }
    }
    t
}

pub(crate) fn v2(space: OutputSpace) -> Vec<u8> {
    let colorants = space.to_xyz_d50();
    let trc = curve_tag(space);
    // Each tag's data; the three TRC entries share one curve.
    let data: [(&[u8; 4], Vec<u8>); 7] = [
        (b"desc", desc_tag(&description(space))),
        (b"cprt", text_tag(COPYRIGHT)),
        (b"wtpt", xyz_tag(ICC_D50)),
        (b"rXYZ", xyz_tag(column(&colorants, 0))),
        (b"gXYZ", xyz_tag(column(&colorants, 1))),
        (b"bXYZ", xyz_tag(column(&colorants, 2))),
        (b"rTRC", trc),
    ];
    let tag_count = data.len() + 2;
    let mut offset = 128 + 4 + 12 * tag_count;
    let mut table = Vec::new();
    let mut body = Vec::new();
    let mut trc_entry = (0, 0);
    for (signature, bytes) in &data {
        table.extend(*signature);
        table.extend((offset as u32).to_be_bytes());
        table.extend((bytes.len() as u32).to_be_bytes());
        if *signature == b"rTRC" {
            trc_entry = (offset, bytes.len());
        }
        body.extend(bytes);
        // Tag data starts on a 4-byte boundary.
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 128 + 4 + 12 * tag_count + body.len();
    }
    for signature in [b"gTRC", b"bTRC"] {
        table.extend(signature);
        table.extend((trc_entry.0 as u32).to_be_bytes());
        table.extend((trc_entry.1 as u32).to_be_bytes());
    }
    let size = 128 + 4 + table.len() + body.len();

    let mut out = Vec::with_capacity(size);
    out.extend((size as u32).to_be_bytes());
    out.extend([0; 4]); // preferred CMM: none
    out.extend(0x0210_0000u32.to_be_bytes()); // version 2.1
    out.extend(b"mntr");
    out.extend(b"RGB ");
    out.extend(b"XYZ ");
    for v in CREATION_DATE {
        out.extend(v.to_be_bytes());
    }
    out.extend(b"acsp");
    out.extend([0; 4]); // platform
    out.extend([0; 4]); // flags
    out.extend([0; 4]); // manufacturer
    out.extend([0; 4]); // model
    out.extend([0; 8]); // attributes
    out.extend([0; 4]); // rendering intent: perceptual
    for v in ICC_D50 {
        out.extend(s15f16(v));
    }
    out.extend([0; 4]); // creator
    out.extend([0; 16]); // profile ID: not computed (optional in v2)
    out.extend([0; 28]); // reserved
    debug_assert_eq!(out.len(), 128);
    out.extend((tag_count as u32).to_be_bytes());
    out.extend(table);
    out.extend(body);
    debug_assert_eq!(out.len(), size);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_reproducible_and_parse() {
        for space in OutputSpace::ALL {
            for (bytes, version) in [(v4(space), 4), (v2(space), 2)] {
                assert_eq!(bytes[8], version, "{space:?}");
                assert_eq!(&bytes[36..40], b"acsp");
                assert_eq!(
                    u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize,
                    bytes.len()
                );
                assert_eq!(&bytes[24..26], &2026u16.to_be_bytes());
                let parsed = ColorProfile::new_from_slice(&bytes)
                    .unwrap_or_else(|e| panic!("{space:?} v{version}: {e:?}"));
                let red = parsed.red_colorant;
                let want = column(&space.to_xyz_d50(), 0);
                assert!(
                    (red.x - want[0]).abs() < 1e-4
                        && (red.y - want[1]).abs() < 1e-4
                        && (red.z - want[2]).abs() < 1e-4,
                    "{space:?} v{version}: {red:?} for {want:?}"
                );
            }
            assert_eq!(v4(space), v4(space));
            assert_eq!(v2(space), v2(space));
        }
    }
}
