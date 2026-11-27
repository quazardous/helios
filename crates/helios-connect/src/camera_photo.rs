//! A real photo of a source's camera ([`crate::camera::Camera`]), from
//! Wikimedia Commons, credited: fetched once per model, kept on disk, and
//! shown instead of the camera's generic drawing when there is one.
//!
//! - **Predictable.** A small curated table ([`commons_file`]) names the
//!   Commons file of each known model; there is no live search. A model
//!   the table does not know keeps its drawing.
//! - **Private.** The only thing sent is the Commons file's name, which
//!   says the model and nothing else of the person or their photos.
//! - **Off the interface thread.** [`CameraPhotos::photo`] never waits:
//!   the first ask starts a thread that reads the disk cache, else fetches
//!   the file through the [`Fetcher`], and says when it is done. Until
//!   then, offline, or once a fetch failed, the answer is `None` and the
//!   host shows the drawing.
//! - **Credited.** The file's author and licence come from Commons with
//!   the picture, are kept beside it ([`Credit`]), and the host shows them
//!   (`Photo: <author>, <licence>, Wikimedia Commons`).
//! - **A switch.** [`CameraPhotos::set_enabled`]: off, nothing is fetched
//!   nor shown (drawings only).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::camera::Camera;

/// The curated table: a camera's name, as [`Camera::name`] says it, and
/// the Commons file showing it (without its `File:` prefix). Kept sorted
/// by brand, then model.
///
/// # Adding a model
///
/// 1. Its name: what [`Camera::name`] gives for its EXIF make and model
///    (`NIKON CORPORATION` + `NIKON Z 6` is `Nikon Z 6`); the test
///    `every_curated_model_is_named_as_its_exif_says` checks it.
/// 2. A file on Wikimedia Commons showing the body alone (front or three
///    quarters, plain background), under a free licence (CC0, CC BY,
///    CC BY-SA, public domain): its file page's "Licensing" section says
///    it, as does the API
///    (`api.php?action=query&prop=imageinfo&iiprop=extmetadata&titles=File:<name>`,
///    its `LicenseShortName` and `Artist`). No fair-use or "non-free"
///    file, and one with an `Artist`: the credit line needs it.
/// 3. Its name as Commons spells it, `File:` dropped, in this table.
///
/// Each file was checked on Commons (exists, licence) on 2026-11-27.
const COMMONS_FILES: &[(&str, &str)] = &[
    // Canon
    (
        "Canon EOS 5D Mark IV",
        "Canon EOS 5D Mark IV (Front), 1803241116, ako.jpg",
    ),
    ("Canon EOS 6D", "Canon EOS 6D+EF 24-105mm F4L IS USM.jpg"),
    ("Canon EOS 90D", "Canon EOS 90D.jpg"),
    ("Canon EOS R5", "Canon EOS R5.jpg"),
    // Fujifilm
    ("Fujifilm X-T3", "Fujifilm X-T3 7 sep 2018b.jpg"),
    ("Fujifilm X-T4", "Fujifilm X-T4 29 apr 2020a.jpg"),
    ("Fujifilm X100V", "Fujifilm X100V 9 feb 2020a.jpg"),
    // Nikon
    ("Nikon D750", "Nikon d750 cap off.jpg"),
    ("Nikon D7500", "Nikon D7500 (retouched).jpg"),
    ("Nikon D850", "Nikon D850 001.jpg"),
    ("Nikon Z 50", "Nikon Z 50 1.jpg"),
    ("Nikon Z 6", "Nikon Z 6 7 sep 2018b.jpg"),
    ("Nikon Z 7", "Nikon Z 7 28 nov 2018a.jpg"),
    // Panasonic
    ("Panasonic DC-GH5", "Panasonic.Lumix.DC-GH5.jpg"),
    // Ricoh
    ("Ricoh GR III", "Ricoh GR III (5).jpg"),
    // Sony
    ("Sony ILCE-6400", "Sony ILCE-6400 01.jpg"),
    ("Sony ILCE-7M3", "Sony ILCE-7M3 18.jpg"),
];

/// The Commons file of `camera`'s photo, when the curated table knows
/// its model.
pub fn commons_file(camera: &Camera) -> Option<&'static str> {
    let name = camera.name();
    COMMONS_FILES
        .iter()
        .find(|(model, _)| model.eq_ignore_ascii_case(&name))
        .map(|(_, file)| *file)
}

/// Who a photo is by and under which licence, as Commons says it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credit {
    /// The Commons file (`Nikon Z 50 1.jpg`).
    pub file: String,
    /// Its author, as plain text.
    pub author: String,
    /// Its licence's short name (`CC BY-SA 4.0`).
    pub licence: String,
}

