//! The library changed, one signal: an import or an undo fills the
//! Library once (one `cursor.refresh` of the cursor shown), and the
//! photos an import brought are selected in the pass the fill lands,
//! headless, over a vault on disk read through the desktop's node.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_pop::api::history::{Answer, Request};
use egui_pop::model::commands::Origin as PopOrigin;
use helios_core::Bus;
use helios_gui::import_end;
use helios_gui::library_history::LIBRARY;
use helios_gui::persist::Persist;
use helios_gui::{Shell, headless, helios_panels};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-library-changed-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A folder of `n` small pictures of their own under `dir`.
fn roll(dir: &Path, folder: &str, n: u8) -> PathBuf {
    let roll = dir.join(folder);
    std::fs::create_dir_all(&roll).unwrap();
    for i in 0..n {
        image::RgbImage::from_fn(16, 12, |x, y| {
            image::Rgb([i.wrapping_mul(53), (x * 9) as u8, (y * 13) as u8 ^ i])
        })
        .save(roll.join(format!("{i}.png")))
        .unwrap();
    }
    roll
}

/// A desktop as `main` wires it: the shell over a library on disk, its
/// imports' ends and its undo steps filling the Library again.
struct Desktop {
    h: Harness<'static, Shell>,
    persist: Persist<SqliteStore>,
    /// The host's "fill the Library again" (an import ended, a step
    /// undone).
    refill: Arc<AtomicBool>,
    at: f64,
    dir: PathBuf,
}

impl Drop for Desktop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn desktop(name: &str) -> Desktop {
    let dir = temp(name);
    let persist = Persist::new(
        SqliteStore::open(&dir.join("vault.db")).unwrap(),
        Arc::new(Bus::new()),
    );
    let mut shell = Shell::new(helios_panels(), None);
    let v = persist.vault().clone();
    shell.set_import_history(Arc::new(move || {
        helios_library::import_history::list(&v, None)
    }));
    shell.set_import_ends(None, egui::Context::default());
    let refill = Arc::new(AtomicBool::new(false));
    let filled = refill.clone();
    shell
        .listen_import_ended_by(import_end::REFILL, move |_| {
            filled.store(true, Ordering::Relaxed);
        })
        .unwrap();
    // Undo and redo walk the library's steps through the node, then the
    // Library is filled again.
    let (node, filled) = (persist.node().clone(), refill.clone());
    shell
        .library_history()
        .set_apply(Arc::new(move |changes, forward| {
            let r = node
                .request(
                    "library.changes.apply",
                    json!({ "changes": changes, "forward": forward }),
                )
                .map_err(|e| e.message)
                .and_then(|walked| serde_json::from_value(walked).map_err(|e| e.to_string()));
            filled.store(true, Ordering::Relaxed);
            r
        }));
    persist.fill_library(&mut shell).unwrap();
    let h = headless::harness(shell, egui::vec2(1280.0, 800.0));
    let mut d = Desktop {
        h,
        persist,
        refill,
        at: 0.0,
        dir,
    };
    d.settle();
    d
}

impl Desktop {
    /// One pass of the desktop's loop: the shell's frame, then the host's
    /// turn (as `main`'s `App::ui`).
    fn pass(&mut self) {
        self.at += 0.05;
        self.h.step();
        let shell = self.h.state_mut();
        if self.refill.swap(false, Ordering::Relaxed) {
            self.persist.ask_fill(shell, true);
        }
        self.persist.frame(shell, self.at);
        if self.persist.take_locations_changed() {
            shell.refresh_volumes();
            self.persist.ask_fill(shell, true);
        }
        let _ = self.persist.take_filled();
    }

    /// Passes until nothing is left to read or write, then a few more.
    fn settle(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut quiet = 0;
        while quiet < 10 {
            self.pass();
            if self.persist.busy() {
                quiet = 0;
            } else {
                quiet += 1;
            }
            assert!(Instant::now() < deadline, "the library never settled");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn refreshes(&self) -> u64 {
        self.persist.reader().refreshes()
    }

    fn library_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .h
            .state()
            .library()
            .entries
            .iter()
            .map(|e| e.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Imports `folder` as the user's `library.import` does: on a thread
    /// of its own, its lines and its end said to the shell.
    fn import(&mut self, folder: PathBuf) {
        let (vault, jobs) = (self.persist.vault().clone(), self.h.state().jobs());
        let (lines, ends) = (self.h.state().import_sink(), self.h.state().import_ends());
        let args = json!({ "paths": [folder], "origin": "user" });
        std::thread::spawn(move || {
            let mut emit = |line: &Value| {
                let _ = lines.send((args.clone(), line.clone()));
            };
            let options = helios_gui::importers::options(&args).unwrap();
            match helios_library::import::import(&vault, &jobs, &[folder], &options, 1, &mut emit) {
                Ok(report) => ends.ended(&args, report, None),
                Err(e) => ends.failed(&args, &e),
            }
        })
        .join()
        .unwrap();
    }
}

/// An import fills the Library once, and the photos it brought are
/// selected in the pass that shows them, not a pass later.
#[test]
fn an_import_fills_the_library_once_and_selects_its_photos_as_the_fill_lands() {
    let mut d = desktop("import");
    let roll = roll(&d.dir, "roll", 3);
    let before = d.refreshes();
    d.import(roll);
    let deadline = Instant::now() + Duration::from_secs(30);
    let new = loop {
        d.pass();
        let ids = d.library_ids();
        if ids.len() == 3 {
            break ids;
        }
        assert!(Instant::now() < deadline, "the import never showed");
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut selected = d.h.state().library().selection();
    selected.sort();
    assert_eq!(selected, new, "selected in the pass the fill landed");
    d.settle();
    assert_eq!(d.refreshes() - before, 1, "one fill for one import");
}

/// An undo fills the Library once.
#[test]
fn an_undo_fills_the_library_once() {
    let mut d = desktop("undo");
    let roll = roll(&d.dir, "roll", 2);
    d.import(roll);
    d.settle();
    assert_eq!(d.library_ids().len(), 2, "the import shows");
    let before = d.refreshes();
    let undone =
        d.h.state()
            .library_history()
            .undo(&Request::new(LIBRARY), &PopOrigin::User);
    assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    d.settle();
    assert!(d.library_ids().is_empty(), "the import undone");
    assert_eq!(d.refreshes() - before, 1, "one fill for one undo");
}
