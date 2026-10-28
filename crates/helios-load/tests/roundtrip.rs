//! Round trips through the colour tags, export and load together:
//!
//! - a colour chart (named patches: greys from black to white with
//!   near-black and near-white steps, everyday colours, colours outside
//!   the smaller spaces, a highlight above white and a negative channel)
//!   exported in every output space and format, loaded back and compared
//!   with what the space can hold, in CIEDE2000;
//! - export, load, export again: a second or third generation stays where
//!   the first one was;
//! - the gamut mapping's promises seen from the loaded file: colours the
//!   space holds come back unchanged, the others inside it with their
//!   Oklab hue and lightness, a highlight above white as white;
//! - pictures tagged the way other software tags them, made here: a v2
//!   sRGB profile in the style of the common IEC 61966-2.1 one (table
//!   curves shared by the three channels, a D65 media white), a legacy PNG
//!   with only gAMA and cHRM, a 16-bit PNG with only cICP and the 10-bit
//!   BT.2020 transfer code.
//!
//! Tolerances (CIEDE2000, `ΔE00`, on CIELAB with the D65 white):
//!
//! - 16-bit PNG and TIFF: 0.05. The 16-bit rounding is under 0.015; ICC
//!   profiles store colorants and curve parameters as s15Fixed16, which
//!   moves linear values by up to 2.5e-5 from the exact primaries.
//! - 8-bit PNG: 2.0. Under 0.4 in sRGB, Display P3 and Adobe RGB; the
//!   BT.2020 curve at 8 bits is coarse near black, where rounding alone
//!   gives up to 1.7 on the near-black colours.
//! - JPEG (quality 100): 4.0 from L* 20 up, and linear channels within
//!   6e-3 below, where CIELAB magnifies a code value of difference.

use std::io::Cursor;
use std::path::PathBuf;

use helios_colour::{D65, Mat3, OutputSpace, apply, inverse, srgb_decode, xy_to_xyz};
use helios_core::{Image, Region};
use helios_export::{Format, encode_in};
use helios_load::{ColourTag, Loaded, load};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::tiff::TiffEncoder;
use image::{DynamicImage, ImageBuffer, Rgb};

/// Side of a patch: JPEG keeps flat colour inside its 8 x 8 blocks and
/// 16 x 16 chroma blocks.
const PATCH: u32 = 16;

/// A named colour of the chart, in linear Rec.2020 (the working space).
struct Patch {
    name: &'static str,
    rgb: [f64; 3],
}

/// Linear sRGB to linear Rec.2020: colours every output space holds.
fn from_srgb(rgb: [f64; 3]) -> [f64; 3] {
    let to_working = inverse(&OutputSpace::Srgb.from_rec2020()).unwrap();
    apply(&to_working, rgb)
}

/// An 8-bit sRGB colour, as charts publish them.
fn srgb8(rgb: [u8; 3]) -> [f64; 3] {
    from_srgb(rgb.map(|v| srgb_decode(f64::from(v) / 255.0)))
}

fn grey(v: f64) -> [f64; 3] {
    [v; 3]
}

/// The chart. The colours of the `ColorChecker` (their published sRGB
/// values), sRGB's own primaries on its gamut's edge, near-black colours
/// and the grey steps sit inside every output space; the Rec.2020
/// primaries, a bright orange and a deep blue sit outside sRGB, Display P3
/// and Adobe RGB; a highlight above white and a negative blue sit outside
/// every space, Rec.2020 included.
fn patches() -> Vec<Patch> {
    let p = |name, rgb| Patch { name, rgb };
    vec![
        p("black", grey(0.0)),
        p("near black 0.1 %", grey(0.001)),
        p("near black 0.5 %", grey(0.005)),
        p("shadow grey", grey(0.03)),
        p("middle grey", grey(0.18)),
        p("near white 90 %", grey(0.9)),
        p("near white 99 %", grey(0.99)),
        p("white", grey(1.0)),
        p("near-black red", from_srgb([0.004, 0.002, 0.001])),
        p("near-black blue", from_srgb([0.002, 0.003, 0.006])),
        p("dark skin", srgb8([115, 82, 68])),
        p("light skin", srgb8([194, 150, 130])),
        p("blue sky", srgb8([98, 122, 157])),
        p("foliage", srgb8([87, 108, 67])),
        p("orange", srgb8([214, 126, 44])),
        p("purplish blue", srgb8([80, 91, 166])),
        p("moderate red", srgb8([193, 90, 99])),
        p("cyan", srgb8([8, 133, 161])),
        p("sRGB red", from_srgb([1.0, 0.0, 0.0])),
        p("sRGB blue", from_srgb([0.0, 0.0, 1.0])),
        p("Rec.2020 red", [1.0, 0.0, 0.0]),
        p("Rec.2020 green", [0.0, 1.0, 0.0]),
        p("Rec.2020 blue", [0.0, 0.0, 1.0]),
        p("Rec.2020 yellow", [1.0, 1.0, 0.0]),
        p("bright orange", [0.9, 0.3, 0.0]),
        p("deep blue", [0.0, 0.0, 0.05]),
        p("above white", [1.6, 1.2, 0.9]),
        p("negative blue", [0.2, 0.3, -0.1]),
    ]
}

