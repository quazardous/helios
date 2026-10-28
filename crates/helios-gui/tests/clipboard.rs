//! Copy and paste through egui-pop's clipboard, with Helios's types: a
//! parameter's value copied as text that pastes back, onto the same
//! kind only, as one undo step; an agent's own slot.

use egui::vec2;
use egui_pop::api::clipboard::{self, PasteRequest};
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

const EV: &str = "node/exposure/exposure";
const BLACK: &str = "node/exposure/black";
const BIAS: &str = "node/exposure/compensate_bias";

fn harness() -> egui_kittest::Harness<'static, Shell> {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(64, 48), 4, |_, _, _| 0.2),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    let shell = Shell::new(helios_panels(), None);
    helios_values::bind_node(&mut shell.values.lock().unwrap(), &g, exp).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

fn value(h: &egui_kittest::Harness<'_, Shell>, path: &str) -> Value {
    h.state()
        .values
        .lock()
        .unwrap()
        .get(path)
        .unwrap()
        .value
        .clone()
}

fn steps(h: &egui_kittest::Harness<'_, Shell>) -> Vec<String> {
    h.state()
        .values
        .lock()
        .unwrap()
        .undo_steps()
        .iter()
        .map(|s| s.label.clone())
        .collect()
}

fn run(h: &mut egui_kittest::Harness<'_, Shell>, id: &str, args: serde_json::Value) {
    run_as(h, id, args, Origin::User);
}

fn run_as(
    h: &mut egui_kittest::Harness<'_, Shell>,
    id: &str,
    args: serde_json::Value,
    origin: Origin,
) {
    h.state_mut().run(id, args, origin).unwrap();
    h.run();
}

#[test]
fn a_copied_value_pastes_back_as_one_undo_step() {
    let mut h = harness();
    run(&mut h, "param.set", json!({ "path": EV, "value": 0.3 }));
    let before = steps(&h).len();
    run(&mut h, "param.copyValue", json!({ "path": EV }));
    // A copy is no undo step; its text is the value as shown.
    assert_eq!(steps(&h).len(), before);
    let item = clipboard::with_owner(h.state().app().handle(), |c| c.get(&clipboard::Slot::User))
        .flatten()
        .expect("the user's slot holds the copy");
    assert_eq!(item.kind, helios_gui::clipboard::PARAM);
    assert_eq!(item.text(), Some("+0.30 EV"));
    // Pasted onto the same parameter after a change: back, one step.
    run(&mut h, "param.set", json!({ "path": EV, "value": 1.0 }));
    run(&mut h, "param.paste", json!({ "path": EV }));
    assert_eq!(value(&h, EV), Value::Float(0.3));
    let s = steps(&h);
    assert_eq!(s.len(), before + 2, "{s:?}");
    assert!(s.last().unwrap().starts_with("Paste"), "{s:?}");
    // The text alone, as the system clipboard gives it back, pastes too.
    run(&mut h, "param.set", json!({ "path": EV, "value": 1.0 }));
    let app = h.state().app().handle().clone();
    let target = helios_gui::clipboard::PARAM_ENDPOINT;
    h.state()
        .clip_param()
        .pin(EV, helios_gui::clipboard::param::Text::Value);
    let out = clipboard::paste(
        &app,
        PasteRequest::new(target).text("-0,50 EV"),
        &egui_pop::model::commands::Origin::User,
    );
    h.state().clip_param().unpin();
    assert!(out.ok(), "{out:?}");
    assert_eq!(value(&h, EV), Value::Float(-0.5));
}

#[test]
fn a_path_copied_pastes_its_value_and_kinds_must_match() {
    let mut h = harness();
    run(&mut h, "param.set", json!({ "path": EV, "value": 0.75 }));
    run(&mut h, "param.copyPath", json!({ "path": EV }));
    let item = clipboard::with_owner(h.state().app().handle(), |c| c.get(&clipboard::Slot::User))
        .flatten()
        .unwrap();
    assert_eq!(item.text(), Some(EV));
    // A float onto a float: the value; a dry run first changes nothing.
    let before = value(&h, BLACK);
    let plan = h
        .state_mut()
        .run(
            "param.paste",
            json!({ "path": BLACK, "dry_run": true }),
            Origin::User,
        )
        .unwrap();
    h.run();
    assert_eq!(value(&h, BLACK), before, "{plan:?}");
    run(&mut h, "param.paste", json!({ "path": BLACK }));
    assert_eq!(value(&h, BLACK), Value::Float(0.75));
    // A number onto a switch: refused, the switch as it was.
    let bias = value(&h, BIAS);
    h.state_mut()
        .run("param.paste", json!({ "path": BIAS }), Origin::User)
        .unwrap();
    h.run();
    assert_eq!(value(&h, BIAS), bias);
}

