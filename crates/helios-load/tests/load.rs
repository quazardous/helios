//! Loaders on darktable's public integration test images (the
//! `src/tests/integration` submodule of a darktable checkout). The raw tests
//! are skipped, saying so, when the images are not there: set
//! `HELIOS_TEST_IMAGES` to their directory.

use std::path::{Path, PathBuf};

use helios_core::Region;
use helios_load::{Kind, load, preview};
use helios_photo::Cfa;

fn images() -> Option<PathBuf> {
    let dir = std::env::var_os("HELIOS_TEST_IMAGES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../darktable/src/tests/integration/images")
        });
    if dir.join("mire1.cr2").is_file() {
        Some(dir)
    } else {
        eprintln!("skipped: no test images in {}", dir.display());
        None
    }
}

#[test]
fn a_bayer_raw_loads_as_a_mosaic_with_its_camera() {
    let Some(dir) = images() else { return };
    let loaded = load(&dir.join("mire1.cr2")).unwrap();
    assert_eq!(loaded.hash.len(), 64);
    assert_eq!(loaded.image.channels(), 1);
    let camera = loaded.camera.expect("a raw has a camera");
    assert_eq!(camera.maker, "Canon");
    assert!(matches!(camera.cfa, Cfa::Bayer(_)), "{:?}", camera.cfa);
    assert!(camera.white > camera.black[0], "{camera:?}");
    assert!(camera.iso > 0.0 && camera.exposure_time > 0.0, "{camera:?}");
    assert!(camera.white_balance[0] > 0.0 && camera.white_balance[2] > 0.0);
    // Raw units: not scaled, not black-subtracted (a module's job). Values
    // may pass the white level: clipping is a module's job too.
    let data = loaded.image.data();
    let max = data.iter().copied().fold(0.0f32, f32::max);
    assert!(f64::from(max) > camera.black[0], "max {max}");
    assert!(
        camera
            .white_balance
            .iter()
            .all(|v| v.is_finite() && *v > 0.0),
        "the missing second green is the first: {camera:?}"
    );
    assert!(
        helios_photo::color::camera_to_working(&camera.xyz_to_camera).is_some(),
        "a usable color matrix: {camera:?}"
    );
    // The context modules read is complete.
    let back = helios_photo::CameraInfo::from_context(&camera.to_context()).unwrap();
    assert_eq!(back, camera);
}

#[test]
fn an_x_trans_raw_loads_with_its_six_by_six_pattern() {
    let Some(dir) = images() else { return };
    let loaded = load(&dir.join("xtransIV.raf")).unwrap();
    let camera = loaded.camera.unwrap();
    assert_eq!(camera.maker, "Fujifilm");
    assert!(matches!(camera.cfa, Cfa::XTrans(_)), "{:?}", camera.cfa);
}

#[test]
fn every_test_raw_loads_and_carries_a_preview() {
    let Some(dir) = images() else { return };
    let err = load(&dir.join("bayer4.crw")).err().unwrap();
    assert!(
        err.contains("CYGM"),
        "a CYGM sensor is refused by name: {err}"
    );
    for name in [
        "hlrecovery.arw",
        "mire1.cr2",
        "mire1-xtrans.raf",
        "xtransIV.raf",
    ] {
        let path = dir.join(name);
        assert_eq!(Kind::of(&path), Kind::Raw);
        let loaded = load(&path).unwrap_or_else(|e| panic!("{e}"));
        let r = loaded.image.region();
        assert!(r.width > 100 && r.height > 100, "{name}: {r:?}");
        if let Some(p) = preview(&path).unwrap() {
            assert_eq!(p.channels(), 4, "{name}");
        }
    }
}

#[test]
fn a_picture_loads_linear_in_the_working_space() {
    let dir = std::env::temp_dir().join(format!("helios-load-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("grey.png");
    // sRGB 50 % grey, 2 x 1, and white.
    let img = image::RgbImage::from_raw(2, 1, vec![128, 128, 128, 255, 255, 255]).unwrap();
    img.save(&path).unwrap();
    let loaded = load(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(loaded.camera.is_none());
    assert_eq!(loaded.image.region(), Region::sized(2, 1));
    assert_eq!(loaded.image.channels(), 4);
    let grey = loaded.image.pixel(0, 0);
    for c in 0..3 {
        assert!(
            (grey[c] - 0.216).abs() < 0.002,
            "sRGB 128 is 21.6 % linear: {grey:?}"
        );
    }
    assert_eq!(grey[3], 1.0, "no alpha in the file: opaque");
    let white = loaded.image.pixel(1, 0);
    assert!(
        white[..3].iter().all(|v| (v - 1.0).abs() < 1e-3),
        "{white:?}"
    );
}

#[test]
fn errors_name_the_file() {
    let err = load(Path::new("/nonexistent/a.cr2")).err().unwrap();
    assert!(err.contains("/nonexistent/a.cr2"), "{err}");
    let dir = std::env::temp_dir().join(format!("helios-load-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("garbage.nef");
    std::fs::write(&bad, b"not a raw").unwrap();
    let err = load(&bad).err().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(err.contains("garbage.nef"), "{err}");
}
