//! The Caches settings page and its commands: `cache.stats` says what the
//! library's thumbnail cache holds per kind, against its budget, and
//! `cache.clear` clears one kind or every one; the page lists them with a
//! "Clear" each and "Clear caches".

use egui::vec2;
use helios_gui::{Shell, caches, config, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_thumbs::{Key, LARGE, Rgb8, SMALL};
use serde_json::{Value, json};

fn thumb(seed: u8) -> Rgb8 {
    Rgb8 {
        width: 64,
        height: 48,
        pixels: (0..64 * 48 * 3)
            .map(|i| (i as u8).wrapping_mul(seed))
            .collect(),
    }
}

/// The row of `kind` in the library at `dir`.
fn row(stats: &Value, kind: &str, dir: &std::path::Path) -> Value {
    let scope = format!("silo:library:{}", dir.display());
    stats["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == kind && k["scope"] == scope.as_str())
        .unwrap_or_else(|| panic!("no {kind} row in {stats}"))
        .clone()
}

#[test]
fn the_caches_page_shows_each_kind_and_clears_it() {
    let root = std::env::temp_dir().join(format!("helios-caches-page-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cache = helios_gui::thumbnails::open_cache(&root.join("library.db"), 1 << 30).unwrap();
    cache.list();
    let dir = cache.dir().to_path_buf();
    for (i, id) in ["aaaa", "bbbb", "cccc"].into_iter().enumerate() {
        cache
            .put(&Key::preview(id), SMALL, &thumb(i as u8 + 1))
            .unwrap();
    }
    cache.put(&Key::preview("aaaa"), LARGE, &thumb(9)).unwrap();

    let mut shell = Shell::new(helios_panels(), None);
    caches::register(&mut shell).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let shell = h.state_mut();
    let stats = shell
        .run(caches::STATS_COMMAND, json!({}), Origin::User)
        .unwrap();
    let small = row(&stats, "thumb.256", &dir);
    assert_eq!(small["entries"], 3);
    assert_eq!(small["tier"], "disk");
    assert_eq!(small["budget"], 1u64 << 30);
    assert!(
        small["sizeText"].as_str().unwrap().contains("of 1.0 GiB"),
        "{small}"
    );
    assert_eq!(row(&stats, "thumb.1024", &dir)["entries"], 1);
    assert!(
        stats["summary"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Caches hold")
    );
    // The disk tier counts them.
    let disk = stats["tiers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tier"] == "disk")
        .unwrap()
        .clone();
    assert!(disk["used"].as_u64().unwrap() >= cache.usage().0);

    // One kind cleared, the other kept.
    let freed = shell
        .run(
            caches::CLEAR_COMMAND,
            json!({ "kind": "thumb.1024" }),
            Origin::User,
        )
        .unwrap();
    assert!(freed["freed"].as_u64().unwrap() > 0);
    assert!(!cache.contains(&Key::preview("aaaa"), LARGE));
    assert!(cache.contains(&Key::preview("aaaa"), SMALL));
    // Every kind.
    shell
        .run(caches::CLEAR_COMMAND, json!({}), Origin::User)
        .unwrap();
    assert_eq!(cache.usage(), (0, 0));
    let stats = shell
        .run(caches::STATS_COMMAND, json!({}), Origin::User)
        .unwrap();
    assert_eq!(row(&stats, "thumb.256", &dir)["entries"], 0);
    assert!(dir.join("CACHEDIR.TAG").exists(), "the tag stays");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_caches_page_lists_the_caches_with_their_actions() {
    let schema = helios_config::Schema::core();
    let manifest = config::manifest(&schema, &|m| m.english.clone());
    let text = serde_json::to_string(&manifest).unwrap();
    for needle in [caches::STATS_COMMAND, caches::CLEAR_COMMAND, "Clear caches"] {
        assert!(text.contains(needle), "the manifest lacks {needle}");
    }
}

#[test]
fn the_render_cache_setting_reaches_the_desktop_engine() {
    use helios_gui::config::{Settings, Sources};
    let shell = Shell::new(helios_panels(), None);
    // A size set on the machine wins over the share.
    let settings = Settings::resolve(&Sources {
        global: Some("[system.cache]\nbudget = 512\nshare = 50\n".into()),
        ..Sources::default()
    });
    assert_eq!(caches::render_budget(&settings), Some(512 << 20));
    caches::apply_settings(&shell, &settings);
    assert_eq!(shell.viewport().engine().budget(), Some(512 << 20));
    assert_eq!(
        helios_cache::Service::global().budget(helios_cache::Tier::Ram),
        Some(512 << 20)
    );
    // A share alone: that share of the machine's memory.
    let settings = Settings::resolve(&Sources {
        global: Some("[system.cache]\nshare = 10\n".into()),
        ..Sources::default()
    });
    let tenth = helios_cache::memory_share(10.0, helios_cache::physical_memory());
    caches::apply_settings(&shell, &settings);
    assert_eq!(shell.viewport().engine().budget(), tenth);
    // A share of 0: no limit.
    let settings = Settings::resolve(&Sources {
        global: Some("[system.cache]\nshare = 0\n".into()),
        ..Sources::default()
    });
    caches::apply_settings(&shell, &settings);
    assert_eq!(shell.viewport().engine().budget(), None);
}
