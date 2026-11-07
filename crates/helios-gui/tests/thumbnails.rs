//! The library's thumbnails through the cache: an offline photo shows its
//! cached thumbnail, a file with no preview its placeholder, and the fast
//! 8-bit read looks like the old floating-point path.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::vec2;
use helios_gui::library::Entry;
use helios_gui::{Shell, headless, helios_panels};
use helios_thumbs::{Key, SMALL, ThumbCache};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-gui-thumbs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A JPEG of a photo-like picture: gradients and a few edges.
fn picture_jpeg(path: &Path, w: u32, h: u32) {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let edge = if (x / 97 + y / 61) % 2 == 0 { 40 } else { 0 };
        image::Rgb([
            (x * 200 / w) as u8 + edge,
            (y * 200 / h) as u8 + edge,
            (((x * 3 + y) / 13) % 180) as u8 + edge,
        ])
    });
    img.save_with_format(path, image::ImageFormat::Jpeg)
        .unwrap();
}

#[test]
fn an_offline_photo_shows_its_cached_thumbnail_and_a_file_without_preview_its_placeholder() {
    let dir = scratch("library");
    let cache = Arc::new(ThumbCache::open(&dir.join("thumbs"), 1 << 30).unwrap());
    // "off" was seen before: its thumbnail is cached, its volume is gone.
    let cached = helios_thumbs::Rgb8 {
        width: SMALL,
        height: 170,
        pixels: vec![150; (SMALL * 170 * 3) as usize],
    };
    cache.put(&Key::preview("off"), SMALL, &cached).unwrap();
    let junk = dir.join("junk.orf");
    std::fs::write(&junk, [3u8; 5000]).unwrap();
    let entries = vec![
        Entry {
            id: "off".into(),
            name: "off.cr2".into(),
            presence: helios_vault::Presence::Offline,
            ..Entry::default()
        },
        Entry {
            id: "junk".into(),
            name: "junk.orf".into(),
            ..Entry::default()
        },
    ];
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries);
    let files = vec![("junk".to_string(), junk)];
    let loader = helios_gui::thumbnails::start(&shell, Some(cache), &files, |_| {});
    let mut h = headless::harness(shell, vec2(1024.0, 700.0));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        h.run();
        let s = h.state();
        let library = s.library();
        if library.has_thumbnail("off") && library.has_thumbnail("junk") {
            break;
        }
        assert!(Instant::now() < deadline, "thumbnails within 30 s");
        std::thread::sleep(Duration::from_millis(20));
    }
    let s = h.state();
    let library = s.library();
    assert!(
        !library.has_placeholder("off"),
        "the cached thumbnail shows"
    );
    assert!(
        library.has_placeholder("junk"),
        "no preview: the placeholder"
    );
    drop(loader);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The mean absolute difference of two pictures of one size, per channel.
fn mean_difference(a: &egui::ColorImage, b: &egui::ColorImage) -> f64 {
    assert_eq!(a.size, b.size);
    let sum: u64 = a
        .pixels
        .iter()
        .zip(&b.pixels)
        .map(|(p, q)| {
            (0..3)
                .map(|c| u64::from(p.to_array()[c].abs_diff(q.to_array()[c])))
                .sum::<u64>()
        })
        .sum();
    sum as f64 / (a.pixels.len() * 3) as f64
}

#[test]
fn the_fast_8_bit_read_looks_like_the_old_path() {
    let dir = scratch("match");
    let path = dir.join("p.jpg");
    picture_jpeg(&path, 3000, 2000);
    // The old path: decoded whole in floating point, to the working
    // space, scaled, back to sRGB.
    let old = helios_gui::library::thumbnail(&helios_load::load(&path).unwrap().image);
    let new =
        helios_gui::library::rgb_image(&helios_thumbs::read::read(&path, SMALL).unwrap().unwrap());
    let diff = mean_difference(&old, &new);
    eprintln!("mean difference, synthetic JPEG: {diff:.2} levels");
    assert!(diff < 4.0, "mean difference {diff:.2} levels of 255");
    let _ = std::fs::remove_dir_all(&dir);
    // darktable's test raws, when the checkout is there: the same
    // embedded preview through both paths.
    let Some(images) = test_images() else { return };
    for name in ["mire1.cr2", "hlrecovery.arw", "xtransIV.raf"] {
        let raw = images.join(name);
        let Some(preview) = helios_load::preview(&raw).unwrap() else {
            continue;
        };
        let old = helios_gui::library::thumbnail(&preview);
        let new = helios_gui::library::rgb_image(
            &helios_thumbs::read::read(&raw, SMALL).unwrap().unwrap(),
        );
        if old.size != new.size {
            // Another embedded preview (another crop): not comparable.
            eprintln!(
                "{name}: {:?} against {:?}, not compared",
                old.size, new.size
            );
            continue;
        }
        let diff = mean_difference(&old, &new);
        eprintln!("mean difference, {name}: {diff:.2} levels");
        assert!(
            diff < 8.0,
            "{name}: mean difference {diff:.2} levels of 255"
        );
    }
}

fn test_images() -> Option<PathBuf> {
    let dir = std::env::var_os("HELIOS_TEST_IMAGES").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../darktable/src/tests/integration/images")
        },
        PathBuf::from,
    );
    dir.join("mire1.cr2").is_file().then_some(dir)
}
