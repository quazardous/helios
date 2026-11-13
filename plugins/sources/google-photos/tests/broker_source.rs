//! The plugin's program driven through the plugin broker as an import
//! connector (`helios_plugin::source`), against a fake Photos Picker
//! API: a picking opened (its page handed to the opener, never a real
//! browser), waited for, listed and imported through
//! `helios_library::fetch::import_remote`; two pickings adding to one
//! source without duplicates, an expired download address renewed on the
//! way; a cancelled wait; a refused token leaving the account needing
//! attention.

mod fake_picker;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fake_picker::{FakePicker, TOKEN};
use helios_connect::account::{Account, AccountBook, AccountState, MemoryAccounts};
use helios_connect::{Cancel, Connector, ConnectorError, ConnectorProvider, TokenFn};
use helios_core::jobs::Jobs;
use helios_library::fetch::{self, Remote};
use helios_library::import::{Duplicates, Layout, Options, Placement};
use helios_plugin::source::{self, SourceProvider};
use helios_plugin::{Broker, Policy, Scope};
use helios_vault::{MemoryStore, Vault};

const PLUGIN: &str = "io.github.quazardous.helios.google-photos";

struct Place(PathBuf);

impl Place {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "helios-gphotos-broker-{name}-{}",
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

fn png(seed: u32) -> Vec<u8> {
    let picture = image::RgbImage::from_fn(32, 24, |x, y| {
        let v = ((x * (seed + 5) + y * 3 + seed * 13) % 256) as u8;
        image::Rgb([v, v.wrapping_add(60), v / 3])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// The plugin installed in `place` (its manifest pointing at `api`, its
/// program the one this crate built), found by a broker, its
/// capabilities granted.
fn broker(place: &Path, api: &str) -> Arc<Broker> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = place.join("plugins").join("google-photos");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = std::fs::read_to_string(here.join("helios-plugin.toml"))
        .unwrap()
        .replace("https://photospicker.googleapis.com/v1", api);
    std::fs::write(dir.join("helios-plugin.toml"), manifest).unwrap();
    let program = dir.join(format!(
        "helios-google-photos{}",
        std::env::consts::EXE_SUFFIX
    ));
    std::fs::copy(env!("CARGO_BIN_EXE_helios-google-photos"), &program).unwrap();
    let mut broker = Broker::new(Policy::default());
    broker.discover(&[(Scope::User, place.join("plugins"))]);
    assert!(broker.ids().any(|i| i == PLUGIN), "{:?}", broker.rejected());
    broker.grant(PLUGIN, "network").unwrap();
    broker.grant(PLUGIN, "folder.write").unwrap();
    Arc::new(broker)
}

fn token(value: &'static str) -> TokenFn {
    Arc::new(move |_: &str| Ok(value.to_owned()))
}

fn options(root: &Path) -> Options {
    Options {
        placement: Placement::Copy {
            into: Some(root.to_path_buf()),
        },
        layout: Layout::Flat,
        duplicates: Duplicates::Skip,
        ..Options::default()
    }
}

#[test]
fn a_picking_through_the_broker_imports_the_two_photos_picked() {
    let p = Place::new("pick");
    let (a, b) = (png(1), png(2));
    let fake = FakePicker::start(
        2,
        vec![
            ("m1", "IMG_1.png", false, a.clone()),
            ("m2", "IMG_2.png", false, b.clone()),
        ],
    );
    let broker = broker(&p.0, &fake.base);
    let providers = source::providers(&broker);
    assert_eq!(providers.len(), 1);
    let provider = &providers[0];
    let manifest = provider.manifest();
    assert_eq!(manifest.id, "google-photos");
    assert!(manifest.caps.picks && manifest.caps.accounts);
    let steps: Vec<&str> = manifest.connect.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(steps, ["google", "probe"]);

    let photos = provider.connector(token(TOKEN));
    let account = Account::new("google-photos", "Google Photos", &[]).account_ref();
    photos.probe(Some(&account), &Cancel::new()).unwrap();
    let opened = Mutex::new(Vec::new());
    let (source, items) = source::pick_and_list(
        &photos,
        &account,
        Instant::now() + Duration::from_secs(30),
        &Cancel::new(),
        &|url| opened.lock().unwrap().push(url.to_owned()),
    )
    .unwrap()
    .expect("picked before the deadline");
    let opened = opened.into_inner().unwrap();
    assert_eq!(opened.len(), 1);
    // The probe's session was s1: the picking has one of its own.
    assert!(opened[0].ends_with("/pick/s2/autoclose"), "{opened:?}");
    assert_eq!(source.connector, "google-photos");
    assert_eq!(source.id, account.id);
    let ids: Vec<&str> = items.iter().map(|i| i.id.0.as_str()).collect();
    assert_eq!(ids, ["m1", "m2"]);
    assert_eq!(items[0].metadata["tiff:Model"], "Pixel 8");

    let vault = Vault::new(MemoryStore::default());
    let jobs = Jobs::new(Arc::default());
    let remote = Remote {
        connector: &photos,
        source,
        items,
        new_only: false,
    };
    let into = p.0.join("library");
    let report =
        fetch::import_remote(&vault, &jobs, &remote, &options(&into), 1000, &mut |_| {}).unwrap();
    assert_eq!((report.added, report.failed), (2, 0), "{report:?}");
    assert_eq!(std::fs::read(into.join("IMG_1.png")).unwrap(), a);
    assert_eq!(std::fs::read(into.join("IMG_2.png")).unwrap(), b);
    photos.end_pick().unwrap();
    let requests = fake.requests();
    assert!(requests.contains(&"GET /media/m2=d".to_string()));
    assert_eq!(requests.last().unwrap(), "DELETE /v1/sessions/s2");
}

#[test]
fn two_pickings_add_to_one_source_without_duplicates_and_expired_addresses_are_renewed() {
    let p = Place::new("again");
    let (a, b, c) = (png(1), png(2), png(3));
    let fake = FakePicker::start(1, vec![("m1", "IMG_1.png", false, a.clone())]);
    let broker = broker(&p.0, &fake.base);
    let photos = SourceProvider::new(broker, PLUGIN)
        .unwrap()
        .connector(token(TOKEN));
    let account = Account::new("google-photos", "Google Photos", &[]).account_ref();
    let vault = Vault::new(MemoryStore::default());
    let jobs = Jobs::new(Arc::default());
    let into = p.0.join("library");
    let pick = || {
        let (source, items) = source::pick_and_list(
            &photos,
            &account,
            Instant::now() + Duration::from_secs(30),
            &Cancel::new(),
            &|_| {},
        )
        .unwrap()
        .expect("picked");
        let remote = Remote {
            connector: &photos,
            source,
            items,
            new_only: false,
        };
        let report =
            fetch::import_remote(&vault, &jobs, &remote, &options(&into), 1000, &mut |_| {})
                .unwrap();
        photos.end_pick().unwrap();
        report
    };
    let first = pick();
    assert_eq!((first.added, first.failed), (1, 0), "{first:?}");
    // A new picking, whenever wanted: m1 again, already imported, and two
    // new ones, the first download's address expired meanwhile.
    fake.pick(vec![
        ("m1", "IMG_1.png", false, a.clone()),
        ("m2", "IMG_2.png", false, b.clone()),
        ("m3", "IMG_3.png", false, c.clone()),
    ]);
    fake.expire_downloads(1);
    let second = pick();
    assert_eq!((second.added, second.failed), (2, 0), "{second:?}");
    assert_eq!(second.unfetched, 1, "m1 known by its id: {second:?}");
    assert_eq!(std::fs::read(into.join("IMG_2.png")).unwrap(), b);
    assert_eq!(std::fs::read(into.join("IMG_3.png")).unwrap(), c);
    let requests = fake.requests();
    let sessions: Vec<&String> = requests
        .iter()
        .filter(|r| r.as_str() == "POST /v1/sessions")
        .collect();
    assert_eq!(sessions.len(), 2, "a session of its own each time");
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.as_str() == "GET /media/m1=d")
            .count(),
        1,
        "m1 fetched once: {requests:?}"
    );
    // The expired address made the picking be listed again.
    let listings = requests
        .iter()
        .filter(|r| r.starts_with("GET /v1/mediaItems?sessionId=s2"))
        .count();
    assert!(
        listings >= 3,
        "listed, then again for fresh addresses: {requests:?}"
    );
}

#[test]
fn a_cancelled_wait_ends_the_picking() {
    let p = Place::new("cancel");
    // Never picked: the wait goes on until cancelled.
    let fake = FakePicker::start(u32::MAX, vec![("m1", "a.png", false, png(1))]);
    let broker = broker(&p.0, &fake.base);
    let photos = SourceProvider::new(broker, PLUGIN)
        .unwrap()
        .connector(token(TOKEN));
    let account = Account::new("google-photos", "Google Photos", &[]).account_ref();
    let cancel = Cancel::new();
    {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            cancel.cancel();
        });
    }
    let started = Instant::now();
    let waited = source::pick_and_list(
        &photos,
        &account,
        Instant::now() + Duration::from_mins(1),
        &cancel,
        &|_| {},
    );
    assert_eq!(waited.unwrap_err(), ConnectorError::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "cancelled at once"
    );
    let requests = fake.requests();
    assert!(!requests.iter().any(|r| r.starts_with("GET /v1/mediaItems")));
    // The session is deleted once the plugin answers its last wait.
    let deadline = Instant::now() + Duration::from_secs(20);
    while !fake
        .requests()
        .contains(&"DELETE /v1/sessions/s1".to_string())
    {
        assert!(Instant::now() < deadline, "the picking ended");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn an_expired_picking_session_is_said_expired_through_the_broker() {
    let p = Place::new("expired");
    let fake = FakePicker::expiring(u32::MAX, vec![], "2020-01-01T00:00:00Z");
    let broker = broker(&p.0, &fake.base);
    let photos = SourceProvider::new(broker, PLUGIN)
        .unwrap()
        .connector(token(TOKEN));
    let account = Account::new("google-photos", "Google Photos", &[]).account_ref();
    let started = Instant::now();
    let picked = source::pick_and_list(
        &photos,
        &account,
        Instant::now() + Duration::from_mins(1),
        &Cancel::new(),
        &|_| {},
    );
    assert_eq!(picked.unwrap_err(), ConnectorError::Expired);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "said at once, not waited out"
    );
}

