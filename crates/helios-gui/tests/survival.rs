//! Survival scenarios for the desktop: a child process (this test binary,
//! run again on one test) opens a vault, rates and labels an image, edits
//! it through the value store as widgets do, and either quits or is killed
//! with SIGKILL in the middle of a drag. After each, the history, its end
//! (the head version), the ratings, the labels and the metadata must be
//! what the last saved step left, and reopening shows exactly that. A
//! workspace's layout, changed then killed, is kept too; so is an
//! agent's edit made through the desktop (the node's MCP tools), with the
//! desktop's own step after it.
//!
//! The child says what it saved on stdout from the `edit.saved` events of
//! the bus: the parent reads those lines, it never sleeps. These scenarios
//! gate a release: `cargo survival` runs them alone.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use helios_core::{Bus, Graph, NodeId};
use helios_gui::persist::{FLUSH_SECONDS, Persist};
use helios_gui::{Shell, helios_panels};
use helios_shell::commands::Origin;
use helios_vault::fields::{Command as Field, LABELS, RATING, REJECTED};
use helios_vault::{ImageId, Vault};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

const CHILD: &str = "HELIOS_GUI_SURVIVAL_CHILD";
/// How long the child may stay silent: only a hang reaches it.
const CHILD_DEADLINE: std::time::Duration = std::time::Duration::from_mins(1);
const EV: &str = "node/exposure/exposure";

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

fn set(shell: &Shell, ev: f64, gesture: Option<helios_values::GestureId>) {
    let mut set = helios_values::Set::new(
        EV,
        helios_values::Change::To(helios_core::Value::Float(ev)),
        Origin::User,
    );
    if let Some(g) = gesture {
        set = set.gesture(g);
    }
    shell.values.lock().unwrap().set(set).unwrap();
}

fn shown(shell: &Shell) -> f64 {
    shell
        .values
        .lock()
        .unwrap()
        .get(EV)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

/// A shell on the vault at `path`, the photo open with its saved edit.
fn open(path: &Path, photo: &Path) -> (Shell, Persist<SqliteStore>, ImageId) {
    let mut shell = Shell::new(helios_panels(), None);
    let mut persist = Persist::new(SqliteStore::open(path).unwrap(), Arc::new(Bus::new()));
    persist.fill_library(&mut shell).unwrap();
    shell.library_mut().select(0);
    let image = persist.vault().import_file(photo, 1).unwrap();
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, image, graph, exp, "output")
        .unwrap();
    (shell, persist, image)
}

/// Not a test of its own: the desktop the scenarios quit or kill.
#[test]
fn survival_child() {
    let Ok(dir) = std::env::var(CHILD) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let scenario = std::env::var("HELIOS_GUI_SURVIVAL_SCENARIO").unwrap();
    if scenario == "agent" {
        // The workspace held open: an agent's edit answered, then a step
        // of the desktop's own, then killed.
        let mut h = workspace_window(&dir);
        writeln!(std::io::stdout(), "\nlistening").unwrap();
        std::io::stdout().flush().unwrap();
        assert!(h.state().wait_workspace_request(CHILD_DEADLINE));
        h.run();
        h.state_mut()
            .edit_workspace(&json!({ "name": "Harbour" }), &Origin::User)
            .unwrap();
        h.run();
        writeln!(std::io::stdout(), "\nready").unwrap();
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        return;
    }
    if scenario == "layout" {
        // A panel moved to a window of its own, the workspace described
        // and its culling set, then killed.
        let mut h = workspace_window(&dir);
        h.state_mut()
            .run("helios.log.detach", json!(null), Origin::User)
            .unwrap();
        h.run();
        h.state_mut()
            .edit_workspace(
                &json!({ "description": "Harbour", "library": { "auto_advance": true } }),
                &Origin::User,
            )
            .unwrap();
        h.run();
        // On a line of its own: the test harness may have started one
        // ("test survival_child ... ").
        writeln!(std::io::stdout(), "\nready").unwrap();
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        return;
    }
    let (mut shell, mut persist, _) = open(&dir.join("vault.db"), &dir.join("photo.png"));
    let events = persist.bus().subscribe();
    let mut out = std::io::stdout();
    let mut frame = |shell: &mut Shell, persist: &mut Persist<SqliteStore>, now: f64| {
        persist.frame(shell, now);
        while let Ok(e) = events.try_recv() {
            if e.kind == "edit.saved" {
                writeln!(
                    out,
                    "saved {} {}",
                    e.payload["step"],
                    e.payload["version"].as_str().unwrap()
                )
                .unwrap();
            }
        }
        out.flush().unwrap();
    };
    // The library: three stars, a label.
    shell.library_mut().rate(3).unwrap();
    shell.library_mut().toggle_label("green").unwrap();
    frame(&mut shell, &mut persist, 100.0);
    // One write, then a drag saved while it goes.
    set(&shell, 1.0, None);
    frame(&mut shell, &mut persist, 101.0);
    let drag = shell
        .values
        .lock()
        .unwrap()
        .begin_gesture(Origin::User, "drag exposure");
    set(&shell, 1.3, Some(drag));
    frame(&mut shell, &mut persist, 102.0);
    set(&shell, 1.7, Some(drag));
    frame(&mut shell, &mut persist, 102.0 + FLUSH_SECONDS);
    if scenario == "kill" {
        // Still dragging, past the last save: this much is lost.
        set(&shell, 2.2, Some(drag));
        frame(&mut shell, &mut persist, 102.5 + FLUSH_SECONDS);
        writeln!(std::io::stdout(), "ready").unwrap();
        std::io::stdout().flush().unwrap();
        // Until the parent kills it (or goes away).
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        return;
    }
    set(&shell, 1.9, Some(drag));
    shell.values.lock().unwrap().end(drag);
    frame(&mut shell, &mut persist, 104.0);
    shell.values.lock().unwrap().undo(&Origin::User).unwrap();
    frame(&mut shell, &mut persist, 105.0);
    shell.library_mut().toggle_reject().unwrap();
    frame(&mut shell, &mut persist, 106.0);
    writeln!(std::io::stdout(), "ready").unwrap();
    // Quits: everything dropped, nothing more to save.
}

