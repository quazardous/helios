//! darktable's sidecars kept in a vault: each photo's base sidecar and its
//! duplicates (`IMG_0001_01.CR2.xmp`...) become versions of the one image,
//! side by side, and sidecars whose image is missing are reported.
//!
//! A darktable duplicate is an edit variant of the same file, which is what
//! a Helios version is: each sidecar becomes a first edit of the image
//! (no parents), so the image has one head per sidecar, none merged into
//! another. The edit records darktable's version number
//! (`darktable.duplicate`): two duplicates with the same history stay two
//! versions, and importing the same sidecar again changes nothing. Only the
//! base sidecar gives the image its rating.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use helios_xmp::files::{Matched, SidecarFile};
use serde_json::{Value, json};

/// Who imported a sidecar: the origin of its version and the author of
/// its rating in the audit trail.
pub const IMPORT_XMP: &str = "import:xmp";

/// Reads and imports a sidecar's text once, for the network and the vault.
pub fn read_sidecar(xml: &str) -> Result<(helios_xmp::Sidecar, helios_xmp::Imported), String> {
    helios_xmp::import_text(xml, &helios_xmp::Layouts::built_in())
}

/// The label of a sidecar's version: darktable's duplicate number and the
/// name darktable shows for it, if any.
fn label(version: u32, name: Option<&str>) -> String {
    let duplicate = if version > 0 {
        format!(" duplicate {version:02}")
    } else {
        String::new()
    };
    match name.filter(|n| !n.is_empty()) {
        Some(name) => format!("{IMPORT_XMP}{duplicate} ({name})"),
        None => format!("{IMPORT_XMP}{duplicate}"),
    }
}

/// Keeps a darktable sidecar's import in a vault: the image file, its edit
/// as a version with origin `import:xmp` (the network
/// [`helios_xmp::Imported::edit`] describes, with the import report under
/// `import` and the darktable edit, items not applied included, under
/// `darktable`), darktable's duplicate number `version` (0 for the base
/// sidecar), and, for the image's `base` sidecar only, its rating, only
/// when the vault has none: a sidecar never overwrites a rating chosen
/// here (darktable's `-1` is a reject). The version is a first edit, so
/// importing the same sidecar again keeps one version; an image already
/// edited gets another head, its edit untouched.
pub fn keep_sidecar<S: helios_vault::Store>(
    vault: &helios_vault::Vault<S>,
    image: &Path,
    version: u32,
    base: bool,
    sidecar: &helios_xmp::Sidecar,
    imported: &helios_xmp::Imported,
    now: u64,
) -> Result<Value, String> {
    use helios_vault::fields::{Command, Op, RATING, REJECTED};
    let id = vault.import_file(image, now)?;
    let mut edit = imported.edit();
    edit["import"] = json!(imported.report);
    // Duplicates with the same history stay apart.
    edit["darktable"]["duplicate"] = json!(version);
    let kept = helios_vault::Version::new(id, Vec::new(), edit, IMPORT_XMP, now)
        .labelled(&label(version, sidecar.version_name.as_deref()));
    let added = vault.add_version(&kept)?;
    let rating = match sidecar.rating {
        _ if !base => None,
        Some(-1) => Some(Command::with(id, REJECTED, Op::SetIfAbsent(json!(true)))),
        Some(stars @ 0..=5) => Some(Command::with(id, RATING, Op::SetIfAbsent(json!(stars)))),
        _ => None,
    };
    let rating_set = match rating {
        Some(command) => vault.apply(&[command], IMPORT_XMP, None, now)?.changed > 0,
        None => false,
    };
    Ok(json!({
        "image": id.hex(),
        "version": kept.id.hex(),
        "label": kept.label,
        "added": added,
        "rating": sidecar.rating,
        "rating_set": rating_set,
    }))
}

/// Folder listings, read once per folder, then matched (see
/// [`helios_xmp::files`]).
#[derive(Default)]
pub struct Listings(BTreeMap<PathBuf, Matched>);

impl Listings {
    /// The folder of `image`, matched.
    pub fn folder(&mut self, dir: &Path) -> Result<&Matched, String> {
        if !self.0.contains_key(dir) {
            let matched = helios_xmp::files::scan(dir)?;
            self.0.insert(dir.to_path_buf(), matched);
        }
        Ok(&self.0[dir])
    }

