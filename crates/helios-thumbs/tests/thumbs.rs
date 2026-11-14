//! The thumbnail cache, its fast read and its pool.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

use helios_thumbs::read::{choose, embedded_jpegs};
use helios_thumbs::{Job, Key, LARGE, Loader, Outcome, Rgb8, SMALL, ThumbCache, placeholder};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-thumbs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A JPEG of a smooth gradient, `w`×`h`.
fn gradient_jpeg(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([
            (x * 255 / w) as u8,
            (y * 255 / h) as u8,
            ((x + y) * 127 / (w + h)) as u8,
        ])
    });
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut out), 90)
        .encode_image(&img)
        .unwrap();
    out
}

/// A thumbnail of noise (it compresses badly: a large entry).
fn noise(seed: u32) -> Rgb8 {
    let mut s = seed.wrapping_mul(2_654_435_761).max(1);
    let pixels = (0..SMALL * 170 * 3)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as u8
        })
        .collect();
    Rgb8 {
        width: SMALL,
        height: 170,
        pixels,
    }
}

/// The outcome of one job, within a deadline.
fn one(loader_cache: Option<std::sync::Arc<ThumbCache>>, job: Job) -> Outcome {
    let (tx, rx) = channel();
    let tx = std::sync::Mutex::new(tx);
    let loader = Loader::start(loader_cache, 1, move |id, outcome| {
        tx.lock().unwrap().send((id.to_string(), outcome)).is_ok()
    });
    loader.load([job]);
    let (_, outcome) = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("an outcome within 30 s");
    outcome
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn the_fast_read_takes_the_smallest_embedded_jpeg_large_enough() {
    // A "raw": junk, a 160 px thumbnail, a 1000 px and a 3000 px preview.
    let mut file = vec![0u8; 4096];
    let small = gradient_jpeg(160, 120);
    let middle = gradient_jpeg(1000, 667);
    let large = gradient_jpeg(3000, 2000);
    let mut at = Vec::new();
    for part in [&small, &middle, &large] {
        at.push(file.len());
        file.extend_from_slice(part);
        file.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0x00, 1, 2, 3]);
    }
    let found = embedded_jpegs(&file);
    assert_eq!(found.len(), 3, "{found:?}");
    let order = choose(&found, SMALL);
    assert_eq!((order[0].offset, order[0].width), (at[1], 1000));
    assert_eq!(choose(&found, 2000)[0].width, 3000);
    // None large enough: the largest.
    assert_eq!(choose(&found[..1], SMALL)[0].width, 160);
    let rgb = helios_thumbs::read::from_bytes(file, helios_load::Kind::Raw, SMALL).unwrap();
    assert_eq!((rgb.width, rgb.height), (256, 171));
    assert_eq!(rgb.pixels.len(), (256 * 171 * 3) as usize);
}

/// A grey JPEG, `w`×`h`, every pixel `v`: one component, as a phone's
/// Ultra HDR gain map or depth map is stored.
fn grey_jpeg(w: u32, h: u32, v: u8) -> Vec<u8> {
    let img = image::GrayImage::from_pixel(w, h, image::Luma([v]));
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut out), 90)
        .encode_image(&img)
        .unwrap();
    out
}

/// A phone's JPEG: its colour picture, then a smaller grey gain map
/// appended after it (as an Ultra HDR or Motion Photo file holds it).
fn phone_jpeg(gain: u8) -> Vec<u8> {
    let mut file = gradient_jpeg(2000, 1500);
    file.extend_from_slice(&grey_jpeg(1000, 750, gain));
    file
}

/// The mean distance of a thumbnail's red and blue from its green: 0 for
/// a grey picture.
fn chroma(rgb: &Rgb8) -> f64 {
    let n = rgb.pixels.len() / 3;
    let sum: u64 = rgb
        .pixels
        .chunks_exact(3)
        .map(|p| u64::from(p[0].abs_diff(p[1])) + u64::from(p[2].abs_diff(p[1])))
        .sum();
    sum as f64 / n as f64
}

