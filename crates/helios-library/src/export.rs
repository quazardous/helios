//! A batch export asked for over library images: each image's file, its
//! head version, and its output name decided now (see `docs/export.md`).
//!
//! [`plan`] turns images and settings into a
//! [`helios_export::batch::BatchRequest`]: plain data, which the desktop
//! runs on its scheduler and a node could take as a workflow run. Names
//! come from the settings' pattern, the import's renaming fields
//! ([`crate::rename::Pattern`]) read from the signals the import kept
//! (`{date}`, `{camera}`…) and the image's place in the batch (`{seq}`),
//! the extension from the format. Names are decided once, here: two
//! images never get one name, and a name taken on disk is numbered
//! (`name-2.jpg`), replaced or left out, as the settings say.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use helios_export::batch::{BatchRequest, Clash, Item, Settings, extension};
use helios_vault::{EditSession, ImageId, Location, Store, Vault};

use crate::rename::{Naming, Pattern};
use crate::signals::Signals;

/// Where an image goes, and why it is left out when it is.
fn place(
    folder: &Path,
    name: &str,
    ext: &str,
    clash: Clash,
    taken: &mut HashSet<PathBuf>,
) -> (PathBuf, Option<String>) {
    let first = folder.join(format!("{name}.{ext}"));
    // Another image of this batch writes it: always another name.
    let in_batch = taken.contains(&first);
    let on_disk = first.exists();
    let chosen = match clash {
        Clash::Overwrite if !in_batch => Some(first),
        Clash::Skip if !in_batch && on_disk => {
            return (first, Some("its file exists".into()));
        }
        _ if !in_batch && !on_disk => Some(first),
        _ => None,
    };
    let path = chosen.unwrap_or_else(|| {
        (2..u64::MAX)
            .map(|n| folder.join(format!("{name}-{n}.{ext}")))
            .find(|p| !taken.contains(p) && !p.exists())
            .expect("a free name")
    });
    taken.insert(path.clone());
    (path, None)
}

/// The batch request exporting `images` (in this order) with `settings`,
/// rendered at `node` (named for the queue). An image without a copy
/// present is refused in the request, not here.
pub fn plan<S: Store>(
    vault: &Vault<S>,
    images: &[ImageId],
    settings: Settings,
    node: Option<String>,
) -> Result<BatchRequest, String> {
    let pattern = Pattern::parse(&settings.pattern)?;
    let ext = extension(&settings.output.format);
    let mut taken = HashSet::new();
    let mut items = Vec::with_capacity(images.len());
    for (i, image) in images.iter().enumerate() {
        let source = vault
            .locations(image)?
            .into_iter()
            .find(Location::present)
            .map(|l| l.path);
        let Some(source) = source else {
            items.push(Item {
                image: image.hex(),
                version: None,
                source: PathBuf::new(),
                output: PathBuf::new(),
                skip: None,
                error: Some("no copy of the image is present".into()),
            });
            continue;
        };
        let (_, head) = EditSession::open(vault, *image)?;
        let signals: Signals = vault
            .signals(image)?
            .and_then(|s| serde_json::from_value(s).ok())
            .unwrap_or_default();
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = pattern.name(&Naming {
            stem: &stem,
            signals: &signals,
            seq: i as u64 + 1,
        });
        let (output, skip) = place(&settings.folder, &name, ext, settings.clash, &mut taken);
        items.push(Item {
            image: image.hex(),
            version: head.map(|v| v.id.hex()),
            source,
            output,
            skip,
            error: None,
        });
    }
    Ok(BatchRequest::new(node, settings, items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_numbered_replaced_or_left_out_as_asked() {
        let dir = std::env::temp_dir().join(format!("helios-export-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.jpg"), b"there").unwrap();
        let mut taken = HashSet::new();
        // Numbered: past the file on disk, then past the batch's own.
        assert_eq!(
            place(&dir, "a", "jpg", Clash::Number, &mut taken),
            (dir.join("a-2.jpg"), None)
        );
        assert_eq!(
            place(&dir, "a", "jpg", Clash::Number, &mut taken),
            (dir.join("a-3.jpg"), None)
        );
        // Replaced on disk, but never two images on one name.
        let mut taken = HashSet::new();
        assert_eq!(
            place(&dir, "a", "jpg", Clash::Overwrite, &mut taken),
            (dir.join("a.jpg"), None)
        );
        assert_eq!(
            place(&dir, "a", "jpg", Clash::Overwrite, &mut taken),
            (dir.join("a-2.jpg"), None)
        );
        // Left out when on disk.
        let mut taken = HashSet::new();
        let (_, skip) = place(&dir, "a", "jpg", Clash::Skip, &mut taken);
        assert!(skip.is_some());
        assert_eq!(
            place(&dir, "b", "jpg", Clash::Skip, &mut taken),
            (dir.join("b.jpg"), None)
        );
        assert_eq!(
            place(&dir, "b", "jpg", Clash::Skip, &mut taken),
            (dir.join("b-2.jpg"), None)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
