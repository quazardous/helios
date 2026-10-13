//! Closing: egui-pop's exit runs Helios's `on_exit` (the workspace's
//! layout) before the `app.exit` step, whose default saves the
//! interface's settings, keys and layout.

use std::cell::RefCell;
use std::rc::Rc;

use egui_pop::model::layout::Layout;
use helios_gui::{Shell, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

#[test]
fn exit_runs_on_exit_then_the_exit_step_saves() {
    let dir = std::env::temp_dir().join(format!("helios-exit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seen: Rc<RefCell<Option<Layout>>> = Rc::default();
    let at_exit = seen.clone();
    let mut shell = Shell::builder(helios_panels(), Some(dir.clone()))
        .on_exit(move |app| *at_exit.borrow_mut() = Some(app.layout()))
        .build();
    shell
        .run("helios.log.detach", json!(null), Origin::User)
        .unwrap();
    shell
        .bind_user_keys("ctrl+shift+e", "vault.reject")
        .unwrap();
    // The queued detach applies at the next frame; a harness draws one.
    let mut h = helios_gui::headless::harness(shell, egui::vec2(800.0, 500.0));
    h.run();
    h.state_mut().exit().unwrap();
    // The workspace's layout, as the window closed.
    let layout = seen.borrow().clone().expect("on_exit ran");
    assert_eq!(layout.auxiliary[0].panel.as_str(), "helios.log");
    // Then egui-pop's step saved the interface's documents.
    for doc in ["settings", "keymap", "layout"] {
        assert!(dir.join(format!("{doc}.json")).is_file(), "{doc}.json");
    }
    let keymap = std::fs::read_to_string(dir.join("keymap.json")).unwrap();
    assert!(keymap.contains("vault.reject"), "{keymap}");
    // The run is journaled as egui-pop's step.
    let app = h.state().app().handle().clone();
    assert!(
        app.step_traces()
            .iter()
            .any(|t| t.step.as_str() == "app.exit")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The desktop's quit, in its order: the closing frame writes what the
/// user changed, the window (the shell, the vault's link) goes, and the
/// vault closes after it on a thread of its own, within its deadline. A
/// rating made just before the close is kept, and the vault was closed
/// cleanly.
#[test]
fn the_vault_closes_after_the_window_and_keeps_the_last_rating() {
    use helios_gui::persist::Persist;
    use helios_gui::quit::{Finisher, VAULT_DEADLINE};
    use helios_vault::fields::RATING;
    use helios_vault_sqlite::SqliteStore;

    let dir = std::env::temp_dir().join(format!("helios-quit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    image::RgbImage::from_fn(8, 6, |x, y| {
        image::Rgb([(x * 30) as u8, (y * 40) as u8, 60])
    })
    .save(dir.join("photo.png"))
    .unwrap();
    let db = dir.join("vault.db");
    let image = {
        let mut shell = Shell::new(helios_panels(), None);
        let mut persist = Persist::new(
            SqliteStore::open(&db).unwrap(),
            std::sync::Arc::new(helios_core::Bus::new()),
        );
        let image = persist
            .vault()
            .import_file(&dir.join("photo.png"), 1)
            .unwrap();
        persist.fill_library(&mut shell).unwrap();
        shell.library_mut().select(0);
        // Held by the quit, as `main` holds it.
        let vault = persist.vault().clone();
        // Rated, then the window is asked to close: the closing frame
        // writes the rating.
        shell.library_mut().rate(4).unwrap();
        persist.frame(&mut shell, 10.0);
        drop(persist);
        drop(shell);
        // The window is gone; the vault closes against its deadline.
        let mut finisher = Finisher::default();
        finisher.spawn("vault closed", VAULT_DEADLINE, move || drop(vault));
        assert!(finisher.finish().is_empty());
        image
    };
    let store = SqliteStore::open(&db).unwrap();
    assert!(!store.report().unclean_shutdown, "closed cleanly");
    let vault = helios_vault::Vault::new(store);
    let row = vault.fields_of(&[image], &[RATING]).unwrap().remove(0);
    assert_eq!(row.get(RATING), Some(&json!(4)));
    drop(vault);
    let _ = std::fs::remove_dir_all(&dir);
}
