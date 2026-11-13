//! The Picker flow against a fake Photos Picker API, then the picked
//! items imported through `helios_library::fetch::import_remote` as any
//! fetched source: a session created, polled until the picking is done,
//! its items listed page by page, their originals downloaded and
//! imported as verified copies, the session deleted. A refused token is
//! "sign-in needed", asked once and never retried.

mod fake_picker;

use std::path::{Path, PathBuf};
use std::time::Duration;

use fake_picker::{FakePicker, TOKEN};
use helios_connect::{Cancel, Connector, ConnectorError, ItemKind, Originals};
use helios_core::jobs::Jobs;
use helios_library::fetch::{self, Remote};
use helios_library::import::{Duplicates, Layout, Options, Placement};
use helios_plugin_google_photos::{Download, GooglePhotos, Picker, source};
use helios_vault::{MemoryStore, Vault};

struct Place(PathBuf);

impl Place {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "helios-google-photos-{name}-{}",
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
fn picked_items_are_listed_page_by_page_downloaded_and_imported() {
    let p = Place::new("import");
    let (a, b, c) = (png(1), png(2), png(3));
    let fake = FakePicker::start(
        3,
        vec![
            ("m1", "IMG_1.png", false, a.clone()),
            ("m2", "IMG_2.png", false, b.clone()),
            ("m3", "IMG_3.png", false, c.clone()),
        ],
    );
    let picker = Picker::new(&fake.base, TOKEN);
    let session = picker.create(None).unwrap();
    assert!(session.picker_uri.ends_with("/pick/s1"));
    assert!(session.autoclose_uri().ends_with("/pick/s1/autoclose"));
    assert!(!session.items_set);
    let set = picker
        .wait(&session.id, Some(Duration::from_secs(20)), &Cancel::new())
        .unwrap()
        .expect("picked before the deadline");
    assert!(set.items_set);

    let photos = GooglePhotos::new(picker, &session.id);
    assert_eq!(photos.describe().caps.originals, Originals::Rendered);
    let account = source("me@example.org", "Google Photos");
    let items = photos.list_all(&account, &Cancel::new()).unwrap();
    let ids: Vec<&str> = items.iter().map(|i| i.id.0.as_str()).collect();
    assert_eq!(ids, ["m1", "m2", "m3"]);
    assert!(
        items
            .iter()
            .all(|i| i.claim.is_none() && i.kind == ItemKind::Photo)
    );
    assert_eq!(items[0].metadata["tiff:Model"], "Pixel 8");

    let vault = Vault::new(MemoryStore::default());
    let jobs = Jobs::new(std::sync::Arc::default());
    let remote = Remote {
        connector: &photos,
        source: account,
        items,
        new_only: false,
    };
    let report =
        fetch::import_remote(&vault, &jobs, &remote, &options(&p.0), 1000, &mut |_| {}).unwrap();
    assert_eq!((report.added, report.failed), (3, 0), "{report:?}");
    assert_eq!(std::fs::read(p.0.join("IMG_1.png")).unwrap(), a);
    assert_eq!(std::fs::read(p.0.join("IMG_3.png")).unwrap(), c);

    photos.picker().delete(&session.id).unwrap();
    let requests = fake.requests();
    assert!(requests.contains(&"GET /v1/mediaItems?sessionId=s1&pageSize=100".to_string()));
    assert!(
        requests.contains(&"GET /v1/mediaItems?sessionId=s1&pageSize=100&pageToken=2".to_string())
    );
    assert!(requests.contains(&"GET /media/m2=d".to_string()));
    assert_eq!(requests.last().unwrap(), "DELETE /v1/sessions/s1");
    let polls = requests
        .iter()
        .filter(|r| *r == "GET /v1/sessions/s1")
        .count();
    assert_eq!(polls, 3, "polled until the items were set");
}

#[test]
fn listing_before_the_picking_is_done_is_transient() {
    let fake = FakePicker::start(1000, vec![("m1", "a.png", false, png(1))]);
    let picker = Picker::new(&fake.base, TOKEN);
    let session = picker.create(Some(10)).unwrap();
    assert!(matches!(
        picker.list(&session.id, None, 50),
        Err(ConnectorError::Transient(_))
    ));
    // The deadline comes first: nothing picked, no error.
    let waited = picker
        .wait(&session.id, Some(Duration::from_millis(60)), &Cancel::new())
        .unwrap();
    assert_eq!(waited, None);
}

#[test]
fn an_expired_session_is_said_expired_and_the_next_picking_is_a_new_session() {
    let fake = FakePicker::expiring(
        1000,
        vec![("m1", "a.png", false, png(1))],
        "2020-01-01T00:00:00Z",
    );
    let picker = Picker::new(&fake.base, TOKEN);
    let first = picker.create(None).unwrap();
    assert_eq!(
        first.expires_in(helios_plugin_google_photos::epoch("2021-01-01T00:00:00Z").unwrap()),
        Some(Duration::ZERO)
    );
    // Said at once, not waited out to the deadline.
    let started = std::time::Instant::now();
    assert_eq!(
        picker.wait(&first.id, Some(Duration::from_secs(20)), &Cancel::new()),
        Err(ConnectorError::Expired)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    // Picking again creates a session of its own, its own page.
    let second = picker.create(None).unwrap();
    assert_ne!(second.id, first.id);
    assert_ne!(second.picker_uri, first.picker_uri);
    let posts = fake
        .requests()
        .iter()
        .filter(|r| *r == "POST /v1/sessions")
        .count();
    assert_eq!(posts, 2);
}

#[test]
fn a_session_lasts_until_its_expire_time() {
    let fake = FakePicker::expiring(1, Vec::new(), "2030-01-01T00:00:10Z");
    let session = Picker::new(&fake.base, TOKEN).create(None).unwrap();
    let at = helios_plugin_google_photos::epoch("2030-01-01T00:00:00Z").unwrap();
    assert_eq!(session.expires_in(at), Some(Duration::from_secs(10)));
    assert_eq!(session.expires_in(at + 60), Some(Duration::ZERO));
}

#[test]
fn a_refused_token_asks_for_a_reconnection_without_retrying() {
    let fake = FakePicker::start(1, vec![("m1", "a.png", false, png(1))]);
    let refused = Picker::new(&fake.base, "expired");
    assert_eq!(refused.create(None), Err(ConnectorError::AuthNeeded));
    assert_eq!(
        refused.list("s1", None, 10),
        Err(ConnectorError::AuthNeeded)
    );
    let photos = GooglePhotos::new(Picker::new(&fake.base, "expired"), "s1");
    assert_eq!(
        photos.probe(None, &Cancel::new()),
        Err(ConnectorError::AuthNeeded)
    );
    let download = Download {
        base_url: format!("{}/media/m1", fake.base.trim_end_matches("/v1")),
        video: false,
    };
    let mut sink = Vec::new();
    assert_eq!(
        refused.download(&download, &mut sink, &Cancel::new()),
        Err(ConnectorError::AuthNeeded)
    );
    assert!(sink.is_empty());
    assert_eq!(
        fake.requests()
            .iter()
            .filter(|r| r.starts_with("GET /media/"))
            .count(),
        1,
        "asked once, not retried"
    );
}

#[test]
fn the_manifest_declares_the_google_sign_in_then_a_test() {
    let text = include_str!("../helios-plugin.toml");
    let manifest = helios_plugin::manifest::Manifest::parse(text).unwrap();
    let steps: Vec<helios_connect::ConnectStep> = manifest
        .connect
        .iter()
        .map(|s| serde_json::from_value(serde_json::to_value(s).unwrap()).unwrap())
        .collect();
    assert_eq!(steps.len(), 2);
    let helios_connect::StepKind::Oauth {
        authorize,
        token,
        scopes,
        revoke,
        client_id,
    } = &steps[0].kind
    else {
        panic!("{:?}", steps[0]);
    };
    assert_eq!(authorize, "https://accounts.google.com/o/oauth2/v2/auth");
    assert_eq!(token, "https://oauth2.googleapis.com/token");
    assert_eq!(
        revoke.as_deref(),
        Some("https://oauth2.googleapis.com/revoke")
    );
    assert_eq!(scopes, &helios_plugin_google_photos::SCOPES);
    assert!(
        helios_connect::oauth::asks_email(&steps),
        "the account shown"
    );
    assert_eq!(client_id, &None, "never built in");
    assert_eq!(steps[1].kind, helios_connect::StepKind::Test);
}

/// The scopes Google's verification and the privacy page list are the
/// ones the sign-in asks, no more, no fewer.
#[test]
fn the_scopes_documented_are_the_ones_the_code_asks() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
    let doc = read("docs/google-verification.md");
    let privacy = read("site/privacy.html");
    let section = doc
        .split("**Data access:**")
        .nth(1)
        .and_then(|s| s.split("4. **Audience").next())
        .expect("the data access step");
    let listed: Vec<&str> = section.split('`').skip(1).step_by(2).collect();
    assert_eq!(listed, helios_plugin_google_photos::SCOPES, "{section}");
    for scope in helios_plugin_google_photos::SCOPES {
        let short = scope.rsplit('/').next().unwrap();
        assert!(
            privacy.contains(&format!("<code>{short}</code>")),
            "the privacy page names {short}"
        );
    }
}
