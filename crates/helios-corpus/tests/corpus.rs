//! The raw corpus checks, locally: set `HELIOS_RAW_CORPUS` to a directory
//! filled by `tools/bench/fill-corpus.sh`. Without it the test is skipped,
//! saying so, and `cargo test` stays offline. The bench box runs the same
//! checks as its `raw-corpus` job.

use std::path::{Path, PathBuf};

use helios_corpus::{
    Manifest, THUMB_IMAGES, Thumb, Thumbs, check_file, judge_thumb, junit, read_thumb_image,
    read_thumbs, repo_root, run, summary,
};

#[test]
fn the_manifest_reads_and_admits_cc0_only() {
    let manifest = Manifest::read(&repo_root().join("tests/raw-corpus.toml")).unwrap();
    assert!(manifest.checked().count() >= 20);
    for f in &manifest.file {
        assert_eq!(f.licence, "CC0-1.0", "{}", f.id);
        assert!(
            ["decode", "unsupported"].contains(&f.expect.as_str()),
            "{}",
            f.id
        );
    }
    let thumbs = read_thumbs(&repo_root().join("tests/raw-corpus.thumbs.toml")).unwrap();
    for id in thumbs.keys() {
        assert!(
            manifest.checked().any(|f| &f.id == id),
            "stale thumbnail {id}"
        );
    }
}

#[test]
fn every_corpus_file_passes_its_checks() {
    let Some(dir) = std::env::var_os("HELIOS_RAW_CORPUS").map(PathBuf::from) else {
        eprintln!(
            "skipped: HELIOS_RAW_CORPUS is not set (fill a directory with \
             tools/bench/fill-corpus.sh <dir>, then set it to that directory)"
        );
        return;
    };
    let manifest = Manifest::read(&repo_root().join("tests/raw-corpus.toml")).unwrap();
    let thumbs = read_thumbs(&repo_root().join("tests/raw-corpus.thumbs.toml")).unwrap();
    let outcomes = run(&dir, &manifest, &thumbs);
    let failed: Vec<String> = outcomes
        .iter()
        .flat_map(|o| o.failures.iter().map(move |f| format!("{}: {f}", o.id)))
        .collect();
    assert!(
        failed.is_empty(),
        "{}\n{}",
        failed.join("\n"),
        summary(&outcomes)
    );
}

#[test]
fn a_missing_file_fails_and_says_so_in_the_report() {
    let manifest = Manifest::read(&repo_root().join("tests/raw-corpus.toml")).unwrap();
    let entry = manifest.checked().next().unwrap();
    let outcome = check_file(Path::new("/nonexistent"), entry, &Thumbs::new());
    assert_eq!(outcome.failures.len(), 1, "{:?}", outcome.failures);
    assert!(outcome.failures[0].starts_with("missing"));
    assert!(junit(&[outcome]).contains("<failure message=\"missing"));
}

#[test]
fn every_blessed_image_matches_its_hash() {
    let thumbs = read_thumbs(&repo_root().join("tests/raw-corpus.thumbs.toml")).unwrap();
    let dir = repo_root().join(THUMB_IMAGES);
    for (id, hash) in &thumbs {
        let image = read_thumb_image(&dir, id).unwrap_or_else(|| panic!("{id}: no blessed image"));
        assert_eq!(&image.hash(), hash, "{id}: bless again");
    }
}

fn thumb(rgb: Vec<u8>) -> Thumb {
    Thumb {
        width: rgb.len() / 3,
        height: 1,
        rgb,
    }
}

#[test]
fn a_thumbnail_is_judged_by_its_hash_then_by_its_pixels() {
    let blessed = thumb(vec![100; 3 * 1000]);
    let hash = blessed.hash();
    // the same pixels: the hash alone
    assert_eq!(judge_thumb(Some(&hash), &blessed, None), Ok(None));
    assert!(
        judge_thumb(None, &blessed, None)
            .unwrap_err()
            .contains("no expectation")
    );
    // one value off by one: within the bounds, a note to bless
    let mut close = blessed.rgb.clone();
    close[0] = 101;
    let note = judge_thumb(Some(&hash), &thumb(close.clone()), Some(&blessed))
        .unwrap()
        .expect("a note");
    assert!(note.contains("bless"), "{note}");
    // without the blessed image the hash decides
    assert!(judge_thumb(Some(&hash), &thumb(close), None).is_err());
    // a block gone dark: beyond the bounds
    let mut dark = blessed.rgb.clone();
    for v in &mut dark[..30] {
        *v = 10;
    }
    let e = judge_thumb(Some(&hash), &thumb(dark), Some(&blessed)).unwrap_err();
    assert!(e.contains("Delta E") && e.contains("max"), "{e}");
    // another size
    assert!(judge_thumb(Some(&hash), &thumb(vec![100; 6]), Some(&blessed)).is_err());
    // a blessed image out of step with its hash
    assert!(
        judge_thumb(Some(&"0".repeat(16)), &blessed, Some(&blessed))
            .unwrap_err()
            .contains("bless again")
    );
}

#[test]
fn a_thumbnail_reads_back_from_its_ppm() {
    let t = Thumb {
        width: 2,
        height: 1,
        rgb: vec![1, 2, 3, 4, 5, 6],
    };
    let back = Thumb::from_ppm(&t.ppm()).unwrap();
    assert_eq!((back.width, back.height, back.rgb), (2, 1, t.rgb.clone()));
    assert!(Thumb::from_ppm(b"P5\n1 1\n255\n\0").is_err());
    assert!(Thumb::from_ppm(b"P6\n2 2\n255\n\0").is_err(), "short");
}
