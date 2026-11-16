//! An import's end, published once (`helios_gui::import_end`), headless:
//! the same import run through the `library.import` command's thread, the
//! photos chosen in a service's page (imported in-process, in two
//! batches) and the node's promise passes through the same listeners of
//! `import.ended`, in the same declared order, with the same effects: an
//! undo step, the backups "after each import" asked, the Library filled,
//! the cameras' looks put on, the end in the Log panel, the session shown
//! and the photos it brought selected.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use helios_gui::import_end::{self, NodeWatch, ORDER};
use helios_gui::{Shell, headless, helios_panels};
use helios_node_core::import_promise::ImportWatch as _;
use helios_shell::commands::Origin;
use helios_vault::{MemoryStore, Vault};
use serde_json::{Value, json};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-import-end-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A small picture of its own at `path` (`seed`: its content).
fn picture(path: &Path, seed: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(16, 12, |x, y| {
        image::Rgb([seed, (x * 9) as u8, (y * 13) as u8 ^ seed])
    })
    .save(path)
    .unwrap();
}

/// A folder of `n` new pictures under `dir`.
fn roll(dir: &Path, folder: &str, n: u8) -> PathBuf {
    let roll = dir.join(folder);
    let seed = folder.bytes().fold(0u8, u8::wrapping_add);
    for i in 0..n {
        picture(&roll.join(format!("{i}.png")), seed.wrapping_add(i * 31));
    }
    roll
}