fn chart(patches: &[Patch]) -> Image {
    Image::from_fn(
        Region::sized(patches.len() as u32 * PATCH, PATCH),
        3,
        |x, _, c| patches[(x / i64::from(PATCH)) as usize].rgb[c as usize] as f32,
    )
}

/// The colour at the middle of patch `i`, as f64.
fn patch(image: &Image, i: usize) -> [f64; 3] {
    let (i, side) = (i as i64, i64::from(PATCH));
    let p = image.pixel(i * side + side / 2, side / 2);
    [p[0], p[1], p[2]].map(f64::from)
}

/// Writes `bytes` to a fresh file named `name` (unique per test case) and
/// loads it.
fn load_bytes(bytes: &[u8], name: &str) -> Loaded {
    let dir = scratch(name);
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    let loaded = load(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let _ = std::fs::remove_dir_all(&dir);
    loaded
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-load-roundtrip-{}-{name}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A file name for a format and a space, so that cases never share one.
fn file_name(what: &str, format: &Format, space: OutputSpace) -> String {
    let ext = match format {
        Format::Jpeg { .. } => "jpg".to_string(),
        Format::Png { bits } => format!("{bits}.png"),
        Format::Tiff => "tif".to_string(),
    };
    format!("{what}-{space:?}.{ext}")
}

fn formats() -> [Format; 4] {
    [
        Format::Jpeg { quality: 100 },
        Format::Png { bits: 8 },
        Format::Png { bits: 16 },
        Format::Tiff,
    ]
}

/// Whether `space` holds a working-space colour as it is.
fn holds(space: OutputSpace, rgb: [f64; 3]) -> bool {
    apply(&space.from_rec2020(), rgb)
        .iter()
        .all(|v| (-1e-9..=1.0 + 1e-9).contains(v))
}

/// What a working-space colour becomes in `space`, back in the working
/// space: the one gamut mapping, applied to what the image holds (f32).
fn through(space: OutputSpace, rgb: [f64; 3]) -> [f64; 3] {
    let held = rgb.map(|v| f64::from(v as f32));
    let mapped = space.gamut_map().map(held);
    apply(&inverse(&space.from_rec2020()).unwrap(), mapped)
}

// --- Colour difference: CIELAB and CIEDE2000 ---------------------------

/// CIELAB (D65 white) of a linear Rec.2020 colour.
fn lab(rgb: [f64; 3]) -> [f64; 3] {
    let xyz = apply(&OutputSpace::Rec2020.to_xyz(), rgb);
    let white = xy_to_xyz(D65);
    let delta: f64 = 6.0 / 29.0;
    let f = |t: f64| {
        if t > delta.powi(3) {
            t.cbrt()
        } else {
            t / (3.0 * delta * delta) + 4.0 / 29.0
        }
    };
    let [fx, fy, fz] = [0, 1, 2].map(|i| f(xyz[i] / white[i]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 between two CIELAB colours (Sharma, Wu and Dalal, 2005).
fn delta_e00(one: [f64; 3], two: [f64; 3]) -> f64 {
    let [l1, a1, b1] = one;
    let [l2, a2, b2] = two;
    let pow7 = |v: f64| v.powi(7);
    let c_mean = f64::midpoint(a1.hypot(b1), a2.hypot(b2));
    let g = 0.5 * (1.0 - (pow7(c_mean) / (pow7(c_mean) + pow7(25.0))).sqrt());
    let (a1, a2) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1, c2) = (a1.hypot(b1), a2.hypot(b2));
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        }
    };
    let (h1, h2) = (hue(b1, a1), hue(b2, a2));
    let d_l = l2 - l1;
    let d_c = c2 - c1;
    let d_h = if c1 * c2 == 0.0 {
        0.0
    } else {
        let d = h2 - h1;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let d_hh = 2.0 * (c1 * c2).sqrt() * (d_h.to_radians() / 2.0).sin();
    let l_mean = f64::midpoint(l1, l2);
    let c_mean = f64::midpoint(c1, c2);
    let h_mean = if c1 * c2 == 0.0 {
        h1 + h2
    } else if (h1 - h2).abs() <= 180.0 {
        f64::midpoint(h1, h2)
    } else if h1 + h2 < 360.0 {
        (h1 + h2 + 360.0) / 2.0
    } else {
        (h1 + h2 - 360.0) / 2.0
    };
    let cos = |deg: f64| deg.to_radians().cos();
    let t =
        1.0 - 0.17 * cos(h_mean - 30.0) + 0.24 * cos(2.0 * h_mean) + 0.32 * cos(3.0 * h_mean + 6.0)
            - 0.20 * cos(4.0 * h_mean - 63.0);
    let d_theta = 30.0 * (-((h_mean - 275.0) / 25.0).powi(2)).exp();
    let r_c = 2.0 * (pow7(c_mean) / (pow7(c_mean) + pow7(25.0))).sqrt();
    let s_l = 1.0 + 0.015 * (l_mean - 50.0).powi(2) / (20.0 + (l_mean - 50.0).powi(2)).sqrt();
    let s_c = 1.0 + 0.045 * c_mean;
    let s_h = 1.0 + 0.015 * c_mean * t;
    let r_t = -(2.0 * d_theta).to_radians().sin() * r_c;
    let (l, c, h) = (d_l / s_l, d_c / s_c, d_hh / s_h);
    (l * l + c * c + h * h + r_t * c * h).sqrt()
}

/// ΔE00 between two linear Rec.2020 colours.
fn delta_e(one: [f64; 3], two: [f64; 3]) -> f64 {
    delta_e00(lab(one), lab(two))
}

#[test]
fn the_colour_difference_matches_the_published_pairs() {
    // Pairs 1, 7, 17 and 25 of Sharma, Wu and Dalal's test data.
    let pairs = [
        ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
        ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
        ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
        (
            [60.2574, -34.0099, 36.2677],
            [60.4626, -34.1751, 39.4387],
            1.2644,
        ),
    ];
    for (one, two, want) in pairs {
        let got = delta_e00(one, two);
        assert!((got - want).abs() < 1e-4, "{one:?} {two:?}: {got}");
        assert!((delta_e00(two, one) - want).abs() < 1e-4);
    }
    assert_eq!(delta_e([0.3, 0.2, 0.1], [0.3, 0.2, 0.1]), 0.0);
}

/// Checks a loaded colour against the one wanted, with `format`'s
/// tolerance (see the top of the file).
fn assert_close(format: &Format, got: [f64; 3], want: [f64; 3], what: &str) {
    let de = delta_e(got, want);
    let ok = match format {
        Format::Png { bits: 16 } | Format::Tiff => de <= 0.05,
        Format::Png { .. } => de <= 2.0,
        Format::Jpeg { .. } => {
            if lab(want)[0] >= 20.0 {
                de <= 4.0
            } else {
                got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 6e-3)
            }
        }
    };
    assert!(ok, "{what}: {got:?} for {want:?}, ΔE00 {de:.4}");
}

#[test]
fn the_chart_comes_back_through_every_space_and_format() {
    let patches = patches();
    let image = chart(&patches);
    for space in OutputSpace::ALL {
        let mut outside = 0;
        for format in formats() {
            let name = file_name("chart", &format, space);
            let bytes = encode_in(&image, &format, space).unwrap();
            let loaded = load_bytes(&bytes, &name);
            let colour = loaded.colour.expect("a picture reports its colour");
            assert!(colour.ignored.is_empty(), "{name}: {colour:?}");
            assert!(!colour.assumed(), "{name}: {colour:?}");
            for (i, p) in patches.iter().enumerate() {
                if !holds(space, p.rgb) {
                    outside += 1;
                }
                let want = through(space, p.rgb);
                let what = format!("{name}, {}", p.name);
                assert_close(&format, patch(&loaded.image, i), want, &what);
            }
        }
        // The chart reaches outside every space: the highlight and the
        // negative blue, and for the smaller spaces the Rec.2020 colours
        // too.
        let least = if space == OutputSpace::Rec2020 { 2 } else { 6 };
        assert!(outside >= 4 * least, "{space:?}: {outside} outside");
    }
}

/// The file's values over [0, 1], every channel of every pixel.
fn file_values(bytes: &[u8]) -> Vec<f64> {
    let decoded = image::load_from_memory(bytes).unwrap().to_rgb16();
    decoded
        .into_raw()
        .into_iter()
        .map(|v| f64::from(v) / 65535.0)
        .collect()
}

#[test]
fn a_reloaded_export_exports_the_same() {
    // Export, load, export again, twice: the colour tag found stays the
    // same, and each generation stays where the first one was. Losslessly
    // stored, no channel moves by more than 1e-4 in linear light: 8-bit
    // files keep their code values, but where s15Fixed16 profile colorants
    // turn an exact 0 into 1e-5, which Adobe RGB's pure gamma writes as a
    // code value of 1 or 2; 16-bit files move by a code value or so. JPEG
    // is compared on its code values, each generation within 4 of the
    // last at the patches' centres.
    let patches = patches();
    let image = chart(&patches);
    for space in OutputSpace::ALL {
        for format in formats() {
            let mut files = vec![encode_in(&image, &format, space).unwrap()];
            let mut loads = Vec::new();
            for generation in 1..=3 {
                let name = file_name(&format!("generation{generation}"), &format, space);
                let loaded = load_bytes(files.last().unwrap(), &name);
                files.push(encode_in(&loaded.image, &format, space).unwrap());
                loads.push(loaded);
            }
            let what = file_name("generations", &format, space);
            let lossless = !matches!(format, Format::Jpeg { .. });
            for pair in loads.windows(2) {
                let (before, after) = (&pair[0], &pair[1]);
                assert_eq!(before.colour, after.colour, "{what}");
                for (i, p) in patches.iter().enumerate().filter(|_| lossless) {
                    let (b, a) = (patch(&before.image, i), patch(&after.image, i));
                    assert!(
                        b.iter().zip(a).all(|(b, a)| (b - a).abs() <= 1e-4),
                        "{what}, {}: {a:?} after {b:?}",
                        p.name
                    );
                }
            }
            if !lossless {
                let width = patches.len() as u32 * PATCH;
                let centre = |values: &[f64], i: u32| -> [f64; 3] {
                    let at = (((PATCH / 2) * width + i * PATCH + PATCH / 2) * 3) as usize;
                    [values[at], values[at + 1], values[at + 2]]
                };
                let values: Vec<Vec<f64>> = files.iter().map(|f| file_values(f)).collect();
                for pair in values.windows(2) {
                    for (i, p) in patches.iter().enumerate() {
                        let (b, a) = (centre(&pair[0], i as u32), centre(&pair[1], i as u32));
                        assert!(
                            b.iter().zip(a).all(|(b, a)| (b - a).abs() <= 4.0 / 255.0),
                            "{what}, {}: {a:?} after {b:?}",
                            p.name
                        );
                    }
                }
            }
        }
    }
}

// --- Oklab, for the gamut mapping's promises ----------------------------

/// Oklab's matrices (Björn Ottosson, 2020): CIE XYZ (D65) to cone
/// responses, then cube-rooted responses to L, a, b.
const XYZ_TO_LMS: Mat3 = [
    [0.818_933_010_1, 0.361_866_742_4, -0.128_859_713_7],
    [0.032_984_543_6, 0.929_311_871_5, 0.036_145_638_7],
    [0.048_200_301_8, 0.264_366_269_1, 0.633_851_707_0],
];
const LMS_TO_LAB: Mat3 = [
    [0.210_454_255_3, 0.793_617_785_0, -0.004_072_046_8],
    [1.977_998_495_1, -2.428_592_205_0, 0.450_593_709_9],
    [0.025_904_037_1, 0.782_771_766_2, -0.808_675_766_0],
];

/// Oklab of a linear Rec.2020 colour.
fn oklab(rgb: [f64; 3]) -> [f64; 3] {
    let lms = apply(&XYZ_TO_LMS, apply(&OutputSpace::Rec2020.to_xyz(), rgb));
    apply(&LMS_TO_LAB, lms.map(f64::cbrt))
}

/// The angle between the hues of two Oklab colours, in radians.
fn hue_gap(one: [f64; 3], two: [f64; 3]) -> f64 {
    let d = (one[2].atan2(one[1]) - two[2].atan2(two[1])).rem_euclid(std::f64::consts::TAU);
    d.min(std::f64::consts::TAU - d)
}

#[test]
fn the_gamut_mapping_holds_from_export_to_load() {
    // Read back from 16-bit files: colours the space holds are unchanged;
    // the others are inside it, with their Oklab lightness (within 2e-3)
    // and hue (within 0.02 rad, where some chroma is left to carry one);
    // a highlight above white comes back as white.
    let patches = patches();
    let image = chart(&patches);
    let white = grey(1.0);
    for space in OutputSpace::ALL {
        let to_space = space.from_rec2020();
        for format in [Format::Png { bits: 16 }, Format::Tiff] {
            let name = file_name("gamut", &format, space);
            let loaded = load_bytes(&encode_in(&image, &format, space).unwrap(), &name);
            let mut hues = 0;
            for (i, p) in patches.iter().enumerate() {
                let got = patch(&loaded.image, i);
                let what = format!("{name}, {}", p.name);
                if holds(space, p.rgb) {
                    let de = delta_e(got, p.rgb);
                    assert!(de <= 0.05, "{what}: {got:?}, ΔE00 {de:.4}");
                    continue;
                }
                let in_space = apply(&to_space, got);
                assert!(
                    in_space.iter().all(|v| (-1e-4..=1.0 + 1e-4).contains(v)),
                    "{what}: {in_space:?} in the space"
                );
                let (before, after) = (oklab(p.rgb), oklab(got));
                if before[0] >= 1.0 {
                    let de = delta_e(got, white);
                    assert!(de <= 0.05, "{what}: {got:?} for white, ΔE00 {de:.4}");
                    continue;
                }
                assert!(
                    (after[0] - before[0]).abs() <= 2e-3,
                    "{what}: lightness {} for {}",
                    after[0],
                    before[0]
                );
                if after[1].hypot(after[2]) > 0.02 {
                    let gap = hue_gap(after, before);
                    assert!(gap <= 0.02, "{what}: hue off by {gap} rad");
                    hues += 1;
                }
            }
            // The Rec.2020 primaries and secondary, the orange and the
            // deep blue, at the least, outside the smaller spaces.
            let least = if space == OutputSpace::Rec2020 { 1 } else { 5 };
            assert!(hues >= least, "{name}: only {hues} hues checked");
        }
    }
}

// --- Pictures tagged by other software ----------------------------------

/// The chart's colours sRGB holds, as 8-bit values in `space`, flat.
fn rgb8_chart(space: OutputSpace, patches: &[Patch]) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    let m = space.from_rec2020();
    ImageBuffer::from_fn(patches.len() as u32 * PATCH, PATCH, |x, _| {
        let linear = apply(&m, patches[(x / PATCH) as usize].rgb);
        Rgb(linear.map(|v| (space.encode(v.clamp(0.0, 1.0)) * 255.0).round() as u8))
    })
}

fn in_srgb(patches: Vec<Patch>) -> Vec<Patch> {
    patches
        .into_iter()
        .filter(|p| holds(OutputSpace::Srgb, p.rgb))
        .collect()
}

/// A PNG written through `png`, with the chunks `info` holds and an
/// optional cICP chunk; `samples` are RGB, big-endian when 16-bit.
fn png_with(
    (width, height): (u32, u32),
    samples: &[u8],
    depth: png::BitDepth,
    info: png::Info<'static>,
    cicp: Option<[u8; 4]>,
) -> Vec<u8> {
    let mut info = info;
    info.width = width;
    info.height = height;
    let mut out = Vec::new();
    let mut encoder = png::Encoder::with_info(&mut out, info).unwrap();
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(depth);
    let mut writer = encoder.write_header().unwrap();
    if let Some(cicp) = cicp {
        writer.write_chunk(png::chunk::cICP, &cicp).unwrap();
    }
    writer.write_image_data(samples).unwrap();
    writer.finish().unwrap();
    out
}

fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag(xyz: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    for v in xyz {
        t.extend(s15f16(v));
    }
    t
}

/// A v2 sRGB display profile written the way the common
/// "sRGB IEC61966-2.1" profile is: its colorants (to five decimals), a
/// media white of D65 rather than the connection space's D50, and one
/// 1024-entry curve table that the three TRC tags share.
fn iec_srgb_v2() -> Vec<u8> {
    let mut desc = b"desc\0\0\0\0".to_vec();
    let text = b"sRGB IEC61966-2.1";
    desc.extend(be32(text.len() as u32 + 1));
    desc.extend(text);
    desc.push(0);
    desc.extend([0; 4 + 4 + 2 + 1 + 67]);
    let mut curve = b"curv\0\0\0\0".to_vec();
    curve.extend(be32(1024));
    for i in 0..1024 {
        let linear = srgb_decode(f64::from(i) / 1023.0);
        curve.extend(((linear * 65535.0).round() as u16).to_be_bytes());
    }
    // The data, each tag on a four-byte boundary; the TRC tags all point
    // at the one curve.
    let data: [(&[u8; 4], Vec<u8>); 6] = [
        (b"desc", desc),
        (b"wtpt", xyz_tag([0.950_455, 1.0, 1.089_050])),
        (b"rXYZ", xyz_tag([0.436_066, 0.222_488, 0.013_916])),
        (b"gXYZ", xyz_tag([0.385_147, 0.716_873, 0.097_076])),
        (b"bXYZ", xyz_tag([0.143_066, 0.060_608, 0.714_096])),
        (b"rTRC", curve),
    ];
    let shared = [b"gTRC", b"bTRC"];
    let count = data.len() + shared.len();
    let mut offset = 128 + 4 + 12 * count;
    let mut table = Vec::new();
    let mut body = Vec::new();
    let mut curve_at = (0, 0);
    for (sig, bytes) in &data {
        table.push((**sig, offset, bytes.len()));
        if *sig == b"rTRC" {
            curve_at = (offset, bytes.len());
        }
        body.extend(bytes);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 128 + 4 + 12 * count + body.len();
    }
    for sig in shared {
        table.push((*sig, curve_at.0, curve_at.1));
    }
    let size = 128 + 4 + 12 * count + body.len();
    let mut p = Vec::with_capacity(size);
    p.extend(be32(size as u32));
    p.extend(b"Lino");
    p.extend(be32(0x0210_0000));
    p.extend(b"mntrRGB XYZ ");
    // 1998-02-09 06:49:00.
    for v in [1998u16, 2, 9, 6, 49, 0] {
        p.extend(v.to_be_bytes());
    }
    p.extend(b"acspMSFT");
    p.extend(be32(0));
    p.extend(b"IEC sRGB");
    p.extend([0; 8]);
    p.extend(be32(0));
    p.extend(s15f16(0.9642));
    p.extend(s15f16(1.0));
    p.extend(s15f16(0.8249));
    p.extend(b"HP  ");
    p.resize(128, 0);
    p.extend(be32(count as u32));
    for (sig, at, len) in table {
        p.extend(sig);
        p.extend(be32(at as u32));
        p.extend(be32(len as u32));
    }
    p.extend(body);
    assert_eq!(p.len(), size);
    p
}

#[test]
fn a_v2_srgb_profile_with_shared_table_curves() {
    let patches = in_srgb(patches());
    let img = rgb8_chart(OutputSpace::Srgb, &patches);
    let profile = iec_srgb_v2();
    let size = (img.width(), img.height());
    let mut info = png::Info::default();
    info.icc_profile = Some(profile.clone().into());
    let mut jpeg = Cursor::new(Vec::new());
    let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, 100);
    image::ImageEncoder::set_icc_profile(&mut encoder, profile.clone()).unwrap();
    DynamicImage::ImageRgb8(img.clone())
        .write_with_encoder(encoder)
        .unwrap();
    let mut tiff = Cursor::new(Vec::new());
    let mut encoder = TiffEncoder::new(&mut tiff);
    image::ImageEncoder::set_icc_profile(&mut encoder, profile).unwrap();
    DynamicImage::ImageRgb8(img.clone())
        .write_with_encoder(encoder)
        .unwrap();
    let files = [
        (Format::Jpeg { quality: 100 }, jpeg.into_inner(), "iec.jpg"),
        (
            Format::Png { bits: 8 },
            png_with(size, img.as_raw(), png::BitDepth::Eight, info, None),
            "iec.png",
        ),
        // An 8-bit TIFF: lossless 8 bits, PNG's tolerance.
        (Format::Png { bits: 8 }, tiff.into_inner(), "iec.tif"),
    ];
    for (format, bytes, name) in files {
        let loaded = load_bytes(&bytes, name);
        let colour = loaded.colour.unwrap();
        assert_eq!(
            colour.from,
            ColourTag::Icc {
                description: Some("sRGB IEC61966-2.1".to_string())
            },
            "{name}: {colour:?}"
        );
        assert!(colour.ignored.is_empty(), "{name}: {colour:?}");
        for (i, p) in patches.iter().enumerate() {
            let what = format!("{name}, {}", p.name);
            assert_close(&format, patch(&loaded.image, i), p.rgb, &what);
        }
    }
}

