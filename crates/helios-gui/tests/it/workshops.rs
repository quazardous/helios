//! Workshops behind their switch (`interface.experimental.workshops`):
//! off, no Workshops panel; on, the panel is declared, closed until
//! opened, and says it is experimental.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_pop::model::ids::PanelId;
use helios_gui::{Experimental, Shell, headless, helios_panels_with, workshops};
use helios_shell::commands::Origin;
use serde_json::json;

fn harness(on: bool) -> Harness<'static, Shell> {
    let panels = helios_panels_with(Experimental { workshops: on });
    let mut h = headless::harness(Shell::new(panels, None), vec2(1280.0, 800.0));
    h.run();
    h
}

fn declared(h: &Harness<'_, Shell>) -> bool {
    h.state()
        .panels()
        .iter()
        .any(|p| p.id.as_str() == workshops::PANEL)
}

#[test]
fn off_there_is_no_workshops_panel() {
    let mut h = harness(false);
    assert!(!declared(&h));
    assert!(
        h.state_mut()
            .run("helios.workshops.focus", json!(null), Origin::User)
            .is_err()
    );
}

#[test]
fn on_the_panel_opens_and_says_it_is_experimental() {
    let mut h = harness(true);
    assert!(declared(&h));
    let id = PanelId::new(workshops::PANEL).unwrap();
    assert!(!h.state().app().handle().panel_visible(&id));
    h.state_mut()
        .run("helios.workshops.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
    assert!(h.state().app().handle().panel_visible(&id));
    h.get_by_label_contains("Experimental: may change");
}

// ---------------------------------------------------------------------
// workshop.apply, with a workspace and an image open
// ---------------------------------------------------------------------

use std::path::{Path, PathBuf};

use helios_core::{Image, Region};
use helios_gui::templates;
use helios_template::Source;
use serde_json::Value as Json;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-workshops-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A window on a blank workspace with a grey image open through it,
/// workshops `on` or off.
fn window(root: &Path, on: bool) -> Harness<'static, Shell> {
    let config = root.join("config");
    let dir = root.join("photos");
    std::fs::create_dir_all(&dir).unwrap();
    let panels = helios_panels_with(Experimental { workshops: on });
    let mut shell = Shell::builder(panels, Some(config.join("ui")))
        .workspace(&dir)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &dir).unwrap();
    let image = Image::from_fn(Region::sized(16, 12), 4, |x, _, c| {
        if c == 3 {
            1.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let v = 0.05 + x as f32 / 40.0;
            v
        }
    });
    let built = shell
        .workspace_bundle()
        .unwrap()
        .build_with(&templates::modules(), Source::picture(&image), None)
        .unwrap();
    shell
        .set_view_source(built.graph, built.view.0, &built.view.1)
        .unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

/// The frames a step takes, the image kept open through a network change.
fn follow(h: &mut Harness<'static, Shell>) {
    for _ in 0..3 {
        h.run();
    }
    if h.state_mut().take_network_changed() {
        assert!(h.state_mut().keep_image_open().unwrap(), "kept in place");
    }
    for _ in 0..2 {
        h.run();
    }
}

/// Runs a command as a person would; its answer from the history.
fn answer(h: &mut Harness<'static, Shell>, id: &str, args: Json) -> Result<Json, String> {
    let out = h.state_mut().run(id, args, Origin::User);
    follow(h);
    match out {
        Ok(v) if v == json!({ "queued": true }) => {
            h.state()
                .history()
                .into_iter()
                .rev()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id} never ran"))
                .result
        }
        other => other,
    }
}

fn nodes(h: &Harness<'static, Shell>) -> Vec<String> {
    h.state()
        .workspace_state()
        .unwrap()
        .bundle
        .template
        .network
        .nodes
        .iter()
        .map(|n| n.name.clone())
        .collect()
}

