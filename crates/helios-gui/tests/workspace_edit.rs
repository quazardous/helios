//! A workspace edited on the desktop after it was made from a template:
//! its name, description and icon, its library settings, its keys and
//! tools, each an undo step written to its files at once; saved as a
//! template after a review; reset to its template as one undo step; parts
//! of a template added through their review, a side picked for each
//! conflict; and what agents run with their dry runs.

use std::path::{Path, PathBuf};

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::model::ids::PanelId;
use egui_pop::model::keymap::Layer;
use egui_pop::model::panels::Area;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use helios_template::workspace::{self, Files};
use serde_json::{Value, json};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-wsedit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn desktop() -> helios_template::Catalogue {
    let shell = Shell::new(helios_panels(), None);
    templates::catalogue(&shell.template_panels(), shell.specs())
}

/// A workspace made from a built-in template under `root`, and a window
/// on it: its layout loaded, its state opened for editing.
fn window(root: &Path, template: &str) -> (Harness<'static, Shell>, PathBuf, Files) {
    let config = root.join("config");
    let created = templates::create_workspace(
        &templates::find(template).unwrap(),
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
    (h, config, files)
}

fn run(h: &mut Harness<'static, Shell>, id: &str, args: Value) {
    h.state_mut().run(id, args, Origin::User).unwrap();
    for _ in 0..3 {
        h.run();
    }
}

fn dialog(h: &mut Harness<'static, Shell>) -> Value {
    h.state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap()
}

fn answer(h: &mut Harness<'static, Shell>, values: Value) {
    run(h, "dialog.answer", json!({ "values": values }));
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

fn panel(id: &str) -> PanelId {
    PanelId::new(id).unwrap()
}

#[test]
fn name_description_and_icon_are_edited_in_a_form_kept_and_undone() {
    let root = temp("meta");
    let (mut h, _, files) = window(&root, "helios.portrait-retouch");
    run(&mut h, "workspace.edit", json!(null));
    let form = dialog(&mut h);
    assert_eq!(form["command"], json!("workspace.edit"), "{form}");
    assert_eq!(form["fields"][0]["key"], json!("name"));
    assert_eq!(form["fields"][0]["value"]["text"], json!("work"));
    let before = steps(&h);
    answer(
        &mut h,
        json!({ "name": "Faces", "description": "Club headshots", "icon": "portrait" }),
    );
    assert_eq!(steps(&h), before + 1, "one undo step");
    let state = h.state().workspace_state().unwrap();
    assert_eq!(state.meta.name, "Faces");
    assert_eq!(h.state().window_title(), "Faces — Helios");
    // Written at once: what a kill would leave.
    let kept = files.load().unwrap();
    assert_eq!(kept.meta.description, "Club headshots");
    assert_eq!(kept.meta.icon.as_deref(), Some("portrait"));
    undo(&mut h);
    assert_eq!(h.state().workspace_state().unwrap().meta.name, "work");
    assert_eq!(files.load().unwrap().meta.name, "work");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_library_settings_are_edited_after_creation() {
    let root = temp("library");
    let (mut h, _, files) = window(&root, "helios.portrait-retouch");
    assert!(h.state().import_source().is_none());
    run(&mut h, "workspace.library", json!(null));
    let form = dialog(&mut h);
    let keys: Vec<&str> = form["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        [
            "source",
            "placement",
            "rename",
            "duplicates",
            "suggest_stacks",
            "auto_advance"
        ]
    );
    answer(
        &mut h,
        json!({
            "source": "folder", "placement": "copy", "rename": "{date}-{seq}",
            "duplicates": "ask", "suggest_stacks": true, "auto_advance": true
        }),
    );
    let source = h.state().import_source().cloned().unwrap();
    assert_eq!(
        (source.source.as_str(), source.placement.as_str()),
        ("folder", "copy")
    );
    assert!(h.state().suggests_stacks());
    assert!(h.state().library().auto_advance);
    let kept = files.load().unwrap().bundle.template.library.unwrap();
    assert!(kept.auto_advance);
    // A renaming without a copy is refused, nothing changed.
    let refused = h.state_mut().edit_workspace(
        &json!({ "library": { "import": { "source": "card", "rename": "x" } } }),
        &Origin::User,
    );
    assert!(refused.unwrap_err().contains("only a copy is renamed"));
    assert!(h.state().library().auto_advance);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn keys_and_tools_are_the_workspace_s_own_and_a_reset_is_one_undo_step() {
    let root = temp("reset");
    let (mut h, _, files) = window(&root, "helios.portrait-retouch");
    // Its own keys and tools, over the template's.
    h.state_mut()
        .edit_workspace(
            &json!({ "tools": {
                "toolbox": ["mask.add.circle"],
                "keymap": [{ "keys": "B", "command": "mask.add.circle" }]
            } }),
            &Origin::User,
        )
        .unwrap();
    h.run();
    let bound = |h: &Harness<'static, Shell>, command: &str| {
        h.state()
            .app()
            .handle()
            .export_keymap(Layer::Workspace)
            .bindings
            .iter()
            .any(|b| b.command.as_deref() == Some(command))
    };
    assert!(bound(&h, "mask.add.circle"));
    assert!(!bound(&h, "mask.add.ellipse"));
    // A panel moved, then reset: one step back to the template, its
    // layout too.
    run(&mut h, "helios.masks.moveTo", json!({ "area": "end_side" }));
    let before = steps(&h);
    let dry = h.state_mut().reset_workspace(true, &Origin::User).unwrap();
    assert_eq!(dry["changed"], json!(true));
    assert_eq!(steps(&h), before);
    run(&mut h, "workspace.reset", json!(null));
    assert_eq!(steps(&h), before + 1, "one undo step");
    let state = h.state().workspace_state().unwrap();
    let template = templates::find("helios.portrait-retouch").unwrap();
    assert_eq!(state.bundle.template.tools, template.template.tools);
    assert!(bound(&h, "mask.add.ellipse"));
    assert_eq!(
        h.state().layout().area_of(&panel("helios.masks")),
        Some(Area::StartSide)
    );
    assert_eq!(
        files.load().unwrap().bundle.template.tools,
        template.template.tools
    );
    // Undone: the workspace's own keys, tools and layout are back.
    undo(&mut h);
    assert!(!bound(&h, "mask.add.ellipse"));
    assert_eq!(
        h.state()
            .workspace_state()
            .unwrap()
            .bundle
            .template
            .tools
            .toolbox,
        ["mask.add.circle"]
    );
    assert_eq!(
        h.state().layout().area_of(&panel("helios.masks")),
        Some(Area::EndSide)
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn save_as_template_reviews_then_writes_a_bundle_that_creates_the_same_workspace() {
    let root = temp("save");
    let (mut h, config, files) = window(&root, "helios.import-cull-dedupe");
    h.state_mut()
        .edit_workspace(&json!({ "name": "Wedding" }), &Origin::User)
        .unwrap();
    // An agent's dry run: the review, nothing written.
    let dry = h
        .state_mut()
        .save_workspace_as_template(&json!({
            "id": "user.wedding", "name": "Wedding", "description": "Import and cull a wedding",
            "dry_run": true
        }))
        .unwrap();
    assert_eq!(dry["ok"], json!(true), "{dry}");
    assert_eq!(dry["parameters"][0]["id"], json!("import_source"));
    assert!(!workspace::template_dir(&config, "user.wedding").exists());
    // The review form: what goes in, what is asked on create.
    run(&mut h, "workspace.saveAsTemplate", json!(null));
    let form = dialog(&mut h);
    assert_eq!(form["command"], json!("workspace.saveAsTemplate"), "{form}");
    let keys: Vec<&str> = form["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys[0], "id");
    assert!(keys.contains(&"ask:import_source"), "{keys:?}");
    assert_eq!(form["fields"][0]["value"]["text"], json!("user.wedding"));
    answer(
        &mut h,
        json!({
            "description": "Import and cull a wedding",
            "name_fr": "Mariage",
            "description_fr": "Importer et trier un mariage"
        }),
    );
    let dir = workspace::template_dir(&config, "user.wedding");
    let saved = helios_template::Bundle::load(&dir).unwrap();
    assert!(saved.validate(&desktop()).is_ok());
    assert!(
        saved
            .texts
            .iter()
            .any(|t| t.locale == "fr" && t.source.contains("Mariage"))
    );
    // Listed with the built-in templates, and created again the same.
    assert!(
        templates::all(&config)
            .iter()
            .any(|b| b.template.id == "user.wedding")
    );
    let again =
        templates::create_workspace(&saved, &desktop(), &root.join("again"), &config).unwrap();
    let second = Files::new(&config, &again.workspace).load().unwrap();
    let first = files.load().unwrap();
    assert_eq!(
        second.bundle.template.network,
        first.bundle.template.network
    );
    assert_eq!(second.bundle.template.tools, first.bundle.template.tools);
    assert_eq!(
        second.bundle.template.library,
        first.bundle.template.library
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn parts_are_added_through_their_review_as_one_undo_step() {
    let root = temp("import");
    let (mut h, _, files) = window(&root, "helios.portrait-retouch");
    // An agent's dry run: the changes and the conflicts, nothing done.
    let dry = h
        .state_mut()
        .import_workspace_parts(
            &json!({ "source": "helios.import-cull-dedupe", "dry_run": true }),
            &Origin::User,
        )
        .unwrap();
    assert_eq!(dry["unresolved"], json!(["view"]), "{dry}");
    let before = steps(&h);
    // Picked from the sources, the parts asked, then the review.
    run(&mut h, "workspace.importParts", json!(null));
    let picker = dialog(&mut h);
    assert_eq!(picker["kind"], json!("choose"), "{picker}");
    run(
        &mut h,
        "dialog.answer",
        json!({ "choice": "helios.import-cull-dedupe" }),
    );
    let parts = dialog(&mut h);
    assert_eq!(
        parts["command"],
        json!("workspace.importParts.review"),
        "{parts}"
    );
    answer(&mut h, json!({}));
    let review = dialog(&mut h);
    assert_eq!(
        review["command"],
        json!("workspace.importParts.apply"),
        "{review}"
    );
    assert_eq!(review["fields"][0]["key"], json!("view"));
    assert_eq!(steps(&h), before, "nothing done before it is answered");
    answer(&mut h, json!({ "view": "theirs" }));
    assert_eq!(steps(&h), before + 1, "one undo step");
    let state = h.state().workspace_state().unwrap();
    let t = &state.bundle.template;
    assert_eq!(t.network.view.node, "import_cull_dedupe");
    assert!(t.library.is_some());
    assert!(h.state().import_source().is_some());
    assert_eq!(
        h.state().layout().area_of(&panel("helios.vault")),
        Some(Area::Center)
    );
    assert!(h.state_mut().take_network_changed());
    assert_eq!(files.load().unwrap().bundle.template.network, t.network);
    // The parts of the workspace now, as agents list them.
    let parts = h.state().workspace_parts(None).unwrap();
    assert_eq!(parts["parts"][0]["part"], json!("network"));
    undo(&mut h);
    assert_eq!(
        h.state()
            .workspace_state()
            .unwrap()
            .bundle
            .template
            .network
            .view
            .node,
        "retouch"
    );
    std::fs::remove_dir_all(&root).unwrap();
}
