//! The node's background works as the Activity focus shows them: the
//! image map's job and the thumbnail re-check, started as at launch on a
//! small library (in memory, its thumbnail cache in a temporary folder),
//! each say what they do or what they wait on within seconds, never
//! "Running, 0 so far" with nothing said.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_core::jobs::Jobs;
use helios_gui::{Shell, headless, helios_panels};
use helios_inspect::client::Client;
use helios_library::map::Record;
use helios_library::map_job::Feed;
use helios_node_core::endpoint::VaultService;
use helios_node_core::map::{ImageMap, MapSettings, THUMBS_KIND, ThumbCheck};
use helios_node_core::node::NodeJobs as _;
use helios_node_core::promises::Promises;
use helios_shell::commands::Origin;
use helios_vault::promises::{Filter, Promise, State};
use helios_vault::{MemoryStore, Vault};
use serde_json::json;

/// The standing promise of `kind`, if there is one.
fn standing(vault: &Vault<MemoryStore>, kind: &str) -> Option<Promise> {
    vault
        .promises(&Filter {
            kind: Some(kind.into()),
            ..Filter::default()
        })
        .unwrap()
        .into_iter()
        .next()
}

/// The note of a running promise.
fn note(p: &Promise) -> Option<&str> {
    match &p.state {
        State::Running { note, .. } => note.as_deref(),
        _ => None,
    }
}

#[test]
fn the_image_map_and_the_thumbnail_recheck_say_what_they_do_or_wait_on() {
    let dir =
        std::env::temp_dir().join(format!("helios-gui-background-jobs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bus = Arc::new(helios_core::Bus::new());
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let jobs = Jobs::new(bus.clone());
    let promises = Arc::new(Promises::new(vault.clone(), jobs.clone()));
    let service =
        VaultService::new(vault.clone(), bus, jobs.clone(), "test").with_promises(promises);
    let client = Arc::new(Client::in_process(Arc::new(service)).with_origin("user"));
    // As at launch: the map's job and the re-check over the library's
    // thumbnail cache.
    let cache = Arc::new(helios_thumbs::ThumbCache::open(&dir.join("thumbs"), 1 << 20).unwrap());
    let listed = cache.clone();
    let thumbs: ThumbCheck = (Box::new(cache), Box::new(move || listed.list()));
    let record = Record::parse("import");
    let settings = MapSettings {
        record,
        defaults: true,
        exclude: String::new(),
        prune: helios_library::map_job::Settings {
            limit: 1000,
            keep_missing_days: 30,
            record,
        },
    };
    let map = ImageMap::start(
        vault.clone(),
        jobs,
        settings,
        vec![dir.clone()],
        Feed::default(),
        Some(thumbs),
    );
    let mut shell = Shell::new(helios_panels(), None);
    shell.follow_activity(client);
    let mut h: Harness<'_, Shell> = headless::harness(shell, vec2(1280.0, 800.0));
    // Each says, within seconds, that it is up to date and waits.
    let deadline = Instant::now() + Duration::from_secs(20);
    let said = |kind: &str, expected: &str| {
        standing(&vault, kind).is_some_and(|p| note(&p) == Some(expected))
    };
    let (map_said, thumbs_said) = (
        helios_library::map_job::UP_TO_DATE,
        helios_library::thumb_check::UP_TO_DATE,
    );
    while !(said("map", map_said) && said(THUMBS_KIND, thumbs_said)) {
        assert!(
            Instant::now() < deadline,
            "the background works said nothing: map {:?}, thumbs {:?}",
            standing(&vault, "map").map(|p| p.state),
            standing(&vault, THUMBS_KIND).map(|p| p.state),
        );
        h.step();
        std::thread::sleep(Duration::from_millis(50));
    }
    // The Activity focus shows it in the job's detail and log.
    let id = standing(&vault, "map").unwrap().id;
    h.state_mut()
        .run("activity.show", json!({ "id": id }), Origin::User)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    // Its current step and its log line.
    while h.query_all_by_label(map_said).count() < 2 {
        assert!(Instant::now() < deadline, "the detail does not say it");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(map.wait(Duration::from_secs(20)), "they stop with the node");
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
