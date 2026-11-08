//! A picture's EXIF orientation: read at load with its pixels kept as
//! stored (the `flip` step turns them), read without decoding for the
//! geometry darktable's masks need, and applied to a glance.

use std::path::{Path, PathBuf};

use helios_load::Kind;

const W: u32 = 30;
const H: u32 = 20;

/// An Exif block whose first directory holds the `Orientation` tag.
fn exif(tag: u16) -> Vec<u8> {
    let mut b = b"II*\0".to_vec();
    b.extend(8u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(0x0112u16.to_le_bytes());
    b.extend(3u16.to_le_bytes());
    b.extend(1u32.to_le_bytes());
    b.extend(tag.to_le_bytes());
    b.extend([0, 0]);
    b.extend(0u32.to_le_bytes());
    b
}

/// A `W x H` JPEG with a gradient, tagged `tag` when given.
fn jpeg(tag: Option<u16>) -> Vec<u8> {
    use image::ImageEncoder as _;
    let img =
        image::RgbImage::from_fn(W, H, |x, y| image::Rgb([(x * 8) as u8, (y * 12) as u8, 90]));
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95);
    if let Some(tag) = tag {
        enc.set_exif_metadata(exif(tag)).unwrap();
    }
    enc.write_image(img.as_raw(), W, H, image::ExtendedColorType::Rgb8)
        .unwrap();
    out
}

fn written(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-load-orientation-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn size(path: &Path) -> (u32, u32) {
    let r = helios_load::load(path).unwrap().image.region();
    (r.width, r.height)
}

#[test]
fn a_picture_s_orientation_is_read_its_pixels_kept_as_stored() {
    for (name, tag, want) in [
        ("six.jpg", Some(6), 6),
        ("one.jpg", Some(1), 1),
        ("none.jpg", None, 1),
        ("out-of-range.jpg", Some(9), 1),
    ] {
        let bytes = jpeg(tag);
        let path = written(name, &bytes);
        let loaded = helios_load::load(&path).unwrap();
        assert_eq!(loaded.orientation, want, "{name}");
        assert_eq!(size(&path), (W, H), "{name}");
        assert_eq!(helios_load::picture_orientation(&bytes), want, "{name}");
        assert_eq!(
            helios_load::picture_geometry(&path).unwrap(),
            Some((W, H, want)),
            "{name}"
        );
    }
    assert_eq!(Kind::of(Path::new("a.cr2")), Kind::Raw);
    assert_eq!(helios_load::picture_geometry(Path::new("a.cr2")), Ok(None));
}

#[test]
fn a_turned_picture_s_glance_is_upright() {
    let six = written("glance-six.jpg", &jpeg(Some(6)));
    let one = written("glance-one.jpg", &jpeg(Some(1)));
    let stored = helios_load::glance(&one).unwrap().preview.unwrap();
    let turned = helios_load::glance(&six).unwrap().preview.unwrap();
    assert_eq!(stored.region(), helios_core::Region::sized(W, H));
    assert_eq!(turned.region(), helios_core::Region::sized(H, W));
    // Turned 90 degrees clockwise: the left column comes from the
    // bottom row.
    let h = i64::from(H);
    for v in 0..i64::from(W) {
        for u in 0..h {
            assert_eq!(turned.pixel(u, v), stored.pixel(v, h - 1 - u), "({u}, {v})");
        }
    }
}