struct World {
    h: Harness<'static, Shell>,
    vault: Arc<Vault<MemoryStore>>,
    jobs: helios_core::jobs::Jobs,
    /// The backups "after each import" asked.
    backups: Arc<AtomicUsize>,
    /// The Library filled again.
    refills: Arc<AtomicUsize>,
    /// The cameras' looks put on by the desktop (not by the node).
    looks: Arc<AtomicUsize>,
    dir: PathBuf,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A shell whose host does what `main.rs` does: `library.import` runs on
/// a thread of its own and says its end through the shell's publisher,
/// the backups and the Library's fill listen to `import.ended`, and the
/// cameras' looks (a fake, one look per import) are put on by the
/// publisher.
fn world(name: &str) -> World {
    let dir = temp(name);
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let (backups, refills, looks) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    );
    let mut shell = Shell::new(helios_panels(), None);
    let v = vault.clone();
    shell.set_import_history(Arc::new(move || {
        helios_library::import_history::list(&v, None)
    }));
    let looked = looks.clone();
    shell.set_import_ends(
        Some(Arc::new(move |_changes| {
            looked.fetch_add(1, Ordering::SeqCst);
            Ok(1)
        })),
        egui::Context::default(),
    );
    let asked = backups.clone();
    shell
        .listen_import_ended_by(import_end::BACKUPS, move |end| {
            if end.outcome.is_ok() {
                asked.fetch_add(1, Ordering::SeqCst);
            }
        })
        .unwrap();
    let filled = refills.clone();
    shell
        .listen_import_ended_by(import_end::REFILL, move |_| {
            filled.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    let jobs = shell.jobs();
    let (v, lines, ends, j) = (
        vault.clone(),
        shell.import_sink(),
        shell.import_ends(),
        jobs.clone(),
    );
    shell
        .register(
            helios_shell::commands::CommandSpec::new("library.import", "Import", "Library"),
            move |args, _| {
                let options = helios_gui::importers::options(args)?;
                let sources: Vec<PathBuf> = args["paths"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect();
                let (v, jobs, lines, ends, asked) = (
                    v.clone(),
                    j.clone(),
                    lines.clone(),
                    ends.clone(),
                    args.clone(),
                );
                std::thread::spawn(move || {
                    let mut emit = |line: &Value| {
                        let _ = lines.send((asked.clone(), line.clone()));
                    };
                    match helios_library::import::import(
                        &v, &jobs, &sources, &options, 1, &mut emit,
                    ) {
                        Ok(report) => ends.ended(&asked, report, None),
                        Err(e) => ends.failed(&asked, &e),
                    }
                });
                Ok(json!({ "queued": true }))
            },
        )
        .unwrap();
    let h = headless::harness(shell, egui::vec2(1280.0, 800.0));
    World {
        h,
        vault,
        jobs,
        backups,
        refills,
        looks,
        dir,
    }
}

/// What one import's end left.
#[derive(Debug, PartialEq)]
struct Effects {
    /// The listeners that ran, in order.
    ran: Vec<String>,
    undo_steps: usize,
    backups: usize,
    refills: usize,
    camera_looks: u64,
    /// The Log panel's lines saying it ended.
    logged: usize,
    /// The Library shows an import session.
    session: bool,
    /// The photos it brought, selected.
    selected: bool,
}

impl World {
    /// The Log panel's lines saying an import ended.
    fn logged(&self) -> usize {
        self.h
            .state()
            .log()
            .entries()
            .iter()
            .filter(|e| e.message.english.contains("photos imported"))
            .count()
    }

    fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .vault
            .images()
            .unwrap()
            .into_iter()
            .map(|i| i.id.hex())
            .collect();
        ids.sort();
        ids
    }

    /// Runs `import` (which starts an import on a thread of its own),
    /// waits for its `import.ended` on the journal, fills the Library as
    /// the host does, and says what the end left.
    fn effects(&mut self, import: impl FnOnce(&mut Self)) -> Effects {
        let before = self.ids();
        let handle = self.h.state().app().handle().clone();
        let since = handle.last_event();
        let (backups, refills) = (
            self.backups.load(Ordering::SeqCst),
            self.refills.load(Ordering::SeqCst),
        );
        let undo = self.h.state().library_history().line().undo.len();
        let logged = self.logged();
        import(self);
        let deadline = Instant::now() + Duration::from_secs(30);
        let ended = loop {
            self.h.step();
            if let Some(e) = handle
                .events_since(since)
                .into_iter()
                .find(|e| e.name == import_end::ENDED || e.name == import_end::FAILED)
            {
                break e;
            }
            assert!(Instant::now() < deadline, "no import end in 30 s");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(ended.name, import_end::ENDED, "{}", ended.data);
        // The host fills the Library with every photo of the vault.
        let all = self.ids();
        self.h.state_mut().set_library(
            all.iter()
                .map(|id| helios_gui::library::Entry {
                    id: id.clone(),
                    name: format!("{id}.png"),
                    ..helios_gui::library::Entry::default()
                })
                .collect(),
        );
        self.h.run_steps(3);
        let new: Vec<String> = all.into_iter().filter(|i| !before.contains(i)).collect();
        assert!(!new.is_empty(), "the import brought photos");
        let mut selection = self.h.state().library().selection();
        selection.sort();
        Effects {
            ran: ended.data["ran"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r.as_str().unwrap().to_owned())
                .collect(),
            undo_steps: self.h.state().library_history().line().undo.len() - undo,
            backups: self.backups.load(Ordering::SeqCst) - backups,
            refills: self.refills.load(Ordering::SeqCst) - refills,
            camera_looks: ended.data["camera_looks"].as_u64().unwrap_or(0),
            logged: self.logged() - logged,
            session: self.h.state().library().import_session().is_some(),
            selected: selection == new,
        }
    }
}

#[test]
fn every_import_path_runs_the_same_listeners_in_order_with_the_same_effects() {
    let mut w = world("paths");
    let expected = Effects {
        ran: ORDER.iter().map(|(id, _)| (*id).to_owned()).collect(),
        undo_steps: 1,
        backups: 1,
        refills: 1,
        camera_looks: 1,
        logged: 1,
        session: true,
        selected: true,
    };
    // The `library.import` command, run on its own thread.
    let local = roll(&w.dir, "local", 2);
    let by_command = w.effects(|w| {
        w.h.state_mut()
            .run(
                "library.import",
                json!({ "paths": [local], "origin": "user" }),
                Origin::User,
            )
            .unwrap();
    });
    assert_eq!(by_command, expected, "the library.import command");
    // Photos chosen in a service's page, imported in-process in two
    // batches (as the import window's thread does): one end for both.
    let (a, b) = (roll(&w.dir, "remote-a", 1), roll(&w.dir, "remote-b", 2));
    let chosen = w.effects(|w| {
        let (vault, jobs) = (w.vault.clone(), w.jobs.clone());
        let (lines, ends) = (w.h.state().import_sink(), w.h.state().import_ends());
        std::thread::spawn(move || {
            let args = json!({ "origin": "user" });
            let mut reports = Vec::new();
            for batch in [a, b] {
                let mut emit = |line: &Value| {
                    let _ = lines.send((args.clone(), line.clone()));
                };
                let options = helios_gui::importers::options(&args).unwrap();
                reports.push(
                    helios_library::import::import(&vault, &jobs, &[batch], &options, 1, &mut emit)
                        .unwrap(),
                );
            }
            ends.ended(&args, import_end::merged(reports), None);
        });
    });
    assert_eq!(chosen, expected, "the photos chosen in a service's page");
    // The node's promise: the node puts the looks on itself, then tells
    // the desktop's watch.
    let node = roll(&w.dir, "node", 2);
    let looks_before = w.looks.load(Ordering::SeqCst);
    let promised = w.effects(|w| {
        let (vault, jobs) = (w.vault.clone(), w.jobs.clone());
        let watch = NodeWatch::new(w.h.state().import_sink(), w.h.state().import_ends());
        std::thread::spawn(move || {
            let request = json!({ "rule": { "items": [node] }, "origin": "user" });
            let options = helios_gui::importers::options(&json!({})).unwrap();
            let mut report = helios_library::import::import(
                &vault,
                &jobs,
                &[node],
                &options,
                1,
                &mut |line: &Value| watch.line(&request, line),
            )
            .unwrap();
            watch.ended(&request, &mut report, 1);
        });
    });
    assert_eq!(promised, expected, "the node's promise");
    assert_eq!(
        w.looks.load(Ordering::SeqCst),
        looks_before,
        "the node's looks are not put on twice"
    );
}

#[test]
fn the_import_end_s_listeners_run_in_a_declared_order() {
    let w = world("order");
    assert_eq!(
        w.h.state().import_end_order(),
        "import.ended\n\
         \x20 1. library.history (owner helios.import): nothing\n\
         \x20 2. backups.on-import (owner helios.import): after library.history\n\
         \x20 3. library.refill (owner helios.import): after backups.on-import\n\
         \x20 4. storage.cards-waiting (owner helios.import): after library.refill\n\
         \x20 5. duplicates.collect (owner helios.import): after storage.cards-waiting\n\
         \x20 6. log.import (owner helios.import): after duplicates.collect\n\
         \x20 7. import.toast (owner helios.import): after log.import\n\
         \x20 8. library.import-session (owner helios.import): after import.toast\n\
         \x20 9. import-window.history (owner helios.import): after library.import-session\n\
         \x20 10. library.select-imported (owner helios.import): after import-window.history\n"
    );
}
