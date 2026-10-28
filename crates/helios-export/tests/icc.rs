//! Colour tags: every export carries the profile of its output space, read
//! back from the written file by readers that are not the writer.
//!
//! The profile bytes come out of each file through `image`'s decoders (and
//! the cICP chunk through `png`'s), then a small ICC reader written here,
//! from ICC.1, checks the header, the colorants and the tone curves against
//! the space's own numbers. moxcms parses each profile too.

use std::io::Cursor;

use helios_core::{Image, Region};
use helios_export::{Format, OutputSpace, encode, encode_in};
use image::codecs::tiff::TiffDecoder;
use image::{ImageDecoder, ImageFormat, ImageReader};

/// Linear Rec.2020 patches: black, middle grey, white, the three
/// primaries, and a brown inside every output space's gamut.
const PATCHES: [[f64; 3]; 7] = [
    [0.0, 0.0, 0.0],
    [0.18, 0.18, 0.18],
    [1.0, 1.0, 1.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.3, 0.2, 0.1],
];

fn chart() -> Image {
    let colors = PATCHES;
    Image::from_fn(Region::sized(colors.len() as u32, 2), 3, |x, _, c| {
        colors[x as usize][c as usize] as f32
    })
}

fn formats() -> [Format; 4] {
    [
        Format::Jpeg { quality: 95 },
        Format::Png { bits: 8 },
        Format::Png { bits: 16 },
        Format::Tiff,
    ]
}

/// The ICC profile a file carries, as its format's decoder finds it.
fn embedded_profile(bytes: &[u8]) -> Option<Vec<u8>> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .unwrap();
    if reader.format() == Some(ImageFormat::Tiff) {
        // Not through `ImageReader`: its default limits give the TIFF
        // decoder a value budget the size of the pixel buffer (image
        // 0.25.10, `TiffDecoder::set_limits`), so on a small image the
        // ICC tag exceeds it and `icc_profile` answers None although the
        // tag is there. The decoder alone keeps `tiff`'s own limits.
        let mut decoder = TiffDecoder::new(Cursor::new(bytes)).unwrap();
        return decoder.icc_profile().unwrap();
    }
    reader.into_decoder().unwrap().icc_profile().unwrap()
}

/// The cICP code points of a PNG, through `png`'s decoder.
fn png_cicp(bytes: &[u8]) -> Option<[u8; 4]> {
    let reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    reader.info().coding_independent_code_points.map(|c| {
        [
            c.color_primaries,
            c.transfer_function,
            c.matrix_coefficients,
            u8::from(c.is_video_full_range_image),
        ]
    })
}

/// What the test reads of a matrix/TRC profile.
struct Profile {
    major_version: u8,
    class: [u8; 4],
    data_space: [u8; 4],
    pcs: [u8; 4],
    /// rXYZ, gXYZ, bXYZ.
    colorants: [[f64; 3]; 3],
    /// rTRC, gTRC, bTRC, evaluated as functions.
    curves: Vec<Box<dyn Fn(f64) -> f64>>,
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}

fn s15f16(b: &[u8], at: usize) -> f64 {
    f64::from(be32(b, at) as i32) / 65536.0
}

fn sig(b: &[u8], at: usize) -> [u8; 4] {
    b[at..at + 4].try_into().unwrap()
}

/// A tag's data, by signature.
fn tag(b: &[u8], name: [u8; 4]) -> &[u8] {
    let count = be32(b, 128) as usize;
    for i in 0..count {
        let entry = 132 + 12 * i;
        if sig(b, entry) == name {
            let (offset, size) = (be32(b, entry + 4) as usize, be32(b, entry + 8) as usize);
            assert!(offset + size <= b.len(), "tag inside the profile");
            return &b[offset..offset + size];
        }
    }
    panic!("no {} tag", String::from_utf8_lossy(&name));
}

