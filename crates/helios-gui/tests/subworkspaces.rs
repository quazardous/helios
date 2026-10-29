//! Sub-workspaces as places (`helios.sub-workspace`): entered under
//! their parent's place by command, quick open or the graph, named by
//! their name in the breadcrumb, in their own arrangement; inside, only
//! their nodes are shown and written; one deleted while shown is left,
//! and one gone when the workspace opens again is cut.

use std::path::{Path, PathBuf};

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::model::ids::PanelId;
use egui_pop::model::panels::Area;
use helios_core::{Graph, Image, ModuleRef, Params, Region};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_values::{Descriptor, Scope};
use serde_json::{Value, json};

const EV: &str = "node/exposure/exposure";
const OPACITY: &str = "node/blend/opacity";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-subworkspaces-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("photos")).unwrap();
    dir
}

/// A window on a workspace not made from a template (the blank network:
/// a source, an exposure and a mask into a blend), with values for the
/// exposure and the blend as an image opened would declare them.
fn window(root: &Path) -> Harness<'static, Shell> {
    let config = root.join("config");
    let dir = root.join("photos");
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&dir)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &dir).unwrap();
    {
        let mut g = Graph::new();
        let src = g.add_source(
            "source",
            Image::from_fn(Region::sized(8, 8), 4, |_, _, _| 0.2),
        );
        let exp = g
            .add_module(
                "exposure",
                ModuleRef::new(helios_module_exposure::Exposure),
                &Params::new(),
            )
            .unwrap();
        g.connect(src, "image", exp, "input").unwrap();
        let mut store = shell.values.lock().unwrap();
        helios_values::bind_node(&mut store, &g, exp).unwrap();
        store.declare(OPACITY, Descriptor::float("opacity", 0.0, 1.0, 1.0));
    }
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

fn run_as(
    h: &mut Harness<'static, Shell>,
    id: &str,
    args: Value,
    origin: Origin,
) -> Result<Value, String> {
    let out = h.state_mut().run(id, args, origin);
    // Fixed steps, not `run`: a refusal's toast keeps asking for a
    // repaint until it expires.
    h.run_steps(12);
    out
}

