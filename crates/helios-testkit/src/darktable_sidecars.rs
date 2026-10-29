//! Real darktable sidecars from several eras, fetched when a test runs.
//!
//! Seven sidecars written by darktable itself between 2020 and 2026, from
//! darktable's integration tests (the `darktable-tests` repository, which
//! took them over from darktable's own `src/tests/integration` in January
//! 2021), check that Helios imports old edits as darktable reads them.
//! They are not part of Helios's repository: the test-data repository
//! states no licence of its own, and one of them carries its test image's
//! metadata (author, rights, GPS position). They are fetched from one
//! pinned commit of that repository, checked against their BLAKE3 hashes,
//! and kept in a cache the next runs read; offline (or without `curl`),
//! the tests that need them are skipped, saying so.
//!
//! | Name | Written by darktable | `xmp_version` | Pipe order | Image |
//! |------|----------------------|---------------|------------|-------|
//! | `0001-exposure` | 3.1, 2020-04 | 4 | legacy | `mire1.cr2` |
//! | `0004-masks` | 3.1, 2020-04 | 4 | v3.0 | `mire1.cr2` |
//! | `0033-blending-modes-uniform` | 3.2.1, 2020-11 | 4 | custom list | `mire1.cr2` |
//! | `0035-multiple-modules` | 3.3, 2020-11 | 4 | v3.0 | `mire1.cr2` |
//! | `0097-sigmoid` | 4.1, 2022-11 | 5 | v3.0 | `mire1.cr2` |
//! | `0151-sigmoid-no-hue-preservation` | 4.5, 2023-08 | 5 | v3.0 | `mire1.cr2` |
//! | `0180-spektrafilm-simple` | 5.7 (development), 2026-10 | 5 | v5.0 | `hlrecovery.arw` |
//!
//! What each one covers:
//!
//! - `0001-exposure`: the oldest format here; old params versions migrated
//!   (temperature 3, highlights 2, exposure 5), darktable's legacy order.
//! - `0004-masks`: no highlights in the history, so darktable adds clip
//!   highlights when it reads it; an exposure with a drawn mask, a mask
//!   manager item.
//! - `0033-blending-modes-uniform`: a custom `iop_order_list`, five
//!   exposure instances of which four blend, two colorbalance instances.
//! - `0035-multiple-modules`: many modules Helios does not have (liquify,
//!   retouch, filmic, borders...), all kept.
//! - `0097-sigmoid`, `0151-sigmoid-no-hue-preservation`: the 4.x format,
//!   sigmoid version 1 migrated, a module edited twice, a disabled filmic.
//! - `0180-spektrafilm-simple`: the 5.x format and order, modules from
//!   darktable's scene-referred workflow presets (`_builtin_` instances).
//!
//! The raws they are for are darktable's test images, not fetched here
//! (15 to 21 MB each): the render tests read them from a darktable
//! checkout and skip without them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// The repository the sidecars come from, on GitHub.
pub const REPOSITORY: &str = "darktable-org/darktable-tests";

/// The commit they are read at: every one is byte for byte as darktable
/// last wrote it there.
pub const COMMIT: &str = "3eaf0e640537fdefc01e5a05e9c35ce198b99c2f";

/// A sidecar of the corpus.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// Its name in the tests: darktable's test's folder.
    pub name: &'static str,
    /// Its path in [`REPOSITORY`].
    pub path: &'static str,
    /// The BLAKE3 hash of its bytes, in hex.
    pub blake3: &'static str,
    /// The darktable test image it was written for.
    pub image: &'static str,
}