/// An agent's copies and pastes use its own slot: it does not read
/// what the user copied.
#[test]
fn an_agent_does_not_paste_the_user_s_copy() {
    let mut h = harness();
    run(&mut h, "param.set", json!({ "path": EV, "value": 0.3 }));
    run(&mut h, "param.copyValue", json!({ "path": EV }));
    run(&mut h, "param.set", json!({ "path": EV, "value": 1.0 }));
    let agent = Origin::Agent("retouch".into());
    run_as(&mut h, "param.paste", json!({ "path": EV }), agent.clone());
    assert_eq!(value(&h, EV), Value::Float(1.0));
    // Its own copy, then its paste: its slot.
    run_as(
        &mut h,
        "param.copyValue",
        json!({ "path": EV }),
        agent.clone(),
    );
    run(&mut h, "param.set", json!({ "path": EV, "value": 0.0 }));
    run_as(&mut h, "param.paste", json!({ "path": EV }), agent);
    assert_eq!(value(&h, EV), Value::Float(1.0));
}

/// Images of the library: a copy names them; pasted into a library that
/// lacks them, or as files from another application, they are imported
/// once the user confirms; those it holds stay references.
#[test]
fn images_copied_or_files_pasted_are_imported_once_confirmed() {
    use egui_pop::api::clipboard::CopyRequest;
    use egui_pop::model::commands::Origin as PopOrigin;
    use helios_gui::clipboard::{IMAGES, LIBRARY_ENDPOINT};
    use helios_gui::library::Entry;

    let dir = std::env::temp_dir().join(format!("helios-clip-images-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("b.jpg");
    std::fs::write(&file, b"not a real image").unwrap();
    let entries: Vec<Entry> = ["a", "b"]
        .iter()
        .map(|id| Entry {
            id: id.to_string(),
            name: format!("{id}.jpg"),
            ..Entry::default()
        })
        .collect();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries);
    shell.set_image_files([("b".to_string(), file.clone())]);
    shell.library_mut().select_ids(&["a".into(), "b".into()]);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let app = h.state().app().handle().clone();
    let out = clipboard::copy(&app, CopyRequest::new(LIBRARY_ENDPOINT), &PopOrigin::User);
    assert!(out.ok(), "{out:?}");
    let item = clipboard::with_owner(&app, |c| c.get(&clipboard::Slot::User))
        .flatten()
        .unwrap();
    assert_eq!(item.kind, IMAGES);
    assert_eq!(item.text(), Some("a.jpg\nb.jpg"));
    // The same library holds both: nothing to import.
    let out = clipboard::paste(&app, PasteRequest::new(LIBRARY_ENDPOINT), &PopOrigin::User);
    assert!(!out.ok(), "{out:?}");
    // Another library, without `b`: its file is imported, asked first.
    h.state_mut().set_library(vec![Entry {
        id: "a".into(),
        name: "a.jpg".into(),
        ..Entry::default()
    }]);
    h.run();
    let plan = clipboard::paste(
        &app,
        PasteRequest::new(LIBRARY_ENDPOINT).dry_run(),
        &PopOrigin::User,
    );
    assert!(plan.ok(), "{plan:?}");
    h.run();
    assert!(
        h.state().target_rect("helios.dialog").is_err(),
        "a dry run asks nothing"
    );
    let out = clipboard::paste(&app, PasteRequest::new(LIBRARY_ENDPOINT), &PopOrigin::User);
    assert!(out.ok(), "{out:?}");
    h.run();
    assert!(
        h.state_mut().take_imports().is_empty(),
        "nothing before the answer"
    );
    let current = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    assert_eq!(current["kind"], json!("confirm"));
    h.key_press(egui::Key::Enter);
    h.run();
    let imports = h.state_mut().take_imports();
    assert_eq!(imports.len(), 1, "{imports:?}");
    assert_eq!(imports[0]["paths"], json!([file]));
    // Files named by another application's text: the same import.
    let text = format!("file://{}\n/nowhere/c.jpg", file.display());
    let out = clipboard::paste(
        &app,
        PasteRequest::new(LIBRARY_ENDPOINT).text(&text),
        &PopOrigin::User,
    );
    assert!(out.ok(), "{out:?}");
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    let imports = h.state_mut().take_imports();
    assert_eq!(imports.len(), 1, "{imports:?}");
    assert_eq!(imports[0]["paths"], json!([file]));
    let _ = std::fs::remove_dir_all(&dir);
}
