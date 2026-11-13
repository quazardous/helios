//! The History panel's hover preview of an image's edit: the pointer on
//! a step or a snapshot shows the image as it was there, writing nothing
//! (no step, no save, no vault write); leaving shows the live values
//! again, bit for bit; a drag going on gets no preview. Through egui-pop's
//! history calls, as the panel's rows make them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use egui_pop::api::history::{self, At};
use egui_pop::model::commands::Origin as PopOrigin;
use helios_core::{Bus, Graph, NodeId, Value};
use helios_gui::history::EDIT;
use helios_gui::persist::Persist;
use helios_gui::{Shell, helios_panels};
use helios_shell::commands::Origin;
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;

const EV: &str = "node/exposure/exposure";

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-preview-{name}-{}-{}",
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

/// A desktop on the vault in `dir`, its photo open.
fn open(dir: &Path) -> (Shell, Persist<SqliteStore>, helios_vault::ImageId) {
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
            helios_values::Change::To(Value::Float(ev)),
            Origin::User,
        ))
        .unwrap();
}

/// The exposure the image renders with, once the graph followed the
/// store (as each frame does).
fn rendered(shell: &Shell) -> f64 {
    shell
        .viewport_mut()
        .sync(&mut shell.values().lock().unwrap());
    let viewport = shell.viewport();
    let g = viewport.graph().unwrap();
    let id = g.find("exposure").unwrap();
    g.params(id)
        .unwrap()
        .unwrap()
        .get("exposure")
        .unwrap()
        .as_f64()
        .unwrap()
}

/// The exposure the store holds: what widgets, agents and the save read.
fn live(shell: &Shell) -> f64 {
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

/// What would show a write: the store's counters.
fn counters(shell: &Shell) -> (u64, u64, u64, usize) {
    let s = shell.values().lock().unwrap();
    (
        s.seq(),
        s.history_rev(),
        s.committed(),
        s.undo_steps().len(),
    )
}

fn versions(dir: &Path, image: helios_vault::ImageId) -> usize {
    Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap())
        .versions(&image)
        .unwrap()
        .len()
}

#[test]
fn a_hovered_step_shows_its_values_and_leaving_restores_the_live_ones() {
    let dir = scratch("step");
    let (mut shell, mut persist, image) = open(&dir);
    set(&shell, 1.0);
    persist.frame_settled(&mut shell, 1.0);
    // A live value no other float sums to, to see it come back exactly.
    let now = 0.1 + 0.2;
    set(&shell, now);
    persist.frame_settled(&mut shell, 2.0);
    let snap = history::take_snapshot(
        shell.app().handle(),
        Some(&EDIT),
        "Now",
        None,
        false,
        &PopOrigin::User,
    )
    .unwrap();
    persist.frame_settled(&mut shell, 3.0);
    assert_eq!(rendered(&shell).to_bits(), now.to_bits());
    let before = (counters(&shell), versions(&dir, image));
    let kept = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap())
        .snapshots(&image)
        .unwrap();

    // The pointer on step 1: the image renders its exposure, the panel
    // lists it.
    let p = history::hover(shell.app().handle(), &EDIT, At::Step(1)).expect("a preview");
    assert!(p.shown);
    assert_eq!(p.changes.len(), 1, "{:?}", p.changes);
    assert_eq!(p.changes[0].what, "exposure");
    assert_eq!(p.changes[0].then, "+1.00 EV");
    assert_eq!(p.changes[0].now, "+0.30 EV");
    assert_eq!(rendered(&shell), 1.0);
    // The start: the network's own exposure.
    history::hover(shell.app().handle(), &EDIT, At::Step(0)).expect("a preview");
    assert_eq!(rendered(&shell), 0.0);

    // Nothing written: the store's value, no step, no save, no vault
    // write, even after a frame of the vault's link.
    assert_eq!(live(&shell).to_bits(), now.to_bits());
    persist.frame_settled(&mut shell, 4.0);
    assert_eq!((counters(&shell), versions(&dir, image)), before);
    let after = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap())
        .snapshots(&image)
        .unwrap();
    assert_eq!(after, kept);

    // The pointer leaves: the live value, bit for bit.
    history::end_preview(shell.app().handle());
    assert_eq!(rendered(&shell).to_bits(), now.to_bits());
    assert!(shell.values().lock().unwrap().previewed().is_empty());

    // A snapshot's row: the state it pinned.
    set(&shell, -1.0);
    let p = history::hover(shell.app().handle(), &EDIT, At::Snapshot(snap.id)).expect("a preview");
    assert_eq!(p.changes.len(), 1);
    assert_eq!(rendered(&shell).to_bits(), now.to_bits());
    history::end_preview(shell.app().handle());
    assert_eq!(rendered(&shell), -1.0);
    // A point no jump reaches: no preview, the live values shown.
    assert!(history::hover(shell.app().handle(), &EDIT, At::Step(99)).is_none());
    assert_eq!(rendered(&shell), -1.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_drag_gets_no_preview_and_keeps_its_values() {
    let dir = scratch("drag");
    let (mut shell, mut persist, _) = open(&dir);
    set(&shell, 1.0);
    set(&shell, 2.0);
    persist.frame_settled(&mut shell, 1.0);
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
                helios_values::Change::To(Value::Float(2.5)),
                Origin::User,
            )
            .gesture(g),
        )
        .unwrap();
    let before = counters(&shell);
    // The pointer on a row while the drag goes on: no preview.
    assert!(history::hover(shell.app().handle(), &EDIT, At::Step(1)).is_none());
    assert!(shell.values().lock().unwrap().previewed().is_empty());
    assert_eq!(rendered(&shell), 2.5);
    assert_eq!(counters(&shell), before);
    // The drag ends as it would have: one step at its value.
    history::end_preview(shell.app().handle());
    shell.values().lock().unwrap().end(g);
    assert_eq!(live(&shell), 2.5);
    assert_eq!(rendered(&shell), 2.5);
    assert_eq!(counters(&shell).3, before.3 + 1);
    // A preview, then a write to its value (a key on a slider): the
    // write shows through, and leaving keeps it.
    history::hover(shell.app().handle(), &EDIT, At::Step(1)).expect("a preview");
    assert_eq!(rendered(&shell), 1.0);
    set(&shell, 3.0);
    assert_eq!(rendered(&shell), 3.0);
    history::end_preview(shell.app().handle());
    assert_eq!((live(&shell), rendered(&shell)), (3.0, 3.0));
    let _ = std::fs::remove_dir_all(&dir);
}
