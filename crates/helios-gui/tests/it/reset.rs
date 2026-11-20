//! Settings > Danger zone (`helios_gui::reset`), on a fixture home only:
//! the section says what goes and what stays with the photos counted,
//! "Reset Helios…" opens the wizard, a word other than RESET is refused,
//! RESET hands the plan over (run here: every photo byte for byte the
//! same), and the agent bridge refuses the command whatever it carries.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_gui::reset::{self, Photos, Reset};
use helios_gui::{Shell, headless, helios_panels};
use helios_node_core::reset::{Places, Plan};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

fn until(h: &mut Harness<'_, Shell>, what: &str, done: impl Fn(&mut Harness<'_, Shell>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("never got there: {what}");
}

fn run(h: &mut Harness<'_, Shell>, cmd: &str, args: Value) -> Result<Value, String> {
    let out = h.state_mut().run(cmd, args, Origin::User);
    h.run();
    out
}

/// The reset wizard's state, `None` while it is not open.
fn wizard(h: &mut Harness<'_, Shell>) -> Option<Value> {
    let s = h
        .state_mut()
        .run("admin.wizard.set", json!({ "values": {} }), Origin::User)
        .ok()?;
    (s["id"] == reset::WIZARD || s["wizard"] == reset::WIZARD).then_some(s)
}

/// A fixture home: Helios's four folders with something in each, the
/// library file in the data folder, and a photo folder outside them.
struct Home {
    root: PathBuf,
    places: Places,
    photos: PathBuf,
}

impl Home {
    fn new() -> Home {
        Home::at(&format!("helios-reset-page-{}", std::process::id()))
    }

    /// The home in the temporary folder's `name`.
    fn at(name: &str) -> Home {
        let root = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&root);
        let dir = |p: &str| {
            let d = root.join(p).join("helios");
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        let places = Places {
            config: dir("config"),
            data: dir("data"),
            state: dir("state"),
            cache: dir("cache"),
            library: None,
        };
        std::fs::write(places.config.join("global.toml"), "x = 1\n").unwrap();
        std::fs::write(places.data.join("library.db"), b"catalogue").unwrap();
        std::fs::write(places.state.join("helios.log"), b"log").unwrap();
        std::fs::write(places.cache.join("thumb"), b"cache").unwrap();
        let photos = root.join("Pictures");
        std::fs::create_dir_all(&photos).unwrap();
        for (name, bytes) in [
            ("a.cr3", &b"raw"[..]),
            ("a.jpg", b"jpeg"),
            ("a.cr3.xmp", b"<xmp/>"),
        ] {
            std::fs::write(photos.join(name), bytes).unwrap();
        }
        let places = Places {
            library: Some(places.data.join("library.db")),
            ..places
        };
        Home {
            root,
            places,
            photos,
        }
    }

    /// Every photo file and its bytes.
    fn photo_bytes(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out: Vec<_> = std::fs::read_dir(&self.photos)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                let b = std::fs::read(&p).unwrap();
                (p, b)
            })
            .collect();
        out.sort();
        out
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn shell(home: &Home, plans: Arc<Mutex<Vec<Plan>>>) -> Shell {
    let mut shell = Shell::new(helios_panels(), None);
    let roots = vec![home.photos.clone()];
    reset::register(
        &mut shell,
        Reset {
            places: home.places.clone(),
            photos: Arc::new(move || Photos {
                roots: roots.clone(),
                count: 3,
            }),
            go: Arc::new(move |plan| {
                plans.lock().unwrap().push(plan);
                Ok(())
            }),
        },
    )
    .unwrap();
    shell
}

