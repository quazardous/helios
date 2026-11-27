//! A source's camera, as its connector declares it (`SourceRef::camera`):
//! the import window shows its name on the source's row and in the
//! detail zone, with its kind's drawing named for it. The window knows
//! no connector: a fake card connector declares the camera. A known
//! model shows its credited photo from Wikimedia Commons instead, through
//! a fake fetcher (no network); the setting off or offline, the drawing.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use helios_connect::camera::Camera;
use helios_connect::camera_photo::{CameraPhotos, Credit, Fetcher};
use helios_connect::card::CardProvider;
use helios_connect::local::source_at;
use helios_connect::{
    Access, AccountRef, Cancel, Connector, ConnectorError, ConnectorInfo, ConnectorManifest,
    ConnectorProvider, ConnectorServices, Cursor, FetchSink, Fetched, ItemRef, Page, Settings,
    SourceRef,
};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

/// The card connector, its one card declaring the camera its files say.
struct Declaring {
    inner: Box<dyn Connector>,
    card: SourceRef,
}

impl Connector for Declaring {
    fn describe(&self) -> ConnectorInfo {
        self.inner.describe()
    }

    fn sources(&self, _account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        Ok(vec![self.card.clone()])
    }

    fn list(
        &self,
        source: &SourceRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        self.inner.list(source, from, page, cancel)
    }

    fn access(&self, item: &ItemRef) -> Access {
        self.inner.access(item)
    }

    fn fetch(
        &self,
        item: &ItemRef,
        offset: u64,
        sink: &mut dyn FetchSink,
        cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        self.inner.fetch(item, offset, sink, cancel)
    }
}

struct DeclaringProvider {
    card: SourceRef,
}

impl ConnectorProvider for DeclaringProvider {
    fn manifest(&self) -> ConnectorManifest {
        CardProvider.manifest()
    }

    fn create(
        &self,
        settings: &Settings,
        services: &ConnectorServices,
    ) -> Result<Box<dyn Connector>, ConnectorError> {
        Ok(Box::new(Declaring {
            inner: CardProvider.create(settings, services)?,
            card: self.card.clone(),
        }))
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-import-camera-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("DCIM")).unwrap();
    dir
}

/// The import window open, its card connector declaring `camera` on a
/// card labelled "Holiday card" at `root`.
fn window(root: &std::path::Path, camera: Option<Camera>) -> Harness<'static, Shell> {
    window_with(root, camera, None)
}

/// [`window`], its camera photos fetched by `photos` when given.
fn window_with(
    root: &std::path::Path,
    camera: Option<Camera>,
    photos: Option<CameraPhotos>,
) -> Harness<'static, Shell> {
    let mut card = source_at(
        helios_connect::card::ID,
        root,
        "Holiday card".into(),
        Some(64_000_000_000),
    );
    card.camera = camera;
    let mut shell = Shell::new(helios_panels(), None);
    shell.add_import_connector(std::sync::Arc::new(DeclaringProvider { card }));
    if let Some(photos) = photos {
        shell.set_camera_photos(photos);
    }
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run("library.importWindow", json!({}), Origin::User)
        .unwrap();
    h.run_steps(3);
    h
}

/// Frames stepped until `label` shows (at most 10 s).
fn until_shown(h: &mut Harness<'static, Shell>, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.query_by_label(label).is_none() {
        assert!(Instant::now() < deadline, "{label:?} not shown in 10 s");
        h.step();
    }
}

