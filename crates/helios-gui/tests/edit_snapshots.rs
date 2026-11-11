//! History snapshots of an image's edit: taking one saves nothing, the
//! next save pins it to the version it holds (kept in the vault), a
//! snapshot never pinned is lost when the desktop quits, and a restore is
//! one step an undo takes back. Through egui-pop's history calls, as the
//! History panel and the agents' `history.*` make them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use egui_pop::api::history::{self, Snapshot};
use egui_pop::model::commands::Origin as PopOrigin;
use helios_core::{Bus, Graph, NodeId};
use helios_gui::history::EDIT;
use helios_gui::persist::Persist;
use helios_gui::{Shell, helios_panels};
use helios_shell::commands::Origin;
use helios_vault::{ImageId, Vault};
use helios_vault_sqlite::SqliteStore;

const EV: &str = "node/exposure/exposure";

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-snapshots-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    image::RgbImage::from_fn(12, 8, |x, y| {
        image::Rgb([(x * 20) as u8, (y * 30) as u8, 90])
    })
    .save(dir.join("photo.png"))
    .unwrap();
    dir
}

fn network() -> (Graph, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        helios_core::Image::from_fn(helios_core::Region::sized(16, 8), 4, |_, _, _| 0.25),
    );
    let exp = g
        .add_module(
            "exposure",
            helios_core::ModuleRef::new(helios_module_exposure::Exposure),
            &helios_core::Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    (g, exp)
}

/// A desktop on the vault in `dir`, its photo open with its saved edit.
fn open(dir: &Path) -> (Shell, Persist<SqliteStore>, ImageId) {
    let mut shell = Shell::new(helios_panels(), None);
    let mut persist = Persist::new(
        SqliteStore::open(&dir.join("vault.db")).unwrap(),
        Arc::new(Bus::new()),
    );
    let image = persist
        .vault()
        .import_file(&dir.join("photo.png"), 1)
        .unwrap();
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, image, graph, exp, "output")
        .unwrap();
    (shell, persist, image)
}

fn set(shell: &Shell, ev: f64) {
    shell
        .values()
        .lock()
        .unwrap()
        .set(helios_values::Set::new(
            EV,
            helios_values::Change::To(helios_core::Value::Float(ev)),
            Origin::User,
        ))
        .unwrap();
}

fn shown(shell: &Shell) -> f64 {
    shell
        .values()
        .lock()
        .unwrap()
        .get(EV)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

fn take(shell: &Shell, name: &str) -> Snapshot {
    history::take_snapshot(
        shell.app().handle(),
        Some(&EDIT),
        name,
        None,
        false,
        &PopOrigin::User,
    )
    .unwrap()
}

/// The snapshots' names, a pending one's followed by `*`.
fn listed(shell: &Shell) -> Vec<String> {
    history::snapshots(shell.app().handle(), &EDIT)
        .into_iter()
        .map(|s| if s.pending { s.name + "*" } else { s.name })
        .collect()
}

fn kept(dir: &Path, image: ImageId) -> Vec<helios_vault::snapshots::Snapshot> {
    Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap())
        .snapshots(&image)
        .unwrap()
}

