//! Colour tags on load: a picture's values are read through the space its
//! tags name (cICP, an ICC profile, PNG chunks, Exif), and an untagged or
//! wrongly tagged one as sRGB, with the report saying so.
//!
//! The round trips export with `helios-export` in every output space and
//! format, load the file back and compare with the source colours.

use std::io::Cursor;
use std::path::PathBuf;

use helios_core::{Image, Region};
use helios_export::{Format, OutputSpace, encode_in};
use helios_load::{ColourTag, Loaded, load};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::tiff::TiffEncoder;
use image::{DynamicImage, ImageBuffer, ImageEncoder, Rgb};

/// Side of a patch: JPEG keeps flat colour inside its 8 x 8 blocks and
/// 16 x 16 chroma blocks.
const PATCH: u32 = 16;

/// Linear Rec.2020 colours inside every output space's gamut, then the
/// Rec.2020 primaries (inside Rec.2020 only).
const PATCHES: [[f64; 3]; 8] = [
    [0.0, 0.0, 0.0],
    [0.18, 0.18, 0.18],
    [1.0, 1.0, 1.0],
    [0.3, 0.2, 0.1],
    [0.05, 0.1, 0.2],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
];

fn chart() -> Image {
    Image::from_fn(
        Region::sized(PATCHES.len() as u32 * PATCH, PATCH),
        3,
        |x, _, c| PATCHES[(x / i64::from(PATCH)) as usize][c as usize] as f32,
    )
}

/// Writes `bytes` to a fresh file named `name` and loads it.
fn load_bytes(bytes: &[u8], name: &str) -> Loaded {
    let dir = scratch(name);
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    let loaded = load(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let _ = std::fs::remove_dir_all(&dir);
    loaded
}

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("helios-load-colour-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The colour at the middle of patch `i`.
fn patch(image: &Image, i: usize) -> [f32; 3] {
    let (i, side) = (i as i64, i64::from(PATCH));
    let p = image.pixel(i * side + side / 2, side / 2);
    [p[0], p[1], p[2]]
}

fn near(got: [f32; 3], want: [f64; 3], tolerance: f64) -> bool {
    got.iter()
        .zip(want)
        .all(|(g, w)| (f64::from(*g) - w).abs() < tolerance)
}

/// Encoded values of a linear Rec.2020 colour in `space`, in [0, 1].
fn encoded(space: OutputSpace, working: [f64; 3]) -> [f64; 3] {
    let m = space.from_rec2020();
    [0, 1, 2].map(|i| {
        let v = m[i][0] * working[0] + m[i][1] * working[1] + m[i][2] * working[2];
        space.encode(v.clamp(0.0, 1.0))
    })
}

/// A flat 8-bit RGB picture of `PATCHES[..n]` encoded in `space`.
fn rgb8_chart(space: OutputSpace, n: usize) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    ImageBuffer::from_fn(n as u32 * PATCH, PATCH, |x, _| {
        Rgb(encoded(space, PATCHES[(x / PATCH) as usize]).map(|v| (v * 255.0).round() as u8))
    })
}

/// A PNG written through `png`, with the chunks `info` holds and an
/// optional cICP chunk.
fn png_with(
    img: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    info: png::Info<'static>,
    cicp: Option<[u8; 4]>,
) -> Vec<u8> {
    let mut info = info;
    info.width = img.width();
    info.height = img.height();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::with_info(&mut out, info).unwrap();
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    if let Some(cicp) = cicp {
        writer.write_chunk(png::chunk::cICP, &cicp).unwrap();
    }
    writer.write_image_data(img.as_raw()).unwrap();
    writer.finish().unwrap();
    out
}

fn jpeg_with(
    img: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    icc: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    let mut encoder = JpegEncoder::new_with_quality(&mut out, 100);
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc).unwrap();
    }
    if let Some(exif) = exif {
        encoder.set_exif_metadata(exif).unwrap();
    }
    DynamicImage::ImageRgb8(img.clone())
        .write_with_encoder(encoder)
        .unwrap();
    out.into_inner()
}

fn tiff_with(img: &ImageBuffer<Rgb<u8>, Vec<u8>>, icc: Option<Vec<u8>>) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    let mut encoder = TiffEncoder::new(&mut out);
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc).unwrap();
    }
    DynamicImage::ImageRgb8(img.clone())
        .write_with_encoder(encoder)
        .unwrap();
    out.into_inner()
}

