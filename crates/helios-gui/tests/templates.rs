//! Workspace templates on the desktop: the built-in ones check against
//! the desktop's modules, panels and commands and speak its languages;
//! "New workspace from template" lists them with their scenario, asks
//! where, and the workspace created has the template's network, layout
//! and keys.

use std::path::PathBuf;

use egui::vec2;
use egui_pop::i18n::I18n;
use egui_pop::model::ids::PanelId;
use egui_pop::model::keymap::Layer;
use egui_pop::model::layout::Layout;
use egui_pop::model::panels::Area;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use serde_json::json;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-templates-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn panel(id: &str) -> PanelId {
    PanelId::new(id).unwrap()
}

fn desktop() -> helios_template::Catalogue {
    let shell = Shell::new(helios_panels(), None);
    templates::catalogue(&shell.template_panels(), shell.specs())
}

#[test]
fn the_built_in_templates_check_against_the_desktop() {
    let all = templates::builtin();
    assert!(
        all.iter()
            .any(|b| b.template.id == "helios.portrait-retouch")
    );
    let catalogue = desktop();
    for b in &all {
        let report = b.validate(&catalogue);
        assert!(report.is_ok(), "{}: {:?}", b.template.id, report.problems);
    }
}

#[test]
fn the_built_in_templates_speak_english_and_french() {
    let i = I18n::with_egui_pop("fr");
    i.add_catalogues(helios_i18n::CATALOGUES).unwrap();
    let all = templates::builtin();
    templates::add_texts(&i, &all).unwrap();
    let portrait = templates::find("helios.portrait-retouch").unwrap();
    assert_eq!(i.t(&portrait.name_key(), &[]), "Retouche de portrait");
    assert!(
        i.t(&portrait.scenario_key(), &[])
            .starts_with("Retoucher un portrait")
    );
    i.set_locale("en");
    assert_eq!(i.t(&portrait.name_key(), &[]), portrait.template.meta.name);
    assert_eq!(
        i.t(&portrait.scenario_key(), &[]),
        portrait.template.meta.scenario
    );
}