/// A parameter of a node of the graph shown; `None` without that node.
fn shown(h: &Harness<'static, Shell>, node: &str, param: &str) -> Option<f64> {
    let viewport = h.state().viewport();
    let g = viewport.graph()?;
    let id = g.find(node)?;
    g.params(id).unwrap()?.get(param)?.as_f64()
}

const LIFT: &str = "helios.lift-midtones@1";

#[test]
fn applying_a_workshop_is_one_undo_step_carrying_its_aimed_value() {
    let root = temp("apply");
    let mut h = window(&root, true);
    let before = nodes(&h);
    let view = h
        .state()
        .workspace_state()
        .unwrap()
        .bundle
        .template
        .network
        .view
        .node;
    let steps = h.state().values().lock().unwrap().undo_steps().len();
    // A dry run changes nothing and says what would.
    let dry = answer(
        &mut h,
        "workshop.apply",
        json!({ "workshop": LIFT, "dry_run": true }),
    )
    .unwrap();
    assert_eq!(dry["dry_run"], json!(true));
    assert_eq!(nodes(&h), before);
    let a = answer(&mut h, "workshop.apply", json!({ "workshop": LIFT })).unwrap();
    let node = a["node"].as_str().unwrap().to_string();
    assert_eq!(node, "lift_midtones");
    assert_eq!(a["after"]["node"], json!(view));
    assert_eq!(a["view"], json!(true));
    let aimed = &a["aimed"][0];
    assert_eq!(aimed["param"], json!("brightness"));
    assert_eq!(aimed["reached"], json!(true), "{a}");
    let value = aimed["value"].as_f64().unwrap();
    assert_eq!(a["values"]["brightness"].as_f64(), Some(value));
    assert_eq!(
        dry["aimed"], a["aimed"],
        "the same aim, from the session's cache"
    );
    // One step: the node in the network, its aimed value on the image.
    assert_eq!(
        h.state().values().lock().unwrap().undo_steps().len(),
        steps + 1
    );
    assert!(nodes(&h).contains(&node));
    assert_eq!(shown(&h, &node, "brightness"), Some(value));
    let store_value = h
        .state()
        .values()
        .lock()
        .unwrap()
        .get(&format!("node/{node}/brightness"))
        .and_then(|e| e.value.as_f64());
    assert_eq!(store_value, Some(value));
    // One undo gives back the network and the image together.
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap()
        .expect("a step to undo");
    follow(&mut h);
    assert_eq!(nodes(&h), before);
    assert_eq!(shown(&h, &node, "brightness"), None);
    let state = h.state().workspace_state().unwrap();
    assert_eq!(state.bundle.template.network.view.node, view);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn applying_refuses_what_is_not_a_workshop() {
    let root = temp("refused");
    let mut h = window(&root, true);
    let e = answer(
        &mut h,
        "workshop.apply",
        json!({ "workshop": "helios.masked-adjustment@1" }),
    )
    .unwrap_err();
    assert!(e.contains("without a workshop block"), "{e}");
    let e = answer(&mut h, "workshop.apply", json!({ "workshop": "lift" })).unwrap_err();
    assert!(e.contains("id@version"), "{e}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn off_the_command_is_unknown_and_an_agent_is_refused_with_the_key() {
    let root = temp("off");
    let mut h = window(&root, false);
    assert!(
        h.state_mut()
            .run("workshop.apply", json!({ "workshop": LIFT }), Origin::User)
            .is_err()
    );
    let request = json!({
        "workspace": root.join("photos"),
        "cmd": "workshop.apply",
        "workshop": LIFT,
    });
    let out = h.state_mut().workspace_request(&request);
    assert_eq!(out["ok"], json!(false), "{out}");
    let e = out["error"].as_str().unwrap();
    assert!(e.contains(workshops::SETTING), "{e}");
    // On, the agent's call goes through, as one step.
    let root_on = temp("agent");
    let mut h = window(&root_on, true);
    let request = json!({
        "workspace": root_on.join("photos"),
        "cmd": "workshop.apply",
        "workshop": LIFT,
    });
    let out = h.state_mut().workspace_request(&request);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["answer"]["node"], json!("lift_midtones"));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root_on);
}

