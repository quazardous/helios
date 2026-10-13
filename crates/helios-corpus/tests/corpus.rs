//! The raw corpus checks, locally: set `HELIOS_RAW_CORPUS` to a directory
//! filled by `tools/bench/fill-corpus.sh`. Without it the test is skipped,
//! saying so, and `cargo test` stays offline. The bench box runs the same
//! checks as its `raw-corpus` job.

use std::path::{Path, PathBuf};

use helios_corpus::{Manifest, Thumbs, check_file, junit, read_thumbs, repo_root, run, summary};

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