#[test]
fn a_phone_jpeg_s_thumbnail_is_its_colour_picture_not_its_gain_map() {
    use helios_load::Kind;
    for gain in [128u8, 255] {
        let file = phone_jpeg(gain);
        let found = embedded_jpegs(&file);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(!found[0].grey() && found[1].grey(), "{found:?}");
        // The gain map is the smallest stream large enough: colour first.
        assert_eq!(choose(&found, SMALL)[0].width, 2000);
        let rgb = helios_thumbs::read::from_bytes(file, Kind::Picture, SMALL).unwrap();
        assert_eq!((rgb.width, rgb.height), (256, 192));
        // A gain map all white was a blank white cell.
        let mean = rgb.pixels.iter().map(|&v| f64::from(v)).sum::<f64>() / rgb.pixels.len() as f64;
        assert!(mean < 200.0, "gain {gain}: mean {mean}, a blank cell");
        assert!(chroma(&rgb) > 20.0, "gain {gain}: chroma {}", chroma(&rgb));
    }
    // A grey picture's only stream is still read.
    let grey =
        helios_thumbs::read::from_bytes(grey_jpeg(800, 600, 90), Kind::Picture, SMALL).unwrap();
    assert_eq!((grey.width, grey.height), (256, 192));
    assert!(chroma(&grey) < 1.0);
}

#[test]
fn a_grey_entry_of_a_phone_jpeg_stored_before_is_read_again() {
    let dir = scratch("gain-map");
    let cache = std::sync::Arc::new(ThumbCache::open(&dir.join("cache"), 1 << 30).unwrap());
    let path = write(&dir, "PXL.MP.jpg", &phone_jpeg(200));
    // What the reader before stored: the gain map, grey, marked reader 2.
    let grey = Rgb8 {
        width: SMALL,
        height: 192,
        pixels: vec![200; (SMALL * 192 * 3) as usize],
    };
    for size in [SMALL, LARGE] {
        cache
            .put_jpeg(&Key::preview("pxl"), size, &marked_jpeg_075(&grey))
            .unwrap();
    }
    assert!(!cache.get_marked(&Key::preview("pxl"), SMALL).unwrap().1);
    let job = Job {
        id: "pxl".into(),
        path: Some(path),
    };
    let Outcome::Thumb(thumb) = one(Some(cache.clone()), job) else {
        panic!("a thumbnail");
    };
    assert!(
        chroma(&thumb) > 20.0,
        "read again in colour: {}",
        chroma(&thumb)
    );
    let (stored, current) = cache.get_marked(&Key::preview("pxl"), SMALL).unwrap();
    assert!(current, "stored by this reader");
    assert!(chroma(&stored) > 20.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cache_hit_returns_without_touching_the_file() {
    let dir = scratch("hit");
    let cache = std::sync::Arc::new(ThumbCache::open(&dir.join("cache"), 1 << 30).unwrap());
    let path = write(&dir, "a.jpg", &gradient_jpeg(1200, 800));
    let job = Job {
        id: "a1b2".into(),
        path: Some(path.clone()),
    };
    let Outcome::Thumb(first) = one(Some(cache.clone()), job.clone()) else {
        panic!("a thumbnail read from the file");
    };
    assert_eq!((first.width, first.height), (256, 171));
    assert!(cache.contains(&Key::preview("a1b2"), SMALL));
    // The file gone: a read would fail. Reopened from disk, the cache
    // still answers.
    std::fs::remove_file(&path).unwrap();
    let reopened = std::sync::Arc::new(ThumbCache::open(&dir.join("cache"), 1 << 30).unwrap());
    let Outcome::Thumb(again) = one(Some(reopened), job) else {
        panic!("the cached thumbnail");
    };
    assert_eq!((again.width, again.height), (first.width, first.height));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lru_eviction_keeps_the_cache_under_its_limit() {
    let dir = scratch("lru");
    let limit = 400 * 1024;
    let cache = ThumbCache::open(&dir, limit).unwrap();
    let first = Key::preview("0000");
    cache.put(&first, SMALL, &noise(0)).unwrap();
    for n in 1..40u32 {
        // The first entry used again before each write: never evicted.
        assert!(cache.get(&first, SMALL).is_some(), "entry 0 at {n}");
        cache
            .put(&Key::preview(format!("{n:04}")), SMALL, &noise(n))
            .unwrap();
        let (bytes, _) = cache.usage();
        assert!(bytes <= limit, "{bytes} bytes over {limit} after {n}");
    }
    let (bytes, count) = cache.usage();
    assert!(
        count < 40,
        "nothing evicted: {count} entries, {bytes} bytes"
    );
    assert!(cache.contains(&first, SMALL));
    assert!(!cache.contains(&Key::preview("0001"), SMALL));
    assert!(cache.contains(&Key::preview("0039"), SMALL));
    // On disk too.
    let on_disk: u64 = walk(&dir)
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "jpg"))
        .map(|p| p.metadata().unwrap().len())
        .sum();
    assert_eq!(on_disk, bytes);
    // Reopened under a smaller limit, it shrinks.
    let smaller = ThumbCache::open(&dir, limit / 4).unwrap();
    assert!(smaller.usage().0 <= limit / 4);
    let _ = std::fs::remove_dir_all(&dir);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if e.path().is_dir() {
            files.extend(walk(&e.path()));
        } else {
            files.push(e.path());
        }
    }
    files
}

