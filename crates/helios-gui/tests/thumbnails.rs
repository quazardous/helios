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

#[test]
fn startup_does_not_wait_for_the_cache_index() {
    let dir = scratch("index");
    let vault_file = dir.join("library.helios");
    // A cache seen before, with one entry.
    let thumbs = helios_thumbs::cache_dir(&vault_file);
    let cached = helios_thumbs::Rgb8 {
        width: SMALL,
        height: 170,
        pixels: vec![120; (SMALL * 170 * 3) as usize],
    };
    ThumbCache::open(&thumbs, 1 << 30)
        .unwrap()
        .put(&Key::preview("seen"), SMALL, &cached)
        .unwrap();
    // Opened as the interface thread opens it: the folder is not listed.
    let cache = helios_gui::thumbnails::open_cache(&vault_file, 1 << 30).unwrap();
    assert!(
        !cache.listed(),
        "the interface thread does not list the cache"
    );
    let entries = vec![Entry {
        id: "seen".into(),
        name: "seen.cr2".into(),
        presence: helios_vault::Presence::Offline,
        ..Entry::default()
    }];
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries);
    let mut h = headless::harness(shell, vec2(1024.0, 700.0));
    // The grid draws with the index not there yet: its cell waits.
    for _ in 0..3 {
        h.step();
    }
    assert!(!h.state().library().has_thumbnail("seen"));
    assert!(!cache.listed());
    // The pool lists it, then the cell fills from it.
    let loader = helios_gui::thumbnails::start(h.state(), Some(cache.clone()), &[], |_| {});
    let deadline = Instant::now() + Duration::from_secs(30);
    while !h.state().library().has_thumbnail("seen") {
        assert!(Instant::now() < deadline, "the thumbnail within 30 s");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(cache.listed());
    assert!(!h.state().library().has_placeholder("seen"));
    drop(loader);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compare_shows_the_large_entry_first() {
    let dir = scratch("large");
    let cache = Arc::new(ThumbCache::open(&dir.join("thumbs"), 1 << 30).unwrap());
    let large = helios_thumbs::Rgb8 {
        width: helios_thumbs::LARGE,
        height: 683,
        pixels: vec![100; (helios_thumbs::LARGE * 683 * 3) as usize],
    };
    for id in ["a", "b"] {
        cache
            .put(&Key::preview(id), helios_thumbs::LARGE, &large)
            .unwrap();
    }
    // JPEGs: no render of a head version replaces the large entry.
    let entries: Vec<Entry> = ["a", "b"]
        .iter()
        .map(|id| Entry {
            id: (*id).into(),
            name: format!("{id}.jpg"),
            ..Entry::default()
        })
        .collect();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries);
    shell
        .library_mut()
        .select_ids(&["a".to_string(), "b".to_string()]);
    let loader = helios_gui::thumbnails::start(&shell, Some(cache), &[], |_| {});
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.state_mut()
        .run(
            "helios.compare.focus",
            serde_json::Value::Null,
            helios_shell::commands::Origin::User,
        )
        .expect("the Compare panel");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        h.step();
        let shown = {
            let library = h.state().library();
            (library.large_thumbnail("a"), library.large_thumbnail("b"))
        };
        if let (Some(a), Some(b)) = shown {
            assert_eq!(a[0].max(a[1]), helios_thumbs::LARGE as usize);
            assert_eq!(b[0].max(b[1]), helios_thumbs::LARGE as usize);
            break;
        }
        assert!(Instant::now() < deadline, "the large entries within 30 s");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(loader);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_loupe_shows_the_large_entry_until_its_render_is_complete() {
    let dir = scratch("loupe");
    let cache = Arc::new(ThumbCache::open(&dir.join("thumbs"), 1 << 30).unwrap());
    let large = helios_thumbs::Rgb8 {
        width: helios_thumbs::LARGE,
        height: 683,
        pixels: vec![100; (helios_thumbs::LARGE * 683 * 3) as usize],
    };
    cache
        .put(&Key::preview("a"), helios_thumbs::LARGE, &large)
        .unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(vec![Entry {
        id: "a".into(),
        name: "a.jpg".into(),
        ..Entry::default()
    }]);
    let loader = helios_gui::thumbnails::start(&shell, Some(cache), &[], |_| {});
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.state_mut()
        .run(
            "helios.viewport.focus",
            serde_json::Value::Null,
            helios_shell::commands::Origin::User,
        )
        .expect("the viewport");
    h.step();
    // Opened from the library: the large entry is drawn, scaled to fit,
    // before anything is rendered (no source given yet).
    h.state_mut().open_from_library(Some("a"));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        h.step();
        if let Some(size) = h.state().viewport().stand_in() {
            assert_eq!(size, [helios_thumbs::LARGE as usize, 683]);
            break;
        }
        assert!(Instant::now() < deadline, "the large entry within 30 s");
        std::thread::sleep(Duration::from_millis(20));
    }
    h.step();
    let display = h.state().target_rect("viewport.display").unwrap();
    let drawn = h
        .state()
        .target_rect("viewport.stand-in")
        .expect("the large entry drawn");
    assert!(
        display.contains_rect(drawn.shrink(0.5)),
        "{drawn:?} in {display:?}"
    );
    let fits = |a: f32, b: f32| (a - b).abs() < 1.0;
    assert!(
        fits(drawn.width(), display.width()) || fits(drawn.height(), display.height()),
        "scaled to fit: {drawn:?} in {display:?}"
    );
    // The image made ready: the large entry stays while its render runs.
    let mut g = helios_core::Graph::new();
    let src = g.add_source(
        "source",
        helios_core::Image::from_fn(helios_core::Region::sized(300, 200), 4, |_, _, _| 0.5),
    );
    h.state_mut().set_view_source(g, src, "image").unwrap();
    h.step();
    assert!(h.state().viewport().stand_in().is_some());
    assert!(h.state().target_rect("viewport.stand-in").is_ok());
    // Then the render replaces it.
    for _ in 0..40 {
        let stats = h.state().viewport_stats();
        assert!(stats.wait_idle(Duration::from_mins(1)), "{stats:?}");
        h.step();
        if h.state().viewport().stand_in().is_none() {
            break;
        }
    }
    assert!(
        h.state().viewport().stand_in().is_none(),
        "the render replaced it"
    );
    assert!(h.state().viewport().state().tiles_complete());
    h.step();
    assert!(h.state().target_rect("viewport.stand-in").is_err());
    drop(loader);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A cache holding a 256 px entry for each of `n` images (ids `0000…`).
fn warm_cache(dir: &Path, n: usize) -> (Arc<ThumbCache>, Vec<Entry>) {
    let cache = Arc::new(ThumbCache::open(dir, 1 << 30).unwrap());
    let entries = (0..n)
        .map(|i| {
            let shade = (i * 37 % 200) as u8 + 20;
            let rgb = helios_thumbs::Rgb8 {
                width: SMALL,
                height: 170,
                pixels: (0..SMALL * 170 * 3)
                    .map(|p| shade.wrapping_add((p % 251) as u8 / 4))
                    .collect(),
            };
            let id = format!("{i:016x}");
            cache.put(&Key::preview(id.clone()), SMALL, &rgb).unwrap();
            Entry {
                id,
                name: format!("IMG_{i:04}.CR2"),
                ..Entry::default()
            }
        })
        .collect();
    (cache, entries)
}

/// Steps the window until every cell in view has its thumbnail: the frames
/// it took after the one that first drew cells.
fn frames_to_fill(h: &mut egui_kittest::Harness<'_, Shell>) -> (usize, usize) {
    h.state_mut()
        .run(
            "helios.vault.focus",
            serde_json::Value::Null,
            helios_shell::commands::Origin::User,
        )
        .expect("the Library panel");
    let mut first = None;
    for frame in 0..200 {
        h.step();
        let s = h.state();
        let lib = s.library();
        let in_view = lib.in_view();
        if in_view.is_empty() {
            continue;
        }
        let first = *first.get_or_insert(frame);
        if in_view.iter().all(|id| lib.has_thumbnail(id)) {
            assert!(
                in_view.iter().all(|id| !lib.has_placeholder(id)),
                "cached pictures, not placeholders"
            );
            return (frame - first, in_view.len());
        }
    }
    panic!("the cells in view never all had their thumbnail");
}

/// A warm cache shows the cells in view at once: read on the frame that
/// drew them, not after the pool's queue (the owner saw each cell drawn
/// with its name alone, its picture coming later, one after another).
#[test]
fn a_warm_cache_fills_the_cells_in_view_on_the_next_frame() {
    let dir = scratch("warm-frame");
    let (cache, entries) = warm_cache(&dir.join("thumbs"), 400);
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries);
    // No pool: only the frame's read can fill them. Budget wide enough for
    // a screen on a loaded machine: this checks the path, not its speed.
    shell.library_mut().set_thumb_cache(Some(cache));
    shell.library_mut().set_warm_budget(Duration::from_secs(10));
    let mut h = headless::harness(shell, vec2(1920.0, 1080.0));
    let (frames, cells) = frames_to_fill(&mut h);
    assert!(cells >= 4, "a screen of cells ({cells})");
    assert!(
        frames <= 1,
        "{cells} cells filled {frames} frames after drawn"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same through the desktop's path (`thumbnails::start`: the cache
/// opened unlisted, listed by the pool, the pool running), with the
/// frame's default budget: a screen within three frames of being drawn.
#[test]
fn a_warm_cache_fills_a_screen_within_three_frames_with_the_pool() {
    let dir = scratch("warm-pool");
    let (_, entries) = warm_cache(&dir.join("thumbs"), 2000);
    let cache = Arc::new(ThumbCache::open_unlisted(&dir.join("thumbs"), 1 << 30).unwrap());
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries);
    let loader = helios_gui::thumbnails::start(&shell, Some(cache.clone()), &[], |_| {});
    // The desktop lists the cache while its window opens.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !cache.listed() {
        assert!(Instant::now() < deadline, "listed within 30 s");
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut h = headless::harness(shell, vec2(1920.0, 1080.0));
    let (frames, cells) = frames_to_fill(&mut h);
    assert!(
        frames <= 3,
        "{cells} cells filled {frames} frames after drawn"
    );
    drop(loader);
    let _ = std::fs::remove_dir_all(&dir);
}
