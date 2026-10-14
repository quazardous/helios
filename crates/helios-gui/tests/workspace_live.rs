//! A workspace kept live on the desktop: the node's MCP tools go through
//! the desktop that holds it open, so an agent's edit is seen at once and
//! is one step of Edit > Undo; files changed outside the desktop are
//! taken in as one undo step, part by part, and the desktop's next save
//! never writes over them; without a desktop the tools write the files.

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::model::ids::PanelId;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use helios_template::workspace::{Files, Holder, tools};
use serde_json::{Value, json};

/// Only a hang reaches it.
const DEADLINE: Duration = Duration::from_mins(1);

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-wslive-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn desktop() -> helios_template::Catalogue {
    let shell = Shell::new(helios_panels(), None);
    templates::catalogue(shell.panels(), shell.specs())
}

/// A workspace made from the portrait template under `root`, and a
/// window holding it open.
fn window(root: &Path) -> (Harness<'static, Shell>, PathBuf, PathBuf, Files) {
    let config = root.join("config");
    let created = templates::create_workspace(
        &templates::find("helios.portrait-retouch").unwrap(),
        &desktop(),
        &root.join("work"),
        &config,
    )
    .unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&created.workspace)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &created.workspace).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let files = Files::new(&config, &created.workspace);
    (h, config, created.workspace, files)
}

/// A tool call as an MCP client makes it: `(failed, answer)`.
fn call(name: &str, arguments: &Value) -> (bool, Value) {
    let mut sockets = helios_node::mcp::Sockets {
        inspect: None,
        control: None,
        node_bin: PathBuf::from("helios-node"),
        ai: None,
    };
    let answer = helios_node::mcp::handle(
        &mut sockets,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": arguments } }),
    )
    .unwrap();
    let result = &answer["result"];
    let text = result["content"][0]["text"].as_str().unwrap();
    (
        result["isError"] == true,
        serde_json::from_str(text).unwrap_or(json!(text)),
    )
}

/// A tool call answered by the window: the client waits on a thread of
/// its own while the window runs the frame that answers it.
fn through(h: &mut Harness<'static, Shell>, name: &str, arguments: Value) -> (bool, Value) {
    let (tx, rx) = channel();
    let name = name.to_string();
    std::thread::spawn(move || {
        let _ = tx.send(call(&name, &arguments));
    });
    assert!(
        h.state().wait_workspace_request(DEADLINE),
        "the request never reached the window"
    );
    h.run();
    rx.recv_timeout(DEADLINE).expect("the tool never answered")
}

fn args(dir: &Path, config: &Path, extra: &Value) -> Value {
    let mut a = json!({ "workspace": dir, "config": config });
    for (k, v) in extra.as_object().unwrap() {
        a[k] = v.clone();
    }
    a
}

fn steps(h: &Harness<'static, Shell>) -> usize {
    h.state().values.lock().unwrap().undo_steps().len()
}

fn undo(h: &mut Harness<'static, Shell>) {
    h.state()
        .values
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    h.run();
}

fn description(h: &Harness<'static, Shell>) -> String {
    h.state().workspace_state().unwrap().meta.description
}

fn panel(id: &str) -> PanelId {
    PanelId::new(id).unwrap()
}

