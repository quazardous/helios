//! The images an extract run sends, read from the library
//! (`docs/design/extractors.md`, section 8): a photo's embedded preview,
//! found through its locations in the vault.
//!
//! - A picture (JPEG, PNG, TIFF) is sent as its file: the AI service
//!   scales it down and re-encodes it before it leaves the process.
//! - A raw sends the colour JPEG stream it embeds that is the smallest
//!   one at least [`SIDE`] pixels long (else its largest), decoded at a
//!   DCT scale ([`helios_load::embedded`]) and encoded again as a JPEG.
//! - The version sent is `None` (the embedded preview), part of the
//!   cache key: the head version rendered small comes later.
//!
//! Not yet: the raw's EXIF orientation is not applied to its preview.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use helios_load::Kind;
use helios_load::embedded::{choose, decode_scaled, embedded_jpegs};
use helios_vault::{ImageId, Store, Vault};

use super::extract::{ImageSource, SentImage};

/// The long side a raw's preview is decoded at, at least: a remote
/// provider gets 1024 px at most (`privacy.remote_image_side`).
pub const SIDE: u32 = 1024;

/// A library's photos as the images an extract run sends.
pub struct LibraryPreviews<S: Store> {
    vault: std::sync::Arc<Vault<S>>,
}

impl<S: Store> LibraryPreviews<S> {
    pub fn new(vault: std::sync::Arc<Vault<S>>) -> Self {
        Self { vault }
    }

    /// The path a photo is read at: its first present location.
    fn path(&self, image: &str) -> Result<PathBuf, String> {
        let id = ImageId::from_hex(image)?;
        let locations = self.vault.locations(&id)?;
        locations
            .iter()
            .find(|l| l.present())
            .map(|l| l.path.clone())
            .ok_or_else(|| match locations.first() {
                None => format!("photo {image} is not in the library"),
                Some(l) => format!(
                    "photo {image}: its file is {} ({})",
                    l.presence.as_str(),
                    l.path.display()
                ),
            })
    }
}

impl<S: Store> ImageSource for LibraryPreviews<S> {
    fn image(&self, image: &str) -> Result<SentImage, String> {
        let path = self.path(image)?;
        let bytes = match Kind::of(&path) {
            Kind::Picture => {
                std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Kind::Raw => raw_preview(&path)?,
        };
        Ok(SentImage {
            bytes,
            version: None,
        })
    }
}

/// A raw's embedded colour preview, at least [`SIDE`] long when it has
/// one so large, as a JPEG.
fn raw_preview(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = helios_load::preview_bytes(path)?;
    let found = embedded_jpegs(&bytes);
    let stream = choose(&found, SIDE)
        .into_iter()
        .find(|e| !e.grey())
        .ok_or_else(|| format!("{}: no embedded preview", path.display()))?;
    let rgb = decode_scaled(&bytes[stream.offset..], SIDE)
        .map_err(|e| format!("{}: its preview: {e}", path.display()))?;
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(rgb)
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .map_err(|e| format!("{}: encoding its preview: {e}", path.display()))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use helios_vault::MemoryStore;

    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "helios-library-previews-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_picture_is_sent_as_its_file_and_a_photo_with_no_file_says_why() {
        let dir = dir("picture");
        let file = dir.join("card.png");
        let card = helios_ai::images::test_card(24);
        std::fs::write(&file, &card).unwrap();
        let vault = Arc::new(Vault::new(MemoryStore::default()));
        let id = vault.import_file(&file, 1_000).unwrap();
        let source = LibraryPreviews::new(vault);
        let sent = source.image(&id.hex()).unwrap();
        assert_eq!(sent.bytes, card);
        assert_eq!(sent.version, None);

        let Err(unknown) = source.image(&"0".repeat(64)) else {
            panic!("an unknown photo is sent")
        };
        assert!(unknown.contains("not in the library"), "{unknown}");
        assert!(source.image("nope").is_err());
    }

    #[test]
    fn a_raw_sends_its_embedded_preview_as_a_jpeg() {
        let dir = dir("raw");
        // A file read as a raw (by its extension) whose only stream is a
        // colour JPEG.
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            40,
            30,
            image::Rgb([200, 120, 40]),
        ))
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
        let mut raw = b"not a raw header".to_vec();
        raw.extend(jpeg.into_inner());
        let file = dir.join("shot.xyz");
        std::fs::write(&file, &raw).unwrap();
        let sent = raw_preview(&file).unwrap();
        let back = image::load_from_memory(&sent).unwrap();
        assert_eq!((back.width(), back.height()), (40, 30));
    }
}
