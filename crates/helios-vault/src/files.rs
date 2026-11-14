//! A photo's files (`docs/design/raw-jpeg-pairs.md`): one photo, several
//! files, each with a role.
//!
//! A photo's id is its **master** file's content id (the RAW, or the only
//! file). Its other files (the camera's JPEG or HEIF now, variants and
//! attachments later) are images of the vault, off the Library on their
//! own, each tied to the photo by a row: the image, its photo, its role,
//! its rank among the files of that role, and when and by whom it was
//! attached. The master has no row: a photo with none is a one-file photo.
//!
//! - A file belongs to one photo at most, and a photo's file is never a
//!   master itself (the vault refuses both).
//! - [`Vault::attach_file`] and [`Vault::detach_file`] change one row in
//!   one transaction, with an audit row on the file, and return the step
//!   ([`Changes`]) that [`Vault::revert`] undoes and [`Vault::replay`]
//!   redoes: the row rides in the file's [`crate::ImageRows`].
//! - [`Vault::holds`] answers "is this file held by the Library": in it,
//!   or a file of a photo in it. Every such question asks it, never the
//!   Library flag alone.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::undo::{Change, rows_of};
use crate::{AuditEntry, Changes, ImageId, Store, Vault};

/// The audit field a file's attaching and detaching are recorded under,
/// on the file: its row before and after (`null` when none).
pub const PHOTO_FILE: &str = "photo_file";

/// What a file is to its photo, in the order a photo's files are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    /// The file the photo is: its id, its edits and versions. Never stored.
    Master,
    /// The camera's own JPEG or HEIF of the same shot.
    Camera,
    /// Another in-camera rendering (a crop, a picture style).
    Variant,
    /// Anything else the shot carries (a voice memo, a GPS track).
    Attachment,
}

impl FileRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Camera => "camera",
            Self::Variant => "variant",
            Self::Attachment => "attachment",
        }
    }

    pub fn parse(word: &str) -> Result<Self, String> {
        match word {
            "master" => Ok(Self::Master),
            "camera" => Ok(Self::Camera),
            "variant" => Ok(Self::Variant),
            "attachment" => Ok(Self::Attachment),
            other => Err(format!("`{other}` is not a file role")),
        }
    }
}

/// A file of a photo: one `photo_files` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoFile {
    /// The file (an image of the vault).
    pub image: ImageId,
    /// The photo it belongs to: its master's id.
    pub photo: ImageId,
    pub role: FileRole,
    /// Its place among the photo's files of the same role, from 0.
    pub rank: u32,
    /// When it was attached, and by what: `import`, `user`, `pair:<step>`.
    pub added_at: u64,
    pub added_by: String,
}

impl PhotoFile {
    /// The order a photo's files are listed in: by photo, then role, rank
    /// and image.
    pub fn order(&self) -> (ImageId, FileRole, u32, ImageId) {
        (self.photo, self.role, self.rank, self.image)
    }

    /// Sorts rows in [`PhotoFile::order`].
    pub fn sort(rows: &mut [PhotoFile]) {
        rows.sort_by_key(PhotoFile::order);
    }

    /// The row as its audit entry records it.
    fn audited(&self) -> Value {
        json!({
            "photo": self.photo.hex(),
            "role": self.role.as_str(),
            "rank": self.rank,
        })
    }
}

/// Whether `image` is held by the Library: in it, or a file of a photo in
/// it.
pub(crate) fn holds<S: Store + ?Sized>(store: &S, image: &ImageId) -> Result<bool, String> {
    let on = |id: &ImageId| -> Result<bool, String> {
        Ok(store.library_flag(id)?.is_some_and(|f| f.in_library))
    };
    if on(image)? {
        return Ok(true);
    }
    match store.photo_of(image)? {
        Some(row) => on(&row.photo),
        None => Ok(false),
    }
}

impl<S: Store> Vault<S> {
    /// Whether `image` is held by the Library: in it, or a file of a
    /// photo in it. Every "is this file held" question asks this, never
    /// the Library flag alone.
    pub fn holds(&self, image: &ImageId) -> Result<bool, String> {
        self.read(|store| holds(store, image))
    }

    /// The photo `image` is a file of, with its row; none for a master or
    /// a file of no photo.
    pub fn photo_of(&self, image: &ImageId) -> Result<Option<PhotoFile>, String> {
        self.read(|store| store.photo_of(image))
    }

