//! The mounted card connector: the cards and cameras the system mounts
//! (`helios_storage`), each read from its `DCIM` folder (the DCF rule) in
//! place. Its sources come and go (`caps.detect`): the host gives the
//! cards it polls, else the connector asks the system. It declares no
//! connection process.
//!
//! Each card declares the camera its photos come from
//! ([`SourceRef::camera`]): the EXIF `Make` and `Model` of its first few
//! files under `DCIM`, the most common one when they differ. Reading them
//! takes a moment and [`Connector::sources`] is asked every frame, so the
//! reading runs once per card on its own thread, its answer kept per card
//! identity for the process: the next listing, and the next time the card
//! is plugged, declare it at once.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use helios_core::config::Contracts;
use helios_storage::Card;

use crate::camera::Camera;

use crate::local::{Local, source_at, source_contract};
use crate::{
    Access, AccountRef, Browser, Cancel, Caps, CardsFn, Connector, ConnectorError, ConnectorInfo,
    ConnectorManifest, ConnectorProvider, ConnectorServices, Cursor, FetchSink, Fetched, INTERFACE,
    ItemRef, Needs, Page, Settings, SourceRef, Text,
};

/// The card connector's id.
pub const ID: &str = "card";

fn caps() -> Caps {
    Caps {
        detect: true,
        sizes: true,
        resume: true,
        ..Caps::default()
    }
}

fn label() -> Text {
    Text::new("connect-card", "Cards and cameras")
}

/// The card connector's provider.
pub struct CardProvider;

impl ConnectorProvider for CardProvider {
    fn config(&self) -> Contracts {
        Contracts {
            source: source_contract(true),
            ..Contracts::default()
        }
    }

    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: ID.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            label: label(),
            description: Text::new(
                "connect-card-about",
                "Memory cards, and cameras and phones mounted as a disk",
            ),
            caps: caps(),
            needs: Needs::default(),
            settings: serde_json::Value::Null,
            platforms: Vec::new(),
            interface: INTERFACE,
            connect: Vec::new(),
            browsers: vec![Browser::Folder, Browser::Timeline],
            setup: Some(crate::Setup {
                asks: crate::SetupAsks::Volume,
                icon: "camera".into(),
                auto: Some(crate::AutoImport::Plugged),
                placement: crate::Placement::Copy,
                ..crate::Setup::default()
            }),
        }
    }

    fn create(
        &self,
        _settings: &Settings,
        services: &ConnectorServices,
    ) -> Result<Box<dyn Connector>, ConnectorError> {
        Ok(Box::new(CardConnector {
            local: Local::new(ID, label(), caps()),
            cards: services.cards.clone(),
        }))
    }
}

/// The card connector.
pub struct CardConnector {
    local: Local,
    cards: Option<CardsFn>,
}

impl CardConnector {
    /// The source a card is: its mount point, labelled by its label.
    pub fn source(card: &Card) -> SourceRef {
        source_at(ID, &card.root, card.label.clone(), card.capacity)
    }
}

/// How many of a card's first files are read for its camera.
const CAMERA_FILES: usize = 3;

/// What is known of a card's camera.
#[derive(Clone)]
enum Detected {
    /// Its files are being read.
    Reading,
    /// Read: the camera, or `None` when its files do not say.
    Read(Option<Camera>),
}

/// The cameras read so far, by card identity, for the process.
fn detected() -> &'static Mutex<HashMap<String, Detected>> {
    static DETECTED: OnceLock<Mutex<HashMap<String, Detected>>> = OnceLock::new();
    DETECTED.get_or_init(Mutex::default)
}

/// A card's identity: its label, mount point and size (the system gives
/// no volume serial for every card).
fn identity(card: &Card) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        card.label,
        card.root.display(),
        card.capacity.unwrap_or_default()
    )
}

/// A card's camera when it is read; the first time the card is seen,
/// starts reading it on its own thread and answers `None` meanwhile.
fn camera_of(card: &Card) -> Option<Camera> {
    let key = identity(card);
    let mut known = detected()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match known.get(&key) {
        Some(Detected::Read(camera)) => camera.clone(),
        Some(Detected::Reading) => None,
        None => {
            known.insert(key.clone(), Detected::Reading);
            drop(known);
            let root = card.root.clone();
            let spawned = std::thread::Builder::new()
                .name("card-camera".into())
                .spawn(move || {
                    let camera = read_camera(&root);
                    detected()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(key, Detected::Read(camera));
                });
            if let Err(e) = spawned {
                log::warn!(
                    "card camera: cannot start reading {}: {e}",
                    card.root.display()
                );
            }
            None
        }
    }
}

/// The camera a card's first files name (EXIF `Make` and `Model`), the
/// most common one when they differ; `None` with no `DCIM` or no EXIF.
pub fn read_camera(root: &Path) -> Option<Camera> {
    if !root.join("DCIM").is_dir() {
        return None;
    }
    let files = crate::local::files_of(root).ok()?;
    let mut seen: Vec<(Camera, usize)> = Vec::new();
    for file in files.iter().take(CAMERA_FILES) {
        let Ok(capture) = helios_load::capture::capture(file) else {
            continue;
        };
        let (Some(make), Some(model)) = (capture.make, capture.model) else {
            continue;
        };
        let camera = Camera::new(make, model);
        match seen.iter_mut().find(|(c, _)| *c == camera) {
            Some((_, n)) => *n += 1,
            None => seen.push((camera, 1)),
        }
    }
    // The most common; the first read among equals.
    let best = seen.iter().map(|(_, n)| *n).max()?;
    seen.into_iter().find(|(_, n)| *n == best).map(|(c, _)| c)
}

impl Connector for CardConnector {
    fn describe(&self) -> ConnectorInfo {
        self.local.describe()
    }

