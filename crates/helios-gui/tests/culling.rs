//! Culling in the library: the culling keys, auto-advance, and marks made
//! through egui-pop's library steps (keys, the grid, an agent's
//! `library.*` command) reaching the vault; the search filter as a vault
//! query.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use egui::{Key, vec2};
use egui_kittest::Harness;
use egui_pop::views::library::filter::Filter;
use egui_pop::views::library::{ColorLabel, Flag};
use helios_core::{Bus, ContentHash};
use helios_gui::library::{Change, Entry, Op};
use helios_gui::persist::Persist;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_vault::fields::{LABELS, PICKED, RATING, REJECTED, TAGS};
use helios_vault::{Image, ImageId, Store, Vault};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-culling-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn id(n: u8) -> ImageId {
    ImageId(ContentHash([n; 32]))
}

/// A vault file with images 1 to 3.
fn vault_file(dir: &Path) -> PathBuf {
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    for n in 1..=3 {
        store
            .add_image(&Image {
                id: id(n),
                size: 10,
                partial: ContentHash([n; 32]),
            })
            .unwrap();
    }
    path
}

/// A shell whose library is filled from the vault at `path`, the first
/// image selected, drawn headless.
fn open(path: &Path) -> (Harness<'static, Shell>, Persist<SqliteStore>) {
    let mut shell = Shell::new(helios_panels(), None);
    let persist = Persist::new(SqliteStore::open(path).unwrap(), Arc::new(Bus::new()));
    persist.fill_library(&mut shell).unwrap();
    shell.library_mut().select(0);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    (h, persist)
}

fn stored(path: &Path, n: u8, field: &str) -> Option<Value> {
    let v = Vault::new(SqliteStore::open(path).unwrap());
    v.fields_of(&[id(n)], &[field]).unwrap()[0]
        .get(field)
        .cloned()
}

fn run(h: &mut Harness<'_, Shell>, command: &str, args: Value) {
    h.state_mut()
        .run(command, args, Origin::User)
        .unwrap_or_else(|e| panic!("{command}: {e}"));
}