#[test]
fn a_refused_token_leaves_the_account_needing_a_reconnection() {
    let p = Place::new("auth");
    let fake = FakePicker::start(1, vec![]);
    let broker = broker(&p.0, &fake.base);
    let book = MemoryAccounts::default();
    let mut account = Account::new("google-photos", "Google Photos", &[]);
    account.state = AccountState::Connected;
    book.save(&account).unwrap();
    let photos = SourceProvider::new(broker, PLUGIN)
        .unwrap()
        .connector(token("revoked"));
    let picked = source::pick_and_list(
        &photos,
        &account.account_ref(),
        Instant::now() + Duration::from_secs(30),
        &Cancel::new(),
        &|_| panic!("no page opened"),
    );
    assert_eq!(picked.unwrap_err(), ConnectorError::AuthNeeded);
    assert_eq!(fake.requests().len(), 1, "one refused call, no retry");
    let marked =
        helios_connect::account::needs_attention(&book, &account.id, "sign-in needed").unwrap();
    assert_eq!(marked.state.name(), "needs-attention");
    assert_eq!(
        book.account(&account.id).unwrap().unwrap().state.name(),
        "needs-attention"
    );
}

#[test]
fn glances_come_from_the_previews_within_the_declared_limits_expired_addresses_renewed() {
    let p = Place::new("glance");
    let a = png(4);
    let fake = FakePicker::start(1, vec![("m1", "IMG_1.png", false, a.clone())]);
    let broker = broker(&p.0, &fake.base);
    let provider = SourceProvider::new(broker, PLUGIN).unwrap();
    // The manifest declares the previews and the limits the read-ahead
    // keeps to, the same as the connector's own.
    let caps = provider.manifest().caps;
    assert!(caps.thumbnails);
    assert_eq!(caps.limits, helios_plugin_google_photos::LIMITS);
    let photos = provider.connector(token(TOKEN));
    assert_eq!(photos.describe().caps, caps);
    let account = Account::new("google-photos", "Google Photos", &[]).account_ref();
    let (source, items) = source::pick_and_list(
        &photos,
        &account,
        Instant::now() + Duration::from_secs(30),
        &Cancel::new(),
        &|_| {},
    )
    .unwrap()
    .expect("picked");
    let item = helios_connect::ItemRef {
        source,
        id: items[0].id.clone(),
    };
    assert_eq!(photos.thumbnail(&item, 256).unwrap(), Some(a.clone()));
    // An address past its hour: the picking listed again, then the
    // preview read at the fresh one.
    fake.expire_downloads(1);
    assert_eq!(photos.thumbnail(&item, 256).unwrap(), Some(a));
    photos.end_pick().unwrap();
    let requests = fake.requests();
    let previews = requests
        .iter()
        .filter(|r| r.as_str() == "GET /media/m1=w256-h256")
        .count();
    assert_eq!(
        previews, 3,
        "one, then expired and asked again: {requests:?}"
    );
    assert!(
        !requests.iter().any(|r| r == "GET /media/m1=d"),
        "{requests:?}"
    );
}