#[test]
fn the_danger_zone_resets_only_once_reset_is_typed_and_never_for_an_agent() {
    let home = Home::new();
    let before = home.photo_bytes();
    let plans = Arc::new(Mutex::new(Vec::new()));
    let mut h = headless::harness(shell(&home, plans.clone()), vec2(1280.0, 900.0));
    h.run();

    // The section: what goes, what stays with the photos counted, the
    // snapshot offered first.
    run(&mut h, "admin.open", json!({ "path": reset::SECTION })).unwrap();
    until(&mut h, "the Danger zone drawn", |h| {
        h.query_by_role_and_label(Role::Button, "Reset Helios…")
            .is_some()
    });
    assert!(
        h.query_by_label_contains("your 3 photos").is_some(),
        "the photos kept are counted"
    );
    assert!(
        h.query_all_by_label_contains("moved to the backup folder")
            .next()
            .is_some(),
        "what goes is listed"
    );
    assert!(
        h.query_by_role_and_label(Role::Button, "Snapshot the catalogue first")
            .is_some()
    );

    // "Reset Helios…" opens the wizard; the scope, then the word.
    h.get_by_role_and_label(Role::Button, "Reset Helios…")
        .click();
    until(&mut h, "the reset wizard open", |h| wizard(h).is_some());
    run(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "scope", "value": "everything" }),
    )
    .unwrap();
    run(&mut h, "admin.wizard.next", json!({})).unwrap();
    until(&mut h, "the confirm step", |h| {
        wizard(h).is_some_and(|s| s["step"] == "confirm")
    });

    // Another word: refused, nothing handed over.
    for wrong in ["", "reset", "RESET please"] {
        run(
            &mut h,
            "admin.wizard.set",
            json!({ "key": "confirm", "value": wrong }),
        )
        .unwrap();
        assert!(
            run(&mut h, "admin.wizard.finish", json!({})).is_err(),
            "`{wrong}` resets"
        );
    }
    assert!(
        plans.lock().unwrap().is_empty(),
        "no reset without the word"
    );
    // The command itself asks for the word too.
    assert!(run(&mut h, reset::COMMAND, json!({ "scope": "everything" })).is_err());
    assert!(plans.lock().unwrap().is_empty());

    // The agent bridge refuses it, even with the word.
    let refused = h
        .state_mut()
        .run(
            reset::COMMAND,
            json!({ "scope": "everything", "confirm": reset::WORD }),
            Origin::Agent("helper".into()),
        )
        .unwrap_err();
    assert!(refused.contains("not offered to agents"), "{refused}");
    assert!(plans.lock().unwrap().is_empty(), "an agent never resets");

    // RESET typed: the plan is handed over.
    run(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "confirm", "value": reset::WORD }),
    )
    .unwrap();
    run(&mut h, "admin.wizard.finish", json!({})).unwrap();
    until(&mut h, "the plan handed over", |_| {
        !plans.lock().unwrap().is_empty()
    });
    let plan = plans.lock().unwrap().remove(0);
    assert!(
        plan.targets
            .iter()
            .all(|t| !home.photos.starts_with(&t.path) && !t.path.starts_with(&home.photos)),
        "no photo folder is a target: {plan:?}"
    );

    // Run as the desktop runs it after the quit: Helios's folders go,
    // every photo stays byte for byte.
    let report = helios_node_core::reset::run(&plan);
    assert_eq!(report.failed, None);
    assert!(!home.places.config.exists(), "the settings are put aside");
    assert!(!home.places.cache.exists(), "the caches are removed");
    assert!(
        report.backup.as_deref().is_some_and(Path::exists),
        "the backup folder holds them"
    );
    assert_eq!(home.photo_bytes(), before, "the photos are untouched");
}

/// The section's plan reads the photos' folders: one inside a Helios
/// folder refuses every reset and says why.
#[test]
fn a_photo_folder_inside_helios_folders_refuses_the_reset() {
    let home = Home::at(&format!("helios-reset-refused-{}", std::process::id()));
    let inside = home.places.data.join("Photos");
    std::fs::create_dir_all(&inside).unwrap();
    let plans = Arc::new(Mutex::new(Vec::new()));
    let mut shell = Shell::new(helios_panels(), None);
    let (p, roots) = (plans.clone(), vec![inside]);
    reset::register(
        &mut shell,
        Reset {
            places: home.places.clone(),
            photos: Arc::new(move || Photos {
                roots: roots.clone(),
                count: 1,
            }),
            go: Arc::new(move |plan| {
                p.lock().unwrap().push(plan);
                Ok(())
            }),
        },
    )
    .unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    h.run();
    let plan = run(&mut h, reset::PLAN_COMMAND, json!({})).unwrap();
    assert!(plan["refused"].is_string(), "{plan}");
    let refused = run(
        &mut h,
        reset::COMMAND,
        json!({ "scope": "everything", "confirm": reset::WORD }),
    )
    .unwrap_err();
    assert!(
        refused.contains("never touches a photo folder"),
        "{refused}"
    );
    assert!(plans.lock().unwrap().is_empty());
    assert!(home.places.config.exists(), "nothing moved");
}

#[test]
fn photos_are_read_from_the_folders_counts_and_the_library_folder() {
    let answer = json!({ "folders": [
        { "volume": "v1", "folders": { "/p": 5, "/p/2024": 3, "/p/2025": 2 } },
        { "volume": null, "folders": { "/q/x": 1 } },
    ]});
    let photos = reset::photos_of(&answer, Some(PathBuf::from("/lib")));
    assert_eq!(photos.count, 6);
    assert_eq!(
        photos.roots,
        [
            PathBuf::from("/lib"),
            PathBuf::from("/p"),
            PathBuf::from("/q/x")
        ]
    );
}

/// The Danger zone and the reset wizard's confirm step, as
/// `tests/snapshots/reset-*.png` keep them (regenerated by the box's
/// `gui-snapshots` job with `test=reset`).
#[test]
fn reset_snapshots() {
    use crate::shot::Shots as _;
    let mut results = egui_kittest::SnapshotResults::new();
    // A fixed folder: its paths are in the picture.
    let home = Home::at("helios-reset-shot");
    let mut h = headless::harness(
        shell(&home, Arc::new(Mutex::new(Vec::new()))),
        vec2(1280.0, 900.0),
    );
    h.run();
    run(&mut h, "admin.open", json!({ "path": reset::SECTION })).unwrap();
    until(&mut h, "the Danger zone drawn", |h| {
        h.query_by_label_contains("your 3 photos").is_some()
    });
    h.run_steps(4);
    results.add(h.try_body_shot("reset-danger-zone-pop-dark-1x"));
    run(&mut h, "admin.wizard.open", json!({ "id": reset::WIZARD })).unwrap();
    until(&mut h, "the reset wizard open", |h| wizard(h).is_some());
    run(&mut h, "admin.wizard.next", json!({})).unwrap();
    run(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "confirm", "value": "reset" }),
    )
    .unwrap();
    h.run_steps(4);
    results.add(h.try_body_shot("reset-wizard-confirm-pop-dark-1x"));
}
