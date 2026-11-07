//! Indicative read times, from the JUnit report's test durations: each
//! test builds the same fixture (a raw-like file with a 160 px thumbnail,
//! a 1620 px preview and a full-size 6000 px one, like a Canon CR2's), so
//! `(reads − fixture) / READS` is one fast read (a cache miss: the file
//! in the page cache) and `(hits − fixture) / HITS` one cache hit.
//! Nothing is judged here but a generous ceiling.

use std::io::Cursor;
use std::path::PathBuf;
use std::time::Instant;

use helios_thumbs::{Key, SMALL, ThumbCache};

const READS: u32 = 50;
const HITS: u32 = 500;

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([
            (x * 255 / w) as u8,
            (y * 255 / h) as u8,
            ((x ^ y) & 0x3F) as u8,
        ])
    });
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut out), 90)
        .encode_image(&img)
        .unwrap();
    out
}

/// The fixture: a raw-like file, in a folder of its own.
fn fixture(name: &str) -> (PathBuf, PathBuf) {
    let dir =
        std::env::temp_dir().join(format!("helios-thumbs-times-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut file = vec![0u8; 65_536];
    for (w, h) in [(160, 120), (1620, 1080), (6000, 4000)] {
        file.extend_from_slice(&jpeg(w, h));
        file.extend_from_slice(&[0u8; 4096]);
    }
    let path = dir.join("fixture.cr2");
    std::fs::write(&path, file).unwrap();
    (dir, path)
}

#[test]
fn time_0_fixture_alone() {
    let (dir, path) = fixture("alone");
    assert!(path.is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn time_1_fast_reads_cold() {
    let (dir, path) = fixture("reads");
    let t = Instant::now();
    for _ in 0..READS {
        let rgb = helios_thumbs::read::read(&path, SMALL).unwrap().unwrap();
        assert_eq!(rgb.width, SMALL);
    }
    let each = t.elapsed() / READS;
    eprintln!("fast read: {each:?} an image");
    assert!(each.as_millis() < 500, "{each:?} a read");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn time_2_cache_hits_warm() {
    let (dir, path) = fixture("hits");
    let cache = ThumbCache::open(&dir.join("cache"), 1 << 30).unwrap();
    let key = Key::preview("fixture");
    let rgb = helios_thumbs::read::read(&path, SMALL).unwrap().unwrap();
    cache.put(&key, SMALL, &rgb).unwrap();
    let t = Instant::now();
    for _ in 0..HITS {
        assert!(cache.get(&key, SMALL).is_some());
    }
    let each = t.elapsed() / HITS;
    eprintln!("cache hit: {each:?} an image");
    assert!(each.as_millis() < 100, "{each:?} a hit");
    let _ = std::fs::remove_dir_all(&dir);
}