    fn sources(&self, _account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        let cards = match &self.cards {
            Some(cards) => cards(),
            None => helios_storage::system().cards(),
        };
        Ok(cards
            .iter()
            .map(|card| {
                let mut source = Self::source(card);
                source.camera = camera_of(card);
                source
            })
            .collect())
    }

    fn list(
        &self,
        source: &SourceRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        self.local.list(source, from, page, cancel)
    }

    fn access(&self, item: &ItemRef) -> Access {
        Local::access(item)
    }

    fn fetch(
        &self,
        item: &ItemRef,
        offset: u64,
        sink: &mut dyn FetchSink,
        cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        Local::fetch(item, offset, sink, cancel)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;

    /// A little-endian Exif block naming its camera (IFD0 `Make`, `Model`).
    fn exif(make: &str, model: &str) -> Vec<u8> {
        let make = format!("{make}\0");
        let model = format!("{model}\0");
        let mut b = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        // IFD0 at 8: 2 entries, then its next-IFD offset; data after it.
        let data = 8 + 2 + 2 * 12 + 4;
        b.extend(2u16.to_le_bytes());
        for (tag, text, at) in [
            (0x010f_u16, &make, data),
            (0x0110, &model, data + make.len()),
        ] {
            b.extend(tag.to_le_bytes());
            b.extend(2u16.to_le_bytes());
            b.extend(u32::try_from(text.len()).unwrap().to_le_bytes());
            b.extend(u32::try_from(at).unwrap().to_le_bytes());
        }
        b.extend(0u32.to_le_bytes());
        b.extend(make.as_bytes());
        b.extend(model.as_bytes());
        b
    }

    /// A small JPEG, with an Exif segment naming its camera when given.
    fn jpeg(camera: Option<(&str, &str)>) -> Vec<u8> {
        let mut plain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut plain)
            .encode(&[128u8; 8 * 8 * 3], 8, 8, image::ExtendedColorType::Rgb8)
            .unwrap();
        let Some((make, model)) = camera else {
            return plain;
        };
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend(exif(make, model));
        let mut out = plain[..2].to_vec();
        out.extend([0xff, 0xe1]);
        out.extend(u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        out.extend(payload);
        out.extend(&plain[2..]);
        out
    }

    /// A fake card volume: `files` written under `DCIM/100NIKON` (none:
    /// no `DCIM`).
    fn card(name: &str, files: &[(&str, Vec<u8>)]) -> Card {
        let root: PathBuf =
            std::env::temp_dir().join(format!("helios-card-camera-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        if !files.is_empty() {
            let dir = root.join("DCIM").join("100NIKON");
            std::fs::create_dir_all(&dir).unwrap();
            for (file, bytes) in files {
                std::fs::write(dir.join(file), bytes).unwrap();
            }
        }
        Card {
            label: name.to_uppercase(),
            root,
            capacity: Some(64 << 30),
        }
    }

    fn connector(card: &Card) -> Box<dyn Connector> {
        let cards = vec![card.clone()];
        let services = ConnectorServices {
            cards: Some(Arc::new(move || cards.clone())),
            ..ConnectorServices::default()
        };
        CardProvider
            .create(&Settings::default(), &services)
            .unwrap()
    }

    /// The camera the connector declares once its reading thread is done.
    fn declared(connector: &dyn Connector, card: &Card) -> Option<Camera> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let source = connector.sources(None).unwrap().remove(0);
            let done = matches!(
                detected().lock().unwrap().get(&identity(card)),
                Some(Detected::Read(_))
            );
            if source.camera.is_some() || done || Instant::now() > deadline {
                return connector.sources(None).unwrap().remove(0).camera;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_card_declares_the_camera_its_first_files_name_then_from_its_cache() {
        let nikon = Some(("NIKON CORPORATION", "NIKON Z 50"));
        let card = card(
            "nikon",
            &[("DSC_0001.JPG", jpeg(nikon)), ("DSC_0002.JPG", jpeg(nikon))],
        );
        let connector = connector(&card);
        let camera = declared(connector.as_ref(), &card).expect("a camera");
        assert_eq!(camera.name(), "Nikon Z 50");
        assert_eq!(camera.kind(), Some(crate::camera::CameraKind::Mirrorless));
        // The files gone, the next listing still declares it: read once.
        std::fs::remove_dir_all(card.root.join("DCIM")).unwrap();
        let again = connector.sources(None).unwrap().remove(0);
        assert_eq!(again.camera, Some(camera));
        std::fs::remove_dir_all(&card.root).unwrap();
    }

    #[test]
    fn a_card_with_no_dcim_or_no_exif_declares_no_camera() {
        let bare = card("bare", &[]);
        assert_eq!(declared(connector(&bare).as_ref(), &bare), None);
        let plain = card("plain", &[("DSC_0001.JPG", jpeg(None))]);
        assert_eq!(declared(connector(&plain).as_ref(), &plain), None);
        for c in [bare, plain] {
            std::fs::remove_dir_all(&c.root).unwrap();
        }
    }

    #[test]
    fn a_card_holding_several_cameras_declares_the_most_common() {
        let z50 = Some(("NIKON CORPORATION", "NIKON Z 50"));
        let d750 = Some(("NIKON CORPORATION", "NIKON D750"));
        let root = card(
            "mixed",
            &[
                ("DSC_0001.JPG", jpeg(d750)),
                ("DSC_0002.JPG", jpeg(z50)),
                ("DSC_0003.JPG", jpeg(z50)),
            ],
        );
        let camera = read_camera(&root.root).expect("a camera");
        assert_eq!(camera.name(), "Nikon Z 50");
        std::fs::remove_dir_all(&root.root).unwrap();
    }
}
