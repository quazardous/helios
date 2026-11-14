//! RAW+JPEG pairs stacked: a camera writing both files of one shot gives
//! two images the Library shows as one stack, the raw on top (Lightroom's
//! "Treat JPEG files next to raw files as separate photos" left off,
//! darktable's grouping at import).
//!
//! Two images are a pair when a raw and a JPEG are in the same folder
//! and:
//! - their names are the same but for the extension (`IMG_0001.CR3` and
//!   `IMG_0001.JPG`, case ignored), as the import window shows them as
//!   one cell; or
//! - their names differ (renamed copies) but the camera's clock says the
//!   same moment (within half a second) and the same camera, with no
//!   other raw or JPEG of that folder at that moment to mistake for it.
//!
//! An import stacks the pairs it brings in when its option is on
//! ([`crate::import::Options::stack_pairs`], on by default), as part of
//! its own undo step; "Stack RAW+JPEG Pairs" does it on photos imported
//! before ([`stack_pairs`], one step). A photo already in a stack is
//! left as it is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use helios_vault::{Changes, ImageId, Stack, Store, Vault};

use crate::import_profile::Kind;
use crate::signals::Signals;

/// Who a stack made by an import says made it.
pub const IMPORT: &str = "import";

/// Two captures closer than this are the same moment, in seconds.
const SAME_MOMENT: f64 = 0.5;

/// A file of an image, as pairing looks at it.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub image: ImageId,
    pub path: PathBuf,
    /// When it was taken (the signals' time, read from EXIF only).
    pub taken: Option<f64>,
    /// `maker model`.
    pub camera: Option<String>,
}

impl Candidate {
    /// A file of `image` at `path`, with what its signals say.
    pub fn new(image: ImageId, path: PathBuf, signals: Option<&Signals>) -> Self {
        let exif = signals.filter(|s| s.taken_from.as_deref() == Some("exif"));
        Candidate {
            image,
            path,
            taken: exif.and_then(|s| s.taken),
            camera: exif.and_then(Signals::camera),
        }
    }

    fn kind(&self) -> Kind {
        Kind::of(&self.path)
    }

    fn folder(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new(""))
    }

    fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }

    fn same_moment(&self, other: &Candidate) -> bool {
        match (self.taken, other.taken, &self.camera, &other.camera) {
            (Some(a), Some(b), Some(c), Some(d)) => (a - b).abs() < SAME_MOMENT && c == d,
            _ => false,
        }
    }
}

/// The pairs among `candidates`: each a raw and a JPEG (`(raw, jpeg)`),
/// each image in one pair at most, in the order the raws come.
pub fn find(candidates: &[Candidate]) -> Vec<(ImageId, ImageId)> {
    let mut by_folder: BTreeMap<&Path, (Vec<&Candidate>, Vec<&Candidate>)> = BTreeMap::new();
    for c in candidates {
        let entry = by_folder.entry(c.folder()).or_default();
        match c.kind() {
            Kind::Raw => entry.0.push(c),
            Kind::Jpeg => entry.1.push(c),
            Kind::Video | Kind::Other => {}
        }
    }
    let mut pairs = Vec::new();
    let mut used: Vec<ImageId> = Vec::new();
    for (raws, jpegs) in by_folder.values() {
        // By name first.
        for raw in raws {
            if used.contains(&raw.image) {
                continue;
            }
            let stem = raw.stem();
            if let Some(jpeg) = jpegs
                .iter()
                .find(|j| !used.contains(&j.image) && j.image != raw.image && j.stem() == stem)
            {
                used.extend([raw.image, jpeg.image]);
                pairs.push((raw.image, jpeg.image));
            }
        }
        // Then by the moment and the camera, when only one of each fits.
        for raw in raws {
            if used.contains(&raw.image) {
                continue;
            }
            let fits: Vec<&&Candidate> = jpegs
                .iter()
                .filter(|j| !used.contains(&j.image) && j.same_moment(raw))
                .collect();
            let [jpeg] = fits.as_slice() else {
                continue;
            };
            let rivals = raws
                .iter()
                .filter(|r| !used.contains(&r.image) && r.same_moment(jpeg))
                .count();
            if rivals == 1 && jpeg.image != raw.image {
                used.extend([raw.image, jpeg.image]);
                pairs.push((raw.image, jpeg.image));
            }
        }
    }
    pairs
}

