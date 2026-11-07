//! The identity file Helios leaves at a volume's root,
//! `.helios/volume.json`: an id, the library it belongs to, when it was
//! written. It names a volume when its filesystem cannot (no UUID, a FAT
//! card, a cloned disk, one share under two addresses), and wins over the
//! filesystem's identity when present.
//!
//! It is read whenever present, and written only by an explicit [`mark`]
//! (the user's leave, in later phases), never as a side effect of a scan.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Identity, Volume, VolumeId};

/// The file, relative to the volume's root.
pub const PATH: &str = ".helios/volume.json";

/// What the file holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub id: VolumeId,
    /// The library that marked it.
    #[serde(default)]
    pub library: String,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    pub created: u64,
}

/// Where the marker of a volume mounted at `root` lives.
pub fn path(root: &Path) -> PathBuf {
    root.join(PATH)
}

/// The marker at `root`, if one is there and reads.
pub fn read(root: &Path) -> Option<Marker> {
    let text = std::fs::read_to_string(path(root)).ok()?;
    match serde_json::from_str::<Marker>(&text) {
        Ok(m) if m.id.as_str().starts_with("marker:") => Some(m),
        Ok(m) => {
            log::warn!("volume marker at {}: odd id {}", root.display(), m.id);
            None
        }
        Err(e) => {
            log::warn!("volume marker at {}: {e}", root.display());
            None
        }
    }
}

/// Marks a mounted volume: writes its marker (through a part file then
/// renamed) and returns the volume as the marker now names it. A volume
/// already marked keeps its marker. Refused when it is not mounted.
pub fn mark(volume: &Volume, library: &str, now: u64) -> Result<Volume, String> {
    let root = volume
        .mount
        .as_deref()
        .ok_or_else(|| format!("volume {} is not mounted", volume.name))?;
    let marker = if let Some(existing) = read(root) {
        existing
    } else {
        let marker = Marker {
            id: fresh_id(volume, root, now),
            library: library.to_string(),
            created: now,
        };
        let file = path(root);
        let dir = file.parent().expect("the marker has a folder");
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let part = dir.join("volume.json.part");
        let text = serde_json::to_string_pretty(&marker).map_err(|e| e.to_string())?;
        std::fs::write(&part, text).map_err(|e| format!("{}: {e}", part.display()))?;
        std::fs::rename(&part, &file).map_err(|e| format!("{}: {e}", file.display()))?;
        marker
    };
    Ok(Volume {
        id: marker.id,
        identified_by: Identity::Marker,
        ..volume.clone()
    })
}

/// A new id: the volume's old one, where it is mounted and when, the
/// process and the clock, hashed. Two clones of one disk get two ids.
fn fresh_id(volume: &Volume, root: &Path, now: u64) -> VolumeId {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seed = format!(
        "{}\0{}\0{now}\0{nanos}\0{}",
        volume.id,
        root.display(),
        std::process::id()
    );
    let hash = blake3::hash(seed.as_bytes()).to_hex();
    VolumeId(format!("marker:{}", &hash[..32]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "helios-storage-marker-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn marking_writes_once_and_reading_finds_it() {
        let dir = scratch("mark");
        let mut volume = detect::weak("BACKUP", Some(1_000));
        assert!(mark(&volume, "lib", 1).is_err(), "not mounted");
        volume.mount = Some(dir.clone());
        assert_eq!(read(&dir), None);
        let marked = mark(&volume, "lib-1", 42).unwrap();
        assert_eq!(marked.identified_by, Identity::Marker);
        assert!(marked.id.as_str().starts_with("marker:"));
        let m = read(&dir).unwrap();
        assert_eq!(
            (&m.id, m.library.as_str(), m.created),
            (&marked.id, "lib-1", 42)
        );
        // Marking again keeps the first marker.
        assert_eq!(mark(&volume, "lib-2", 43).unwrap().id, marked.id);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