    /// A photo's files: its master first (with no attach time nor
    /// author: it has no row), then the others by role and rank.
    pub fn files(&self, photo: &ImageId) -> Result<Vec<PhotoFile>, String> {
        self.read(|store| {
            if store.image(photo)?.is_none() {
                return Err(format!("no image {}", photo.hex()));
            }
            let mut all = vec![PhotoFile {
                image: *photo,
                photo: *photo,
                role: FileRole::Master,
                rank: 0,
                added_at: 0,
                added_by: String::new(),
            }];
            let mut rows = store.photo_files(photo)?;
            PhotoFile::sort(&mut rows);
            all.extend(rows);
            Ok(all)
        })
    }

    /// The files of many photos in one call (the Library's windowed
    /// read), the masters left out, in [`PhotoFile::order`].
    pub fn files_of(&self, photos: &[ImageId]) -> Result<Vec<PhotoFile>, String> {
        self.read(|store| {
            let mut rows = store.photo_files_of(photos)?;
            PhotoFile::sort(&mut rows);
            Ok(rows)
        })
    }

    /// Attaches `image` to `photo` as a file with `role` (not the master)
    /// and `rank`, by `who` at `now`. Refused: an unknown image or photo,
    /// a file in the Library on its own (pairing two photos merges them
    /// first), a file already of a photo, a file that has files of its
    /// own, and a photo that is itself a file of another. Returns the
    /// step; empty when the file already was so.
    pub fn attach_file(
        &self,
        photo: &ImageId,
        image: &ImageId,
        role: FileRole,
        rank: u32,
        who: &str,
        now: u64,
    ) -> Result<Changes, String> {
        if role == FileRole::Master {
            return Err("a photo's master is the photo itself, never attached".into());
        }
        if photo == image {
            return Err("a file cannot be attached to itself".into());
        }
        let row = PhotoFile {
            image: *image,
            photo: *photo,
            role,
            rank,
            added_at: now,
            added_by: who.into(),
        };
        self.change_file(image, who, now, |store| {
            for id in [photo, image] {
                if store.image(id)?.is_none() {
                    return Err(format!("no image {}", id.hex()));
                }
            }
            if let Some(was) = store.photo_of(image)? {
                if was.photo == *photo && was.role == role && was.rank == rank {
                    return Ok(None);
                }
                return Err(format!(
                    "{} is already a file of photo {}",
                    image.hex(),
                    was.photo.hex()
                ));
            }
            if store.library_flag(image)?.is_some_and(|f| f.in_library) {
                return Err(format!(
                    "{} is a photo in the Library: pair the two photos instead",
                    image.hex()
                ));
            }
            if !store.photo_files(image)?.is_empty() {
                return Err(format!("{} is a photo with files of its own", image.hex()));
            }
            if let Some(of) = store.photo_of(photo)? {
                return Err(format!(
                    "{} is a file of photo {}, not a photo",
                    photo.hex(),
                    of.photo.hex()
                ));
            }
            store.put_photo_file(&row)?;
            Ok(Some((None, Some(row.audited()))))
        })
    }

    /// Detaches `image` from its photo: it becomes a plain image again,
    /// off the Library. Refused for a file of no photo. Returns the step.
    pub fn detach_file(&self, image: &ImageId, who: &str, now: u64) -> Result<Changes, String> {
        self.change_file(image, who, now, |store| {
            let was = store
                .photo_of(image)?
                .ok_or_else(|| format!("{} is a file of no photo", image.hex()))?;
            store.remove_photo_file(image)?;
            Ok(Some((Some(was.audited()), None)))
        })
    }

    /// Runs `step` on a file's row in one transaction; it returns the
    /// audited values before and after (none: nothing to do). Returns the
    /// step with the file's rows.
    fn change_file(
        &self,
        image: &ImageId,
        who: &str,
        now: u64,
        step: impl FnOnce(&mut S) -> Result<Option<(Option<Value>, Option<Value>)>, String>,
    ) -> Result<Changes, String> {
        let changes = self.transaction(|store| {
            let before = rows_of(&*store, image)?;
            let Some((old, new)) = step(store)? else {
                return Ok(Changes::default());
            };
            store.add_audit(&AuditEntry {
                seq: 0,
                at: now,
                who: who.into(),
                job: None,
                image: *image,
                field: PHOTO_FILE.into(),
                old,
                new,
            })?;
            let change = Change {
                image: *image,
                before,
                after: rows_of(&*store, image)?,
            };
            Ok(Changes {
                images: vec![change],
                ..Changes::default()
            })
        })?;
        if !changes.is_empty() {
            self.catalog.lock().expect("catalog lock").version += 1;
            self.library_changed(1);
        }
        Ok(changes)
    }
}