#[test]
fn a_legacy_png_with_gamma_and_chromaticities() {
    // Adobe RGB said the way PNG writers did before iCCP was common: gAMA
    // with the encoding gamma (1 / 2.2) and cHRM with the primaries.
    let space = OutputSpace::AdobeRgb;
    let patches = in_srgb(patches());
    let img = rgb8_chart(space, &patches);
    let [r, g, b] = space.primaries().map(|[x, y]| (x as f32, y as f32));
    let mut info = png::Info::default();
    info.source_gamma = Some(png::ScaledFloat::new(
        (1.0 / helios_colour::ADOBE_RGB_GAMMA) as f32,
    ));
    info.source_chromaticities = Some(png::SourceChromaticities::new(
        (D65[0] as f32, D65[1] as f32),
        r,
        g,
        b,
    ));
    let size = (img.width(), img.height());
    let bytes = png_with(size, img.as_raw(), png::BitDepth::Eight, info, None);
    let loaded = load_bytes(&bytes, "legacy.png");
    let colour = loaded.colour.unwrap();
    assert!(
        matches!(
            colour.from,
            ColourTag::PngGamma {
                chromaticities: true,
                ..
            }
        ),
        "{colour:?}"
    );
    assert!(colour.ignored.is_empty(), "{colour:?}");
    for (i, p) in patches.iter().enumerate() {
        let what = format!("legacy.png, {}", p.name);
        assert_close(
            &Format::Png { bits: 8 },
            patch(&loaded.image, i),
            p.rgb,
            &what,
        );
    }
}

