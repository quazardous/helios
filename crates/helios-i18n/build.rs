//! Embeds the repository's translations (`i18n/<locale>/*.ftl`, English
//! excepted: English is the text written in the code) so a build carries
//! its catalogs wherever it is installed.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    paths.sort();
    paths
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let root = manifest.join("../../i18n");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut out = String::from("pub(crate) const BUILTIN: &[(&str, &str, &str)] = &[\n");
    for dir in sorted(&root).into_iter().filter(|p| p.is_dir()) {
        let locale = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if locale == "en" {
            continue;
        }
        for file in sorted(&dir) {
            if file.extension().is_some_and(|e| e == "ftl") {
                let name = file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                let path = file.canonicalize().expect("a catalog's path");
                writeln!(
                    out,
                    "    ({locale:?}, {name:?}, include_str!({:?})),",
                    path.display().to_string()
                )
                .expect("write to a string");
            }
        }
    }
    out.push_str("];\n");
    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets it")).join("builtin.rs");
    std::fs::write(dest, out).expect("write the embedded catalogs");
}
