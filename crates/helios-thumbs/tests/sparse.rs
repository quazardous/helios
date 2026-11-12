//! A raw's thumbnail read without its sensor data is the one read from
//! the whole file, pixel for pixel.

use std::path::{Path, PathBuf};

use helios_thumbs::{LARGE, SMALL};

/// The raw corpus (`HELIOS_RAW_CORPUS`) and darktable's integration
/// images (`HELIOS_TEST_IMAGES`, or the sibling darktable checkout).
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

#[test]
fn a_raw_thumbnail_is_the_same_from_its_sparse_read() {
    let raws = raws();
    if raws.is_empty() {
        eprintln!("no raws (HELIOS_RAW_CORPUS, HELIOS_TEST_IMAGES): skipped");
        return;
    }
    for path in raws {
        let whole = std::fs::read(&path).unwrap();
        for long in [SMALL, LARGE] {
            let before =
                helios_thumbs::read::from_bytes(whole.clone(), helios_load::Kind::Raw, long);
            let after = helios_thumbs::read::read(&path, long).unwrap();
            let size = |t: Option<&helios_thumbs::Rgb8>| t.map(|t| (t.width, t.height));
            assert_eq!(
                size(before.as_ref()),
                size(after.as_ref()),
                "{}",
                path.display()
            );
            assert!(
                before.map(|t| t.pixels) == after.map(|t| t.pixels),
                "{}: the {long} px thumbnail differs",
                path.display()
            );
        }
    }
}