/// A little-endian Exif block holding `ColorSpace` and, when given, the
/// interoperability index.
fn exif_block(color_space: u16, interop: Option<&[u8; 3]>) -> Vec<u8> {
    let mut b = b"II*\0".to_vec();
    b.extend(8u32.to_le_bytes());
    // IFD0 at 8: the Exif pointer, to 26.
    b.extend(1u16.to_le_bytes());
    b.extend(0x8769u16.to_le_bytes());
    b.extend(4u16.to_le_bytes());
    b.extend(1u32.to_le_bytes());
    b.extend(26u32.to_le_bytes());
    b.extend(0u32.to_le_bytes());
    // Exif IFD at 26: `ColorSpace`, then the interop pointer, to 56.
    b.extend(if interop.is_some() { 2u16 } else { 1 }.to_le_bytes());
    b.extend(0xA001u16.to_le_bytes());
    b.extend(3u16.to_le_bytes());
    b.extend(1u32.to_le_bytes());
    b.extend(color_space.to_le_bytes());
    b.extend([0, 0]);
    if let Some(index) = interop {
        b.extend(0xA005u16.to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(56u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(4u32.to_le_bytes());
        b.extend(index);
        b.push(0);
    }
    b.extend(0u32.to_le_bytes());
    b
}

#[test]
fn exports_load_back_with_their_colours_in_every_space_and_format() {
    let image = chart();
    // Tolerances on linear values: 8-bit steps are up to about 0.009 near
    // white after the curve; JPEG at quality 100 adds its rounding.
    let formats = [
        (Format::Jpeg { quality: 100 }, "jpg", 0.02),
        (Format::Png { bits: 8 }, "png", 0.01),
        (Format::Png { bits: 16 }, "png", 2e-3),
        (Format::Tiff, "tif", 2e-3),
    ];
    for space in OutputSpace::ALL {
        for (format, ext, tolerance) in &formats {
            let what = format!("{format:?} in {space:?}");
            let bytes = encode_in(&image, format, space).unwrap();
            let loaded = load_bytes(&bytes, &format!("chart.{ext}"));
            let colour = loaded.colour.expect("a picture reports its colour");
            assert!(colour.ignored.is_empty(), "{what}: {colour:?}");
            // PNG says its space in cICP, but for Adobe RGB, which has no
            // code; the others carry the profile.
            let by_cicp = *ext == "png" && space.cicp().is_some();
            if by_cicp {
                assert_eq!(
                    colour.from,
                    ColourTag::Cicp {
                        code_points: space.cicp().unwrap()
                    },
                    "{what}"
                );
                assert_eq!(colour.space, space.name(), "{what}");
            } else {
                assert_eq!(
                    colour.from,
                    ColourTag::Icc {
                        description: Some(format!("Helios {}", space.name()))
                    },
                    "{what}"
                );
            }
            let inside = if space == OutputSpace::Rec2020 { 8 } else { 5 };
            for (i, want) in PATCHES.iter().enumerate().take(inside) {
                let got = patch(&loaded.image, i);
                assert!(
                    near(got, *want, *tolerance),
                    "{what}, patch {i}: {got:?} for {want:?}"
                );
            }
        }
    }
}

#[test]
fn a_small_tiff_keeps_its_profile() {
    // Two by two: a profile larger than the pixels, which `image`'s
    // default limits would drop.
    let image = Image::from_fn(Region::sized(2, 2), 3, |_, _, c| {
        [0.3, 0.2, 0.1][c as usize]
    });
    let bytes = encode_in(&image, &Format::Tiff, OutputSpace::DisplayP3).unwrap();
    let loaded = load_bytes(&bytes, "small.tif");
    let colour = loaded.colour.unwrap();
    assert_eq!(
        colour.from,
        ColourTag::Icc {
            description: Some("Helios Display P3".to_string())
        },
        "{colour:?}"
    );
    let p = loaded.image.pixel(1, 1);
    assert!(near([p[0], p[1], p[2]], [0.3, 0.2, 0.1], 2e-3), "{p:?}");
}

#[test]
fn a_png_tagged_by_cicp_alone() {
    let space = OutputSpace::DisplayP3;
    let img = rgb8_chart(space, 5);
    let bytes = png_with(&img, png::Info::default(), space.cicp());
    let loaded = load_bytes(&bytes, "cicp.png");
    let colour = loaded.colour.unwrap();
    assert_eq!(
        colour.from,
        ColourTag::Cicp {
            code_points: [12, 13, 0, 1]
        }
    );
    assert_eq!(colour.space, "Display P3");
    for (i, want) in PATCHES.iter().enumerate().take(5) {
        let got = patch(&loaded.image, i);
        assert!(near(got, *want, 0.01), "patch {i}: {got:?} for {want:?}");
    }
    // Read as sRGB, the same values would be other colours: the tag is
    // what makes them right.
    let untagged = load_bytes(&png_with(&img, png::Info::default(), None), "plain.png");
    assert!(untagged.colour.unwrap().assumed());
    assert!(!near(patch(&untagged.image, 3), PATCHES[3], 0.01));
}

#[test]
fn untagged_pictures_are_assumed_srgb_and_say_so() {
    let grey = ImageBuffer::from_pixel(PATCH, PATCH, Rgb([128u8, 128, 128]));
    let files = [
        (png_with(&grey, png::Info::default(), None), "grey.png"),
        (jpeg_with(&grey, None, None), "grey.jpg"),
        (tiff_with(&grey, None), "grey.tif"),
    ];
    for (bytes, name) in files {
        let loaded = load_bytes(&bytes, name);
        let colour = loaded.colour.unwrap();
        assert!(colour.assumed(), "{name}: {colour:?}");
        assert_eq!(colour.from, ColourTag::Untagged);
        assert_eq!(colour.space, "sRGB (assumed)");
        assert!(colour.ignored.is_empty(), "{name}: {colour:?}");
        let p = loaded.image.pixel(8, 8);
        // sRGB 128 is 21.6 % linear; JPEG may move a code value.
        assert!(near([p[0], p[1], p[2]], [0.216; 3], 0.004), "{name}: {p:?}");
    }
}

#[test]
fn broken_profiles_load_as_srgb_with_the_reason() {
    let garbage = b"this is not an ICC profile, only bytes where one should be".to_vec();
    let mut truncated = OutputSpace::AdobeRgb.icc_v4();
    truncated.truncate(160);
    let grey = ImageBuffer::from_pixel(PATCH, PATCH, Rgb([128u8, 128, 128]));
    let mut info = png::Info::default();
    info.icc_profile = Some(garbage.clone().into());
    let files = [
        (jpeg_with(&grey, Some(garbage.clone()), None), "broken.jpg"),
        (png_with(&grey, info, None), "broken.png"),
        (tiff_with(&grey, Some(truncated)), "broken.tif"),
    ];
    for (bytes, name) in files {
        let loaded = load_bytes(&bytes, name);
        let colour = loaded.colour.unwrap();
        assert!(colour.assumed(), "{name}: {colour:?}");
        assert_eq!(colour.ignored.len(), 1, "{name}: {colour:?}");
        assert!(colour.ignored[0].contains("ICC"), "{name}: {colour:?}");
        let p = loaded.image.pixel(8, 8);
        assert!(near([p[0], p[1], p[2]], [0.216; 3], 0.004), "{name}: {p:?}");
    }
}

#[test]
fn png_srgb_and_gamma_chunks() {
    let grey = ImageBuffer::from_pixel(PATCH, PATCH, Rgb([128u8, 128, 128]));
    let mut info = png::Info::default();
    info.srgb = Some(png::SrgbRenderingIntent::Perceptual);
    let loaded = load_bytes(&png_with(&grey, info, None), "srgb.png");
    assert_eq!(loaded.colour.unwrap().from, ColourTag::PngSrgb);
    // gAMA 1: the values are linear light (with the sRGB primaries, as no
    // cHRM says otherwise), grey stays grey.
    let mut info = png::Info::default();
    info.source_gamma = Some(png::ScaledFloat::from_scaled(100_000));
    let loaded = load_bytes(&png_with(&grey, info, None), "linear.png");
    let colour = loaded.colour.unwrap();
    assert_eq!(
        colour.from,
        ColourTag::PngGamma {
            gamma: 1.0,
            chromaticities: false
        }
    );
    let p = loaded.image.pixel(8, 8);
    assert!(near([p[0], p[1], p[2]], [128.0 / 255.0; 3], 1e-4), "{p:?}");
}

#[test]
fn a_camera_adobe_rgb_jpeg_is_read_from_its_exif() {
    // As cameras write Adobe RGB: no profile, Exif `ColorSpace`
    // "uncalibrated" and the interoperability index R03.
    let space = OutputSpace::AdobeRgb;
    let img = rgb8_chart(space, 5);
    let bytes = jpeg_with(&img, None, Some(exif_block(0xFFFF, Some(b"R03"))));
    let loaded = load_bytes(&bytes, "camera.jpg");
    let colour = loaded.colour.unwrap();
    assert_eq!(
        colour.from,
        ColourTag::Exif {
            color_space: 0xFFFF,
            interop: Some("R03".to_string())
        }
    );
    assert_eq!(colour.space, "Adobe RGB (1998)");
    for (i, want) in PATCHES.iter().enumerate().take(5) {
        let got = patch(&loaded.image, i);
        assert!(near(got, *want, 0.02), "patch {i}: {got:?} for {want:?}");
    }
    // Exif sRGB is a tag too: not assumed.
    let srgb = jpeg_with(&img, None, Some(exif_block(1, None)));
    let colour = load_bytes(&srgb, "srgb.jpg").colour.unwrap();
    assert!(!colour.assumed());
    assert_eq!(colour.space, "sRGB");
}
