//! RAW+JPEG pairs (`docs/design/raw-jpeg-pairs.md`): a camera writing a
//! raw and a JPEG (or HEIF) of one shot gives two files, which Helios
//! makes one photo, the raw its master and the JPEG its `camera` file
//! ([`helios_vault::pairs`] merges them).
//!
//! Two photos are a pair when a raw and a JPEG are in the same folder
//! and:
//! - their names are the same but for the extension (`IMG_0001.CR3` and
//!   `IMG_0001.JPG`, case ignored), as the import window shows them as
//!   one cell; or
//! - their names differ (renamed copies) but the camera's clock says the
//!   same moment (within half a second) and the same camera, with no
//!   other raw or JPEG of that folder at that moment to mistake for it.
//!
//! "Pair RAW+JPEG Files" ([`pair`]) finds the pairs among photos, or the
//! whole Library (the health check's offer, [`count`]), or pairs exactly
//! two photos when told to (a raw and a JPEG whatever their names); one
//! step. Unpair is the vault's ([`helios_vault::Vault::unpair`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use helios_vault::pairs::Paired;
use helios_vault::{Changes, ImageId, Query, Store, Vault};

use crate::import_profile::Kind;
use crate::signals::Signals;

/// Two captures closer than this are the same moment, in seconds.
const SAME_MOMENT: f64 = 0.5;

/// What a file is to pairing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// A camera raw: the master.
    Raw,
    /// The camera's JPEG or HEIF: the `camera` file.
    Camera,
}

/// What `path` can be in a pair, by its extension.
pub fn side_of(path: &Path) -> Option<Side> {
    match Kind::of(path) {
        Kind::Raw => Some(Side::Raw),
        Kind::Jpeg => Some(Side::Camera),
        Kind::Video => None,
        Kind::Other => {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            matches!(ext.as_str(), "heic" | "heif" | "hif").then_some(Side::Camera)
        }
    }
}

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