/// A window on the scenario's workspace (`photos`), its interface
/// settings and its own files beside it, its saved layout loaded, its
/// state opened for editing.
fn workspace_window(dir: &Path) -> egui_kittest::Harness<'static, Shell> {
    let mut h = helios_gui::headless::harness(workspace_shell(dir), egui::vec2(800.0, 500.0));
    h.run();
    h
}

/// The scenario's workspace as a desktop opens it: its saved layout
/// loaded, its state read from its files.
fn workspace_shell(dir: &Path) -> Shell {
    let mut shell = Shell::builder(helios_panels(), Some(dir.join("ui")))
        .workspace(&dir.join("photos"))
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(dir, &dir.join("photos")).unwrap();
    shell
}

/// A scenario's directory: a vault holding the photo, titled and labelled
/// red by another writer before the desktop starts.
fn place(name: &str) -> (PathBuf, ImageId) {
    let dir =
        std::env::temp_dir().join(format!("helios-gui-survival-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    image::RgbImage::from_fn(12, 8, |x, y| {
        image::Rgb([(x * 20) as u8, (y * 30) as u8, 90])
    })
    .save(dir.join("photo.png"))
    .unwrap();
    let vault = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap());
    let image = vault.import_file(&dir.join("photo.png"), 1).unwrap();
    vault
        .apply(
            &[
                Field::add(image, LABELS, "red"),
                Field::set(image, "title", json!("harbour")),
            ],
            "agent",
            None,
            1,
        )
        .unwrap();
    (dir, image)
}

/// Runs the child on a scenario; returns what it said it saved (step,
/// version), and kills it once it is ready when `kill` is set.
fn run_child(dir: &Path, scenario: &str, kill: bool) -> Vec<(u64, String)> {
    run_child_with(dir, scenario, kill, &mut |_| {})
}

/// [`run_child`], `on_line` seeing each line the child says.
fn run_child_with(
    dir: &Path,
    scenario: &str,
    kill: bool,
    on_line: &mut dyn FnMut(&str),
) -> Vec<(u64, String)> {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "survival_child",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, dir)
        .env("HELIOS_GUI_SURVIVAL_SCENARIO", scenario)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut saved = Vec::new();
    let mut ready = false;
    // The child's lines, read on a thread of their own: a child that
    // stops talking fails the scenario at the deadline, never hangs it.
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        let line = match lines.recv_timeout(CHILD_DEADLINE) {
            Ok(line) => line.unwrap(),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the child said nothing for {CHILD_DEADLINE:?}: killed");
            }
        };
        on_line(&line);
        if let Some(rest) = line.strip_prefix("saved ") {
            let (step, version) = rest.split_once(' ').unwrap();
            saved.push((step.parse().unwrap(), version.to_string()));
        } else if line == "ready" {
            ready = true;
            // Killed now; or read to the end, so the child's last words
            // never meet a closed pipe.
            if kill {
                break;
            }
        }
    }
    assert!(ready, "the child did not get through its scenario");
    if kill {
        child.kill().unwrap();
    }
    let status = child.wait().unwrap();
    assert_eq!(status.success(), !kill, "{status}");
    saved
}

/// The labels of the history leading to the image's one head, and that
/// head.
fn history(vault: &Vault<SqliteStore>, image: ImageId) -> (Vec<String>, helios_vault::Version) {
    let heads = vault.heads(&image).unwrap();
    assert_eq!(heads.len(), 1, "one line of history");
    let labels = vault
        .history(&heads[0].id)
        .unwrap()
        .into_iter()
        .map(|v| v.label.unwrap_or_default())
        .collect();
    (labels, heads[0].clone())
}