/// A tone curve: `curv` (identity, gamma or table) or `para` (ICC.1 table
/// 68, functions 0 to 4).
fn curve(t: &[u8]) -> Box<dyn Fn(f64) -> f64> {
    match &sig(t, 0) {
        b"curv" => {
            let n = be32(t, 8) as usize;
            match n {
                0 => Box::new(|x| x),
                1 => {
                    let g = f64::from(u16::from_be_bytes([t[12], t[13]])) / 256.0;
                    Box::new(move |x: f64| x.powf(g))
                }
                _ => {
                    let table: Vec<f64> = (0..n)
                        .map(|i| f64::from(u16::from_be_bytes([t[12 + 2 * i], t[13 + 2 * i]])))
                        .map(|v| v / 65535.0)
                        .collect();
                    Box::new(move |x: f64| {
                        let p = x.clamp(0.0, 1.0) * (table.len() - 1) as f64;
                        let i = (p.floor() as usize).min(table.len() - 2);
                        let f = p - i as f64;
                        table[i] * (1.0 - f) + table[i + 1] * f
                    })
                }
            }
        }
        b"para" => {
            let function = u16::from_be_bytes([t[8], t[9]]);
            let count = [1, 3, 4, 5, 7][function as usize];
            let p: Vec<f64> = (0..count).map(|i| s15f16(t, 12 + 4 * i)).collect();
            Box::new(move |x: f64| match function {
                0 => x.powf(p[0]),
                1 => {
                    if x >= -p[2] / p[1] {
                        (p[1] * x + p[2]).powf(p[0])
                    } else {
                        0.0
                    }
                }
                2 => {
                    if x >= -p[2] / p[1] {
                        (p[1] * x + p[2]).powf(p[0]) + p[3]
                    } else {
                        p[3]
                    }
                }
                3 => {
                    if x >= p[4] {
                        (p[1] * x + p[2]).powf(p[0])
                    } else {
                        p[3] * x
                    }
                }
                _ => {
                    if x >= p[4] {
                        (p[1] * x + p[2]).powf(p[0]) + p[5]
                    } else {
                        p[3] * x + p[6]
                    }
                }
            })
        }
        other => panic!("unknown curve type {}", String::from_utf8_lossy(other)),
    }
}

fn read_profile(b: &[u8]) -> Profile {
    assert!(b.len() >= 132, "a header and a tag count");
    assert_eq!(be32(b, 0) as usize, b.len(), "the header says the size");
    assert_eq!(&b[36..40], b"acsp", "profile signature");
    let xyz = |name: &[u8; 4]| {
        let t = tag(b, *name);
        assert_eq!(&t[0..4], b"XYZ ");
        [s15f16(t, 8), s15f16(t, 12), s15f16(t, 16)]
    };
    Profile {
        major_version: b[8],
        class: sig(b, 12),
        data_space: sig(b, 16),
        pcs: sig(b, 20),
        colorants: [xyz(b"rXYZ"), xyz(b"gXYZ"), xyz(b"bXYZ")],
        curves: [b"rTRC", b"gTRC", b"bTRC"]
            .iter()
            .map(|n| curve(tag(b, **n)))
            .collect(),
    }
}

