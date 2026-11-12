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

/// A library with one photo, its vault owned by a local node the shell
/// is attached to, the GUI connected as a client: the desktop at its
/// start.
struct Desktop {
    dir: std::path::PathBuf,
    node: helios_gui::node::LocalNode<helios_vault_sqlite::SqliteStore>,
    client: Option<helios_gui::node::Client>,
    persist: helios_gui::persist::Persist<helios_vault_sqlite::SqliteStore>,
    image: helios_vault::ImageId,
}

fn desktop(name: &str) -> (Desktop, egui_kittest::Harness<'static, Shell>) {
    use helios_gui::node::LocalNode;
    use helios_gui::persist::Persist;
    use helios_vault_sqlite::SqliteStore;

    let dir = std::env::temp_dir().join(format!("helios-quit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    image::RgbImage::from_fn(8, 6, |x, y| {
        image::Rgb([(x * 30) as u8, (y * 40) as u8, 60])
    })
    .save(dir.join("photo.png"))
    .unwrap();
    let bus = std::sync::Arc::new(helios_core::Bus::new());
    // A long schedule: what reaches the file is the stop's doing.
    let node = LocalNode::open(
        SqliteStore::open(&dir.join("vault.db")).unwrap(),
        bus.clone(),
        std::time::Duration::from_hours(1),
    );
    let client = Some(node.connect());
    let persist = Persist::on(node.vault().clone(), bus);
    let image = persist
        .vault()
        .import_file(&dir.join("photo.png"), 1)
        .unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.attach_node(node.link());
    persist.fill_library(&mut shell).unwrap();
    shell.library_mut().select(0);
    let mut h = helios_gui::headless::harness(shell, egui::vec2(800.0, 500.0));
    h.ctx.set_embed_viewports(false);
    h.run();
    let d = Desktop {
        dir,
        node,
        client,
        persist,
        image,
    };
    (d, h)
}

impl Desktop {
    /// The GUI ends as the desktop's does: its closing frame sends the
    /// last change, the window's link goes, and the client disconnects
    /// (`on_exit`). Then the node, asked to quit, closes the vault on its
    /// own; the host waits for it.
    fn gui_ends(mut self, mut h: egui_kittest::Harness<'static, Shell>) {
        self.persist.frame(h.state_mut(), 10.0);
        drop(self.persist);
        drop(h);
        let link = self.node.link();
        assert!(link.quitting(), "the shell asked the node to quit");
        assert!(!link.stopped(), "the GUI is still connected");
        drop(self.client.take());
        assert!(
            link.wait_stopped(std::time::Duration::from_secs(10)),
            "the node closed the vault"
        );
        assert!(link.flushes() > 0, "flushed as it stopped");
        self.node.stop();
        let store = helios_vault_sqlite::SqliteStore::open(&self.dir.join("vault.db")).unwrap();
        assert!(!store.report().unclean_shutdown, "closed cleanly");
        let vault = helios_vault::Vault::new(store);
        let row = vault
            .fields_of(&[self.image], &[helios_vault::fields::RATING])
            .unwrap()
            .remove(0);
        assert_eq!(row.get(helios_vault::fields::RATING), Some(&json!(4)));
        drop(vault);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The last window closed: Helios quits through the node, which flushes
/// and closes the vault once the GUI is gone; a rating made just before
/// the close is kept.
#[test]
fn closing_the_last_window_closes_the_vault_through_the_node() {
    let (d, mut h) = desktop("last-window");
    h.state_mut().library_mut().rate(4).unwrap();
    let ctx = h.ctx.clone();
    assert_eq!(
        h.state_mut().main_window_closing(&ctx),
        helios_gui::shell::Closing::Quit
    );
    d.gui_ends(h);
}

/// Quit (the system menu's, File > Quit's command): the same, through the
/// node.
#[test]
fn quitting_closes_the_vault_through_the_node() {
    let (d, mut h) = desktop("quit");
    h.state_mut().library_mut().rate(4).unwrap();
    assert!(!d.node.link().quitting());
    h.state_mut()
        .run("workbench.quit", json!(null), Origin::User)
        .unwrap();
    h.run_steps(2);
    assert!(h.state().quitting());
    d.gui_ends(h);
}