/// The candidates of `images`: each file the vault knows of each, with
/// its signals.
///
/// # Errors
/// The vault cannot be read.
pub fn candidates<S: Store>(
    vault: &Vault<S>,
    images: &[ImageId],
) -> Result<Vec<Candidate>, String> {
    let mut out = Vec::new();
    for image in images {
        let signals: Option<Signals> = vault
            .signals(image)?
            .and_then(|v| serde_json::from_value(v).ok());
        for location in vault.locations(image)? {
            out.push(Candidate::new(*image, location.path, signals.as_ref()));
        }
    }
    Ok(out)
}

/// Stacks the pairs among `candidates` whose images are in no stack yet,
/// each raw on top, made by `who`. Answers the stacks made and the step
/// ([`Changes`]) the vault's revert undoes.
///
/// # Errors
/// The vault cannot be read or written.
pub fn stack<S: Store>(
    vault: &Vault<S>,
    candidates: &[Candidate],
    who: &str,
    now: u64,
) -> Result<(Vec<Stack>, Changes), String> {
    let mut stacks = Vec::new();
    let mut changes = Changes::default();
    for (raw, jpeg) in find(candidates) {
        if vault.stack_of(&raw)?.is_some() || vault.stack_of(&jpeg)?.is_some() {
            continue;
        }
        let (stack, step, _) = vault.group_stack(&[raw, jpeg], Some(raw), who, now)?;
        changes.images.extend(step.images);
        changes.documents.extend(step.documents);
        stacks.push(stack);
    }
    Ok((stacks, changes))
}

/// "Stack RAW+JPEG Pairs" on `images`: the pairs among them stacked, as
/// [`stack`] does; one step.
///
/// # Errors
/// The vault cannot be read or written.
pub fn stack_pairs<S: Store>(
    vault: &Vault<S>,
    images: &[ImageId],
    who: &str,
    now: u64,
) -> Result<(Vec<Stack>, Changes), String> {
    stack(vault, &candidates(vault, images)?, who, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::ContentHash;

    fn id(n: u8) -> ImageId {
        ImageId(ContentHash([n; 32]))
    }

    fn at(n: u8, path: &str) -> Candidate {
        Candidate::new(id(n), PathBuf::from(path), None)
    }

    fn timed(n: u8, path: &str, taken: f64, model: &str) -> Candidate {
        let signals = Signals {
            taken: Some(taken),
            taken_from: Some("exif".into()),
            model: Some(model.into()),
            ..Signals::default()
        };
        Candidate::new(id(n), PathBuf::from(path), Some(&signals))
    }

    #[test]
    fn pairs_are_found_by_name_then_by_moment_and_camera() {
        // Same stem, same folder, case ignored; another folder's is not.
        let found = find(&[
            at(1, "/c/IMG_1.CR3"),
            at(2, "/c/img_1.jpg"),
            at(3, "/c/IMG_2.JPG"),
            at(4, "/d/IMG_2.CR3"),
        ]);
        assert_eq!(found, vec![(id(1), id(2))]);
        // Renamed: the same moment and camera pair them.
        let found = find(&[
            timed(1, "/l/2026-1.cr3", 100.0, "R5"),
            timed(2, "/l/2026-2.jpg", 100.2, "R5"),
            timed(3, "/l/2026-3.jpg", 100.2, "Z 8"),
        ]);
        assert_eq!(found, vec![(id(1), id(2))]);
        // Two JPEGs at that moment: no guess.
        let found = find(&[
            timed(1, "/l/a.cr3", 100.0, "R5"),
            timed(2, "/l/b.jpg", 100.1, "R5"),
            timed(3, "/l/c.jpg", 100.2, "R5"),
        ]);
        assert!(found.is_empty());
        // No time read from the file: no pair by moment.
        assert!(find(&[at(1, "/l/a.cr3"), at(2, "/l/b.jpg")]).is_empty());
    }
}
