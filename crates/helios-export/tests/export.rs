//! Exports: colors come out right in each format, files appear whole, and
//! errors name the file.

use std::path::PathBuf;

use helios_core::{Image, Region};
use helios_export::{Format, encode, write_atomic};

/// Black, white, middle grey and pure Rec.2020 red, linear, 4 channels.
fn swatch() -> Image {
    let colors = [
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [0.18, 0.18, 0.18],
        [1.0, 0.0, 0.0],
    ];
    Image::from_fn(Region::sized(4, 1), 4, |x, _, c| {
        if c == 3 {
            1.0
        } else {
            colors[x as usize][c as usize] as f32
        }
    })
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("helios-export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn png16_holds_srgb_encoded_values() {
    let bytes = encode(&swatch(), &Format::Png { bits: 16 }).unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap().to_rgb16();
    assert_eq!(decoded.dimensions(), (4, 1));
    assert_eq!(decoded.get_pixel(0, 0).0, [0, 0, 0]);
    // The published matrices carry four digits: white lands within 0.02 %.
    let white = decoded.get_pixel(1, 0).0;
    assert!(
        white.iter().all(|&v| v >= 65520),
        "white stays white: {white:?}"
    );
    let grey = f64::from(decoded.get_pixel(2, 0).0[0]) / 65535.0;
    assert!(
        (grey - 0.461).abs() < 0.002,
        "middle grey encodes near 46 %, got {grey}"
    );
    let red = decoded.get_pixel(3, 0).0;
    // Rec.2020 red is out of sRGB: brought inside at its hue and
    // lightness, so green and blue rise a little instead of staying at 0.
    assert!(
        red[0] > 65000 && (1000..30000).contains(&red[1]) && (1000..30000).contains(&red[2]),
        "Rec.2020 red is mapped, not clipped: {red:?}"
    );
}

#[test]
fn rgba8_holds_what_an_8_bit_png_holds_opaque() {
    let image = swatch();
    let rgba = helios_export::srgb8_rgba(&image).unwrap();
    let png = encode(&image, &Format::Png { bits: 8 }).unwrap();
    let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(rgba, decoded.into_raw());
    assert!(rgba.chunks_exact(4).all(|p| p[3] == 255));
}

#[test]
fn every_format_decodes_to_the_right_size() {
    let image = Image::from_fn(Region::sized(17, 9), 4, |x, y, _| ((x + y) as f32) / 30.0);
    for format in [
        Format::Jpeg { quality: 90 },
        Format::Png { bits: 8 },
        Format::Tiff,
    ] {
        let bytes = encode(&image, &format).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (17, 9), "{format:?}");
    }
    assert!(
        encode(&image, &Format::Png { bits: 12 })
            .unwrap_err()
            .english
            .contains("8 or 16 bits")
    );
}

#[test]
fn files_are_replaced_whole_and_no_temporary_file_stays() {
    let d = dir("atomic");
    let path = d.join("out.png");
    write_atomic(&path, b"first").unwrap();
    write_atomic(&path, b"second").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"second");
    let names: Vec<_> = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec!["out.png"]);
    let err = write_atomic(&d.join("missing/out.png"), b"x").unwrap_err();
    assert!(err.english.contains("missing/out.png"), "{err}");
    assert_eq!(err.key, "export-write-failed");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_format_follows_the_extension() {
    assert_eq!(
        Format::from_path("a/b.JPG".as_ref()),
        Some(Format::Jpeg { quality: 92 })
    );
    assert_eq!(Format::from_path("b.tif".as_ref()), Some(Format::Tiff));
    assert_eq!(Format::from_path("b.webp".as_ref()), None);
}

#[test]
fn a_mask_is_written_as_its_values() {
    use helios_export::mask_as_grey;
    let values = [0.0_f32, 0.25, 0.5, 1.0, 1.5];
    let mask = Image::from_fn(Region::sized(5, 1), 1, |x, _, _| values[x as usize]);
    let bytes = encode(&mask_as_grey(&mask).unwrap(), &Format::Png { bits: 8 }).unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
    for (x, v) in values.iter().enumerate() {
        let want = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let got = decoded.get_pixel(x as u32, 0).0;
        assert!(
            got.iter().all(|c| c.abs_diff(want) <= 1),
            "{v}: {got:?}, {want} expected"
        );
    }
    // Only a mask: an RGB image is refused.
    let rgb = Image::from_fn(Region::sized(1, 1), 3, |_, _, _| 0.5);
    assert_eq!(mask_as_grey(&rgb).unwrap_err().key, "export-not-a-mask");
}