/// A camera's photo, ready to show: its picture's bytes (a JPEG, a few
/// hundred pixels wide) and its credit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Photo {
    pub bytes: Arc<[u8]>,
    pub credit: Credit,
}

/// Fetches a Commons file's picture and credit; the network's side,
/// replaced by a fake in tests.
pub trait Fetcher: Send + Sync {
    /// The picture of `file` (a Commons file name) and its credit.
    fn fetch(&self, file: &str) -> Result<(Vec<u8>, Credit), String>;
}

/// What is known of a Commons file this run.
#[derive(Clone, Debug)]
enum State {
    Fetching,
    Ready(Photo),
    /// Offline, refused or unreadable: the drawing stays until the next
    /// run.
    Failed,
}

/// The camera photos: the cache on disk, what this run knows, and the
/// fetcher. Cheap to clone (shared state).
#[derive(Clone)]
pub struct CameraPhotos {
    /// The cache folder; `None` fetches nothing (no place to keep it).
    dir: Option<PathBuf>,
    fetcher: Arc<dyn Fetcher>,
    enabled: bool,
    known: Arc<Mutex<HashMap<&'static str, State>>>,
    /// Said once a photo is ready or failed (the host asks a frame).
    done: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for CameraPhotos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CameraPhotos")
            .field("dir", &self.dir)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl Default for CameraPhotos {
    /// No cache folder: fetches nothing, every camera keeps its drawing.
    fn default() -> Self {
        Self::new(None, Arc::new(Commons::new()))
    }
}

impl CameraPhotos {
    /// The photos kept in `dir`, fetched through `fetcher`; on.
    pub fn new(dir: Option<PathBuf>, fetcher: Arc<dyn Fetcher>) -> Self {
        Self {
            dir,
            fetcher,
            enabled: true,
            known: Arc::default(),
            done: None,
        }
    }

    /// `done` is called once a photo is ready or failed.
    #[must_use]
    pub fn on_done(mut self, done: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.done = Some(done);
        self
    }

    /// The setting: off, no photo is fetched nor shown.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// `camera`'s photo, when it is ready. Never waits: the first ask
    /// reads the cache, else fetches, on a thread of its own; `None`
    /// until then, and for good this run when it failed, the setting is
    /// off or the table does not know the model.
    pub fn photo(&self, camera: &Camera) -> Option<Photo> {
        if !self.enabled {
            return None;
        }
        let file = commons_file(camera)?;
        let dir = self.dir.clone()?;
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        match known.get(file) {
            Some(State::Ready(photo)) => return Some(photo.clone()),
            Some(State::Fetching | State::Failed) => return None,
            None => {}
        }
        known.insert(file, State::Fetching);
        drop(known);
        let fetcher = Arc::clone(&self.fetcher);
        let known = Arc::clone(&self.known);
        let done = self.done.clone();
        let spawned = std::thread::Builder::new()
            .name("helios-camera-photo".into())
            .spawn(move || {
                let state = match load_or_fetch(&dir, file, fetcher.as_ref()) {
                    Ok(photo) => State::Ready(photo),
                    Err(e) => {
                        log::info!("camera photo {file}: {e}; its drawing stays");
                        State::Failed
                    }
                };
                known
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(file, state);
                if let Some(done) = done {
                    done();
                }
            });
        if let Err(e) = spawned {
            log::warn!("camera photo {file}: no thread: {e}");
            self.known
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(file, State::Failed);
        }
        None
    }

    /// `file`'s photo is not shown this run (its picture did not decode):
    /// its camera keeps the drawing.
    pub fn forget(&self, file: &str) {
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(state) = known
            .iter_mut()
            .find_map(|(k, v)| (*k == file).then_some(v))
        {
            *state = State::Failed;
        }
    }