#[test]
fn a_snapshot_is_pinned_to_the_version_of_the_next_save() {
    let dir = scratch("pin");
    let (mut shell, mut persist, image) = open(&dir);
    set(&shell, 1.0);
    // Taken before the step is saved: nothing is written for it.
    let snap = take(&shell, "Bright");
    assert_eq!(listed(&shell), ["Bright*"]);
    assert!(kept(&dir, image).is_empty());
    persist.frame(&mut shell, 1.0);
    // The save of that step pins it.
    assert_eq!(listed(&shell), ["Bright"]);
    let head = persist.session().unwrap().head().unwrap();
    let k = kept(&dir, image);
    assert_eq!(k.len(), 1);
    assert_eq!((k[0].id, k[0].name.as_str()), (snap.id, "Bright"));
    assert_eq!(k[0].version, Some(head));
    // Deleted: gone from the vault at the next frame.
    history::forget_snapshot(
        shell.app().handle(),
        Some(&EDIT),
        snap.id,
        None,
        false,
        &PopOrigin::User,
    )
    .unwrap();
    persist.frame(&mut shell, 2.0);
    assert!(kept(&dir, image).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_snapshot_never_saved_is_lost_when_the_desktop_quits() {
    let dir = scratch("pending");
    let (mut shell, mut persist, image) = open(&dir);
    set(&shell, 1.0);
    persist.frame(&mut shell, 1.0);
    take(&shell, "Kept");
    persist.frame(&mut shell, 2.0);
    // Taken, then another step before the edit is saved: its state is
    // never saved, so it stays pending.
    set(&shell, 1.5);
    take(&shell, "Passed");
    set(&shell, 2.0);
    persist.frame(&mut shell, 3.0);
    assert_eq!(listed(&shell), ["Kept", "Passed*"]);
    // And one taken after the last save, the desktop quitting first.
    take(&shell, "Late");
    drop((shell, persist));
    let names: Vec<String> = kept(&dir, image).into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["Kept"]);
    let (shell, _persist, _) = open(&dir);
    assert_eq!(listed(&shell), ["Kept"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_restore_is_one_step_an_undo_takes_back_after_reopening() {
    let dir = scratch("restore");
    let (mut shell, mut persist, _) = open(&dir);
    // The image unedited, then two steps.
    let start = take(&shell, "Original");
    persist.frame(&mut shell, 1.0);
    set(&shell, 1.0);
    persist.frame(&mut shell, 2.0);
    let bright = take(&shell, "Bright");
    set(&shell, 2.0);
    persist.frame(&mut shell, 3.0);
    drop((shell, persist));
    // Reopened: both are there, restored from the vault.
    let (mut shell, mut persist, _) = open(&dir);
    assert_eq!(listed(&shell), ["Original", "Bright"]);
    assert!((shown(&shell) - 2.0).abs() < 1e-9);
    let steps = |s: &Shell| s.values().lock().unwrap().undo_steps().len();
    let before = steps(&shell);
    let outcome = history::restore(shell.app().handle(), &EDIT, bright.id, &PopOrigin::User);
    assert!(outcome.ok(), "{outcome:?}");
    assert!((shown(&shell) - 1.0).abs() < 1e-9, "{}", shown(&shell));
    assert_eq!(steps(&shell), before + 1, "one step");
    persist.frame(&mut shell, 4.0);
    shell.follow_snapshots();
    assert!(
        shell
            .notices()
            .iter()
            .any(|n| n.key == "gui-history-snapshot-restored"),
        "a notice says it"
    );
    // The image unedited: its parameters as the network has them.
    let outcome = history::restore(shell.app().handle(), &EDIT, start.id, &PopOrigin::User);
    assert!(outcome.ok(), "{outcome:?}");
    assert!(shown(&shell).abs() < 1e-9, "{}", shown(&shell));
    assert!(history::undo(shell.app().handle(), &PopOrigin::User).ok());
    assert!((shown(&shell) - 1.0).abs() < 1e-9);
    assert!(history::undo(shell.app().handle(), &PopOrigin::User).ok());
    assert!((shown(&shell) - 2.0).abs() < 1e-9);
    assert_eq!(steps(&shell), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_restore_waits_for_a_drag_and_a_dry_run_changes_nothing() {
    let dir = scratch("drag");
    let (mut shell, mut persist, _) = open(&dir);
    let snap = take(&shell, "Original");
    persist.frame(&mut shell, 1.0);
    set(&shell, 1.0);
    let g = shell
        .values()
        .lock()
        .unwrap()
        .begin_gesture(Origin::User, "drag exposure");
    shell
        .values()
        .lock()
        .unwrap()
        .set(
            helios_values::Set::new(
                EV,
                helios_values::Change::To(helios_core::Value::Float(1.5)),
                Origin::User,
            )
            .gesture(g),
        )
        .unwrap();
    let outcome = history::restore(shell.app().handle(), &EDIT, snap.id, &PopOrigin::User);
    assert!(!outcome.ok(), "vetoed while the drag goes on");
    shell.values().lock().unwrap().end(g);
    // A dry run, as an agent: the step it would make, nothing written.
    let mut owner = helios_gui::history::HeliosHistory::new(shell.values().clone())
        .with_snapshots(shell.snapshots().clone());
    let answer = egui_pop::api::history::History::restore(
        &mut owner,
        &egui_pop::api::history::Request::new(EDIT).dry_run(),
        snap.id,
        &PopOrigin::Agent("mcp".into()),
    );
    let egui_pop::api::history::Answer::Done(Ok(walked)) = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(walked.steps[0].label.text, "Restore “Original”");
    assert!((shown(&shell) - 1.5).abs() < 1e-9);
    let _ = std::fs::remove_dir_all(&dir);
}
