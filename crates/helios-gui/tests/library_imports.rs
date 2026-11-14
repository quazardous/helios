//! The Library's import sessions (`docs/design/library.md`, the Imports
//! section), headless: an import leaves a session with its photos; the
//! Library shows it (its `import:` filter selecting exactly them in the
//! vault), after a user's import when the setting is on, never after
//! another's; "Show in Library" and Run Workshop… on the selection.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_vault::{MemoryStore, Vault};
use serde_json::{Value, json};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-libimports-{name}-{}",
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

struct Setup {
    h: Harness<'static, Shell>,
    vault: Arc<Vault<MemoryStore>>,
    done: Arc<AtomicBool>,
    dir: PathBuf,
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A shell whose host runs `library.import` on a vault of its own (as
/// `main.rs` does without a node) and reads the import history from it.
fn setup(name: &str) -> Setup {
    let dir = temp(name);
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let done = Arc::new(AtomicBool::new(false));
    let mut shell = Shell::new(helios_panels(), None);
    let v = vault.clone();
    shell.set_import_history(Arc::new(move || {
        helios_library::import_history::list(&v, None)
    }));
    let (v, d, lines, jobs) = (
        vault.clone(),
        done.clone(),
        shell.import_sink(),
        shell.jobs(),
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
                let (v, jobs, d, lines, asked) = (
                    v.clone(),
                    jobs.clone(),
                    d.clone(),
                    lines.clone(),
                    args.clone(),
                );
                std::thread::spawn(move || {
                    let mut emit = |line: &Value| {
                        let _ = lines.send((asked.clone(), line.clone()));
                    };
                    let end = match helios_library::import::import(
                        &v, &jobs, &sources, &options, 1, &mut emit,
                    ) {
                        Ok(report) => json!({ "ended": report }),
                        Err(e) => json!({ "failed": e }),
                    };
                    let _ = lines.send((asked, end));
                    d.store(true, Ordering::Relaxed);
                });
                Ok(json!({ "queued": true }))
            },
        )
        .unwrap();
    let h = headless::harness(shell, egui::vec2(1280.0, 800.0));
    Setup {
        h,
        vault,
        done,
        dir,
    }
}

impl Setup {
    /// Imports a folder of `n` new pictures as `origin` asks; the
    /// Library filled again with every photo of the vault, as the host
    /// does. The ids it brought, sorted.
    fn import(&mut self, folder: &str, n: u8, origin: &str) -> Vec<String> {
        let roll = self.dir.join(folder);
        let seed = folder.bytes().fold(0u8, u8::wrapping_add);
        for i in 0..n {
            picture(&roll.join(format!("{i}.png")), seed.wrapping_add(i * 31));
        }
        let before: Vec<String> = self.ids();
        self.done.store(false, Ordering::Relaxed);
        self.h
            .state_mut()
            .run(
                "library.import",
                json!({ "paths": [roll], "origin": origin }),
                Origin::User,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.done.load(Ordering::Relaxed) {
            assert!(Instant::now() < deadline, "the import not done in 30 s");
            std::thread::sleep(Duration::from_millis(20));
        }
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
        let mut new: Vec<String> = all.into_iter().filter(|i| !before.contains(i)).collect();
        new.sort();
        new
    }

    fn ids(&self) -> Vec<String> {
        self.vault
            .images()
            .unwrap()
            .into_iter()
            .map(|i| i.id.hex())
            .collect()
    }

    /// The photos the Library's filter selects in the vault, sorted.
    fn filtered(&self) -> Vec<String> {
        let query = helios_gui::library::query_of(self.h.state().library().filter());
        let cursor = self.vault.open_cursor(query).unwrap();
        let mut ids: Vec<String> = self
            .vault
            .cursor_ids(cursor.id, 0, 999)
            .unwrap()
            .into_iter()
            .map(|i| i.hex())
            .collect();
        self.vault.close_cursor(cursor.id);
        ids.sort();
        ids
    }

    fn selection(&self) -> Vec<String> {
        let mut s = self.h.state().library().selection();
        s.sort();
        s
    }

    fn sessions(&self) -> Vec<helios_gui::library::ImportSession> {
        self.h.state().library().imports().to_vec()
    }

    fn set_show(&mut self, on: bool) {
        use egui_pop::model::values::{Change, Set, Value as PopValue};
        let store = self.h.state().app().handle().store();
        store
            .lock()
            .unwrap()
            .set(
                Set::new(
                    &format!("settings/{}", helios_gui::shell::SHOW_IMPORT_SETTING),
                    Change::To(PopValue::Bool(on)),
                    egui_pop::model::commands::Origin::User,
                )
                .untracked(),
            )
            .unwrap();
        self.h.step();
    }
}

#[test]
fn a_user_import_is_a_session_the_library_shows_with_its_photos_selected() {
    let mut s = setup("shown");
    assert!(s.h.state().shows_imports_in_library(), "on by default");
    let first = s.import("first", 2, "user");
    let second = s.import("second", 3, "user");
    // The history keeps each session with its photos.
    let records = helios_library::import_history::list(&s.vault, None).unwrap();
    assert_eq!(records.len(), 2);
    let mut kept = records[0].images.clone();
    kept.sort();
    assert_eq!(kept, second, "the newest session holds its photos");
    // The Imports section lists them, newest first, with their counts.
    let sessions = s.sessions();
    assert_eq!(sessions.iter().map(|x| x.count).collect::<Vec<_>>(), [3, 2]);
    assert_eq!(sessions[0].id, records[0].id);
    // The Library shows the last one: its filter selects exactly its
    // photos in the vault, all of them selected.
    let shown = s.h.state().library().import_session();
    assert_eq!(shown.as_deref(), Some(records[0].id.as_str()));
    assert_eq!(s.filtered(), second);
    assert_eq!(s.selection(), second);
    // Show in Library, from the history: the first session.
    s.h.state_mut()
        .run(
            helios_gui::library::IMPORT_SESSION_COMMAND,
            json!({ "id": records[1].id }),
            Origin::User,
        )
        .unwrap();
    s.h.run_steps(2);
    assert_eq!(s.filtered(), first);
    // No session: the whole Library again.
    s.h.state_mut()
        .run(
            helios_gui::library::IMPORT_SESSION_COMMAND,
            json!({ "id": null }),
            Origin::User,
        )
        .unwrap();
    s.h.run_steps(2);
    assert_eq!(s.h.state().library().import_session(), None);
    assert_eq!(s.filtered().len(), 5, "the whole library again");
}

#[test]
fn with_the_setting_off_or_another_s_import_the_selection_does_not_change() {
    let mut s = setup("off");
    s.set_show(false);
    assert!(!s.h.state().shows_imports_in_library());
    s.import("roll", 2, "user");
    assert!(s.selection().is_empty(), "nothing selected");
    assert_eq!(s.h.state().library().import_session(), None);
    // On again: an agent's or an automatic import never changes it.
    s.set_show(true);
    s.import("agent", 2, "agent:tester");
    s.import("card", 1, "auto:EOS_DIGITAL");
    assert!(s.selection().is_empty(), "nothing selected");
    assert_eq!(s.h.state().library().import_session(), None);
    assert_eq!(s.sessions().len(), 3, "every import is listed");
}

#[test]
fn run_workshop_on_the_selection_is_refused_while_the_workshops_are_off() {
    let mut s = setup("workshop");
    s.import("roll", 1, "user");
    s.h.state_mut()
        .run(
            helios_gui::library::RUN_WORKSHOP_COMMAND,
            json!({}),
            Origin::User,
        )
        .unwrap();
    s.h.run_steps(2);
    let said: Vec<&str> =
        s.h.state()
            .notices()
            .iter()
            .map(|n| n.text.as_str())
            .collect();
    assert!(
        said.iter()
            .any(|t| t.contains("interface.experimental.workshops")),
        "{said:?}"
    );
}