#[test]
fn an_agent_s_edit_goes_through_the_desktop_seen_at_once_and_undone_there() {
    let root = temp("through");
    let (mut h, config, dir, files) = window(&root);
    let holder = files
        .holder()
        .expect("the window says it holds the workspace");
    assert_eq!(h.state().workspace_holder(), Some(holder));
    let before = steps(&h);
    let start = description(&h);
    // A dry run answers from the window and changes nothing.
    let changes = json!({ "description": "Agent's", "tools": { "toolbox": ["mask.add.circle"] } });
    let (failed, dry) = through(
        &mut h,
        "helios_workspace_edit",
        args(
            &dir,
            &config,
            &json!({ "changes": changes, "dry_run": true }),
        ),
    );
    assert!(!failed, "{dry}");
    assert_eq!(
        (&dry["via"], &dry["dry_run"], &dry["problems"]),
        (&json!("desktop"), &json!(true), &json!([]))
    );
    assert_eq!((steps(&h), description(&h)), (before, start.clone()));
    // The edit: one step of the window's undo, shown and written at once.
    let (failed, done) = through(
        &mut h,
        "helios_workspace_edit",
        args(&dir, &config, &json!({ "changes": changes })),
    );
    assert!(!failed, "{done}");
    assert_eq!(done["via"], "desktop");
    assert_eq!(steps(&h), before + 1);
    assert_eq!(description(&h), "Agent's");
    assert_eq!(files.load().unwrap().meta.description, "Agent's");
    let (_, state) = through(
        &mut h,
        "helios_workspace_state",
        args(&dir, &config, &json!({})),
    );
    assert_eq!(state["meta"]["description"], "Agent's");
    // The window's next save keeps it.
    h.state_mut()
        .edit_workspace(&json!({ "name": "Faces" }), &Origin::User)
        .unwrap();
    h.run();
    let kept = files.load().unwrap();
    assert_eq!(
        (kept.meta.name.as_str(), kept.meta.description.as_str()),
        ("Faces", "Agent's")
    );
    // The agent's undo never takes back the user's step...
    let (failed, refused) = through(
        &mut h,
        "helios_workspace_undo",
        args(&dir, &config, &json!({})),
    );
    assert!(failed, "{refused}");
    assert_eq!(h.state().workspace_state().unwrap().meta.name, "Faces");
    // ...and takes back its own; so does the window's Edit > Undo.
    undo(&mut h);
    let (failed, undone) = through(
        &mut h,
        "helios_workspace_undo",
        args(&dir, &config, &json!({})),
    );
    assert!(!failed, "{undone}");
    assert_eq!(undone["via"], "desktop");
    assert_eq!(description(&h), start);
    assert_eq!(files.load().unwrap().meta.description, start);
    assert_eq!(steps(&h), before);
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn without_a_desktop_holding_it_the_tools_write_the_files() {
    let root = temp("files");
    let (h, config, dir, files) = window(&root);
    let holder = files.holder().unwrap();
    // The window closes: it lets the workspace go.
    drop(h);
    assert_eq!(files.holder(), None);
    let (failed, done) = call(
        "helios_workspace_edit",
        &args(
            &dir,
            &config,
            &json!({ "changes": { "description": "Closed" } }),
        ),
    );
    assert!(!failed, "{done}");
    assert_eq!(done["via"], "files");
    assert_eq!(files.load().unwrap().meta.description, "Closed");
    // A desktop killed without a word left its holder behind: nothing
    // listens there any more, and the files are written.
    files
        .hold(&Holder {
            pid: holder.pid,
            address: root.join("gone.sock").display().to_string(),
            token: None,
        })
        .unwrap();
    let (failed, done) = call(
        "helios_workspace_edit",
        &args(
            &dir,
            &config,
            &json!({ "changes": { "description": "Killed" } }),
        ),
    );
    assert!(!failed, "{done}");
    assert_eq!(done["via"], "files");
    assert_eq!(files.load().unwrap().meta.description, "Killed");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_desktop_save_never_writes_over_a_change_made_to_the_files_outside_it() {
    let root = temp("concurrent");
    let (mut h, config, dir, files) = window(&root);
    let before = steps(&h);
    let start = description(&h);
    // Another writer changes the files under the window (the tools on
    // the files, as a desktop that has not said it holds the workspace
    // would leave them)...
    tools::edit(
        &config,
        &json!({ "workspace": dir, "changes": { "description": "Outside" } }),
        &helios_node::mcp::workspace_catalogue(),
    )
    .unwrap();
    // ...and the window saves a step of its own before it saw them.
    h.state_mut()
        .edit_workspace(&json!({ "name": "Mine" }), &Origin::User)
        .unwrap();
    h.run();
    let kept = files.load().unwrap();
    assert_eq!(
        (kept.meta.name.as_str(), kept.meta.description.as_str()),
        ("Mine", "Outside")
    );
    assert_eq!(description(&h), "Outside");
    // The change from outside is a step of its own.
    assert_eq!(steps(&h), before + 2);
    undo(&mut h);
    let state = h.state().workspace_state().unwrap();
    assert_eq!(
        (state.meta.name.as_str(), state.meta.description.as_str()),
        ("work", "Outside")
    );
    undo(&mut h);
    assert_eq!(description(&h), start);
    assert_eq!(files.load().unwrap().meta.description, start);
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn files_changed_outside_are_seen_and_reloaded_as_one_step_part_by_part() {
    let root = temp("reload");
    let (mut h, config, dir, files) = window(&root);
    let before = steps(&h);
    // The watcher sees the files change and asks for a frame.
    tools::edit(
        &config,
        &json!({ "workspace": dir, "changes": { "description": "Watched" } }),
        &helios_node::mcp::workspace_catalogue(),
    )
    .unwrap();
    assert!(
        h.state().wait_workspace_files(DEADLINE),
        "the change was not seen"
    );
    h.run();
    assert_eq!(description(&h), "Watched");
    assert_eq!(steps(&h), before + 1);
    // The window moved a panel it has not saved yet; the files got a
    // description and a layout of their own: both taken, the layout
    // being one both changed, said so.
    let mine = {
        let mut l = h.state().layout();
        assert!(l.remove(&panel("helios.masks")));
        l
    };
    h.state_mut().set_layout(mine.clone());
    // Applied in a frame with a button held (a splitter being dragged):
    // shown, not saved yet.
    let press = |pressed| egui::Event::PointerButton {
        pos: egui::pos2(640.0, 400.0),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    h.event(egui::Event::PointerMoved(egui::pos2(640.0, 400.0)));
    h.event(press(true));
    h.step();
    assert!(
        !h.state()
            .layout()
            .placed()
            .contains(&&panel("helios.masks"))
    );
    let mut theirs = files.load().unwrap().bundle.layout;
    assert!(theirs.remove(&panel("helios.params")));
    tools::edit(
        &config,
        &json!({ "workspace": dir, "changes": {
            "description": "Both",
            "layout": serde_json::from_str::<Value>(&theirs.to_json()).unwrap(),
        } }),
        &helios_node::mcp::workspace_catalogue(),
    )
    .unwrap();
    let (taken, both) = h.state_mut().check_workspace_files().unwrap();
    h.event(press(false));
    assert_eq!(
        (taken, both),
        (
            vec!["description".to_string(), "layout".into()],
            vec!["layout".to_string()]
        )
    );
    h.run();
    assert_eq!(description(&h), "Both");
    assert!(
        h.state()
            .layout()
            .placed()
            .contains(&&panel("helios.masks"))
    );
    assert!(
        !h.state()
            .layout()
            .placed()
            .contains(&&panel("helios.params"))
    );
    // One undo gives the window's own back.
    undo(&mut h);
    assert_eq!(description(&h), "Watched");
    assert!(
        !h.state()
            .layout()
            .placed()
            .contains(&&panel("helios.masks"))
    );
    // Files that hold what the window wrote are nothing to reload.
    assert_eq!(h.state_mut().check_workspace_files(), None);
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}
