//! Which of an image's files is read: the one every reader of an image's
//! pixels (export, cull, thumbnails and previews, the darkroom, the
//! camera calibration) opens.
//!
//! An image copied from a card has two locations: the library copy the
//! import wrote, and the card's file it read (noted so a resumed import
//! knows it without a read). The card may be unplugged, or wiped once
//! backed up, and is slower: the library's own copy is read first, then
//! any other copy, a card or another ingest source last.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::storage::{VOLUMES, VolumeId};
use crate::{ImageId, Location, Store, Vault};

/// The library documents' namespace the library folder is kept in.
pub const LIBRARY_SETTINGS: &str = "settings";

/// The library folder's key there: `{"path": "/…"}`.
pub const LIBRARY_FOLDER: &str = "root";

/// How an image's locations are ranked for a read: built once
/// ([`Vault::read_order`]) for any number of images.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadOrder {
    /// The library folder copies go to.
    pub library: Option<PathBuf>,
    /// The volumes holding the library (its role).
    pub library_volumes: BTreeSet<VolumeId>,
    /// The volumes files are only read from: a card or a camera, an
    /// ingest source.
    pub sources: BTreeSet<VolumeId>,
}

impl ReadOrder {
    /// A location's rank, lowest read first: the library folder's copy,
    /// a copy on a library volume, a copy elsewhere, a source's file.
    pub fn rank(&self, location: &Location) -> u8 {
        let on =
            |set: &BTreeSet<VolumeId>| location.volume.as_ref().is_some_and(|v| set.contains(v));
        if self
            .library
            .as_deref()
            .is_some_and(|root| location.path.starts_with(root))
        {
            0
        } else if on(&self.library_volumes) {
            1
        } else if on(&self.sources) {
            3
        } else {
            2
        }
    }

    /// Sorts `locations` the way they are read: present ones first, each
    /// group by [`ReadOrder::rank`], otherwise in their order.
    pub fn sort(&self, locations: &mut [Location]) {
        locations.sort_by_key(|l| (!l.present(), self.rank(l)));
    }

    /// The present location read first, as the map knows it (no look at
    /// the disk: for a library view of thousands).
    pub fn pick<'a>(&self, locations: &'a [Location]) -> Option<&'a Location> {
        locations
            .iter()
            .filter(|l| l.present())
            .min_by_key(|l| self.rank(l))
    }

    /// The location to read a file from, its file there now: the present
    /// ones in rank order, the first whose file is on disk (a library
    /// copy deleted since the last scan gives way to the card's file).
    pub fn file<'a>(&self, locations: &'a [Location]) -> Option<&'a Location> {
        let mut present: Vec<&Location> = locations.iter().filter(|l| l.present()).collect();
        present.sort_by_key(|l| self.rank(l));
        present.into_iter().find(|l| l.path.is_file())
    }
}

/// Whether a volume document names a source: an ingest role, or a card
/// (removable and holding `DCIM`).
fn is_source(doc: &Value) -> bool {
    let ingest = doc["roles"]
        .as_array()
        .is_some_and(|r| r.iter().any(|r| r == "ingest"));
    ingest || (doc["kind"] == "removable" && doc["has_dcim"] == true)
}

fn has_library_role(doc: &Value) -> bool {
    doc["roles"]
        .as_array()
        .is_some_and(|r| r.iter().any(|r| r == "library"))
}

impl<S: Store> Vault<S> {
    /// The library folder copies go to, once given.
    pub fn library_folder(&self) -> Result<Option<PathBuf>, String> {
        Ok(self
            .library_document(LIBRARY_SETTINGS, LIBRARY_FOLDER)?
            .and_then(|v| v["path"].as_str().map(PathBuf::from)))
    }

    /// Remembers the library folder copies go to.
    pub fn set_library_folder(&self, root: &Path) -> Result<(), String> {
        self.set_library_document(
            LIBRARY_SETTINGS,
            LIBRARY_FOLDER,
            Some(&serde_json::json!({ "path": root })),
        )
    }

    /// How images' locations are ranked for a read, from the library
    /// folder and the known volumes.
    pub fn read_order(&self) -> Result<ReadOrder, String> {
        let library = self.library_folder()?;
        let volumes = self.read(|s| s.documents(VOLUMES))?;
        let mut order = ReadOrder {
            library,
            ..ReadOrder::default()
        };
        for (id, doc) in volumes {
            if is_source(&doc) {
                order.sources.insert(VolumeId(id));
            } else if has_library_role(&doc) {
                order.library_volumes.insert(VolumeId(id));
            }
        }
        Ok(order)
    }

    /// The location an image's file is read from ([`ReadOrder::file`]):
    /// the library's copy before any other, a card's file only when no
    /// other is at hand; `None` when none is.
    pub fn file_to_read(&self, id: &ImageId) -> Result<Option<Location>, String> {
        let locations = self.locations(id)?;
        Ok(self.read_order()?.file(&locations).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Presence;

    fn at(path: &str, volume: Option<&str>) -> Location {
        Location {
            volume: volume.map(|v| VolumeId(v.into())),
            ..Location::seen(path.into(), 1, 1, 1)
        }
    }

    #[test]
    fn the_library_copy_is_read_before_a_card_s_file() {
        let order = ReadOrder {
            library: Some("/photos/library".into()),
            sources: BTreeSet::from([VolumeId("card".into())]),
            ..ReadOrder::default()
        };
        let card = at("/media/card/DCIM/a.cr3", Some("card"));
        let elsewhere = at("/data/a.cr3", None);
        let copy = at("/photos/library/2026/a.cr3", None);
        let mut all = vec![card.clone(), elsewhere.clone(), copy.clone()];
        assert_eq!(order.pick(&all), Some(&copy));
        order.sort(&mut all);
        assert_eq!(all, [copy.clone(), elsewhere.clone(), card.clone()]);

        // The library copy missing: another copy, then the card's file.
        let missing = Location {
            presence: Presence::Missing,
            ..copy
        };
        assert_eq!(order.pick(&[card.clone(), missing.clone()]), Some(&card));
        let mut two = vec![missing.clone(), card.clone()];
        order.sort(&mut two);
        assert_eq!(two, [card, missing.clone()]);
        assert_eq!(order.pick(&[missing]), None);
    }
}