// ---------------------------------------------------------------------
// The panel: the list, find, apply, the applied workshop, aim again
// ---------------------------------------------------------------------

fn open_panel(h: &mut Harness<'static, Shell>) {
    h.state_mut()
        .run("helios.workshops.focus", json!(null), Origin::User)
        .unwrap();
    for _ in 0..3 {
        h.run();
    }
}

#[test]
fn the_panel_lists_the_shipped_workshop_and_applies_it() {
    let root = temp("panel");
    let mut h = window(&root, true);
    open_panel(&mut h);
    h.get_by_label_contains("Shipped with Helios");
    h.get_by_label("Lift midtones");
    h.get_by_label("Brighter midtones without losing the highlights");
    // Words found nowhere: nothing listed.
    h.get_by_label("Find").focus();
    h.run();
    h.get_by_label("Find").type_text("landscape");
    h.run();
    h.get_by_label_contains("No workshop found");
    assert!(h.query_by_label("Lift midtones").is_none());
    // A keyword finds it again.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run();
    h.get_by_label("Find").type_text("underexposed");
    h.run();
    h.get_by_label("Lift midtones");
    // Apply, from its row: one undo step, its node selected.
    let steps = h.state().values().lock().unwrap().undo_steps().len();
    h.get_by_label("Apply").click();
    follow(&mut h);
    follow(&mut h);
    assert!(nodes(&h).contains(&"lift_midtones".to_string()));
    assert_eq!(
        h.state().values().lock().unwrap().undo_steps().len(),
        steps + 1
    );
    // Applied: the row says so, and the workshop's sliders show, the
    // targeted one marked with its target.
    h.get_by_label_contains("Applied: lift_midtones");
    h.get_by_label_contains("aimed: midtones at -2.50 stops");
    h.get_by_label("Aim again");
    let _ = std::fs::remove_dir_all(&root);
}

fn brightness(h: &Harness<'static, Shell>) -> Option<f64> {
    h.state()
        .values()
        .lock()
        .unwrap()
        .get("node/lift_midtones/brightness")
        .and_then(|e| e.value.as_f64())
}