#[test]
fn a_card_whose_connector_declares_a_nikon_z_50_shows_its_name_and_drawing() {
    let root = temp("nikon");
    let mut h = window(&root, Some(Camera::new("Nikon", "Z 50")));
    until_shown(&mut h, "Holiday card");
    // Its row names the camera on its line.
    let row = h.get_by_label("Holiday card");
    let said = row.accesskit_node().description().unwrap_or_default();
    assert!(said.contains("Nikon Z 50"), "the row says {said:?}");
    // Selected, its detail shows the camera's name and its drawing.
    row.click();
    h.run_steps(3);
    let _ = h.get_by_role_and_label(Role::Image, "Camera picture: Nikon Z 50");
    let _ = h.get_by_label("Nikon Z 50");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_card_without_a_declared_camera_shows_no_camera_picture() {
    let root = temp("none");
    let mut h = window(&root, None);
    until_shown(&mut h, "Holiday card");
    h.get_by_label("Holiday card").click();
    h.run_steps(3);
    assert!(
        h.query_all_by_role(Role::Image).all(|n| !n
            .accesskit_node()
            .label()
            .unwrap_or_default()
            .starts_with("Camera picture")),
        "no camera picture without a camera"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A Commons stand-in: a small PNG credited to a photographer, or
/// offline.
struct FakeCommons {
    offline: bool,
}

impl Fetcher for FakeCommons {
    fn fetch(&self, file: &str) -> Result<(Vec<u8>, Credit), String> {
        if self.offline {
            return Err("offline".into());
        }
        let picture = image::RgbaImage::from_pixel(32, 20, image::Rgba([90, 90, 90, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(picture)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .map_err(|e| e.to_string())?;
        Ok((
            bytes,
            Credit {
                file: file.to_owned(),
                author: "A. Photographer".into(),
                licence: "CC BY-SA 4.0".into(),
            },
        ))
    }
}

const CREDIT: &str = "Photo: A. Photographer, CC BY-SA 4.0, Wikimedia Commons";

/// The window on a card declaring a Nikon Z 50, its photos kept in
/// `root`'s cache folder and fetched by a fake Commons, `offline` or
/// not; the card selected.
fn nikon_with_photos(root: &std::path::Path, offline: bool, on: bool) -> Harness<'static, Shell> {
    let photos = CameraPhotos::new(
        Some(root.join("camera-photos")),
        std::sync::Arc::new(FakeCommons { offline }),
    );
    let mut h = window_with(root, Some(Camera::new("Nikon", "Z 50")), Some(photos));
    if !on {
        use egui_pop::model::values::{Change, Set, Value as PopValue};
        let store = h.state().app().handle().store();
        store
            .lock()
            .unwrap()
            .set(
                Set::new(
                    &format!("settings/{}", helios_gui::shell::CAMERA_PHOTOS_SETTING),
                    Change::To(PopValue::Bool(false)),
                    egui_pop::model::commands::Origin::User,
                )
                .untracked(),
            )
            .unwrap();
    }
    until_shown(&mut h, "Holiday card");
    h.get_by_label("Holiday card").click();
    h.run_steps(3);
    h
}

/// Frames stepped for `frames` while the fetch runs (a fake answers at
/// once): long enough for a photo that would come to show.
fn settle(h: &mut Harness<'static, Shell>, frames: usize) {
    for _ in 0..frames {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_known_camera_shows_its_credited_photo_from_commons() {
    let root = temp("photo");
    let mut h = nikon_with_photos(&root, false, true);
    until_shown(&mut h, CREDIT);
    // The photo stands for the camera, named as the drawing was.
    let _ = h.get_by_role_and_label(Role::Image, "Camera picture: Nikon Z 50");
    let _ = h.get_by_label("Nikon Z 50");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_setting_off_keeps_the_drawing_without_a_credit() {
    let root = temp("photo-off");
    let mut h = nikon_with_photos(&root, false, false);
    settle(&mut h, 30);
    let _ = h.get_by_role_and_label(Role::Image, "Camera picture: Nikon Z 50");
    assert!(h.query_by_label(CREDIT).is_none(), "no credit, setting off");
    assert!(
        !root.join("camera-photos").exists(),
        "nothing fetched nor kept, setting off"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn offline_keeps_the_drawing_without_a_credit() {
    let root = temp("photo-offline");
    let mut h = nikon_with_photos(&root, true, true);
    settle(&mut h, 30);
    let _ = h.get_by_role_and_label(Role::Image, "Camera picture: Nikon Z 50");
    assert!(h.query_by_label(CREDIT).is_none(), "no credit, offline");
    let _ = std::fs::remove_dir_all(&root);
}
