//! The workspace's history scope: a graph edit (the workspace's network)
//! is not the photo's, so switching photo keeps it undoable, while the
//! photo's own steps leave with the photo. Through the real switch
//! (`Persist::open_image`: `image.open`, its `history.scope`, then the
//! viewport's new source).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::vec2;
use egui_kittest::Harness;
use helios_core::{Bus, Image, Region, Value};
use helios_gui::persist::Persist;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use helios_template::Source;
use helios_vault::ImageId;
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value as Json, json};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-ws-history-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A window on a blank workspace, a vault holding two photos.
fn window(root: &Path) -> (Harness<'static, Shell>, Persist<SqliteStore>, [ImageId; 2]) {
    let config = root.join("config");
    let dir = root.join("photos");
    std::fs::create_dir_all(&dir).unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&dir)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &dir).unwrap();
    crate::host::listen_network(&shell);
    let persist = Persist::new(
        SqliteStore::open(&root.join("vault.db")).unwrap(),
        Arc::new(Bus::new()),
    );
    let photos = ["a.png", "b.png"].map(|name| {
        let file = dir.join(name);
        let tint = if name == "a.png" { 40 } else { 200 };
        image::RgbImage::from_fn(12, 8, |x, y| {
            image::Rgb([(x * 20) as u8, (y * 30) as u8, tint])
        })
        .save(&file)
        .unwrap();
        persist.vault().import_file(&file, 1).unwrap()
    });
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    (h, persist, photos)
}

/// Opens `photo` through the workspace's network as it is now.
fn open(h: &mut Harness<'static, Shell>, persist: &mut Persist<SqliteStore>, photo: ImageId) {
    let picture = Image::from_fn(
        Region::sized(16, 12),
        4,
        |_, _, c| {
            if c == 3 { 1.0 } else { 0.3 }
        },
    );
    let built = h
        .state()
        .workspace_bundle()
        .unwrap()
        .build_with(&templates::modules(), Source::picture(&picture), None)
        .unwrap();
    persist
        .open_image(
            h.state_mut(),
            photo,
            built.graph,
            built.view.0,
            &built.view.1,
        )
        .unwrap();
    persist.settle(h.state_mut());
    follow(h);
}

/// The frames a step takes, then the image kept open through a network
/// that changed (what the host does).
fn follow(h: &mut Harness<'static, Shell>) {
    for _ in 0..3 {
        h.run();
    }
    if crate::host::network_changed() > 0 {
        assert!(h.state_mut().keep_image_open().unwrap(), "kept in place");
    }
    for _ in 0..2 {
        h.run();
    }
}

fn run(h: &mut Harness<'static, Shell>, id: &str, args: Json) {
    h.state_mut().run(id, args, Origin::User).unwrap();
    follow(h);
}

fn set(h: &mut Harness<'static, Shell>, path: &str, v: f64) {
    h.state()
        .values()
        .lock()
        .unwrap()
        .set(helios_values::Set::new(
            path,
            helios_values::Change::To(Value::Float(v)),
            Origin::User,
        ))
        .unwrap();
    follow(h);
}

/// Edit > Undo: what it undid.
fn undo(h: &mut Harness<'static, Shell>) -> Option<String> {
    let undone = h
        .state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    follow(h);
    undone
}

/// Whether the workspace's network has a node named `node`.
fn in_network(h: &Harness<'static, Shell>, node: &str) -> bool {
    let viewport = h.state().viewport();
    viewport.graph().and_then(|g| g.find(node)).is_some()
}

#[test]
fn a_graph_edit_is_undone_after_the_photo_switched() {
    let root = temp("graph");
    let (mut h, mut persist, [a, b]) = window(&root);
    open(&mut h, &mut persist, a);
    set(&mut h, "node/exposure/exposure", 1.0);
    run(
        &mut h,
        "network.add",
        json!({ "nodes": [{ "module": "sigmoid" }] }),
    );
    assert!(in_network(&h, "sigmoid"), "the node added");
    open(&mut h, &mut persist, b);
    assert!(in_network(&h, "sigmoid"), "the network is the workspace's");
    // The graph edit is the workspace's: the switch kept it.
    assert!(undo(&mut h).is_some(), "the graph edit is still undoable");
    assert!(!in_network(&h, "sigmoid"), "the graph edit undone");
    // The first photo's own step left with it: nothing reaches it.
    assert_eq!(undo(&mut h), None, "photo a's step is not undone into b");
    let exposure = h
        .state()
        .values()
        .lock()
        .unwrap()
        .get("node/exposure/exposure")
        .and_then(|e| e.value.as_f64());
    assert_eq!(exposure, Some(0.0), "b keeps its own value");
    let _ = std::fs::remove_dir_all(&root);
}