    /// The sidecars of an image file: its base first, then its duplicates.
    pub fn sidecars(&mut self, image: &Path) -> Result<Vec<SidecarFile>, String> {
        let dir = image
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = image
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("{}: not a file name", image.display()))?;
        Ok(self
            .folder(dir)?
            .images
            .get(name)
            .cloned()
            .unwrap_or_default())
    }
}

/// Reads, imports and keeps one sidecar file of `image`; the JSON says
/// what was kept, or the error. `warn` gets the import's summary once.
pub fn keep_file<S: helios_vault::Store>(
    vault: &helios_vault::Vault<S>,
    image: &Path,
    file: &SidecarFile,
    base: bool,
    now: u64,
    warn: &mut impl FnMut(&Path, &str),
) -> Value {
    let mut out = json!({ "path": file.path, "version": file.version, "exact": file.exact });
    let kept = std::fs::read_to_string(&file.path)
        .map_err(|e| format!("cannot read {}: {e}", file.path.display()))
        .and_then(|xml| {
            let (sidecar, imported) = read_sidecar(&xml)?;
            if let Some(warning) = imported.report.summary() {
                warn(&file.path, &warning.english);
                out["warning"] = json!(warning.english);
                out["warning_message"] = json!(warning);
            }
            keep_sidecar(vault, image, file.version, base, &sidecar, &imported, now)
        });
    match kept {
        Ok(vault) => out["vault"] = vault,
        Err(e) => out["error"] = json!(e),
    }
    out
}

/// Imports darktable sidecars into a vault: each path is a folder (its
/// files, not its subfolders) or an image file. Every image file is
/// imported, with a version per sidecar; one JSON line per image goes to
/// `emit` (`image`, `sidecars`, each with its `version` and what was kept
/// or the error), then one per sidecar whose image is missing
/// (`orphan`, the `image` it is for, its `version`). Returns whether every
/// sidecar was kept.
pub fn import<S: helios_vault::Store>(
    vault: &helios_vault::Vault<S>,
    paths: &[PathBuf],
    now: u64,
    emit: &mut impl FnMut(&Value),
    warn: &mut impl FnMut(&Path, &str),
) -> Result<bool, String> {
    let mut listings = Listings::default();
    let mut all_kept = true;
    for path in paths {
        let path = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (dir, only) = if path.is_dir() {
            (path.clone(), None)
        } else {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| format!("{}: not a file name", path.display()))?
                .to_string();
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            (dir, Some(name))
        };
        let matched = listings.folder(&dir)?.clone();
        if let Some(only) = &only
            && !matched.images.contains_key(only)
        {
            return Err(format!("{}: no such file", path.display()));
        }
        for (name, files) in &matched.images {
            if only.as_ref().is_some_and(|o| o != name) || !is_image(name) {
                continue;
            }
            let image = dir.join(name);
            let id = vault.import_file(&image, now)?;
            let sidecars: Vec<Value> = files
                .iter()
                .enumerate()
                .map(|(i, file)| {
                    let base = i == 0 && file.version == 0;
                    keep_file(vault, &image, file, base, now, warn)
                })
                .collect();
            all_kept &= sidecars.iter().all(|s| s["error"].is_null());
            let conflicts: Vec<u32> = matched
                .conflicts()
                .iter()
                .filter(|(i, _)| i == name)
                .map(|(_, v)| *v)
                .collect();
            emit(&json!({
                "image": image,
                "id": id.hex(),
                "sidecars": sidecars,
                "conflicts": conflicts,
            }));
        }
        if only.is_none() {
            for orphan in &matched.orphans {
                emit(&json!({
                    "orphan": orphan.path,
                    "image": orphan.image,
                    "version": orphan.version,
                }));
            }
        }
    }
    Ok(all_kept)
}

/// A file the vault imports (`helios_vault::IMAGE_EXTENSIONS`).
fn is_image(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| helios_vault::IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duplicate_is_labelled_by_its_number_and_name() {
        assert_eq!(label(0, None), "import:xmp");
        assert_eq!(label(2, Some("bw")), "import:xmp duplicate 02 (bw)");
        assert_eq!(label(1, Some("")), "import:xmp duplicate 01");
    }
}