    /// Whether a photo is still being read or fetched: the host looks again
    /// a moment later.
    pub fn busy(&self) -> bool {
        self.known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .any(|s| matches!(s, State::Fetching))
    }
}

/// A file's name on disk: its Commons name, kept to safe characters.
fn stem(file: &str) -> String {
    file.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// `file`'s photo from the cache in `dir`, else fetched and kept there.
fn load_or_fetch(dir: &Path, file: &str, fetcher: &dyn Fetcher) -> Result<Photo, String> {
    let picture = dir.join(format!("{}.jpg", stem(file)));
    let credit_file = dir.join(format!("{}.json", stem(file)));
    if let (Ok(bytes), Ok(text)) = (
        std::fs::read(&picture),
        std::fs::read_to_string(&credit_file),
    ) && let Ok(credit) = serde_json::from_str::<Credit>(&text)
    {
        return Ok(Photo {
            bytes: bytes.into(),
            credit,
        });
    }
    let (bytes, credit) = fetcher.fetch(file)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(&credit).map_err(|e| e.to_string())?;
    // The picture first: a credit on disk always has its picture.
    std::fs::write(&picture, &bytes).map_err(|e| format!("{}: {e}", picture.display()))?;
    std::fs::write(&credit_file, text).map_err(|e| format!("{}: {e}", credit_file.display()))?;
    Ok(Photo {
        bytes: bytes.into(),
        credit,
    })
}

/// The width asked of Commons's thumbnail, in pixels.
const WIDTH: u32 = 320;

/// The largest picture read, in bytes.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Wikimedia Commons, over its public API: the file's thumbnail and its
/// `Artist` and `LicenseShortName` metadata. Sends only the file's name.
pub struct Commons {
    http: ureq::Agent,
}

impl Default for Commons {
    fn default() -> Self {
        Self::new()
    }
}

impl Commons {
    pub fn new() -> Self {
        let http = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            // Wikimedia asks every client to name itself.
            .user_agent(concat!(
                "Helios/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/quazardous/helios)"
            ))
            .build()
            .into();
        Self { http }
    }
}

impl Fetcher for Commons {
    fn fetch(&self, file: &str) -> Result<(Vec<u8>, Credit), String> {
        let title = format!("File:{file}");
        let width = WIDTH.to_string();
        let text = self
            .http
            .get("https://commons.wikimedia.org/w/api.php")
            .query("action", "query")
            .query("format", "json")
            .query("prop", "imageinfo")
            .query("iiprop", "url|extmetadata")
            .query("iiextmetadatafilter", "Artist|LicenseShortName")
            .query("iiurlwidth", &width)
            .query("titles", &title)
            .call()
            .map_err(|e| e.to_string())?
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())?;
        let (url, credit) = info_of(file, &text)?;
        let bytes = self
            .http
            .get(&url)
            .call()
            .map_err(|e| e.to_string())?
            .body_mut()
            .with_config()
            .limit(MAX_BYTES)
            .read_to_vec()
            .map_err(|e| e.to_string())?;
        Ok((bytes, credit))
    }
}

/// The thumbnail's address and the credit, from Commons's answer.
fn info_of(file: &str, text: &str) -> Result<(String, Credit), String> {
    let json: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let info = json["query"]["pages"]
        .as_object()
        .and_then(|pages| pages.values().next())
        .and_then(|page| page["imageinfo"].get(0))
        .ok_or("no such file on Commons")?;
    let url = info["thumburl"]
        .as_str()
        .or_else(|| info["url"].as_str())
        .ok_or("no picture address")?
        .to_owned();
    let meta = |key: &str| {
        info["extmetadata"][key]["value"]
            .as_str()
            .map(plain)
            .filter(|s| !s.is_empty())
    };
    let credit = Credit {
        file: file.to_owned(),
        author: meta("Artist").ok_or("no author")?,
        licence: meta("LicenseShortName").ok_or("no licence")?,
    };
    Ok((url, credit))
}

/// `html` as plain text: its tags dropped, its spaces folded.
fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in html.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use super::*;

    /// A fetcher that counts its calls; offline, it fails them all.
    struct Fake {
        calls: AtomicUsize,
        offline: bool,
    }