fn run(h: &mut Harness<'static, Shell>, id: &str, args: Value) -> Value {
    run_as(h, id, args, Origin::User).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn key(h: &Harness<'_, Shell>, k: &str) -> String {
    h.state().app().handle().context().get(k).to_string()
}

fn crumbs(h: &Harness<'_, Shell>) -> Vec<String> {
    egui_pop::api::contexts::crumbs(h.state().app().handle())
}

fn scope(h: &Harness<'_, Shell>) -> Option<Scope> {
    h.state().values.lock().unwrap().scope().cloned()
}

fn toasted(h: &Harness<'_, Shell>, text: &str) -> bool {
    h.state()
        .app()
        .handle()
        .service(&egui_pop::api::toasts::TOASTS)
        .unwrap()
        .shown()
        .iter()
        .any(|t| t.toast.text.contains(text))
}

/// The light (the exposure and the mask) with the mask nested in it.
fn delegate(h: &mut Harness<'static, Shell>) {
    run(
        h,
        "network.delegate",
        json!({ "name": "Light & mask", "nodes": ["exposure", "mask"] }),
    );
    run(
        h,
        "network.delegate",
        json!({ "name": "Mask", "nodes": ["mask"] }),
    );
}

#[test]
fn a_sub_workspace_is_entered_under_its_parent_and_scopes_the_edits() {
    let root = temp("enter");
    let mut h = window(&root);
    delegate(&mut h);
    // An id the workspace does not have is refused, the place kept. The
    // command is the shell's, run at the next frame (it answers that it
    // is queued): the refusal is said in a toast.
    run(&mut h, "subworkspace.enter", json!({ "id": "nowhere" }));
    assert_eq!(key(&h, "place"), "root");
    assert!(toasted(&h, "`nowhere`"));
    // The nested one: entered under its parent, named by their names.
    run(&mut h, "subworkspace.enter", json!({ "id": "mask" }));
    assert_eq!(key(&h, "place"), "helios.sub-workspace");
    assert_eq!(key(&h, "payload.id"), "mask");
    assert_eq!(crumbs(&h), ["Helios", "Light & mask", "Mask"]);
    // Its arrangement: the network's, the graph in the centre.
    assert_eq!(
        h.state()
            .layout()
            .area_of(&PanelId::new("helios.graph").unwrap()),
        Some(Area::Center)
    );
    // Up to its parent: the light's nodes may be written, the blend's
    // not, by the user or an agent, and the refusal names it.
    run(&mut h, "nav.up", json!(null));
    assert_eq!(crumbs(&h), ["Helios", "Light & mask"]);
    let inside = scope(&h).expect("inside a sub-workspace, a scope");
    assert_eq!(inside.name, "Light & mask");
    run(&mut h, "param.set", json!({ "path": EV, "value": 0.5 }));
    for origin in [Origin::User, Origin::Agent("a1".into())] {
        let refused = run_as(
            &mut h,
            "param.set",
            json!({ "path": OPACITY, "value": 0.5 }),
            origin,
        )
        .unwrap_err();
        assert!(refused.contains("Light & mask"), "{refused}");
    }
    // The graph shows its nodes only, and the mask nested in it as a
    // group.
    let graph = run(
        &mut h,
        "graph.read",
        json!({ "graph": helios_gui::graph::GRAPH }),
    );
    let mut ids: Vec<&str> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, ["exposure", "mask"], "{graph}");
    // Back at the root: no scope, the blend may be written again.
    run(&mut h, "nav.back", json!(null));
    assert_eq!(key(&h, "place"), "root");
    assert!(scope(&h).is_none());
    run(
        &mut h,
        "param.set",
        json!({ "path": OPACITY, "value": 0.5 }),
    );
    // Quick open goes to one by id; without an id, the command enters
    // the one under the place shown holding the selected node.
    run(
        &mut h,
        helios_gui::quick_open::GO,
        json!({ "item": "subworkspace:light-mask" }),
    );
    assert_eq!(key(&h, "payload.id"), "light-mask");
    run(&mut h, "nav.back", json!(null));
    run(
        &mut h,
        "param.set",
        json!({ "path": helios_gui::params::SELECTION, "value": "mask" }),
    );
    run(&mut h, "subworkspace.enter", json!(null));
    assert_eq!(key(&h, "payload.id"), "light-mask");
    run(&mut h, "subworkspace.enter", json!(null));
    assert_eq!(key(&h, "payload.id"), "mask");
    egui_pop_test::assert_keymap_clean(h.state().app());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_sub_workspace_deleted_or_gone_is_left_with_a_toast() {
    let root = temp("gone");
    {
        let mut h = window(&root);
        delegate(&mut h);
        run(&mut h, "subworkspace.enter", json!({ "id": "light-mask" }));
        // Deleted while shown: back to where it was entered from.
        run(&mut h, "subworkspace.delete", json!({ "id": "light-mask" }));
        assert_eq!(key(&h, "place"), "root");
        assert!(toasted(&h, "light-mask"));
        // The nested one moved up; entered, it is kept for next time.
        run(&mut h, "subworkspace.enter", json!({ "id": "mask" }));
        assert_eq!(crumbs(&h), ["Helios", "Mask"]);
    }
    // Opened again: where it was left.
    {
        let h = window(&root);
        assert_eq!(key(&h, "payload.id"), "mask");
        assert_eq!(crumbs(&h), ["Helios", "Mask"]);
    }
    // Deleted on the workspace's files meanwhile: cut, with a toast.
    helios_template::workspace::tools::delete_subworkspace(
        &root.join("config"),
        &json!({ "workspace": root.join("photos"), "id": "mask" }),
        &helios_gui::templates::modules(),
    )
    .unwrap();
    let mut h = window(&root);
    h.run();
    assert_eq!(key(&h, "place"), "root");
    assert!(scope(&h).is_none());
    assert!(toasted(&h, "mask"));
    let _ = std::fs::remove_dir_all(&root);
}
