//! Which sidecars belong to which image in a folder.
//!
//! darktable writes `IMG_0001.CR2.xmp` for an image and
//! `IMG_0001_01.CR2.xmp`, `IMG_0001_02.CR2.xmp`... for its duplicates (its
//! versions of the same photo, `_NN` with two digits or more,
//! `dt_image_path_append_version`). Files copied through other systems
//! may come back with another case: `IMG_0001.cr2.xmp`, `IMG_0001.CR2.XMP`.
//!
//! Sidecars are matched by listing the folder and reading names, never by
//! a glob, and every sidecar ends up in exactly one place: the base of an
//! image (version 0), one of its duplicates, or an orphan whose image is
//! not there. A name that could be both an image's own sidecar and a
//! duplicate of another (`IMG_01.CR2.xmp` when `IMG_01.CR2` and `IMG.CR2`
//! both exist: camera names can end in `_NN` too) is settled by the
//! sidecar's `xmpMM:DerivedFrom`, else goes to the image of that exact
//! name. Two sidecars for the same version (a case variant next to the
//! original) are both kept, the one spelled as darktable spells it first.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// A sidecar of an image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SidecarFile {
    pub path: PathBuf,
    /// darktable's version of the image: 0 for the base, `NN` for the
    /// duplicate `_NN`.
    pub version: u32,
    /// Spelled exactly as darktable names it for this image; false for a
    /// case variant (`.XMP`, `.cr2.xmp` for a `.CR2`).
    pub exact: bool,
}

/// A sidecar whose image is not in its folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Orphan {
    pub path: PathBuf,
    /// The image file it is for, by its name.
    pub image: String,
    pub version: u32,
}

/// The sidecars of a folder, by image.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Matched {
    /// Every image file named, with its sidecars: the base first, then the
    /// duplicates by version; for one version, the exact name first.
    /// Images without a sidecar have an empty list.
    pub images: BTreeMap<String, Vec<SidecarFile>>,
    pub orphans: Vec<Orphan>,
}

impl Matched {
    /// The sidecar darktable reads for the image itself (version 0), if any.
    pub fn base(&self, image: &str) -> Option<&SidecarFile> {
        self.images.get(image)?.iter().find(|s| s.version == 0)
    }

    /// Versions with more than one sidecar, by image.
    pub fn conflicts(&self) -> Vec<(&str, u32)> {
        let mut out = Vec::new();
        for (image, sidecars) in &self.images {
            for pair in sidecars.windows(2) {
                if pair[0].version == pair[1].version
                    && !out.contains(&(image.as_str(), pair[0].version))
                {
                    out.push((image.as_str(), pair[0].version));
                }
            }
        }
        out
    }
}

/// `name` without a final `.xmp`, in any case.
fn sidecar_stem(name: &str) -> Option<(&str, bool)> {
    let cut = name.len().checked_sub(4)?;
    let (stem, ext) = (name.get(..cut)?, name.get(cut..)?);
    ext.eq_ignore_ascii_case(".xmp")
        .then_some((stem, ext == ".xmp"))
}

/// `IMG_0001_02.CR2` as (`IMG_0001.CR2`, 2): darktable's duplicate suffix,
/// an underscore and the version as darktable prints it (`%02d`: `_01`,
/// `_100`), before the extension. A camera's `IMG_0001` is not version 1
/// of `IMG`: darktable would have written `IMG_01`.
fn duplicate_of(stem: &str) -> Option<(String, u32)> {
    let (base, ext) = stem.rsplit_once('.')?;
    let (name, digits) = base.rsplit_once('_')?;
    if name.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let version: u32 = digits.parse().ok()?;
    (version > 0 && digits == format!("{version:02}")).then(|| (format!("{name}.{ext}"), version))
}

/// The image file a name stands for: the same name, else the one name that
/// differs from it in case only.
fn find<'a>(images: &[&'a str], name: &str) -> Option<(&'a str, bool)> {
    if let Some(exact) = images.iter().find(|i| **i == name) {
        return Some((exact, true));
    }
    let mut folded = images.iter().filter(|i| i.eq_ignore_ascii_case(name));
    match (folded.next(), folded.next()) {
        (Some(one), None) => Some((one, false)),
        _ => None,
    }
}

/// Matches the file names of one folder: `names` are every file in it,
/// `derived_from` reads a sidecar's `xmpMM:DerivedFrom` (only asked when a
/// name is ambiguous). A name ending in `.xmp` (any case) is a sidecar;
/// any other name may be an image.
pub fn match_names(
    dir: &Path,
    names: &[String],
    derived_from: impl Fn(&Path) -> Option<String>,
) -> Matched {
    let mut images: Vec<&str> = Vec::new();
    let mut sidecars: Vec<(&str, &str, bool)> = Vec::new();
    for name in names {
        match sidecar_stem(name) {
            Some((stem, lower)) => sidecars.push((name, stem, lower)),
            None => images.push(name),
        }
    }
    images.sort_unstable();
    let mut out = Matched {
        images: images
            .iter()
            .map(|i| ((*i).to_string(), Vec::new()))
            .collect(),
        orphans: Vec::new(),
    };
    sidecars.sort_unstable();
    for (name, stem, lower) in sidecars {
        let path = dir.join(name);
        let own = find(&images, stem).map(|(image, same)| (image, 0, same));
        let duplicate = duplicate_of(stem).and_then(|(original, version)| {
            find(&images, &original).map(|(image, same)| (image, version, same))
        });
        let chosen = match (own, duplicate) {
            (Some(own), Some(dup)) => {
                // Both readings name an image: the sidecar says which.
                match derived_from(&path) {
                    Some(from) if from == dup.0 && from != own.0 => Some(dup),
                    _ => Some(own),
                }
            }
            (own, dup) => own.or(dup),
        };
        if let Some((image, version, same)) = chosen {
            // `find` only returns names of `images`, which all have a list.
            if let Some(list) = out.images.get_mut(image) {
                list.push(SidecarFile {
                    path,
                    version,
                    exact: same && lower,
                });
            }
            continue;
        }
        // An orphan: the sidecar names its image best; the file name, else.
        let (image, version) = match (derived_from(&path), duplicate_of(stem)) {
            (Some(from), _) if from == stem => (from, 0),
            (Some(from), Some((original, version))) if from == original => (from, version),
            (Some(from), _) => (from, 0),
            (None, guess) => guess.unwrap_or((stem.to_string(), 0)),
        };
        out.orphans.push(Orphan {
            path,
            image,
            version,
        });
    }
    for list in out.images.values_mut() {
        list.sort_by(|a, b| (a.version, !a.exact, &a.path).cmp(&(b.version, !b.exact, &b.path)));
    }
    out
}

/// Reads a sidecar's `xmpMM:DerivedFrom`, or `None` when it cannot.
pub fn read_derived_from(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    crate::Sidecar::parse(&text).ok()?.derived_from
}

/// Lists a folder (not its subfolders) and matches its sidecars.
pub fn scan(dir: &Path) -> Result<Matched, String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        if entry.file_type().is_ok_and(|t| t.is_file())
            && let Ok(name) = entry.file_name().into_string()
        {
            names.push(name);
        }
    }
    Ok(match_names(dir, &names, read_derived_from))
}

/// The sidecars of one image file, from a listing of its folder: the base
/// first, then its duplicates.
pub fn sidecars_of(image: &Path) -> Result<Vec<SidecarFile>, String> {
    let dir = image
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = image
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{}: not a file name", image.display()))?;
    let mut matched = scan(dir)?;
    Ok(matched.images.remove(name).unwrap_or_default())
}
