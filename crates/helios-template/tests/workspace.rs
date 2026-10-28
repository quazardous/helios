//! A workspace made from a template, then edited: saved back as a
//! template (what goes in, what is stripped, what is asked on create),
//! reset to its template as one step, parts of another imported as a
//! copy with their conflicts, and the agents' tools with their dry runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui_pop_model::ids::PanelId;
use egui_pop_model::panels::Area;
use helios_core::ModuleRef;
use helios_template::workspace::{self, Extract, Files, Part, Side, State, Translation, tools};
use helios_template::{Bundle, Catalogue, shipped};
use serde_json::{Value as Json, json};

fn catalogue() -> Catalogue {
    use helios_module_masks as m;
    let mut c = Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(helios_module_sigmoid::Sigmoid))
        .module(ModuleRef::new(m::MaskCombine))
        .module(ModuleRef::new(m::MaskFeather))
        .module(ModuleRef::new(m::MaskBlend))
        .module(ModuleRef::new(m::MaskObject));
    for f in shipped::composites() {
        c.add_composite(f);
    }
    c
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-workspace-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn builtin(id: &str) -> Bundle {
    shipped::templates()
        .into_iter()
        .find(|b| b.template.id == id)
        .unwrap()
}

/// A workspace made from a built-in template under `root`: its
/// directory and its files.
fn made(root: &Path, template: &str, name: &str) -> (PathBuf, Files) {
    let config = root.join("config");
    let (dir, _) = workspace::create(
        &builtin(template),
        &BTreeMap::new(),
        &catalogue(),
        &root.join(name),
        &config,
    )
    .unwrap();
    let files = Files::new(&config, &dir);
    (dir, files)
}

/// Every file of a directory, with its text.
fn texts(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(texts(&p));
        } else {
            out.push((p.clone(), std::fs::read_to_string(&p).unwrap_or_default()));
        }
    }
    out
}

fn panel(id: &str) -> PanelId {
    PanelId::new(id).unwrap()
}