#[test]
fn new_workspace_lists_the_templates_with_their_scenario_then_asks_where() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.state_mut()
        .run("workspace.new", json!(null), Origin::User)
        .unwrap();
    h.run();
    let picker = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    assert_eq!(picker["kind"], json!("choose"));
    assert_eq!(picker["command"], json!("workspace.new"));
    assert_eq!(picker["value_key"], json!("template"));
    let option = picker["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["value"] == json!("helios.portrait-retouch"))
        .expect("Portrait retouch is offered");
    assert_eq!(option["label"], json!("Portrait retouch"));
    assert!(
        option["hint"]
            .as_str()
            .unwrap()
            .starts_with("Retouch a portrait"),
        "{option}"
    );
    // Picking it asks for the directory, the template chosen.
    h.state_mut()
        .run(
            "dialog.answer",
            json!({ "choice": "helios.portrait-retouch" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    h.run();
    let form = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    assert_eq!(form["kind"], json!("form"), "{form}");
    assert_eq!(form["command"], json!("workspace.new"));
    assert_eq!(form["fields"][0]["key"], json!("template"));
    assert_eq!(form["fields"][1]["key"], json!("path"));
    assert_eq!(
        form["fields"][1]["descriptor"]["path"],
        json!({ "kind": "dir" })
    );
}

#[test]
fn a_workspace_from_portrait_retouch_has_its_network_layout_and_keys() {
    let root = temp("create");
    let (config, dir) = (root.join("config"), root.join("portrait"));
    let portrait = templates::find("helios.portrait-retouch").unwrap();
    let created = templates::create_workspace(&portrait, &desktop(), &dir, &config).unwrap();
    assert!(dir.is_dir());
    // The layout document egui-pop loads for the workspace.
    let text =
        std::fs::read_to_string(templates::layout_file(&config, &created.workspace)).unwrap();
    let layout = Layout::from_json(&text).unwrap();
    assert_eq!(
        layout.area_of(&panel("helios.viewport")),
        Some(Area::Center)
    );
    assert_eq!(
        layout.area_of(&panel("helios.masks")),
        Some(Area::StartSide)
    );
    assert_eq!(layout.area_of(&panel("helios.params")), Some(Area::EndSide));
    assert!(layout.hidden.contains(&panel("helios.graph")));
    // The template kept with the workspace, its composite bundled, builds
    // the expected graph.
    let kept = templates::workspace_template(&config, &created.workspace)
        .unwrap()
        .unwrap();
    assert_eq!(kept.template, portrait.template);
    assert_eq!(kept.composites.len(), 1);
    let built = kept.build(&templates::modules()).unwrap();
    let names: Vec<&str> = built.nodes.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "exposure",
            "retouch.blend",
            "retouch.feather",
            "retouch.mask",
            "source"
        ]
    );
    assert_eq!(built.view.0, built.nodes["retouch.blend"]);
    let pointer = templates::read_json(
        &templates::workspace_dir(&config, &created.workspace).join("workspace.json"),
    );
    assert_eq!(pointer["template"]["id"], json!("helios.portrait-retouch"));
    assert_eq!(pointer["profile"], json!("default"));
    // A window on that workspace shows its layout, and the template's
    // keys go into the workspace's keymap layer.
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&created.workspace)
        .build();
    shell.load_layout().unwrap();
    shell.apply_template_tools(&kept.template.meta.name, &kept.template.tools);
    let mut h = headless::harness(shell, vec2(1000.0, 700.0));
    h.run();
    let shown = h.state().layout();
    assert_eq!(shown.area_of(&panel("helios.masks")), Some(Area::StartSide));
    assert_eq!(shown.area_of(&panel("helios.params")), Some(Area::EndSide));
    let keys = h.state().app().handle().export_keymap(Layer::Workspace);
    assert!(
        keys.bindings
            .iter()
            .any(|b| b.command.as_deref() == Some("mask.add.ellipse")),
        "{:?}",
        keys.bindings
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_template_that_does_not_check_creates_nothing() {
    let root = temp("refused");
    let (config, dir) = (root.join("config"), root.join("broken"));
    let mut broken = templates::find("helios.portrait-retouch").unwrap();
    broken.template.network.nodes[0].module = Some("mystery".into());
    let e = templates::create_workspace(&broken, &desktop(), &dir, &config)
        .err()
        .unwrap();
    assert!(e.contains("unknown module `mystery`"), "{e}");
    assert!(!dir.exists());
    assert!(!config.exists());
}

#[test]
fn a_template_may_place_a_panel_an_egui_pop_plugin_declares() {
    let shell = Shell::new(helios_panels(), None);
    let ids: Vec<String> = shell
        .template_panels()
        .iter()
        .map(|p| p.id.as_str().to_string())
        .collect();
    assert!(ids.iter().any(|i| i == "egui_pop.jobs"), "{ids:?}");
    assert!(ids.iter().any(|i| i == "helios.vault"), "{ids:?}");
    // Helios's own catalogue still refuses a panel nobody declares.
    let mut b = templates::find("helios.import-cull-dedupe").unwrap();
    let report = b.validate(&desktop());
    assert!(report.is_ok(), "{:?}", report.problems);
    let text = json!({
        "kind": "layout",
        "version": 2,
        "areas": [["center", { "kind": "tabs", "panels": ["nobody.panel"], "active": 0 }]],
        "auxiliary": [],
        "hidden": []
    });
    b.layout = Layout::from_json(&text.to_string()).unwrap();
    let report = b.validate(&desktop());
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.contains("unknown panel `nobody.panel`")),
        "{:?}",
        report.problems
    );
}