/// The corpus, oldest first.
pub const CORPUS: &[Entry] = &[
    Entry {
        name: "0001-exposure",
        path: "0001-exposure/exposure.xmp",
        blake3: "7edc76fc6b90685d0e72522b0e137f48d62da722a8fd0e7e2ec3672e0dced24f",
        image: "mire1.cr2",
    },
    Entry {
        name: "0004-masks",
        path: "0004-masks/masks.xmp",
        blake3: "d8dd875f4a3a073701ba247db20b12d0a81fef8f17650fb48a79ea0b73d1174f",
        image: "mire1.cr2",
    },
    Entry {
        name: "0033-blending-modes-uniform",
        path: "0033-blending-modes-uniform/blending-modes-uniform.xmp",
        blake3: "bd0e04c8bbe96fb8bbb7ac451aab524ca12e9485d1c32a3fc707ebb0661852d5",
        image: "mire1.cr2",
    },
    Entry {
        name: "0035-multiple-modules",
        path: "0035-multiple-modules/multiple-modules.xmp",
        blake3: "ce166bd79eb7becaef9b7f493337e56c2fdb63369a97521ea089bc19a6dc9e58",
        image: "mire1.cr2",
    },
    Entry {
        name: "0097-sigmoid",
        path: "0097-sigmoid/sigmoid.xmp",
        blake3: "c6a0e24b75d0967f39eb88d7512c9a6235629c70f9be8396aba3cbbda8d3fdad",
        image: "mire1.cr2",
    },
    Entry {
        name: "0151-sigmoid-no-hue-preservation",
        path: "0151-sigmoid-no-hue-preservation/sigmoid-no-hue-preservation.xmp",
        blake3: "a147196a9c1ed023844bafa1e64843d0dc62516b985e3c440b951c5379e0c5df",
        image: "mire1.cr2",
    },
    Entry {
        name: "0180-spektrafilm-simple",
        path: "0180-spektrafilm-simple/spektrafilm-simple.xmp",
        blake3: "3b71807ccd8e22983b0896b06db97ecd778cfccf6ed2a953d5c9a26b6b6fbb8d",
        image: "hlrecovery.arw",
    },
];

/// A sidecar of the corpus, read.
#[derive(Clone, Debug)]
pub struct Fetched {
    pub entry: Entry,
    pub text: String,
}

/// Numbers the partial files this process writes into the cache.
static PART: AtomicU64 = AtomicU64::new(0);

fn blake3_hex(bytes: &[u8]) -> String {
    helios_core::ContentHash::of(bytes).to_string()
}

/// Where a sidecar is kept in `cache`.
fn cached(cache: &Path, entry: &Entry) -> PathBuf {
    cache
        .join(&COMMIT[..12])
        .join(format!("{}.xmp", entry.name))
}

/// The sidecar's bytes from GitHub, or why not (offline, no `curl`).
fn download(entry: &Entry) -> Result<Vec<u8>, String> {
    let url = format!(
        "https://raw.githubusercontent.com/{REPOSITORY}/{COMMIT}/{}",
        entry.path
    );
    let out = Command::new("curl")
        .args(["-sSfL", "--connect-timeout", "10", "--max-time", "60"])
        .args(["--retry", "2"])
        .arg(&url)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// Every sidecar of the corpus, from `cache` or fetched into it, each
/// checked against its hash; `None`, saying why on stderr, when one cannot
/// be fetched (the test is then skipped). A fetched file whose hash is not
/// the pinned one panics: that is a broken pin, not a missing network.
///
/// Tests pass their crate's `CARGO_TARGET_TMPDIR`, so the cache lives with
/// the build and every crate shares it.
///
/// # Panics
///
/// When a fetched sidecar's hash is not its pinned one, or the cache
/// cannot be written.
pub fn fetch(cache: &Path) -> Option<Vec<Fetched>> {
    let mut out = Vec::new();
    for entry in CORPUS {
        let path = cached(cache, entry);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) if blake3_hex(&bytes) == entry.blake3 => bytes,
            _ => {
                let bytes = match download(entry) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        eprintln!("skipped: the darktable sidecar corpus cannot be fetched: {e}");
                        return None;
                    }
                };
                let hash = blake3_hex(&bytes);
                assert_eq!(
                    hash, entry.blake3,
                    "{}: fetched with another hash than the pinned one",
                    entry.name
                );
                let dir = path.parent().expect("a folder");
                std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
                // Written aside, then renamed: a test reading the cache
                // meanwhile never sees half a file.
                let part = dir.join(format!(
                    ".{}.{}-{}.part",
                    entry.name,
                    std::process::id(),
                    PART.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::write(&part, &bytes)
                    .and_then(|()| std::fs::rename(&part, &path))
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                bytes
            }
        };
        let text =
            String::from_utf8(bytes).unwrap_or_else(|e| panic!("{}: not UTF-8: {e}", entry.name));
        out.push(Fetched {
            entry: *entry,
            text,
        });
    }
    Some(out)
}