#[test]
fn an_offline_image_shows_its_cached_thumbnail() {
    let dir = scratch("offline");
    let cache = std::sync::Arc::new(ThumbCache::open(&dir, 1 << 30).unwrap());
    cache.put(&Key::preview("off"), SMALL, &noise(7)).unwrap();
    // No copy at hand (its volume unplugged): the cache answers.
    let Outcome::Thumb(t) = one(
        Some(cache.clone()),
        Job {
            id: "off".into(),
            path: None,
        },
    ) else {
        panic!("the cached thumbnail of an offline photo");
    };
    assert_eq!((t.width, t.height), (SMALL, 170));
    // Nothing cached and no file: unavailable, not pending forever.
    assert!(matches!(
        one(
            Some(cache),
            Job {
                id: "never".into(),
                path: None
            }
        ),
        Outcome::Unavailable
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_with_no_preview_gets_the_placeholder() {
    let dir = scratch("none");
    let path = write(&dir, "x.orf", &[7u8; 10_000]);
    assert!(matches!(
        one(
            None,
            Job {
                id: "x".into(),
                path: Some(path)
            }
        ),
        Outcome::NoPreview
    ));
    let p = placeholder("ORF");
    assert_eq!(p.width, SMALL);
    assert_eq!(p.pixels.len(), (p.width * p.height * 3) as usize);
    let ink = p.pixels.chunks_exact(3).filter(|c| c[0] > 120).count();
    assert!(ink > 1000, "the type and the mark are drawn: {ink} pixels");
    assert_ne!(placeholder("ORF"), placeholder("CRW"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_miss_fills_the_large_entry_after() {
    let dir = scratch("large");
    let cache = std::sync::Arc::new(ThumbCache::open(&dir.join("cache"), 1 << 30).unwrap());
    let path = write(&dir, "b.jpg", &gradient_jpeg(2400, 1600));
    let (tx, rx) = channel();
    let tx = std::sync::Mutex::new(tx);
    let loader = Loader::start(Some(cache.clone()), 1, move |id, o| {
        tx.lock().unwrap().send((id.to_string(), o)).is_ok()
    });
    loader.load([Job {
        id: "b".into(),
        path: Some(path),
    }]);
    rx.recv_timeout(Duration::from_secs(30)).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !cache.contains(&Key::preview("b"), LARGE) {
        assert!(std::time::Instant::now() < deadline, "no large entry");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Asked, it is sent from the cache.
    loader.large("b");
    let (id, outcome) = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(id, "b");
    let Outcome::Large(large) = outcome else {
        panic!("the large entry, not {outcome:?}");
    };
    assert_eq!(large.width.max(large.height), LARGE);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opening_the_cache_does_not_list_it_the_pool_does() {
    let dir = scratch("unlisted");
    let thumbs = dir.join("cache");
    let thumb = Rgb8 {
        width: SMALL,
        height: 100,
        pixels: vec![90; (SMALL * 100 * 3) as usize],
    };
    ThumbCache::open(&thumbs, 1 << 30)
        .unwrap()
        .put(&Key::preview("seen"), SMALL, &thumb)
        .unwrap();
    // What the interface thread does at startup: nothing is listed.
    let cache = std::sync::Arc::new(ThumbCache::open_unlisted(&thumbs, 1 << 30).unwrap());
    assert!(!cache.listed(), "opening does not list the folder");
    // The pool's threads list it, then serve the entry from it (no file
    // to read: the thumbnail can only come from the cache).
    let (tx, rx) = channel();
    let tx = std::sync::Mutex::new(tx);
    let loader = Loader::start(Some(cache.clone()), 2, move |id, o| {
        tx.lock().unwrap().send((id.to_string(), o)).is_ok()
    });
    loader.load([Job {
        id: "seen".into(),
        path: None,
    }]);
    let (_, outcome) = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(matches!(outcome, Outcome::Thumb(_)), "{outcome:?}");
    assert!(cache.listed());
    assert_eq!(cache.usage().1, 1);
    drop(loader);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_thumbnail_dropped_is_queued_again_when_preferred() {
    let dir = scratch("again");
    let cache = std::sync::Arc::new(ThumbCache::open(&dir.join("cache"), 1 << 30).unwrap());
    let path = write(&dir, "d.jpg", &gradient_jpeg(600, 400));
    let (tx, rx) = channel();
    let tx = std::sync::Mutex::new(tx);
    let loader = Loader::start(Some(cache), 1, move |id, o| {
        tx.lock().unwrap().send((id.to_string(), o)).is_ok()
    });
    loader.load([Job {
        id: "d".into(),
        path: Some(path.clone()),
    }]);
    rx.recv_timeout(Duration::from_secs(30)).unwrap();
    // Its texture freed, its cell back in view: read again, from the
    // cache (the file gone, only the cache has it).
    std::fs::remove_file(&path).unwrap();
    loader.prefer(&["d".to_string()]);
    let (id, outcome) = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(id, "d");
    assert!(matches!(outcome, Outcome::Thumb(_)), "{outcome:?}");
    drop(loader);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_pool_reads_the_images_in_view_first() {
    let dir = scratch("order");
    let path = write(&dir, "c.jpg", &gradient_jpeg(600, 400));
    let (tx, rx) = channel();
    let tx = std::sync::Mutex::new(tx);
    let (go, gate) = channel::<()>();
    let gate = std::sync::Mutex::new(gate);
    let loader = Loader::start(None, 1, move |id, _| {
        // The first outcome waits until the view is known.
        if id == "0" {
            let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(30));
        }
        tx.lock().unwrap().send(id.to_string()).is_ok()
    });
    loader.load((0..20).map(|n| Job {
        id: n.to_string(),
        path: Some(path.clone()),
    }));
    // While the first is read, the view moves to 15 and 16.
    std::thread::sleep(Duration::from_millis(50));
    loader.prefer(&["16".to_string(), "15".to_string()]);
    go.send(()).unwrap();
    let order: Vec<String> = (0..20)
        .map(|_| rx.recv_timeout(Duration::from_secs(30)).unwrap())
        .collect();
    let at = |id: &str| order.iter().position(|o| o == id).unwrap();
    assert!(at("16") <= 2 && at("15") <= 2, "{order:?}");
    assert!(at("16") < at("15"), "{order:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_provisional_entry_is_renamed_to_its_full_key() {
    let dir = scratch("rename");
    let cache = ThumbCache::open(&dir, 1 << 30).unwrap();
    let provisional = Key::provisional(1234, &"ab".repeat(32));
    assert!(provisional.is_provisional());
    let full = Key::preview("cd".repeat(32));
    assert!(!full.is_provisional());
    assert_ne!(
        cache.file_of(&provisional, SMALL),
        cache.file_of(&full, SMALL)
    );
    cache.put(&provisional, SMALL, &noise(1)).unwrap();
    let before = cache.usage();
    assert_eq!(cache.rename(&provisional, &full), 1);
    assert!(!cache.contains(&provisional, SMALL));
    assert!(cache.get(&full, SMALL).is_some(), "the file moved with it");
    assert_eq!(cache.usage(), before, "a rename keeps the bytes");
    // Renamed onto an entry already there: that one stays.
    cache.put(&provisional, SMALL, &noise(2)).unwrap();
    assert_eq!(cache.rename(&provisional, &full), 0);
    assert!(!cache.contains(&provisional, SMALL));
    assert_eq!(cache.usage(), before);
    // Reopened, the index matches the folder.
    drop(cache);
    let cache = ThumbCache::open(&dir, 1 << 30).unwrap();
    assert_eq!(cache.usage(), before);
    assert!(cache.contains(&full, SMALL));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `w`×`h` JPEG tagged with the EXIF orientation `tag`.
fn tagged_jpeg(w: u32, h: u32, tag: u16) -> Vec<u8> {
    use image::ImageEncoder as _;
    let mut exif = b"II*\0".to_vec();
    exif.extend(8u32.to_le_bytes());
    exif.extend(1u16.to_le_bytes());
    exif.extend(0x0112u16.to_le_bytes());
    exif.extend(3u16.to_le_bytes());
    exif.extend(1u32.to_le_bytes());
    exif.extend(tag.to_le_bytes());
    exif.extend([0, 0, 0, 0, 0, 0]);
    let img = image::RgbImage::from_fn(w, h, |x, _| image::Rgb([(x % 256) as u8, 60, 120]));
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90);
    enc.set_exif_metadata(exif).unwrap();
    enc.write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
        .unwrap();
    out
}

#[test]
fn a_picture_shot_in_portrait_gets_an_upright_thumbnail() {
    let turned = helios_thumbs::read::from_bytes(
        tagged_jpeg(300, 200, 6),
        helios_load::Kind::Picture,
        SMALL,
    )
    .unwrap();
    assert_eq!((turned.width, turned.height), (171, 256));
    let upright = helios_thumbs::read::from_bytes(
        tagged_jpeg(300, 200, 1),
        helios_load::Kind::Picture,
        SMALL,
    )
    .unwrap();
    assert_eq!((upright.width, upright.height), (256, 171));
    // A raw's bytes are not read as a picture's tags.
    let raw =
        helios_thumbs::read::from_bytes(tagged_jpeg(300, 200, 6), helios_load::Kind::Raw, SMALL)
            .unwrap();
    assert_eq!((raw.width, raw.height), (256, 171));
}

#[test]
fn a_raw_shot_in_portrait_gets_its_embedded_preview_upright() {
    // A "raw" carrying a landscape preview, stored as its sensor is: its
    // EXIF orientation (6, the camera turned right) turns it, as darktable
    // turns a raw's embedded preview.
    let mut file = vec![0u8; 4096];
    file.extend_from_slice(&gradient_jpeg(1200, 800));
    let file = std::sync::Arc::new(file);
    let read = |tag| {
        helios_thumbs::read::from_bytes_tagged(file.clone(), helios_load::Kind::Raw, SMALL, tag)
            .unwrap()
    };
    let upright = read(6);
    assert_eq!((upright.width, upright.height), (171, 256));
    assert_eq!((read(8).width, read(8).height), (171, 256));
    let level = read(1);
    assert_eq!((level.width, level.height), (256, 171));
    // Turned right: the preview's top-left corner is now top-right.
    let at = |rgb: &Rgb8, x: u32, y: u32| rgb.pixels[((y * rgb.width + x) * 3) as usize];
    assert_eq!(at(&upright, upright.width - 1, 0), at(&level, 0, 0));
    // Bytes rawler cannot read have no tag: kept as stored.
    assert_eq!(helios_load::raw_orientation((*file).clone().into()), 1);
}

/// A thumbnail encoded as a JPEG without the reader's mark: an entry
/// stored before pictures were turned.
fn unmarked_jpeg(thumb: &Rgb8) -> Vec<u8> {
    let img = image::RgbImage::from_raw(thumb.width, thumb.height, thumb.pixels.clone()).unwrap();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut out), 80)
        .encode_image(&img)
        .unwrap();
    out
}

#[test]
fn an_old_sideways_entry_is_read_again_and_the_others_are_kept() {
    let dir = scratch("reader");
    let cache = std::sync::Arc::new(ThumbCache::open(&dir.join("cache"), 1 << 30).unwrap());
    let level = write(&dir, "level.jpg", &gradient_jpeg(1200, 800));
    let portrait = write(&dir, "portrait.jpg", &tagged_jpeg(300, 200, 6));
    let mut raw = vec![0u8; 4096];
    raw.extend_from_slice(&gradient_jpeg(1200, 800));
    let raw = write(&dir, "shot.nef", &raw);
    // Entries stored before: noise, 256×170, where a read gives 256×171
    // (171×256 turned).
    for id in ["level", "portrait", "raw"] {
        cache
            .put_jpeg(&Key::preview(id), SMALL, &unmarked_jpeg(&noise(3)))
            .unwrap();
        cache
            .put_jpeg(&Key::preview(id), LARGE, &unmarked_jpeg(&noise(4)))
            .unwrap();
        assert!(!cache.get_marked(&Key::preview(id), SMALL).unwrap().1);
    }
    let size = |id: &str, path: &Path| match one(
        Some(cache.clone()),
        Job {
            id: id.into(),
            path: Some(path.to_path_buf()),
        },
    ) {
        Outcome::Thumb(t) => (t.width, t.height),
        other => panic!("{id}: {other:?}"),
    };
    // The portrait picture's entry was sideways: read again, upright.
    assert_eq!(size("portrait", &portrait), (171, 256));
    let (_, current) = cache.get_marked(&Key::preview("portrait"), SMALL).unwrap();
    assert!(current, "stored by this reader");
    // The picture with nothing to turn keeps its entries, marked: no read.
    assert_eq!(size("level", &level), (256, 170));
    for s in [SMALL, LARGE] {
        let (thumb, current) = cache.get_marked(&Key::preview("level"), s).unwrap();
        assert!(current, "marked at {s}");
        assert_eq!((thumb.width, thumb.height), (256, 170), "kept at {s}");
    }
    // Marked: its file is not read again, even gone.
    std::fs::remove_file(&level).unwrap();
    assert_eq!(size("level", &level), (256, 170));
    // A raw's entry is kept as it is: no read of the raw.
    assert_eq!(size("raw", &raw), (256, 170));
    let (_, current) = cache.get_marked(&Key::preview("raw"), SMALL).unwrap();
    assert!(!current, "a raw's old entry is left as it was");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_cache_is_tagged_counted_by_kind_and_cleared_through_the_service() {
    use helios_cache::{Service, Store, Tier};
    let dir = scratch("service");
    let cache = std::sync::Arc::new(ThumbCache::open(&dir, 1 << 30).unwrap());
    // Backup tools and the image map skip it.
    let tag = std::fs::read_to_string(dir.join("CACHEDIR.TAG")).unwrap();
    assert!(tag.starts_with("Signature: 8a477f597d28d172789f06886806bc55"));
    let (a, b) = (Key::preview("aaaa"), Key::preview("bbbb"));
    cache.put(&a, SMALL, &noise(1)).unwrap();
    cache.put(&b, SMALL, &noise(2)).unwrap();
    cache.put(&a, LARGE, &noise(3)).unwrap();
    assert!(cache.get(&a, SMALL).is_some());
    assert!(cache.get(&Key::preview("cccc"), SMALL).is_none());
    cache.attach();
    let usage = Store::usage(&*cache);
    let of = |id: &str| usage.iter().find(|u| u.kind.id == id).unwrap().clone();
    assert_eq!(of("thumb.256").entries, 2);
    assert_eq!(of("thumb.1024").entries, 1);
    assert_eq!(of("thumb.256").tier, Tier::Disk);
    assert_eq!(of("thumb.256").budget, Some(1 << 30));
    // The service sees it, with its statistics.
    let snapshot = Service::global().snapshot();
    let scope = format!("silo:library:{}", dir.display());
    let small = snapshot["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == "thumb.256" && k["scope"] == scope.as_str())
        .unwrap()
        .clone();
    assert_eq!(small["entries"], 2);
    assert_eq!(small["stats"]["hits"], 1);
    assert_eq!(small["stats"]["misses"], 1);
    // Clearing one kind keeps the other, and the tag.
    let large_bytes = of("thumb.1024").bytes;
    assert_eq!(Store::clear(&*cache, Some("thumb.1024")), large_bytes);
    assert!(!cache.contains(&a, LARGE));
    assert!(cache.contains(&a, SMALL));
    Store::clear(&*cache, None);
    assert_eq!(cache.usage(), (0, 0));
    assert!(dir.join("CACHEDIR.TAG").exists());
    // Reopened, the tag is not taken for an interrupted write.
    let again = ThumbCache::open(&dir, 1 << 30).unwrap();
    assert_eq!(again.usage(), (0, 0));
    assert!(dir.join("CACHEDIR.TAG").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A JPEG as 0.75 stored it: the reader's mark (`helios-thumbs reader 2`)
/// as a comment right after its start marker.
fn marked_jpeg_075(thumb: &Rgb8) -> Vec<u8> {
    let plain = unmarked_jpeg(thumb);
    let mark = b"helios-thumbs reader 2";
    let mut out = plain[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xFE]);
    out.extend_from_slice(&u16::try_from(mark.len() + 2).unwrap().to_be_bytes());
    out.extend_from_slice(mark);
    out.extend_from_slice(&plain[2..]);
    out
}

/// A cache folder as 0.75 wrote it (written here byte for byte, not through
/// today's code: `<two first characters>/<image>.preview.<size>.jpg`, the
/// reader's mark, no CACHEDIR.TAG) is read by today's cache as hits only,
/// opened as the desktop opens it and listed by the pool: nothing is read
/// again from a file, nothing evicted.
#[test]
fn a_cache_written_by_0_75_is_read_as_hits() {
    use helios_cache::Service;
    let root = scratch("compat-075");
    // `.helios/thumbs` beside the catalogue, as 0.75 placed it.
    let dir = helios_thumbs::cache_dir(&root.join("library.helios"));
    assert_eq!(dir, root.join(".helios").join("thumbs"));
    let ids: Vec<String> = (0..24u32)
        .map(|i| blake3::hash(&i.to_le_bytes()).to_hex().to_string())
        .collect();
    for (i, id) in ids.iter().enumerate() {
        let shard = dir.join(&id[..2]);
        std::fs::create_dir_all(&shard).unwrap();
        let jpeg = marked_jpeg_075(&noise(u32::try_from(i).unwrap()));
        std::fs::write(shard.join(format!("{id}.preview.{SMALL}.jpg")), &jpeg).unwrap();
        std::fs::write(shard.join(format!("{id}.preview.{LARGE}.jpg")), &jpeg).unwrap();
    }
    assert!(!dir.join("CACHEDIR.TAG").exists());
    let cache = std::sync::Arc::new(ThumbCache::open_unlisted(&dir, 1 << 30).unwrap());
    // Offline photos: a miss could only answer Unavailable.
    let (tx, rx) = channel();
    let tx = std::sync::Mutex::new(tx);
    let loader = Loader::start(Some(cache.clone()), 4, move |id, outcome| {
        tx.lock().unwrap().send((id.to_string(), outcome)).is_ok()
    });
    loader.load(ids.iter().map(|id| Job {
        id: id.clone(),
        path: None,
    }));
    for _ in &ids {
        let (id, outcome) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("an outcome within 30 s");
        assert!(
            matches!(outcome, Outcome::Thumb(ref t) if t.width == SMALL),
            "{id}: {outcome:?}"
        );
    }
    drop(loader);
    assert_eq!(
        cache.usage().1,
        ids.len() * 2,
        "every entry listed, none evicted"
    );
    let snapshot = Service::global().snapshot();
    let scope = format!("silo:library:{}", dir.display());
    let small = snapshot["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == "thumb.256" && k["scope"] == scope.as_str())
        .unwrap()
        .clone();
    assert_eq!(small["stats"]["hits"], ids.len());
    assert_eq!(small["stats"]["misses"], 0);
    // The files kept their names (nothing rewritten, renamed or removed).
    for id in &ids {
        for size in [SMALL, LARGE] {
            assert!(
                dir.join(&id[..2])
                    .join(format!("{id}.preview.{size}.jpg"))
                    .exists()
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}