/// Checks a profile describes `space`: an RGB display profile with the
/// space's colorants (its matrix adapted to D50) and its tone curve.
fn assert_describes(bytes: &[u8], space: OutputSpace, version: u8, what: &str) {
    let p = read_profile(bytes);
    assert_eq!(p.major_version, version, "{what}: profile version");
    assert_eq!(&p.class, b"mntr", "{what}: display class");
    assert_eq!(&p.data_space, b"RGB ", "{what}: RGB data");
    assert_eq!(&p.pcs, b"XYZ ", "{what}: XYZ connection space");
    let want = space.to_xyz_d50();
    for (c, colorant) in p.colorants.iter().enumerate() {
        for (row, got) in colorant.iter().enumerate() {
            // s15Fixed16 keeps 1/65536.
            assert!(
                (got - want[row][c]).abs() < 1e-4,
                "{what}: colorant {c} row {row}: {got} for {}",
                want[row][c]
            );
        }
    }
    for (c, trc) in p.curves.iter().enumerate() {
        for i in 0..=20 {
            let x = f64::from(i) / 20.0;
            let (got, want) = (trc(x), space.decode(x));
            // Tables and s15Fixed16 parameters: well under an 8-bit step.
            assert!(
                (got - want).abs() < 5e-4,
                "{what}: curve {c} at {x}: {got} for {want}"
            );
        }
    }
    let parsed = moxcms::ColorProfile::new_from_slice(bytes)
        .unwrap_or_else(|e| panic!("{what}: moxcms cannot read it: {e:?}"));
    assert!(
        parsed.red_trc.is_some() && parsed.blue_colorant.y > 0.0,
        "{what}"
    );
}

#[test]
fn every_format_embeds_the_profile_of_its_space() {
    let image = chart();
    for space in OutputSpace::ALL {
        for format in formats() {
            let what = format!("{format:?} in {space:?}");
            let bytes = encode_in(&image, &format, space).unwrap();
            let profile = embedded_profile(&bytes).unwrap_or_else(|| panic!("{what}: no profile"));
            // JPEG carries v2 for older readers, PNG and TIFF v4.
            let version = if matches!(format, Format::Jpeg { .. }) {
                2
            } else {
                4
            };
            assert_eq!(
                profile,
                if version == 2 {
                    space.icc_v2()
                } else {
                    space.icc_v4()
                },
                "{what}: the profile is the space's"
            );
            assert_describes(&profile, space, version, &what);
        }
    }
}

#[test]
fn png_carries_cicp_but_for_adobe_rgb() {
    let image = chart();
    let want = [
        (OutputSpace::Srgb, Some([1, 13, 0, 1])),
        (OutputSpace::DisplayP3, Some([12, 13, 0, 1])),
        (OutputSpace::AdobeRgb, None),
        (OutputSpace::Rec2020, Some([9, 1, 0, 1])),
    ];
    for (space, cicp) in want {
        for bits in [8, 16] {
            let bytes = encode_in(&image, &Format::Png { bits }, space).unwrap();
            assert_eq!(png_cicp(&bytes), cicp, "{space:?}, {bits} bits");
            assert_eq!(space.cicp(), cicp);
        }
    }
}

