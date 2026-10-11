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
    let grey = decoded.get_pixel(2, 0).0[0] as f64 / 65535.0;
    assert!(
        (grey - 0.461).abs() < 0.002,
        "middle grey encodes near 46 %, got {grey}"
    );
    let red = decoded.get_pixel(3, 0).0;
    assert_eq!(
        (red[0], red[1]),
        (65535, 0),
        "Rec.2020 red is out of sRGB: clipped, green at 0"
    );
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
    assert!(err.contains("missing/out.png"), "{err}");
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