#[test]
fn a_sixteen_bit_png_tagged_by_cicp_alone() {
    // Rec.2020 as video tools tag it: primaries 9 and the transfer code
    // of 10-bit BT.2020 (14), or BT.709's (1), the same curve; no profile.
    let space = OutputSpace::Rec2020;
    let patches: Vec<Patch> = patches()
        .into_iter()
        .filter(|p| holds(space, p.rgb))
        .collect();
    let width = patches.len() as u32 * PATCH;
    let mut samples = Vec::new();
    for _ in 0..PATCH {
        for x in 0..width {
            for v in patches[(x / PATCH) as usize].rgb {
                let code = (space.encode(v.clamp(0.0, 1.0)) * 65535.0).round() as u16;
                samples.extend(code.to_be_bytes());
            }
        }
    }
    for transfer in [14, 1] {
        let code_points = [9, transfer, 0, 1];
        let bytes = png_with(
            (width, PATCH),
            &samples,
            png::BitDepth::Sixteen,
            png::Info::default(),
            Some(code_points),
        );
        let name = format!("cicp-{transfer}.png");
        let loaded = load_bytes(&bytes, &name);
        let colour = loaded.colour.unwrap();
        assert_eq!(colour.from, ColourTag::Cicp { code_points }, "{name}");
        // Named as Rec.2020 under either code.
        assert_eq!(colour.space, space.name(), "{name}");
        for (i, p) in patches.iter().enumerate() {
            let what = format!("{name}, {}", p.name);
            assert_close(
                &Format::Png { bits: 16 },
                patch(&loaded.image, i),
                p.rgb,
                &what,
            );
        }
    }
}