#[test]
fn pixels_read_through_their_profile_give_back_the_colours() {
    // A reader that trusts the tag: the file's values through the profile's
    // curves and colorants land on the source colour (in D50 XYZ, where ICC
    // connects), for every patch inside the space's gamut.
    let image = chart();
    let working = OutputSpace::Rec2020.to_xyz_d50();
    let mul = |m: &[[f64; 3]; 3], v: [f64; 3]| {
        [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
    };
    for space in OutputSpace::ALL {
        for format in [Format::Png { bits: 16 }, Format::Tiff] {
            let bytes = encode_in(&image, &format, space).unwrap();
            let profile = read_profile(&embedded_profile(&bytes).unwrap());
            let to_xyz = [0, 1, 2].map(|row| profile.colorants.map(|c| c[row]));
            let decoded = image::load_from_memory(&bytes).unwrap().to_rgb16();
            let in_gamut = [0, 1, 2, 6]
                .into_iter()
                .chain(if space == OutputSpace::Rec2020 {
                    3..6
                } else {
                    0..0
                });
            for x in in_gamut {
                let file = decoded.get_pixel(x as u32, 0).0;
                let linear = [0, 1, 2].map(|c| (profile.curves[c])(f64::from(file[c]) / 65535.0));
                let got = mul(&to_xyz, linear);
                let want = mul(&working, PATCHES[x]);
                assert!(
                    got.iter().zip(want).all(|(g, w)| (g - w).abs() < 2e-3),
                    "{format:?} in {space:?}, patch {x}: {got:?} for {want:?}"
                );
            }
        }
    }
}

#[test]
fn colours_outside_the_space_are_mapped_alike_in_every_format() {
    // Linear Rec.2020 colours outside sRGB, Display P3 and Adobe RGB (the
    // primaries and secondaries, a bright orange, a highlight above
    // white, a negative blue), in flat 16 x 16 patches so that JPEG's
    // blocks see one colour each. Every format holds what the one gamut
    // mapping gives, encoded with the space's curve.
    const SIDE: u32 = 16;
    let colours: [[f64; 3]; 9] = [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 1.0],
        [1.0, 0.0, 1.0],
        [0.9, 0.3, 0.0],
        [1.6, 1.2, 0.9],
        [0.2, 0.3, -0.1],
    ];
    let image = Image::from_fn(
        Region::sized(colours.len() as u32 * SIDE, SIDE),
        3,
        |x, _, c| colours[(x / i64::from(SIDE)) as usize][c as usize] as f32,
    );
    for space in [
        OutputSpace::Srgb,
        OutputSpace::DisplayP3,
        OutputSpace::AdobeRgb,
    ] {
        let map = space.gamut_map();
        let want: Vec<[f64; 3]> = colours
            .iter()
            .map(|c| {
                // The working image holds f32: map what it holds.
                let held = c.map(|v| f64::from(v as f32));
                map.map(held).map(|v| space.encode(v))
            })
            .collect();
        // The values the file holds, over [0, 1], at each patch's centre.
        let read = |format: &Format| -> Vec<[f64; 3]> {
            let bytes = encode_in(&image, format, space).unwrap();
            let decoded = image::load_from_memory(&bytes).unwrap().to_rgb16();
            (0..colours.len() as u32)
                .map(|i| {
                    let p = decoded.get_pixel(i * SIDE + SIDE / 2, SIDE / 2).0;
                    p.map(|v| f64::from(v) / 65535.0)
                })
                .collect()
        };
        // Tolerances: the rounding of 16 and 8 bits, and JPEG's quantising
        // at quality 100 on top of 8 bits.
        for (format, tolerance) in [
            (Format::Png { bits: 16 }, 0.6 / 65535.0),
            (Format::Tiff, 0.6 / 65535.0),
            (Format::Png { bits: 8 }, 0.6 / 255.0),
            (Format::Jpeg { quality: 100 }, 4.0 / 255.0),
        ] {
            let got = read(&format);
            for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                assert!(
                    g.iter().zip(w).all(|(g, w)| (g - w).abs() <= tolerance),
                    "{format:?} in {space:?}, colour {:?}: {g:?} for {w:?}",
                    colours[i]
                );
            }
        }
        // Not a per-channel clip: Rec.2020 red needs a channel that
        // clipping leaves at 0 to hold its hue.
        let m = space.from_rec2020();
        let clipped = m.map(|row| space.encode(row[0].clamp(0.0, 1.0)));
        let red = want[0];
        assert!(
            (0..3).any(|c| clipped[c] == 0.0 && red[c] > 0.1),
            "{space:?}: {red:?} where clipping gives {clipped:?}"
        );
    }
}

#[test]
fn the_default_export_is_tagged_srgb_and_reproducible() {
    let image = chart();
    for format in formats() {
        let first = encode(&image, &format).unwrap();
        assert_eq!(
            first,
            encode_in(&image, &format, OutputSpace::Srgb).unwrap(),
            "{format:?}: sRGB is the default"
        );
        assert_eq!(
            first,
            encode(&image, &format).unwrap(),
            "{format:?}: same image, same bytes"
        );
        let profile = embedded_profile(&first).expect("an sRGB export is tagged");
        let version = if matches!(format, Format::Jpeg { .. }) {
            2
        } else {
            4
        };
        assert_describes(&profile, OutputSpace::Srgb, version, &format!("{format:?}"));
    }
}