#[test]
fn aiming_again_is_one_value_step_that_undo_takes_back() {
    let root = temp("aim");
    let mut h = window(&root, true);
    answer(&mut h, "workshop.apply", json!({ "workshop": LIFT })).unwrap();
    let aimed = brightness(&h).unwrap();
    // Moved by hand: the user's value now.
    {
        let mut store = h.state().values().lock().unwrap();
        store
            .set(helios_values::Set::new(
                "node/lift_midtones/brightness",
                helios_values::Change::To(helios_core::Value::Float(0.0)),
                Origin::User,
            ))
            .unwrap();
    }
    follow(&mut h);
    assert_eq!(brightness(&h), Some(0.0));
    let state = h.state().workspace_state().unwrap();
    let steps = h.state().values().lock().unwrap().undo_steps().len();
    // A dry run says the value, sets nothing.
    let dry = answer(
        &mut h,
        "workshop.aim",
        json!({ "node": "lift_midtones", "dry_run": true }),
    )
    .unwrap();
    assert_eq!(dry["values"]["brightness"].as_f64(), Some(aimed));
    assert_eq!(brightness(&h), Some(0.0));
    // Aimed again, the selected workshop by default: one value step.
    let a = answer(&mut h, "workshop.aim", json!({})).unwrap();
    assert_eq!(a["node"], json!("lift_midtones"));
    assert_eq!(a["aimed"][0]["reached"], json!(true), "{a}");
    assert_eq!(brightness(&h), Some(aimed));
    let store_steps = h.state().values().lock().unwrap().undo_steps().len();
    assert_eq!(store_steps, steps + 1);
    assert_eq!(
        h.state().workspace_state().unwrap().to_json(),
        state.to_json(),
        "the workspace does not change"
    );
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap()
        .expect("a step to undo");
    follow(&mut h);
    assert_eq!(brightness(&h), Some(0.0));
    // A module is not a workshop.
    let e = answer(&mut h, "workshop.aim", json!({ "node": "exposure" })).unwrap_err();
    assert!(e.contains("no node") || e.contains("not a workshop"), "{e}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn suggest_ranks_the_name_then_the_keywords_then_the_intent() {
    let root = temp("suggest");
    let mut h = window(&root, true);
    for (words, matched) in [
        ("lift", "name"),
        ("underexposed", "keyword"),
        ("keep highlights", "intent"),
    ] {
        let a = answer(&mut h, "workshop.suggest", json!({ "intent": words })).unwrap();
        let first = &a["workshops"][0];
        assert_eq!(first["workshop"], json!(LIFT), "{a}");
        assert_eq!(first["matched"], json!(matched), "{words}: {a}");
        assert_eq!(first["from"], json!("shipped"));
    }
    let a = answer(&mut h, "workshop.suggest", json!({ "intent": "landscape" })).unwrap();
    assert_eq!(a["workshops"], json!([]));
    // Applied, it says as which node.
    answer(&mut h, "workshop.apply", json!({ "workshop": LIFT })).unwrap();
    let a = answer(&mut h, "workshop.suggest", json!({ "intent": "lift" })).unwrap();
    assert_eq!(a["workshops"][0]["applied"], json!(["lift_midtones"]));
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------
// Save as workshop
// ---------------------------------------------------------------------

fn pinned(h: &Harness<'static, Shell>, node: &str) -> Option<String> {
    h.state()
        .workspace_state()
        .unwrap()
        .bundle
        .template
        .network
        .nodes
        .iter()
        .find(|n| n.name == node)
        .and_then(|n| n.composite.as_ref())
        .map(ToString::to_string)
}

fn composite_file(h: &Harness<'static, Shell>, r: &str) -> Option<helios_template::CompositeFile> {
    h.state()
        .workspace_state()
        .unwrap()
        .bundle
        .composites
        .into_iter()
        .find(|f| f.reference().to_string() == r)
}

/// The window with the exposure made a macro, `look` (`user.look@1`),
/// its exposure exposed.
fn with_a_macro(root: &Path, on: bool) -> Harness<'static, Shell> {
    let mut h = window(root, on);
    answer(
        &mut h,
        "macro.make",
        json!({
            "nodes": ["exposure"],
            "name": "Look",
            "expose": [ { "name": "brightness", "node": "exposure", "param": "exposure" } ],
        }),
    )
    .unwrap();
    assert_eq!(pinned(&h, "look").as_deref(), Some("user.look@1"));
    h
}

#[test]
fn saving_a_macro_as_a_workshop_is_its_next_version_and_one_undo_step() {
    let root = temp("save");
    let mut h = with_a_macro(&root, true);
    let steps = h.state().values().lock().unwrap().undo_steps().len();
    let dry = answer(
        &mut h,
        "workshop.save",
        json!({ "node": "look", "intent": "Darker", "dry_run": true }),
    )
    .unwrap();
    assert_eq!(dry["dry_run"], json!(true));
    assert_eq!(pinned(&h, "look").as_deref(), Some("user.look@1"));
    let a = answer(
        &mut h,
        "workshop.save",
        json!({
            "node": "look",
            "intent": "Darker, for moody shots",
            "keywords": ["dark", "moody"],
            "targets": [ { "param": "brightness", "measure": "luma.median", "value": -3.0 } ],
        }),
    )
    .unwrap();
    assert_eq!(a["to"], json!({ "id": "user.look", "version": 2 }), "{a}");
    assert_eq!(pinned(&h, "look").as_deref(), Some("user.look@2"));
    let w = composite_file(&h, "user.look@2").unwrap().workshop.unwrap();
    assert_eq!(w.intent, "Darker, for moody shots");
    assert_eq!(w.keywords, ["dark", "moody"]);
    assert_eq!(w.targets[0].param, "brightness");
    assert_eq!(
        h.state().values().lock().unwrap().undo_steps().len(),
        steps + 1
    );
    // A workshop now: suggested, and aimed again on the image.
    let found = answer(&mut h, "workshop.suggest", json!({ "intent": "moody" })).unwrap();
    assert_eq!(found["workshops"][0]["workshop"], json!("user.look@2"));
    assert_eq!(found["workshops"][0]["from"], json!("workspace"));
    let aimed = answer(&mut h, "workshop.aim", json!({ "node": "look" })).unwrap();
    assert_eq!(aimed["aimed"][0]["param"], json!("brightness"), "{aimed}");
    // Undo the aim, then the save: the node back on version 1, the file gone.
    for _ in 0..2 {
        h.state()
            .values()
            .lock()
            .unwrap()
            .undo(&Origin::User)
            .unwrap()
            .expect("a step to undo");
        follow(&mut h);
    }
    assert_eq!(pinned(&h, "look").as_deref(), Some("user.look@1"));
    assert!(composite_file(&h, "user.look@2").is_none());
    // Publishing stays macro.publish's, asked first.
    let asked = answer(&mut h, "macro.publish", json!({ "node": "look" })).unwrap();
    assert_eq!(asked["asked"], json!(true), "{asked}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_save_form_asks_with_the_target_prefilled_from_the_image() {
    let root = temp("save-form");
    let mut h = with_a_macro(&root, true);
    // The Modules panel offers it on a macro without a block.
    h.state_mut()
        .run("helios.modules.focus", json!(null), Origin::User)
        .unwrap();
    for _ in 0..3 {
        h.run();
    }
    h.get_by_label("Save as workshop…");
    let asked = answer(&mut h, "workshop.save", json!({ "node": "look" })).unwrap();
    assert_eq!(asked["asked"], json!(true), "{asked}");
    let dialog = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    let text = dialog.to_string();
    assert!(text.contains("Save look as a workshop"), "{text}");
    assert!(text.contains("the image now: midtones"), "{text}");
    assert!(text.contains("measure_brightness"), "{text}");
    // What the form answers: the measure chosen, the value given.
    let a = answer(
        &mut h,
        "workshop.save",
        json!({
            "node": "look",
            "intent": "Darker",
            "keywords": "dark, moody ,",
            "measure_brightness": "luma.high",
            "value_brightness": -0.5,
        }),
    )
    .unwrap();
    assert_eq!(a["workshop"]["keywords"], json!(["dark", "moody"]), "{a}");
    assert_eq!(
        a["workshop"]["targets"],
        json!([ { "param": "brightness", "measure": "luma.high", "value": -0.5 } ])
    );
    // A workshop now: the panel no longer offers it.
    for _ in 0..3 {
        h.run();
    }
    assert!(h.query_by_label("Save as workshop…").is_none());
    // No target chosen: a workshop without targets.
    let a = answer(
        &mut h,
        "workshop.save",
        json!({ "node": "look", "intent": "Just darker", "measure_brightness": "none" }),
    )
    .unwrap();
    assert_eq!(a["workshop"]["targets"], json!(null), "{a}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn off_save_as_workshop_is_not_offered() {
    let root = temp("save-off");
    let mut h = with_a_macro(&root, false);
    h.state_mut()
        .run("helios.modules.focus", json!(null), Origin::User)
        .unwrap();
    for _ in 0..3 {
        h.run();
    }
    h.get_by_label("Publish…");
    assert!(h.query_by_label("Save as workshop…").is_none());
    let _ = std::fs::remove_dir_all(&root);
}