/// The pairs among `candidates`: each a raw and a JPEG or HEIF
/// (`(raw, jpeg)`), each image in one pair at most, in the order the raws
/// come.
pub fn find(candidates: &[Candidate]) -> Vec<(ImageId, ImageId)> {
    type Sides<'a> = (Vec<&'a Candidate>, Vec<&'a Candidate>);
    let mut by_folder: BTreeMap<&Path, Sides> = BTreeMap::new();
    for c in candidates {
        let entry = by_folder.entry(c.folder()).or_default();
        match side_of(&c.path) {
            Some(Side::Raw) => entry.0.push(c),
            Some(Side::Camera) => entry.1.push(c),
            None => {}
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
/// its signals. Images that are a photo's file, or have files of their
/// own, are left out: they are paired already.
///
/// # Errors
/// The vault cannot be read.
pub fn candidates<S: Store>(
    vault: &Vault<S>,
    images: &[ImageId],
) -> Result<Vec<Candidate>, String> {
    let mut out = Vec::new();
    for image in images {
        if vault.photo_of(image)?.is_some() || vault.files(image)?.len() > 1 {
            continue;
        }
        let signals: Option<Signals> = vault
            .signals(image)?
            .and_then(|v| serde_json::from_value(v).ok());
        for location in vault.locations(image)? {
            out.push(Candidate::new(*image, location.path, signals.as_ref()));
        }
    }
    Ok(out)
}

/// Every photo of the Library.
///
/// # Errors
/// The vault cannot be read.
pub fn library_photos<S: Store>(vault: &Vault<S>) -> Result<Vec<ImageId>, String> {
    let cursor = vault.open_cursor(Query::default())?;
    let ids = vault.cursor_ids(cursor.id, 0, cursor.count);
    vault.close_cursor(cursor.id);
    ids
}

/// How many pairs the Library shows as two photos: the health check's
/// offer.
///
/// # Errors
/// The vault cannot be read.
pub fn count<S: Store>(vault: &Vault<S>) -> Result<usize, String> {
    let photos = library_photos(vault)?;
    Ok(find(&candidates(vault, &photos)?).len())
}

/// What a photo is to pairing, by its files' extensions.
fn side_of_image<S: Store>(vault: &Vault<S>, image: &ImageId) -> Result<Option<Side>, String> {
    Ok(vault
        .locations(image)?
        .iter()
        .find_map(|l| side_of(&l.path)))
}

/// Two photos paired as they are: a raw and a JPEG or HEIF, in either
/// order, whatever their names.
fn exact<S: Store>(vault: &Vault<S>, images: &[ImageId]) -> Result<(ImageId, ImageId), String> {
    let [a, b] = images else {
        return Err(format!(
            "pairing photos as they are takes two photos, not {}",
            images.len()
        ));
    };
    match (side_of_image(vault, a)?, side_of_image(vault, b)?) {
        (Some(Side::Raw), Some(Side::Camera)) => Ok((*a, *b)),
        (Some(Side::Camera), Some(Side::Raw)) => Ok((*b, *a)),
        _ => Err("a pair is a raw and a JPEG or HEIF".into()),
    }
}

/// "Pair RAW+JPEG Files": the pairs found among `images` (the whole
/// Library when none) made one photo each, as the vault merges them; one
/// step. With `exact`, the two photos given are paired as they are;
/// without it, two photos with no pair found among them are too. Answers
/// the pairs made and the step.
///
/// # Errors
/// No pair found, or one the vault refuses (nothing is written then).
pub fn pair<S: Store>(
    vault: &Vault<S>,
    images: Option<&[ImageId]>,
    exact_pair: bool,
    who: &str,
    now: u64,
) -> Result<(Vec<Paired>, Changes), String> {
    let all;
    let images = match images {
        Some(ids) => ids,
        None => {
            all = library_photos(vault)?;
            &all
        }
    };
    let pairs = if exact_pair {
        vec![exact(vault, images)?]
    } else {
        let found = find(&candidates(vault, images)?);
        if found.is_empty() && images.len() == 2 {
            vec![exact(vault, images)?]
        } else {
            found
        }
    };
    if pairs.is_empty() {
        return Err("no RAW+JPEG pair among the photos".into());
    }
    vault.pair(&pairs, who, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::ContentHash;
    use helios_vault::{Image, Location, MemoryStore};

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
        // A HEIF is a camera file too.
        let found = find(&[at(1, "/c/IMG_1.CR3"), at(2, "/c/IMG_1.HIF")]);
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

    #[test]
    fn sides_follow_the_extension() {
        assert_eq!(side_of(Path::new("a.NEF")), Some(Side::Raw));
        assert_eq!(side_of(Path::new("a.jpeg")), Some(Side::Camera));
        assert_eq!(side_of(Path::new("a.heic")), Some(Side::Camera));
        assert_eq!(side_of(Path::new("a.png")), None);
        assert_eq!(side_of(Path::new("a.mov")), None);
    }

    /// A vault of photos at `paths`, each in the Library.
    fn vault(paths: &[&str]) -> Vault<MemoryStore> {
        let mut store = MemoryStore::default();
        for (n, p) in paths.iter().enumerate() {
            let n = u8::try_from(n + 1).unwrap();
            store
                .add_to_library(&Image {
                    id: id(n),
                    size: 10,
                    partial: ContentHash([n; 32]),
                })
                .unwrap();
            store
                .put_location(&id(n), &Location::seen(PathBuf::from(p), 10, 1, 1))
                .unwrap();
        }
        Vault::new(store)
    }

    #[test]
    fn the_library_s_pairs_are_counted_and_paired_in_one_step() {
        let v = vault(&["/c/A.CR3", "/c/A.JPG", "/c/B.CR3", "/c/B.JPG", "/c/C.JPG"]);
        assert_eq!(count(&v).unwrap(), 2);
        let (made, step) = pair(&v, None, false, "user", 5).unwrap();
        assert_eq!(made.len(), 2);
        assert_eq!(made[0].photo, id(1).hex());
        assert_eq!(made[0].file, id(2).hex());
        assert_eq!(count(&v).unwrap(), 0, "paired: nothing left to offer");
        assert!(
            pair(&v, None, false, "user", 6).is_err(),
            "nothing left to pair"
        );
        v.revert(&step).unwrap();
        assert_eq!(count(&v).unwrap(), 2, "undone: two pairs again");
    }

    #[test]
    fn two_photos_are_paired_as_they_are_or_refused() {
        let v = vault(&["/raw/a.nef", "/jpg/x.jpg", "/jpg/y.jpg"]);
        // No pair by the rules: two photos given are paired as they are,
        // the raw the master whatever the order.
        let (made, _) = pair(&v, Some(&[id(2), id(1)]), false, "user", 5).unwrap();
        assert_eq!(made[0].photo, id(1).hex());
        // Two JPEGs are no pair; three photos are not two.
        let v = vault(&["/raw/a.nef", "/jpg/x.jpg", "/jpg/y.jpg"]);
        assert!(pair(&v, Some(&[id(2), id(3)]), true, "user", 5).is_err());
        assert!(pair(&v, Some(&[id(1), id(2), id(3)]), true, "user", 5).is_err());
        assert!(pair(&v, Some(&[id(1), id(2), id(3)]), false, "user", 5).is_err());
    }
}
