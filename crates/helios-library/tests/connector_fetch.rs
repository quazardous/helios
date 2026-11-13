//! Import from a connector whose items are fetched
//! (`helios_library::fetch`), against a fake remote: it serves bytes from
//! an offset, can cut a fetch halfway, and can lie about an item's hash
//! or its bytes. Covered: the happy path, a resume after a cut, a
//! mismatch retried then failed, duplicates caught before any byte is
//! fetched (by item id, by claimed hash, by a probable match on a
//! re-encoding source), and the `remote` volume with its sightings.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use helios_connect::land::digest;
use helios_connect::{
    Access, AccountRef, Cancel, Caps, Claim, Connector, ConnectorError, ConnectorInfo, Cursor,
    FetchSink, Fetched, HashAlgo, Item, ItemId, ItemKind, ItemRef, Originals, Page, SourceRef,
    Text,
};
use helios_core::jobs::Jobs;
use helios_library::fetch::{self, Remote};
use helios_library::import::{Duplicates, Layout, Options, Placement, Report};
use helios_storage::{VolumeKind, registry, strategy};
use helios_vault::{Capture, ImageId, MemoryStore, Presence, Vault};
use serde_json::Value;

struct Place(PathBuf);

impl Place {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "helios-connector-fetch-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A small PNG of its own for each `seed`.
fn png(seed: u32) -> Vec<u8> {
    let picture = image::RgbImage::from_fn(32, 24, |x, y| {
        let v = ((x * (seed + 3) + y * 7 + seed * 11) % 256) as u8;
        image::Rgb([v, v.wrapping_add(40), v / 2])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A remote source in memory.
struct Fake {
    caps: Caps,
    /// Each item's bytes, by id.
    files: BTreeMap<String, Vec<u8>>,
    /// Items whose next fetch stops at this byte with a transient error.
    cuts: Mutex<HashMap<String, u64>>,
    /// Items whose next fetches serve one byte wrong, this many times.
    corrupt: Mutex<HashMap<String, u32>>,
    /// Each fetch call: the item and the offset asked for.
    calls: Mutex<Vec<(String, u64)>>,
    /// Bytes written into sinks.
    served: AtomicU64,
}

impl Fake {
    fn new(originals: Originals, files: &[(&str, Vec<u8>)]) -> Self {
        Self {
            caps: Caps {
                accounts: true,
                claims: Some(HashAlgo::Md5),
                sizes: true,
                resume: true,
                originals,
                ..Caps::default()
            },
            files: files
                .iter()
                .map(|(id, bytes)| ((*id).to_string(), bytes.clone()))
                .collect(),
            cuts: Mutex::default(),
            corrupt: Mutex::default(),
            calls: Mutex::default(),
            served: AtomicU64::new(0),
        }
    }

    fn calls(&self) -> Vec<(String, u64)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Connector for Fake {
    fn describe(&self) -> ConnectorInfo {
        ConnectorInfo {
            id: "fake".into(),
            label: Text::new("connect-fake", "Fake remote"),
            caps: self.caps.clone(),
        }
    }

    fn sources(&self, _account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        Ok(Vec::new())
    }

    fn list(
        &self,
        _source: &SourceRef,
        _from: Option<&Cursor>,
        _page: usize,
        _cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        Err(ConnectorError::Unsupported)
    }

    fn access(&self, _item: &ItemRef) -> Access {
        Access::Fetch
    }

    fn fetch(
        &self,
        item: &ItemRef,
        offset: u64,
        sink: &mut dyn FetchSink,
        _cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        self.calls.lock().unwrap().push((item.id.0.clone(), offset));
        let mut bytes = self
            .files
            .get(&item.id.0)
            .ok_or(ConnectorError::NotFound)?
            .clone();
        if let Some(n) = self.corrupt.lock().unwrap().get_mut(&item.id.0)
            && *n > 0
        {
            *n -= 1;
            let last = bytes.len() - 1;
            bytes[last] ^= 0xff;
        }
        let cut = self.cuts.lock().unwrap().remove(&item.id.0);
        let end = cut.map_or(bytes.len() as u64, |c| c.min(bytes.len() as u64));
        let from = offset.min(end) as usize;
        for chunk in bytes[from..end as usize].chunks(7) {
            sink.accept(chunk)
                .map_err(|e| ConnectorError::Fatal(e.to_string()))?;
            self.served.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        if cut.is_some() {
            return Err(ConnectorError::Transient("connection reset".into()));
        }
        Ok(Fetched {
            bytes: end - offset.min(end),
            mime: Some("image/png".into()),
        })
    }
}

fn source(account: &str) -> SourceRef {
    SourceRef {
        connector: "fake".into(),
        id: account.into(),
        label: format!("Fake: {account}"),
        root: None,
        capacity: None,
    }
}

/// An item of `bytes`, claiming their MD5 (or `claim` when given).
fn item(id: &str, bytes: &[u8], claim: Option<&str>) -> Item {
    Item {
        id: ItemId(id.into()),
        path: format!("Camera/{id}.png"),
        size: Some(bytes.len() as u64),
        modified: Some(1_700_000_000),
        taken: None,
        kind: ItemKind::Photo,
        mime: Some("image/png".into()),
        metadata: BTreeMap::new(),
        claim: Some(Claim {
            algorithm: HashAlgo::Md5,
            digest: claim.map_or_else(
                || digest(&HashAlgo::Md5, bytes).unwrap(),
                ToString::to_string,
            ),
        }),
        revision: None,
        group: None,
    }
}

fn options(root: &Path) -> Options {
    Options {
        // A fetched item is always copied, whatever is asked.
        placement: Placement::Copy {
            into: Some(root.to_path_buf()),
        },
        layout: Layout::Flat,
        duplicates: Duplicates::Skip,
        ..Options::default()
    }
}

fn run(
    vault: &Vault<MemoryStore>,
    fake: &Fake,
    account: &str,
    items: Vec<Item>,
    new_only: bool,
    options: &Options,
) -> (Report, Vec<Value>) {
    let jobs = Jobs::new(std::sync::Arc::default());
    let remote = Remote {
        connector: fake,
        source: source(account),
        items,
        new_only,
    };
    let mut lines = Vec::new();
    let report = fetch::import_remote(vault, &jobs, &remote, options, 1000, &mut |l: &Value| {
        lines.push(l.clone());
    })
    .unwrap();
    (report, lines)
}

fn id_of(bytes: &[u8]) -> ImageId {
    ImageId(helios_core::ContentHash(*blake3::hash(bytes).as_bytes()))
}

/// The part files left in the library folder.
fn parts(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root.join(".helios-part/fetch"))
        .map(|d| d.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default()
}

#[test]
fn fetched_items_become_verified_copies_with_their_claims_and_sightings() {
    let p = Place::new("happy");
    let (a, b) = (png(1), png(2));
    let fake = Fake::new(Originals::Exact, &[("a", a.clone()), ("b", b.clone())]);
    let vault = Vault::new(MemoryStore::default());
    let mut asked = options(&p.0);
    // Even asked to reference, a fetched item is copied (choice C3).
    asked.placement = Placement::Reference;
    helios_library::import::set_library_root(&vault, &p.0).unwrap();
    let (report, lines) = run(
        &vault,
        &fake,
        "me",
        vec![item("a", &a, None), item("b", &b, None)],
        true,
        &asked,
    );
    assert_eq!(
        (report.files, report.added, report.fetched, report.failed),
        (2, 2, 2, 0),
        "{lines:?}"
    );
    assert_eq!(report.copied, 2);
    assert_eq!(report.read, (a.len() + b.len()) as u64);
    for (bytes, name) in [(&a, "a.png"), (&b, "b.png")] {
        let id = id_of(bytes);
        let dest = p.0.join(name);
        // A verified copy in the library folder, flagged in the Library.
        assert_eq!(std::fs::read(&dest).unwrap(), *bytes);
        assert!(vault.in_library(&id).unwrap());
        let flag = vault.library_flag(&id).unwrap().unwrap();
        assert!(flag.by.unwrap().starts_with("import-"));
        // The claim checked is in the table.
        let md5 = digest(&HashAlgo::Md5, bytes).unwrap();
        assert_eq!(vault.claimed("md5", &md5).unwrap(), Some(id));
        // Two places: the copy, present, and the item, offline.
        let locations = vault.locations(&id).unwrap();
        assert_eq!(locations.len(), 2, "{locations:?}");
        assert!(locations.iter().any(|l| l.path == dest && l.present()));
    }
    assert!(lines.iter().all(|l| l["claim"] == "checked"), "{lines:?}");
    // No part file is left, nor its folder.
    assert!(!p.0.join(".helios-part").exists());
    // Undone, the images leave the Library; the copies stay.
    vault.revert(&report.changes).unwrap();
    assert!(!vault.in_library(&id_of(&a)).unwrap());
    assert!(p.0.join("a.png").is_file());
}

#[test]
fn the_remote_source_is_a_volume_and_its_items_sightings_on_it() {
    let p = Place::new("volume");
    let a = png(3);
    let fake = Fake::new(Originals::Exact, &[("a", a.clone())]);
    let vault = Vault::new(MemoryStore::default());
    run(
        &vault,
        &fake,
        "me@example.org",
        vec![item("a", &a, None)],
        true,
        &options(&p.0),
    );
    let volumes = registry::known(&vault).unwrap();
    assert_eq!(volumes.len(), 1);
    let volume = &volumes[0];
    assert_eq!(volume.id.as_str(), "remote:fake:me@example.org");
    assert_eq!(volume.kind, VolumeKind::Remote);
    assert_eq!(volume.connector(), Some("fake"));
    assert_eq!(volume.name, "Fake: me@example.org");
    assert_eq!(volume.last_seen.as_ref().map(|s| s.at), Some(1000));
    // A remote source never takes a backup.
    let mut backup = volume.clone();
    backup.roles.insert(helios_storage::Role::Backup);
    assert!(strategy::may_back_up_to(&backup).is_err());
    // The item is a sighting on it, at its id, linked to its image.
    let seen = vault
        .sighting_at(Some(&volume.id), Some(Path::new("a")), Path::new(""))
        .unwrap()
        .expect("a sighting at the item id");
    assert_eq!(seen.image, Some(id_of(&a)));
    assert_eq!(seen.path, PathBuf::from("Camera/a.png"));
    assert_eq!(seen.size, a.len() as u64);
    assert_eq!(seen.presence, Presence::Offline);
    assert_eq!(seen.seen_by, "import");
}

#[test]
fn a_fetch_cut_halfway_goes_on_from_the_bytes_landed() {
    let p = Place::new("resume");
    let a = png(4);
    let half = a.len() as u64 / 2;
    let fake = Fake::new(Originals::Exact, &[("a", a.clone())]);
    fake.cuts.lock().unwrap().insert("a".to_string(), half);
    let vault = Vault::new(MemoryStore::default());
    let (report, lines) = run(
        &vault,
        &fake,
        "me",
        vec![item("a", &a, None)],
        true,
        &options(&p.0),
    );
    assert_eq!(report.added, 1, "{lines:?}");
    assert_eq!(
        fake.calls(),
        [("a".to_string(), 0), ("a".to_string(), half)]
    );
    // Each byte crossed once.
    assert_eq!(fake.served.load(Ordering::Relaxed), a.len() as u64);
    assert_eq!(report.read, a.len() as u64);
    assert_eq!(lines[0]["resumed_at"], half);
    assert_eq!(lines[0]["claim"], "checked");
    assert_eq!(std::fs::read(p.0.join("a.png")).unwrap(), a);
}

/// A remote that cuts each fetch at a byte, then is gone.
struct Cutting<'a>(&'a Fake, u64);
impl Connector for Cutting<'_> {
    fn describe(&self) -> ConnectorInfo {
        self.0.describe()
    }
    fn sources(&self, a: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        self.0.sources(a)
    }
    fn list(
        &self,
        s: &SourceRef,
        f: Option<&Cursor>,
        n: usize,
        c: &Cancel,
    ) -> Result<Page, ConnectorError> {
        self.0.list(s, f, n, c)
    }
    fn access(&self, i: &ItemRef) -> Access {
        self.0.access(i)
    }
    fn fetch(
        &self,
        item: &ItemRef,
        offset: u64,
        sink: &mut dyn FetchSink,
        cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        self.0
            .cuts
            .lock()
            .unwrap()
            .insert(item.id.0.clone(), self.1);
        let _ = self.0.fetch(item, offset, sink, cancel);
        Err(ConnectorError::Fatal("gone for the day".into()))
    }
}

#[test]
fn a_part_left_by_an_earlier_run_is_resumed() {
    let p = Place::new("resume-later");
    let a = png(5);
    let half = a.len() / 2;
    let fake = Fake::new(Originals::Exact, &[("a", a.clone())]);
    // Cut at half, then gone for the day: the run gives up and keeps the
    // part.
    let vault = Vault::new(MemoryStore::default());
    let jobs = Jobs::new(std::sync::Arc::default());
    let cutting = Cutting(&fake, half as u64);
    let remote = Remote {
        connector: &cutting,
        source: source("me"),
        items: vec![item("a", &a, None)],
        new_only: true,
    };
    let report =
        fetch::import_remote(&vault, &jobs, &remote, &options(&p.0), 1000, &mut |_| {}).unwrap();
    assert_eq!(report.failed, 1);
    let left = parts(&p.0);
    assert_eq!(left.len(), 1, "the part is kept");
    assert_eq!(std::fs::read(&left[0]).unwrap(), a[..half]);
    // The next run goes on from it.
    fake.calls.lock().unwrap().clear();
    let (report, lines) = run(
        &vault,
        &fake,
        "me",
        vec![item("a", &a, None)],
        true,
        &options(&p.0),
    );
    assert_eq!(report.added, 1, "{lines:?}");
    assert_eq!(fake.calls(), [("a".to_string(), half as u64)]);
    assert_eq!(std::fs::read(p.0.join("a.png")).unwrap(), a);
    assert!(parts(&p.0).is_empty());
}

#[test]
fn a_lying_claim_is_fetched_again_once_then_the_item_fails() {
    let p = Place::new("mismatch");
    let (a, b) = (png(6), png(7));
    let fake = Fake::new(Originals::Exact, &[("a", a.clone()), ("b", b.clone())]);
    let vault = Vault::new(MemoryStore::default());
    let (report, lines) = run(
        &vault,
        &fake,
        "me",
        vec![
            item("a", &a, Some("00000000000000000000000000000000")),
            item("b", &b, None),
        ],
        true,
        &options(&p.0),
    );
    assert_eq!(
        (report.failed, report.added, report.retried),
        (1, 1, 1),
        "{lines:?}"
    );
    // Twice from zero, then given up; the other item goes on.
    assert_eq!(
        fake.calls(),
        [
            ("a".to_string(), 0),
            ("a".to_string(), 0),
            ("b".to_string(), 0)
        ]
    );
    let failed = &lines[0];
    assert_eq!(failed["status"], "failed");
    assert!(
        failed["error"]
            .as_str()
            .unwrap()
            .contains("not the ones announced"),
        "{failed}"
    );
    // Nothing half-verified becomes an image, and no part is left.
    assert!(!vault.in_library(&id_of(&a)).unwrap());
    assert!(!p.0.join("a.png").exists());
    assert!(parts(&p.0).is_empty());
}

#[test]
fn bytes_damaged_once_are_fetched_again_and_kept_when_right() {
    let p = Place::new("damaged");
    let a = png(8);
    let fake = Fake::new(Originals::Exact, &[("a", a.clone())]);
    fake.corrupt.lock().unwrap().insert("a".to_string(), 1);
    let vault = Vault::new(MemoryStore::default());
    let (report, lines) = run(
        &vault,
        &fake,
        "me",
        vec![item("a", &a, None)],
        true,
        &options(&p.0),
    );
    assert_eq!((report.added, report.retried), (1, 1), "{lines:?}");
    assert!(lines[0]["retried"].as_str().unwrap().contains("md5"));
    assert_eq!(std::fs::read(p.0.join("a.png")).unwrap(), a);
}

#[test]
fn duplicates_are_caught_by_item_id_and_by_claimed_hash_before_any_byte() {
    let p = Place::new("claimed");
    let a = png(9);
    let first = Fake::new(Originals::Exact, &[("a", a.clone())]);
    let vault = Vault::new(MemoryStore::default());
    run(
        &vault,
        &first,
        "me",
        vec![item("a", &a, None)],
        true,
        &options(&p.0),
    );
    // The same item listed again: known by its id.
    first.calls.lock().unwrap().clear();
    let (report, lines) = run(
        &vault,
        &first,
        "me",
        vec![item("a", &a, None)],
        true,
        &options(&p.0),
    );
    assert_eq!((report.skipped, report.unfetched), (1, 1), "{lines:?}");
    assert_eq!(lines[0]["duplicate"]["of"], "item");
    assert!(first.calls().is_empty());
    // Another account holds the same bytes under another id: its claim
    // names the image.
    let other = Fake::new(Originals::Exact, &[("x9", a.clone())]);
    let (report, lines) = run(
        &vault,
        &other,
        "work",
        vec![item("x9", &a, None)],
        true,
        &options(&p.0),
    );
    assert_eq!((report.skipped, report.unfetched), (1, 1), "{lines:?}");
    assert_eq!(lines[0]["duplicate"]["of"], "claim");
    assert_eq!(lines[0]["duplicate"]["before_fetch"], true);
    assert!(other.calls().is_empty());
    assert_eq!(other.served.load(Ordering::Relaxed), 0);
    assert_eq!(report.read, 0);
    // Its sighting on the other account's volume is linked all the same.
    let seen = vault
        .sighting_at(
            Some(&helios_storage::VolumeId("remote:fake:work".into())),
            Some(Path::new("x9")),
            Path::new(""),
        )
        .unwrap()
        .unwrap();
    assert_eq!(seen.image, Some(id_of(&a)));
    // Asked, it is listed for an answer, still not fetched.
    let mut ask = options(&p.0);
    ask.duplicates = Duplicates::Ask;
    let (report, _) = run(
        &vault,
        &other,
        "work",
        vec![item("x9", &a, None)],
        true,
        &ask,
    );
    assert_eq!(report.asked, 1);
    assert!(other.calls().is_empty());
    // Kept on purpose, it is fetched: one more copy.
    let mut keep = options(&p.0);
    keep.duplicates = Duplicates::Keep;
    let (report, lines) = run(
        &vault,
        &other,
        "work",
        vec![item("x9", &a, None)],
        true,
        &keep,
    );
    assert_eq!(report.fetched, 1, "{lines:?}");
    assert_eq!(other.calls().len(), 1);
}

#[test]
fn a_probable_match_is_not_fetched_from_a_re_encoding_source_but_is_from_an_exact_one() {
    let p = Place::new("probable");
    let original = png(10);
    let vault = Vault::new(MemoryStore::default());
    // The original in the Library, taken at a known time by a known camera.
    let first = Fake::new(Originals::Exact, &[("o", original.clone())]);
    run(
        &vault,
        &first,
        "card",
        vec![item("o", &original, None)],
        true,
        &options(&p.0),
    );
    let id = id_of(&original);
    let capture = Capture {
        captured: Some(1_600_000_000.0),
        model: Some("EOS R6".into()),
        width: Some(6000),
        height: Some(4000),
        ..Capture::default()
    };
    vault.set_captures(&[(id, capture)], None).unwrap();
    // A service's re-encoded copy: other bytes, no claim, the same moment.
    let rendered = png(11);
    let listed = |bytes: &[u8]| {
        let mut i = item("g1", bytes, None);
        i.claim = None;
        i.taken = Some(1_600_000_000);
        i.metadata.insert("tiff:Model".into(), "EOS R6".into());
        i.metadata
            .insert("exif:PixelXDimension".into(), "4000".into());
        i.metadata
            .insert("exif:PixelYDimension".into(), "6000".into());
        i
    };
    let service = Fake::new(Originals::Rendered, &[("g1", rendered.clone())]);
    let (report, lines) = run(
        &vault,
        &service,
        "me",
        vec![listed(&rendered)],
        true,
        &options(&p.0),
    );
    assert_eq!((report.skipped, report.unfetched), (1, 1), "{lines:?}");
    assert_eq!(lines[0]["duplicate"]["of"], "probable");
    assert_eq!(lines[0]["duplicate"]["image"], id.hex());
    assert!(service.calls().is_empty());
    // Listed, not linked: a probable match is never final.
    let seen = vault
        .sighting_at(
            Some(&helios_storage::VolumeId("remote:fake:me".into())),
            Some(Path::new("g1")),
            Path::new(""),
        )
        .unwrap()
        .unwrap();
    assert_eq!(seen.image, None);
    // Ticked by the person ("New only" off), it is fetched.
    let (report, _) = run(
        &vault,
        &service,
        "me",
        vec![listed(&rendered)],
        false,
        &options(&p.0),
    );
    assert_eq!(report.added, 1);
    // An exact-original source without a claim is fetched and settled by
    // its BLAKE3: the very original is a duplicate after the download.
    let phone = Fake::new(Originals::Exact, &[("g1", original.clone())]);
    let (report, lines) = run(
        &vault,
        &phone,
        "phone",
        vec![listed(&original)],
        true,
        &options(&p.0),
    );
    assert_eq!((report.fetched, report.skipped), (1, 1), "{lines:?}");
    assert_eq!(lines[0]["duplicate"]["of"], "library");
    assert_eq!(phone.calls().len(), 1);
}

/// What the import window marks "in your Library" before any byte is
/// fetched: an item imported before (by its id, unchanged) or whose claim
/// names a Library image; a new item is not.
#[test]
fn in_library_tells_the_items_known_before_any_byte() {
    let p = Place::new("in-library");
    let (a, b) = (png(21), png(22));
    let fake = Fake::new(Originals::Exact, &[("a", a.clone())]);
    let vault = Vault::new(MemoryStore::default());
    let listed = [
        item("a", &a, None),
        item("b", &b, None),
        item("x", &a, None),
    ];
    let before = fetch::in_library(&vault, &source("me"), &listed).unwrap();
    assert_eq!(before, [false, false, false]);
    run(
        &vault,
        &fake,
        "me",
        vec![item("a", &a, None)],
        true,
        &options(&p.0),
    );
    let fetched = fake.calls().len();
    // `a` by its id, `x` by its claim (the same bytes); `b` is new.
    let after = fetch::in_library(&vault, &source("me"), &listed).unwrap();
    assert_eq!(after, [true, false, true]);
    assert_eq!(fake.calls().len(), fetched, "nothing fetched to tell");
}