#[test]
fn culling_keys_mark_the_vault_through_the_library_steps() {
    let dir = scratch("keys");
    let path = vault_file(&dir);
    let (mut h, mut persist) = open(&path);
    run(&mut h, "helios.vault.focus", json!(null));
    h.run();
    // Lightroom's keys: P picks, 4 rates, 6 labels red.
    h.key_press(Key::P);
    h.key_press(Key::Num4);
    h.key_press(Key::Num6);
    h.run();
    persist.frame(h.state_mut(), 1.0);
    assert_eq!(stored(&path, 1, PICKED), Some(json!(true)));
    assert_eq!(stored(&path, 1, RATING), Some(json!(4)));
    assert_eq!(stored(&path, 1, LABELS), Some(json!(["red"])));
    // X rejects: the pick goes, the stars stay.
    h.key_press(Key::X);
    h.run();
    persist.frame(h.state_mut(), 2.0);
    assert_eq!(stored(&path, 1, PICKED), None);
    assert_eq!(stored(&path, 1, REJECTED), Some(json!(true)));
    assert_eq!(stored(&path, 1, RATING), Some(json!(4)));
    // U unflags.
    h.key_press(Key::U);
    h.run();
    persist.frame(h.state_mut(), 3.0);
    assert_eq!(stored(&path, 1, REJECTED), None);
    let entry = h.state().library().entries[0].clone();
    assert_eq!(
        (entry.rating, entry.picked, entry.rejected),
        (4, false, false)
    );
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_agent_s_library_commands_reach_the_vault() {
    let dir = scratch("agent");
    let path = vault_file(&dir);
    let (mut h, mut persist) = open(&path);
    let library = helios_gui::library::LIBRARY;
    // egui-pop's commands queue the step for the grid, which runs it at
    // its next pass, as if the user had asked.
    run(
        &mut h,
        "library.tag",
        json!({ "library": library, "add": ["sea", "beach"] }),
    );
    run(
        &mut h,
        "library.flag",
        json!({ "library": library, "flag": "pick" }),
    );
    run(
        &mut h,
        "library.label",
        json!({ "library": library, "label": "blue", "on": true }),
    );
    run(
        &mut h,
        "library.rate",
        json!({ "library": library, "rating": 2 }),
    );
    h.run();
    h.run();
    persist.frame(h.state_mut(), 1.0);
    assert_eq!(stored(&path, 1, TAGS), Some(json!(["beach", "sea"])));
    assert_eq!(stored(&path, 1, PICKED), Some(json!(true)));
    assert_eq!(stored(&path, 1, LABELS), Some(json!(["blue"])));
    assert_eq!(stored(&path, 1, RATING), Some(json!(2)));
    assert_eq!(stored(&path, 2, RATING), None, "only the selection");
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn auto_advance_moves_on_after_each_mark() {
    let entries: Vec<Entry> = ["a", "b", "c"]
        .iter()
        .map(|id| Entry {
            id: id.to_string(),
            name: format!("{id}.raf"),
            ..Entry::default()
        })
        .collect();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries);
    shell.library_mut().select(0);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    run(&mut h, "vault.autoAdvance", json!({ "on": true }));
    run(&mut h, "helios.vault.focus", json!(null));
    h.run();
    h.key_press(Key::Num5);
    h.run();
    h.key_press(Key::X);
    h.run();
    let s = h.state_mut();
    assert_eq!(s.library().selected, Some(2), "two marks, two steps");
    let changes = s.take_library_changes();
    let change = |image: &str, field: &str, op| Change {
        image: image.into(),
        field: field.into(),
        op,
    };
    assert_eq!(
        changes,
        [
            change("a", RATING, Op::Set(Some(json!(5)))),
            change("b", REJECTED, Op::Set(Some(json!(true)))),
        ]
    );
    // Off again: a mark stays on its image.
    run(&mut h, "vault.autoAdvance", json!(null));
    h.key_press(Key::Num7);
    h.run();
    let s = h.state_mut();
    assert_eq!(s.library().selected, Some(2));
    assert_eq!(s.library().entries[2].labels, ["yellow"]);
}

#[test]
fn several_images_are_marked_at_once() {
    let entries: Vec<Entry> = ["a", "b", "c"]
        .iter()
        .map(|id| Entry {
            id: id.to_string(),
            name: format!("{id}.jpg"),
            ..Entry::default()
        })
        .collect();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries);
    {
        let mut library = shell.library_mut();
        library.select_ids(&["a".into(), "c".into()]);
        library.flag(Flag::Pick).unwrap();
        library.toggle_label(ColorLabel::Green.name()).unwrap();
        library.tag(&["trip".into()], &[]).unwrap();
    }
    let library = shell.library();
    for i in [0, 2] {
        let e = &library.entries[i];
        assert!(e.picked, "{}", e.id);
        assert_eq!(e.labels, ["green"]);
        assert_eq!(e.tags, ["trip"]);
    }
    assert!(!library.entries[1].picked);
}

#[test]
fn the_search_filter_is_a_vault_query() {
    let dir = scratch("filter");
    let path = vault_file(&dir);
    let other = Vault::new(SqliteStore::open(&path).unwrap());
    other
        .apply(
            &[
                helios_vault::Command::set(id(1), RATING, json!(4)),
                helios_vault::Command::set(id(2), RATING, json!(5)),
                helios_vault::Command::set(id(2), PICKED, json!(true)),
                helios_vault::Command::add(id(3), TAGS, "sea"),
            ],
            "user",
            None,
            1,
        )
        .unwrap();
    let (mut h, mut persist) = open(&path);
    let picked_and_rated = Filter {
        min_rating: Some(3),
        flags: vec![Flag::Pick],
        ..Filter::default()
    };
    h.state_mut()
        .library_mut()
        .set_filter(picked_and_rated.clone());
    let query = helios_gui::library::query_of(&picked_and_rated);
    assert_eq!(query.min_rating, Some(3));
    assert_eq!(query.flags, ["pick"]);
    persist.frame(h.state_mut(), 2.0);
    assert_eq!(h.state().library().shown(), [id(2).hex()]);
    let tagged = Filter {
        tags: vec!["sea".into()],
        ..Filter::default()
    };
    h.state_mut().library_mut().set_filter(tagged);
    persist.frame(h.state_mut(), 3.0);
    assert_eq!(h.state().library().shown(), [id(3).hex()]);
    // Cleared: every image again.
    h.state_mut().library_mut().set_filter(Filter::default());
    persist.frame(h.state_mut(), 4.0);
    assert_eq!(h.state().library().shown().len(), 3);
    h.run();
    drop((h, persist, other));
    let _ = std::fs::remove_dir_all(&dir);
}