#[test]
fn a_workspace_reads_as_its_template_until_edited_and_keeps_its_edits() {
    let root = temp("state");
    let (_, files) = made(&root, "helios.portrait-retouch", "portrait");
    let state = files.load().unwrap();
    assert_eq!(state.meta.name, "portrait");
    assert_eq!(
        state.bundle.template,
        builtin("helios.portrait-retouch").template
    );
    // Edited: a name, a tool, a key; written, read back the same.
    let edited = state
        .edited(&json!({
            "name": "Faces",
            "description": "Headshots for the club",
            "icon": "portrait",
            "tools": {
                "toolbox": ["mask.add.ellipse", "mask.add.brush"],
                "keymap": [{ "keys": "B", "command": "mask.add.brush" }]
            }
        }))
        .unwrap();
    assert!(edited.problems(&catalogue()).is_empty());
    files.save(&edited, true).unwrap();
    let read = files.load().unwrap();
    assert_eq!(read, edited);
    assert_eq!(read.meta.icon.as_deref(), Some("portrait"));
    // As one value and back (the desktop's undoable value).
    assert_eq!(State::from_json(&read.to_json()).unwrap(), read);
    // What a workspace does not know is refused.
    assert!(state.edited(&json!({ "colour": 1 })).is_err());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn graph_positions_are_a_view_kept_with_the_workspace_and_follow_its_nodes() {
    let root = temp("positions");
    let (_, files) = made(&root, "helios.portrait-retouch", "portrait");
    let state = files.load().unwrap();
    let node = state.bundle.template.network.nodes[0].name.clone();
    let placed = state
        .edited(&json!({ "positions": { node.clone(): [120.0, -40.5] } }))
        .unwrap();
    assert!(placed.problems(&catalogue()).is_empty());
    files.save(&placed, true).unwrap();
    let read = files.load().unwrap();
    assert_eq!(read.bundle.template.positions[&node], [120.0, -40.5]);
    // A node taken out of the network takes its position with it.
    let mut network = read.bundle.template.network.clone();
    network.nodes.retain(|n| n.name != node);
    network
        .connections
        .retain(|c| c.from != node && c.to != node);
    let without = read.edited(&json!({ "network": network })).unwrap();
    assert!(!without.bundle.template.positions.contains_key(&node));
    // An edit drops a position for no node; one written by hand in the
    // files is a problem.
    let pruned = state
        .edited(&json!({ "positions": { "nowhere": [0.0, 0.0] } }))
        .unwrap();
    assert!(pruned.bundle.template.positions.is_empty());
    let mut stray = state.clone();
    stray
        .bundle
        .template
        .positions
        .insert("nowhere".into(), [0.0, 0.0]);
    assert!(
        stray
            .problems(&catalogue())
            .iter()
            .any(|p| p.contains("nowhere")),
        "{:?}",
        stray.problems(&catalogue())
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn saved_as_a_template_then_created_again_gives_an_equivalent_workspace() {
    let root = temp("again");
    let config = root.join("config");
    let (_, files) = made(&root, "helios.portrait-retouch", "portrait");
    let state = files
        .load()
        .unwrap()
        .edited(&json!({
            "tools": {
                "toolbox": ["mask.add.ellipse", "mask.add.path"],
                "default": "mask.add.path",
                "keymap": [{ "keys": "Shift+P", "command": "mask.add.path" }]
            },
            "library": { "auto_advance": true }
        }))
        .unwrap();
    files.save(&state, true).unwrap();
    let x = Extract {
        id: "user.faces".into(),
        version: 1,
        name: "Faces".into(),
        description: "Brighten faces through a drawn mask".into(),
        icon: Some("portrait".into()),
        translations: BTreeMap::from([(
            "fr".to_string(),
            Translation {
                name: "Visages".into(),
                description: "Éclaircir les visages à travers un masque dessiné".into(),
            },
        )]),
        ..Extract::default()
    };
    let review = workspace::review(&files.load().unwrap(), &x, &catalogue());
    assert!(review.problems.is_empty(), "{:?}", review.problems);
    let dir = workspace::save_template(&review, &config, false).unwrap();
    assert_eq!(dir, workspace::template_dir(&config, "user.faces"));
    // Versioned: the same version again is refused, a newer one taken.
    assert!(workspace::save_template(&review, &config, false).is_err());
    // Validated, with its text in English and French, and its presets'
    // text carried under the new id.
    let saved = Bundle::load(&dir).unwrap();
    assert!(saved.validate(&catalogue()).is_ok());
    let fr = saved.texts.iter().find(|t| t.locale == "fr").unwrap();
    assert!(
        fr.source.contains("user-faces-name = Visages"),
        "{}",
        fr.source
    );
    assert!(
        fr.source.contains("user-faces-preset-soft = Doux"),
        "{}",
        fr.source
    );
    assert_eq!(saved.template.meta.icon.as_deref(), Some("portrait"));
    // Listed among the user's templates, and found by its id.
    assert_eq!(workspace::user_templates(&config).len(), 1);
    let again = workspace::source(&config, "user.faces").unwrap();
    let (_, built) = workspace::create(
        &again,
        &BTreeMap::new(),
        &catalogue(),
        &root.join("faces"),
        &config,
    )
    .unwrap();
    assert!(built.nodes.contains_key("retouch.blend"));
    let second = Files::new(&config, &root.join("faces").canonicalize().unwrap())
        .load()
        .unwrap();
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
    assert_eq!(second.bundle.layout, first.bundle.layout);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_template_never_keeps_absolute_paths_data_or_history() {
    let root = temp("strip");
    let config = root.join("config");
    let (_, files) = made(&root, "helios.import-cull-dedupe", "shoot");
    let mut network = files.load().unwrap().bundle.template.network;
    network.nodes.push(
        serde_json::from_value(json!({
            "name": "object",
            "module": "mask_object",
            "params": {
                "prompt": { "text": "/home/someone/pictures/reference.jpg" },
                "mask": { "text": "iVBORw0KGgo".repeat(40) }
            }
        }))
        .unwrap(),
    );
    network.connections.push(
        serde_json::from_value(
            json!({ "from": "source", "port": "image", "to": "object", "input": "input" }),
        )
        .unwrap(),
    );
    let state = files
        .load()
        .unwrap()
        .edited(&json!({ "network": network }))
        .unwrap();
    assert!(
        state.problems(&catalogue()).is_empty(),
        "{:?}",
        state.problems(&catalogue())
    );
    files.save(&state, true).unwrap();
    let x = Extract {
        id: "user.intake".into(),
        version: 1,
        name: "Intake".into(),
        description: "Import and cull".into(),
        ..Extract::default()
    };
    let review = workspace::review(&state, &x, &catalogue());
    assert!(review.problems.is_empty(), "{:?}", review.problems);
    let what: Vec<&str> = review.stripped.iter().map(|s| s.what.as_str()).collect();
    assert_eq!(what, ["images", "history", "data", "path"]);
    // The path and the import source are asked on create.
    let asked: Vec<&str> = review
        .bundle
        .template
        .parameters
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(asked, ["object_prompt", "import_source"]);
    let dir = workspace::save_template(&review, &config, false).unwrap();
    for (path, text) in texts(&dir) {
        assert!(!text.contains("/home/"), "{}: {text}", path.display());
        assert!(!text.contains("iVBORw0KGgo"), "{}", path.display());
    }
    // Created again, the answers go where each parameter points.
    let saved = Bundle::load(&dir).unwrap();
    let answers = BTreeMap::from([
        (
            "object_prompt".to_string(),
            json!("/media/new/reference.jpg"),
        ),
        ("import_source".to_string(), json!("folder")),
    ]);
    let (made_dir, _) =
        workspace::create(&saved, &answers, &catalogue(), &root.join("next"), &config).unwrap();
    let next = Files::new(&config, &made_dir).load().unwrap();
    let t = serde_json::to_value(&next.bundle.template).unwrap();
    assert_eq!(t.pointer("/library/import/source"), Some(&json!("folder")));
    assert!(t.to_string().contains("/media/new/reference.jpg"));
    // A wrong answer is refused before anything is written.
    let wrong = BTreeMap::from([("import_source".to_string(), json!("cloud"))]);
    assert!(workspace::create(&saved, &wrong, &catalogue(), &root.join("nope"), &config).is_err());
    assert!(!root.join("nope").exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_review_says_what_it_cannot_write() {
    let root = temp("refused");
    let (_, files) = made(&root, "helios.portrait-retouch", "portrait");
    let state = files.load().unwrap();
    let x = Extract {
        id: "helios.portrait-retouch".into(),
        version: 1,
        name: String::new(),
        description: "x".into(),
        without: vec![Part::Network],
        ask: Some(vec!["nothing".into()]),
        ..Extract::default()
    };
    let review = workspace::review(&state, &x, &catalogue());
    let problems = review.problems.join("\n");
    for expected in [
        "always go in",
        "built-in template",
        "`nothing` is not a value",
        "name: empty",
    ] {
        assert!(problems.contains(expected), "{expected}: {problems}");
    }
    assert!(workspace::save_template(&review, &root.join("config"), false).is_err());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_reset_is_one_step_back_to_the_template_and_is_undone() {
    let root = temp("reset");
    let config = root.join("config");
    let (dir, files) = made(&root, "helios.portrait-retouch", "portrait");
    let args = |extra: Json| {
        let mut a = json!({ "workspace": dir });
        for (k, v) in extra.as_object().unwrap() {
            a[k] = v.clone();
        }
        a
    };
    tools::edit(
        &config,
        &args(json!({ "changes": { "name": "Faces", "tools": { "toolbox": ["mask.add.path"] } } })),
        &catalogue(),
    )
    .unwrap();
    let edited = files.load().unwrap();
    assert_eq!(edited.bundle.template.tools.toolbox, ["mask.add.path"]);
    // A dry run says what it would do and writes nothing.
    let dry = tools::reset(&config, &args(json!({ "dry_run": true })), &catalogue()).unwrap();
    assert_eq!(dry["changed"], json!(true));
    assert_eq!(files.load().unwrap(), edited);
    tools::reset(&config, &args(json!({})), &catalogue()).unwrap();
    let reset = files.load().unwrap();
    assert_eq!(
        reset.bundle.template,
        builtin("helios.portrait-retouch").template
    );
    // The workspace's own name stays.
    assert_eq!(reset.meta.name, "Faces");
    // One step: undone, the edit is back.
    assert_eq!(
        tools::undo(&config, &args(json!({}))).unwrap()["undone"],
        json!("reset to template")
    );
    assert_eq!(files.load().unwrap(), edited);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn parts_are_imported_as_a_copy_and_conflicts_are_picked() {
    let root = temp("import");
    let (_, files) = made(&root, "helios.portrait-retouch", "portrait");
    let mut layout = files.load().unwrap().bundle.layout;
    layout.place(&panel("helios.vault"), Area::Bottom);
    let target = files
        .load()
        .unwrap()
        .edited(&json!({
            "layout": serde_json::from_str::<Json>(&layout.to_json()).unwrap(),
            "tools": {
                "toolbox": ["mask.add.path"],
                "keymap": [{ "keys": "Mod+I", "command": "mask.add.path" }]
            }
        }))
        .unwrap();
    let source = builtin("helios.import-cull-dedupe");
    let none = BTreeMap::new();
    let dry = workspace::import(&target, &source, &Part::ALL, &none, &catalogue()).unwrap();
    let mut unresolved: Vec<&str> = dry.unresolved().iter().map(|c| c.id.as_str()).collect();
    unresolved.sort_unstable();
    assert_eq!(unresolved, ["key:Mod+I", "panel:helios.vault", "view"]);
    // The network comes as one opened composite, its nodes renamed.
    assert_eq!(dry.renamed["exposure"], "import_cull_dedupe.exposure");
    let picks = BTreeMap::from([
        ("view".to_string(), Side::Ours),
        ("key:Mod+I".to_string(), Side::Theirs),
        ("panel:helios.vault".to_string(), Side::Theirs),
    ]);
    let merged = workspace::import(&target, &source, &Part::ALL, &picks, &catalogue()).unwrap();
    assert!(merged.unresolved().is_empty());
    let s = &merged.state;
    assert!(
        s.problems(&catalogue()).is_empty(),
        "{:?}",
        s.problems(&catalogue())
    );
    let t = &s.bundle.template;
    let names: Vec<&str> = t.network.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["exposure", "retouch", "import_cull_dedupe"]);
    assert_eq!(t.network.view.node, "retouch");
    assert!(
        t.tools
            .keymap
            .iter()
            .any(|b| b.keys == "Mod+I" && b.command == "library.importSource")
    );
    assert!(t.tools.toolbox.contains(&"vault.flag.pick".to_string()));
    assert!(t.library.is_some());
    assert_eq!(
        s.bundle.layout.area_of(&panel("helios.vault")),
        Some(Area::Center)
    );
    assert_eq!(
        s.bundle.layout.area_of(&panel("helios.stacks")),
        Some(Area::EndSide)
    );
    let built = s.bundle.build(&catalogue()).unwrap();
    assert!(built.nodes.contains_key("import_cull_dedupe.exposure"));
    // Nothing says where the parts came from: imported again, they are
    // new names and a new composite version.
    let twice = workspace::import(s, &source, &[Part::Network], &picks, &catalogue()).unwrap();
    let last = twice.state.bundle.template.network.nodes.last().unwrap();
    assert_eq!(last.name, "import_cull_dedupe_2");
    assert_eq!(last.composite.as_ref().unwrap().version, 2);
    assert!(
        !serde_json::to_string(&twice.state.to_json())
            .unwrap()
            .contains("helios.import-cull-dedupe\"")
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn agents_dry_run_then_apply_through_the_tools() {
    let root = temp("tools");
    let config = root.join("config");
    let (dir, files) = made(&root, "helios.portrait-retouch", "portrait");
    let before = files.load().unwrap();
    // The parts of a template, by id.
    let parts = tools::parts_of(
        &config,
        &json!({ "workspace": dir, "source": "helios.import-cull-dedupe" }),
    )
    .unwrap();
    let names: Vec<&str> = parts["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["part"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["network", "layout", "keys", "toolbox", "library"]);
    // An import's dry run: its diff and conflicts, nothing written.
    let dry = tools::import_parts(
        &config,
        &json!({ "workspace": dir, "source": "helios.import-cull-dedupe", "dry_run": true }),
        &catalogue(),
    )
    .unwrap();
    assert_eq!(dry["unresolved"], json!(["view"]));
    assert_eq!(files.load().unwrap(), before);
    // Applied with a conflict left: refused.
    let refused = tools::import_parts(
        &config,
        &json!({ "workspace": dir, "source": "helios.import-cull-dedupe", "parts": ["network"] }),
        &catalogue(),
    );
    assert!(refused.unwrap_err().contains("view"));
    tools::import_parts(
        &config,
        &json!({
            "workspace": dir,
            "source": "helios.import-cull-dedupe",
            "parts": ["network", "library"],
            "picks": { "view": "theirs" }
        }),
        &catalogue(),
    )
    .unwrap();
    let after = files.load().unwrap();
    assert_eq!(
        after.bundle.template.network.view.node,
        "import_cull_dedupe"
    );
    assert!(after.bundle.template.library.is_some());
    // Save as template: the dry run reviews, writes nothing.
    let review = tools::save_as_template(
        &config,
        &json!({
            "workspace": dir,
            "id": "user.mixed",
            "name": "Mixed",
            "description": "Retouch and import",
            "dry_run": true
        }),
        &catalogue(),
    )
    .unwrap();
    assert_eq!(review["ok"], json!(true), "{review}");
    assert_eq!(review["parameters"][0]["id"], json!("import_source"));
    assert!(!workspace::template_dir(&config, "user.mixed").exists());
    let written = tools::save_as_template(
        &config,
        &json!({
            "workspace": dir,
            "id": "user.mixed",
            "name": "Mixed",
            "description": "Retouch and import",
            "ask": []
        }),
        &catalogue(),
    )
    .unwrap();
    assert!(
        workspace::template_dir(&config, "user.mixed").is_dir(),
        "{written}"
    );
    assert!(written["parameters"].as_array().unwrap().is_empty());
    // An edit's dry run checks it: a module this Helios lacks.
    let mut network = after.bundle.template.network.clone();
    network.nodes[0].module = Some("mystery".into());
    let dry = tools::edit(
        &config,
        &json!({ "workspace": dir, "changes": { "network": network }, "dry_run": true }),
        &catalogue(),
    )
    .unwrap();
    assert!(dry["problems"].to_string().contains("mystery"), "{dry}");
    assert!(
        tools::edit(
            &config,
            &json!({ "workspace": dir, "changes": { "network": network } }),
            &catalogue(),
        )
        .is_err()
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn what_a_workspace_writes_reads_back_the_same_from_its_files() {
    // A desktop tells a change made outside it by comparing the files
    // with what it wrote: a round trip that moved anything would read
    // as a change.
    let root = temp("roundtrip");
    for bundle in shipped::templates() {
        let id = bundle.template.id.clone();
        let (_, files) = made(&root, &id, &id.replace('.', "-"));
        let state = files.load_on_disk().unwrap();
        let edited = state
            .edited(&json!({ "name": "Edited", "description": "Kept" }))
            .unwrap();
        files.save(&edited, false).unwrap();
        assert_eq!(
            files.load_on_disk().unwrap().to_json(),
            edited.to_json(),
            "{id}"
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn files_changed_under_a_desktop_merge_part_by_part() {
    let root = temp("merge");
    let (_, files) = made(&root, "helios.portrait-retouch", "portrait");
    let base = files.load_on_disk().unwrap();
    let edit = |s: &State, changes: Json| s.edited(&changes).unwrap();
    // The desktop moved a panel; the files got a new description and
    // toolbox: both kept.
    let mut ours = base.clone();
    assert!(ours.bundle.layout.remove(&panel("helios.masks")));
    let theirs = edit(
        &base,
        json!({ "description": "Agent", "tools": { "toolbox": ["mask.add.path"] } }),
    );
    let m = workspace::merge(&base, &ours, &theirs).unwrap();
    assert_eq!(m.taken, ["description", "tools"]);
    assert!(m.both.is_empty());
    assert_eq!(m.state.meta.description, "Agent");
    assert_eq!(m.state.bundle.template.tools.toolbox, ["mask.add.path"]);
    assert_eq!(m.state.bundle.layout, ours.bundle.layout);
    // Both renamed it: the files' name wins, and is said.
    let ours = edit(&base, json!({ "name": "Mine" }));
    let theirs = edit(&base, json!({ "name": "Theirs" }));
    let m = workspace::merge(&base, &ours, &theirs).unwrap();
    assert_eq!((m.taken.clone(), m.both), (vec!["name"], vec!["name"]));
    assert_eq!(m.state.meta.name, "Theirs");
    // The same change on both sides, or none on theirs: nothing taken.
    let m = workspace::merge(&base, &ours, &ours).unwrap();
    assert!(m.taken.is_empty());
    let m = workspace::merge(&base, &ours, &base).unwrap();
    assert!(m.taken.is_empty());
    assert_eq!(m.state, ours);
    // An icon taken away in the files is taken away.
    let with_icon = edit(&base, json!({ "icon": "portrait" }));
    let without = edit(&with_icon, json!({ "icon": null }));
    let m = workspace::merge(&with_icon, &with_icon, &without).unwrap();
    assert_eq!(m.state.meta.icon, None);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_agent_s_undo_on_the_files_is_refused_once_they_changed_since() {
    let root = temp("undo-changed");
    let config = root.join("config");
    let (dir, files) = made(&root, "helios.portrait-retouch", "portrait");
    let args = json!({ "workspace": dir, "changes": { "name": "Agent's" } });
    tools::edit(&config, &args, &catalogue()).unwrap();
    // A desktop renamed it after the agent's step.
    let now = files.load().unwrap();
    files
        .save(&now.edited(&json!({ "name": "User's" })).unwrap(), false)
        .unwrap();
    let refused = tools::undo(&config, &json!({ "workspace": dir })).unwrap_err();
    assert!(refused.contains("changed since"), "{refused}");
    assert_eq!(files.load().unwrap().meta.name, "User's");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_desktop_says_it_holds_a_workspace_and_lets_it_go() {
    let root = temp("holder");
    let (_, files) = made(&root, "helios.portrait-retouch", "portrait");
    assert_eq!(files.holder(), None);
    let holder = workspace::Holder {
        pid: 7,
        address: "/run/user/1/helios-desktop-7.sock".into(),
        token: None,
    };
    files.hold(&holder).unwrap();
    assert_eq!(files.holder(), Some(holder));
    // Another process's release leaves it held.
    files.release(8);
    assert!(files.holder().is_some());
    files.release(7);
    assert_eq!(files.holder(), None);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_key_taken_away_is_kept_as_a_removal_and_imported_as_one() {
    let root = temp("removal");
    let (_, files) = made(&root, "helios.portrait-retouch", "portrait");
    let state = files.load().unwrap();
    let keymap = json!([
        { "keys": "", "command": "-mask.add.circle" },
        { "keys": "", "command": "-mask.add.path" },
        { "keys": "B", "command": "mask.add.circle" },
    ]);
    let edited = state
        .edited(&json!({ "tools": { "keymap": keymap } }))
        .unwrap();
    assert_eq!(edited.problems(&catalogue()), Vec::<String>::new());
    // Keys missing elsewhere are still refused.
    let bad = state
        .edited(&json!({ "tools": { "keymap": [
            { "keys": "", "command": "mask.add.circle" },
        ] } }))
        .unwrap();
    assert_eq!(bad.problems(&catalogue()), ["tools.keymap[0]: no keys"],);
    // Imported into a workspace holding one of them: the other is added,
    // no conflict between two removals.
    let mut ours = state.clone();
    ours.bundle.template.tools.keymap = vec![helios_template::format::Binding {
        keys: String::new(),
        command: "-mask.add.circle".into(),
        when: String::new(),
    }];
    let merged = workspace::import(
        &ours,
        &edited.bundle,
        &[Part::Keys],
        &BTreeMap::new(),
        &catalogue(),
    )
    .unwrap();
    assert!(merged.unresolved().is_empty());
    assert_eq!(merged.state.bundle.template.tools.keymap.len(), 3);
    std::fs::remove_dir_all(&root).unwrap();
}

/// What leaves for another application (the system clipboard) loses
/// what "Save as template" strips: paths become file names, secrets and
/// embedded data go, each said.
#[test]
fn strip_for_outside_keeps_file_names_and_drops_secrets_and_data() {
    let mut v = json!({
        "lut": "/srv/luts/film/lut.cube",
        "windows": "C:\\Looks\\warm.cube",
        "api_token": "abc",
        "blob": "x".repeat(workspace::DATA_LENGTH + 1),
        "list": ["~/notes.txt", "plain"],
        "amount": 0.5,
    });
    let what = workspace::strip_for_outside(&mut v);
    assert_eq!(v["lut"], "lut.cube");
    assert_eq!(v["windows"], "warm.cube");
    assert_eq!(v["api_token"], "");
    assert_eq!(v["blob"], "");
    assert_eq!(v["list"], json!(["notes.txt", "plain"]));
    assert_eq!(v["amount"], 0.5);
    let mut sorted = what.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        [
            "a local path",
            "a local path",
            "a local path",
            "a secret",
            "embedded data"
        ]
    );
    // A bare text is checked too: it is what a text fallback holds.
    let mut text = json!("/home/someone/lut.cube");
    assert_eq!(workspace::strip_for_outside(&mut text), ["a local path"]);
    assert_eq!(text, "lut.cube");
}

#[test]
fn a_node_off_the_render_path_may_wait_for_its_wires() {
    let root = temp("unwired");
    let (_, files) = made(&root, "helios.portrait-retouch", "portrait");
    let state = files.load().unwrap();
    let c = catalogue();
    let wire = |from: &str, port: &str, to: &str| json!({ "from": from, "port": port, "to": to, "input": "input" });
    let with = |connections: Vec<Json>| {
        let mut net = serde_json::to_value(&state.bundle.template.network).unwrap();
        net["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "name": "exposure_2", "module": "exposure" }));
        let all = net["connections"].as_array_mut().unwrap();
        for c in connections {
            all.retain(|k| !(k["to"] == c["to"] && k["input"] == c["input"]));
            all.push(c);
        }
        state.edited(&json!({ "network": net })).unwrap()
    };
    // Added unconnected (the graph view's add, before its wires): fine,
    // the presets too.
    assert_eq!(with(vec![]).problems(&c), Vec::<String>::new());
    // Wired in front of what the view shows while its own input waits:
    // refused, the view would read a node missing its input.
    let problems = with(vec![wire("exposure_2", "output", "exposure")]).problems(&c);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("input `input` of `exposure_2` is not connected")),
        "{problems:?}"
    );
    // Its input wired too: fine.
    let wired = with(vec![
        wire("source", "image", "exposure_2"),
        wire("exposure_2", "output", "exposure"),
    ]);
    assert_eq!(wired.problems(&c), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(&root);
}