    impl Fake {
        fn new(offline: bool) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                offline,
            })
        }
    }

    impl Fetcher for Fake {
        fn fetch(&self, file: &str) -> Result<(Vec<u8>, Credit), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.offline {
                return Err("offline".into());
            }
            Ok((
                b"jpeg".to_vec(),
                Credit {
                    file: file.to_owned(),
                    author: "A. Photographer".into(),
                    licence: "CC BY-SA 4.0".into(),
                },
            ))
        }
    }

    fn settled(photos: &CameraPhotos) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while photos.busy() {
            assert!(Instant::now() < deadline, "the fetch never ended");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn z50() -> Camera {
        Camera::new("NIKON CORPORATION", "NIKON Z 50")
    }

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("helios-camera-photo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_curated_table_names_known_models_only() {
        assert_eq!(commons_file(&z50()), Some("Nikon Z 50 1.jpg"));
        assert_eq!(
            commons_file(&Camera::new("SONY", "ILCE-7M3")),
            Some("Sony ILCE-7M3 18.jpg")
        );
        assert_eq!(commons_file(&Camera::new("Acme", "Box 1")), None);
    }

    #[test]
    fn every_curated_model_is_named_as_its_exif_says() {
        // Make and model as the cameras write them in EXIF.
        let exif = [
            ("Canon", "Canon EOS 5D Mark IV"),
            ("Canon", "Canon EOS 6D"),
            ("Canon", "Canon EOS 90D"),
            ("Canon", "Canon EOS R5"),
            ("FUJIFILM", "X-T3"),
            ("FUJIFILM", "X-T4"),
            ("FUJIFILM", "X100V"),
            ("NIKON CORPORATION", "NIKON D750"),
            ("NIKON CORPORATION", "NIKON D7500"),
            ("NIKON CORPORATION", "NIKON D850"),
            ("NIKON CORPORATION", "NIKON Z 50"),
            ("NIKON CORPORATION", "NIKON Z 6"),
            ("NIKON CORPORATION", "NIKON Z 7"),
            ("Panasonic", "DC-GH5"),
            ("RICOH IMAGING COMPANY, LTD.", "RICOH GR III"),
            ("SONY", "ILCE-6400"),
            ("SONY", "ILCE-7M3"),
        ];
        assert_eq!(exif.len(), COMMONS_FILES.len(), "one EXIF pair per row");
        for (make, model) in exif {
            let camera = Camera::new(make, model);
            assert!(
                commons_file(&camera).is_some(),
                "{make} {model} is named {:?}, not in the table",
                camera.name()
            );
        }
        // A row per model, no doubles.
        let mut names: Vec<_> = COMMONS_FILES.iter().map(|(m, _)| *m).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), COMMONS_FILES.len());
    }

    #[test]
    fn a_known_model_shows_its_photo_and_credit_once_fetched() {
        let fake = Fake::new(false);
        let photos = CameraPhotos::new(Some(dir("known")), fake.clone());
        assert_eq!(photos.photo(&z50()), None, "never waits on the fetch");
        settled(&photos);
        let photo = photos.photo(&z50()).expect("the photo, once fetched");
        assert_eq!(&*photo.bytes, b"jpeg");
        assert_eq!(photo.credit.author, "A. Photographer");
        assert_eq!(photo.credit.licence, "CC BY-SA 4.0");
        assert_eq!(photo.credit.file, "Nikon Z 50 1.jpg");
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        // An unknown model is never fetched.
        assert_eq!(photos.photo(&Camera::new("Acme", "Box 1")), None);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn offline_keeps_the_drawing_and_does_not_ask_again_this_run() {
        let fake = Fake::new(true);
        let photos = CameraPhotos::new(Some(dir("offline")), fake.clone());
        assert_eq!(photos.photo(&z50()), None);
        settled(&photos);
        assert_eq!(photos.photo(&z50()), None);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_setting_off_fetches_and_shows_nothing() {
        let fake = Fake::new(false);
        let mut photos = CameraPhotos::new(Some(dir("off")), fake.clone());
        photos.set_enabled(false);
        assert_eq!(photos.photo(&z50()), None);
        settled(&photos);
        assert_eq!(photos.photo(&z50()), None);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_cache_on_disk_is_reused_at_the_next_open() {
        let dir = dir("reuse");
        let first = Fake::new(false);
        let photos = CameraPhotos::new(Some(dir.clone()), first.clone());
        photos.photo(&z50());
        settled(&photos);
        assert!(photos.photo(&z50()).is_some());
        // The next run: a new set over the same folder, offline.
        let second = Fake::new(true);
        let again = CameraPhotos::new(Some(dir), second.clone());
        again.photo(&z50());
        settled(&again);
        let photo = again.photo(&z50()).expect("read from the cache");
        assert_eq!(photo.credit.author, "A. Photographer");
        assert_eq!(second.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn commons_s_answer_gives_the_thumbnail_and_a_plain_credit() {
        let answer = r#"{"query":{"pages":{"1":{"imageinfo":[{
            "thumburl":"https://upload.wikimedia.org/x/320px-Z.jpg",
            "url":"https://upload.wikimedia.org/x/Z.jpg",
            "extmetadata":{
                "Artist":{"value":"<a href=\"//commons.wikimedia.org/wiki/User:Someone\">Some  One</a>"},
                "LicenseShortName":{"value":"CC BY-SA 4.0"}}}]}}}}"#;
        let (url, credit) = info_of("Z.jpg", answer).unwrap();
        assert_eq!(url, "https://upload.wikimedia.org/x/320px-Z.jpg");
        assert_eq!(credit.author, "Some One");
        assert_eq!(credit.licence, "CC BY-SA 4.0");
        assert!(info_of("Z.jpg", r#"{"query":{"pages":{"-1":{}}}}"#).is_err());
    }
}