fn ev_of(edit: &Value) -> f64 {
    edit["modules"][0]["params"]["exposure"]["float"]
        .as_f64()
        .unwrap()
}

fn fields(vault: &Vault<SqliteStore>, image: ImageId) -> Vec<Option<Value>> {
    let row = vault
        .fields_of(&[image], &[RATING, LABELS, REJECTED, "title"])
        .unwrap()
        .remove(0);
    [RATING, LABELS, REJECTED, "title"]
        .iter()
        .map(|f| row.get(*f).cloned())
        .collect()
}

#[test]
fn survival_quit_and_reopen_keeps_the_history_and_the_library() {
    let (dir, image) = place("quit");
    let saved = run_child(&dir, "quit", false);
    let vault = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap());
    let (labels, head) = history(&vault, image);
    assert_eq!(
        labels,
        [
            format!("set {EV}"),
            "drag exposure (in progress)".into(),
            "drag exposure".into(),
            "undo drag exposure".into(),
        ]
    );
    // The end of the history is the last step the desktop announced.
    assert_eq!(saved.last().unwrap(), &(4, head.id.hex()));
    assert!((ev_of(&head.edit) - 1.0).abs() < 1e-9, "{}", head.edit);
    assert_eq!(
        fields(&vault, image),
        [
            Some(json!(3)),
            Some(json!(["green", "red"])),
            Some(json!(true)),
            Some(json!("harbour")),
        ]
    );
    drop(vault);
    // Reopened, the desktop shows that.
    let (shell, _persist, _) = open(&dir.join("vault.db"), &dir.join("photo.png"));
    assert!((shown(&shell) - 1.0).abs() < 1e-9, "{}", shown(&shell));
    let entry = shell.library().entries[0].clone();
    assert_eq!(
        (entry.rating, entry.rejected, entry.labels),
        (3, true, vec!["red".to_string(), "green".to_string()])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn survival_killed_in_the_middle_of_a_drag_keeps_the_last_saved_step() {
    let (dir, image) = place("kill");
    let saved = run_child(&dir, "kill", true);
    let vault = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap());
    let (labels, head) = history(&vault, image);
    assert_eq!(
        labels,
        [format!("set {EV}"), "drag exposure (in progress)".into()]
    );
    assert_eq!(saved.last().unwrap(), &(2, head.id.hex()));
    // The drag as it was last saved, not where the kill caught it.
    assert!((ev_of(&head.edit) - 1.7).abs() < 1e-9, "{}", head.edit);
    assert_eq!(
        fields(&vault, image),
        [
            Some(json!(3)),
            Some(json!(["green", "red"])),
            None,
            Some(json!("harbour")),
        ]
    );
    drop(vault);
    let (shell, _persist, _) = open(&dir.join("vault.db"), &dir.join("photo.png"));
    assert!((shown(&shell) - 1.7).abs() < 1e-9, "{}", shown(&shell));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn survival_killed_after_moving_a_panel_and_editing_the_workspace_keeps_both() {
    let (dir, _) = place("layout");
    run_child(&dir, "layout", true);
    // Reopened, before any frame: the layout document and the
    // workspace's files as the kill left them.
    let shell = workspace_shell(&dir);
    let detached: Vec<String> = shell
        .layout()
        .auxiliary
        .iter()
        .map(|a| a.panel.to_string())
        .collect();
    assert_eq!(detached, ["helios.log"]);
    let state = shell.workspace_state().unwrap();
    assert_eq!(state.meta.description, "Harbour");
    assert!(shell.library().auto_advance);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn survival_killed_after_an_agent_s_edit_through_the_desktop_keeps_it() {
    let (dir, _) = place("agent");
    let workspace = dir.join("photos");
    let mut agent = None;
    let mut on_line = |line: &str| {
        if line == "listening" {
            let arguments = json!({
                "workspace": workspace, "config": dir,
                "changes": { "description": "From the agent" },
            });
            agent = Some(std::thread::spawn(move || {
                let mut sockets = helios_node::mcp::Sockets {
                    inspect: None,
                    control: None,
                    node_bin: PathBuf::from("helios-node"),
                    ai: None,
                };
                helios_node::mcp::handle(
                    &mut sockets,
                    &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": { "name": "helios_workspace_edit", "arguments": arguments } }),
                )
                .unwrap()
            }));
        }
    };
    run_child_with(&dir, "agent", true, &mut on_line);
    let answer = agent.expect("the child listened").join().unwrap();
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(answer["result"]["isError"], false, "{text}");
    assert!(
        text.contains("\"desktop\""),
        "answered by the desktop: {text}"
    );
    // Reopened: the agent's edit and the desktop's step after it.
    let shell = workspace_shell(&dir);
    let state = shell.workspace_state().unwrap();
    assert_eq!(
        (state.meta.name.as_str(), state.meta.description.as_str()),
        ("Harbour", "From the agent")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
