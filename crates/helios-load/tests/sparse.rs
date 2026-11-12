//! A raw's previews and metadata read without its sensor data: the bytes
//! read, before and after, and the same previews and tags as the whole
//! file gives.

use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_load::sparse::{self, Counting};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

/// A little-endian TIFF entry.
fn entry(out: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: u32) {
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&typ.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&value.to_le_bytes());
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([x as u8, y as u8, 90]));
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut out), 85)
        .encode_image(&img)
        .unwrap();
    out
}

/// A TIFF "raw": IFD0 a reduced-resolution image whose JPEG is given by
/// `JPEGInterchangeFormat`, one sub-IFD a CFA strip of `raw` bytes.
fn tiny_raw(raw: usize) -> (Vec<u8>, (usize, usize), (usize, usize)) {
    let preview = jpeg(320, 200);
    let ifd0 = 8u32;
    let sub = ifd0 + 2 + 4 * 12 + 4;
    let data = sub + 2 + 4 * 12 + 4;
    let jpeg_at = data;
    let strip_at = jpeg_at + preview.len() as u32;
    let mut f = Vec::new();
    f.extend_from_slice(b"II*\0");
    f.extend_from_slice(&ifd0.to_le_bytes());
    f.extend_from_slice(&4u16.to_le_bytes());
    entry(&mut f, 0xFE, 4, 1, 1);
    entry(&mut f, 0x14A, 4, 1, sub);
    entry(&mut f, 0x201, 4, 1, jpeg_at);
    entry(&mut f, 0x202, 4, 1, preview.len() as u32);
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&4u16.to_le_bytes());
    entry(&mut f, 0xFE, 4, 1, 0);
    entry(&mut f, 0x106, 3, 1, 32803);
    entry(&mut f, 0x111, 4, 1, strip_at);
    entry(&mut f, 0x117, 4, 1, raw as u32);
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&preview);
    f.extend((0..raw).map(|i| (i % 251) as u8 | 1));
    (
        f,
        (jpeg_at as usize, preview.len()),
        (strip_at as usize, raw),
    )
}

#[test]
fn a_tiff_raw_is_read_without_its_sensor_data() {
    let (file, (jpeg_at, jpeg_len), (strip_at, strip_len)) = tiny_raw(4 << 20);
    let mut reader = Counting::new(Cursor::new(file.clone()));
    let sparse = sparse::read(&mut reader).unwrap();
    assert_eq!(sparse.len(), file.len());
    // The preview is there, byte for byte; the strip's start too.
    assert_eq!(
        sparse[jpeg_at..jpeg_at + jpeg_len],
        file[jpeg_at..jpeg_at + jpeg_len]
    );
    assert_eq!(
        sparse[strip_at..strip_at + 1024],
        file[strip_at..strip_at + 1024]
    );
    // The rest of the strip is not read.
    assert!(
        sparse[strip_at + strip_len - 1024..]
            .iter()
            .all(|&b| b == 0)
    );
    assert!(
        reader.bytes < (1 << 20),
        "read {} bytes of {}",
        reader.bytes,
        file.len()
    );
}

#[test]
fn a_layout_not_known_is_read_whole_once() {
    for file in [vec![7u8; 1 << 20], b"II*\0".to_vec(), jpeg(64, 64)] {
        let mut reader = Counting::new(Cursor::new(file.clone()));
        assert_eq!(sparse::read(&mut reader).unwrap(), file);
        assert_eq!(reader.bytes, file.len() as u64);
    }
}

/// The raws to measure: the raw corpus (`HELIOS_RAW_CORPUS`) and
/// darktable's integration images (`HELIOS_TEST_IMAGES`, or the sibling
/// darktable checkout), when there.
fn raws() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["HELIOS_RAW_CORPUS", "HELIOS_TEST_IMAGES"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    dirs.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../darktable/src/tests/integration/images"),
    );
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().as_ref().map(std::fs::DirEntry::path))
            .filter(|p| p.is_file() && helios_load::Kind::of(p) == helios_load::Kind::Raw)
            .collect();
        found.sort();
        out.extend(found);
    }
    out
}

/// What rawler gives from a buffer: the previews' pixels and the tags.
fn seen(bytes: Arc<Vec<u8>>) -> Option<(Vec<Vec<u8>>, String)> {
    let source = RawSource::new_from_shared_vec(bytes);
    let decoder = rawler::get_decoder(&source).ok()?;
    let params = RawDecodeParams::default();
    let pixels = [
        decoder.thumbnail_image(&source, &params),
        decoder.preview_image(&source, &params),
        decoder.full_image(&source, &params),
    ]
    .into_iter()
    .map(|r| {
        r.ok()
            .flatten()
            .map_or_else(Vec::new, image::DynamicImage::into_bytes)
    })
    .collect();
    let meta = decoder.raw_metadata(&source, &params).map_or_else(
        |_| String::new(),
        |m| format!("{} {} {:?}", m.make, m.model, m.exif),
    );
    Some((pixels, meta))
}

#[test]
fn real_raws_give_the_same_previews_and_tags_from_far_fewer_bytes() {
    let raws = raws();
    if raws.is_empty() {
        eprintln!("no raws (HELIOS_RAW_CORPUS, HELIOS_TEST_IMAGES): skipped");
        return;
    }
    eprintln!(
        "{:<40} {:>12} {:>12} {:>6} {:>6}",
        "file", "before", "after", "%", "reads"
    );
    for path in raws {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        // Before: the whole file, as the previews were read.
        let mut whole = Vec::new();
        let mut before = Counting::new(File::open(&path).unwrap());
        before.read_to_end(&mut whole).unwrap();
        let mut after = Counting::new(File::open(&path).unwrap());
        let sparse = sparse::read(&mut after).unwrap();
        eprintln!(
            "{name:<40} {:>12} {:>12} {:>5.1}% {:>6}",
            before.bytes,
            after.bytes,
            100.0 * after.bytes as f64 / before.bytes as f64,
            after.reads
        );
        assert!(
            after.bytes <= before.bytes,
            "{name}: read more than the file"
        );
        assert_eq!(sparse.len(), whole.len(), "{name}");
        let Some(want) = seen(Arc::new(whole)) else {
            continue;
        };
        let got = seen(Arc::new(sparse))
            .unwrap_or_else(|| panic!("{name}: no decoder on the sparse read"));
        assert!(want.0 == got.0, "{name}: the previews differ");
        assert_eq!(want.1, got.1, "{name}: the tags differ");
    }
}
