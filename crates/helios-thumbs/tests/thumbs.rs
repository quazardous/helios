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
    let on_disk: u64 = walk(&dir).iter().map(|p| p.metadata().unwrap().len()).sum();
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