#[test]
fn a_workspace_from_import_cull_dedupe_is_laid_out_and_set_for_culling() {
    let root = temp("intake");
    let (config, dir) = (root.join("config"), root.join("shoot"));
    let intake = templates::find("helios.import-cull-dedupe").unwrap();
    let created = templates::create_workspace(&intake, &desktop(), &dir, &config).unwrap();
    let kept = templates::workspace_template(&config, &created.workspace)
        .unwrap()
        .unwrap();
    assert_eq!(kept.template, intake.template);
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&created.workspace)
        .build();
    shell.load_layout().unwrap();
    shell.apply_template_tools(&kept.template.meta.name, &kept.template.tools);
    shell.apply_template_library(kept.template.library.as_ref());
    assert!(shell.library().auto_advance);
    assert!(shell.suggests_stacks());
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    // The grid central, the stacks and duplicates beside, the filmstrip
    // under, the editing panels away.
    let shown = h.state().layout();
    assert_eq!(shown.area_of(&panel("helios.vault")), Some(Area::Center));
    assert_eq!(shown.area_of(&panel("helios.compare")), Some(Area::Center));
    assert_eq!(shown.area_of(&panel("helios.stacks")), Some(Area::EndSide));
    assert_eq!(
        shown.area_of(&panel("helios.duplicates")),
        Some(Area::EndSide)
    );
    assert_eq!(
        shown.area_of(&panel("helios.filmstrip")),
        Some(Area::Bottom)
    );
    // egui-pop's jobs centre beside the filmstrip, for the import.
    assert_eq!(shown.area_of(&panel("egui_pop.jobs")), Some(Area::Bottom));
    assert!(shown.hidden.contains(&panel("helios.masks")));
    // The loupe takes the culling keys, in the workspace's layer.
    let keys = h.state().app().handle().export_keymap(Layer::Workspace);
    assert!(
        keys.bindings.iter().any(|b| {
            b.command.as_deref() == Some("vault.flag.reject") && b.when.contains("helios.viewport")
        }),
        "{:?}",
        keys.bindings
    );
    // Importing from its source: a card, copied and renamed, duplicates
    // asked about; without a path, the path is asked.
    h.state_mut()
        .run("library.importSource", json!(null), Origin::User)
        .unwrap();
    h.run();
    let form = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    assert_eq!(form["command"], json!("library.importSource"), "{form}");
    assert_eq!(form["fields"][0]["key"], json!("path"));
    h.state_mut()
        .run(
            "library.importSource",
            json!({ "path": "/media/card" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({
            "path": "/media/card",
            "copy": true,
            "rename": "{date}-{seq}",
            "duplicates": "ask"
        })]
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_intake_template_speaks_french() {
    let i = I18n::with_egui_pop("fr");
    i.add_catalogues(helios_i18n::CATALOGUES).unwrap();
    templates::add_texts(&i, &templates::builtin()).unwrap();
    let intake = templates::find("helios.import-cull-dedupe").unwrap();
    assert_eq!(
        i.t(&intake.name_key(), &[]),
        "Importer, trier, dédoublonner"
    );
}

#[test]
fn a_workspace_from_develop_preview_shows_the_image_with_its_parameters_and_scopes() {
    let root = temp("develop");
    let (config, dir) = (root.join("config"), root.join("develop"));
    let develop = templates::find("helios.develop-preview").unwrap();
    let created = templates::create_workspace(&develop, &desktop(), &dir, &config).unwrap();
    let kept = templates::workspace_template(&config, &created.workspace)
        .unwrap()
        .unwrap();
    assert_eq!(kept.template, develop.template);
    // Exposure into sigmoid, on the desktop's own modules.
    let built = kept.build(&templates::modules()).unwrap();
    let names: Vec<&str> = built.nodes.keys().map(String::as_str).collect();
    assert_eq!(names, ["exposure", "sigmoid", "source"]);
    assert_eq!(built.view.0, built.nodes["sigmoid"]);
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&created.workspace)
        .build();
    shell.load_layout().unwrap();
    shell.apply_template_tools(&kept.template.meta.name, &kept.template.tools);
    shell.apply_template_library(kept.template.library.as_ref());
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    // The image central, the parameters and the scopes beside it, the
    // filmstrip under to pick the picture, the panels of other tasks away.
    let shown = h.state().layout();
    assert_eq!(shown.area_of(&panel("helios.viewport")), Some(Area::Center));
    assert_eq!(shown.area_of(&panel("helios.params")), Some(Area::EndSide));
    assert_eq!(shown.area_of(&panel("helios.scopes")), Some(Area::EndSide));
    assert_eq!(
        shown.area_of(&panel("helios.filmstrip")),
        Some(Area::Bottom)
    );
    for p in ["helios.graph", "helios.masks"] {
        assert!(shown.hidden.contains(&panel(p)), "{p}");
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_develop_template_speaks_french() {
    let i = I18n::with_egui_pop("fr");
    i.add_catalogues(helios_i18n::CATALOGUES).unwrap();
    templates::add_texts(&i, &templates::builtin()).unwrap();
    let develop = templates::find("helios.develop-preview").unwrap();
    assert_eq!(i.t(&develop.name_key(), &[]), "Développer (aperçu)");
    assert!(i.t(&develop.scenario_key(), &[]).contains("aperçu intégré"));
}
