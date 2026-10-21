//! Sub-workspaces as data: named scopes over nodes of a workspace's own
//! network, read and written with the template (old files unchanged),
//! checked, delegated and deleted as undoable steps with dry runs, never
//! given an id twice, nested, copied with a new id on import, and left by
//! the nodes taken out of the network.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use helios_core::ModuleRef;
use helios_template::workspace::{self, Files, Part, Side, State, tools};
use helios_template::{Bundle, Catalogue, SubWorkspace, parse_template, schema, shipped};
use serde_json::{Value as Json, json};

fn catalogue() -> Catalogue {
    use helios_module_masks as m;
    let mut c = Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
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
    let dir = std::env::temp_dir().join(format!(
        "helios-subworkspaces-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn portrait() -> Bundle {
    shipped::templates()
        .into_iter()
        .find(|b| b.template.id == "helios.portrait-retouch")
        .unwrap()
}

/// A workspace made from the portrait template under `root`.
fn made(root: &Path, name: &str) -> (PathBuf, Files) {
    let config = root.join("config");
    let (dir, _) = workspace::create(
        &portrait(),
        &BTreeMap::new(),
        &catalogue(),
        &root.join(name),
        &config,
    )
    .unwrap();
    let files = Files::new(&config, &dir);
    (dir, files)
}

/// The portrait workspace's state, not written anywhere.
fn state() -> State {
    State {
        meta: workspace::Meta {
            name: "portrait".into(),
            ..workspace::Meta::default()
        },
        bundle: portrait(),
    }
}

fn nodes(names: &[&str]) -> Vec<String> {
    names.iter().map(ToString::to_string).collect()
}

fn ids(state: &State) -> Vec<&str> {
    state
        .bundle
        .template
        .subworkspaces
        .iter()
        .map(|s| s.id.as_str())
        .collect()
}

#[test]
fn a_template_without_sub_workspaces_reads_and_writes_as_before() {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates/portrait-retouch.helios-template/template.json"),
    )
    .unwrap();
    let t = parse_template(&text).unwrap();
    assert!(t.subworkspaces.is_empty());
    assert!(t.retired_subworkspaces.is_empty());
    // Written back, it names neither: an older Helios still reads it.
    let written = serde_json::to_string(&t).unwrap();
    assert!(!written.contains("subworkspaces"), "{written}");
    assert!(portrait().validate(&catalogue()).problems.is_empty());
}

#[test]
fn sub_workspaces_round_trip_and_fit_the_schema() {
    let mut t = serde_json::to_value(&portrait().template).unwrap();
    t["subworkspaces"] = json!([
        { "id": "face", "name": "Face", "nodes": ["exposure", "retouch"] },
        { "id": "mask", "name": "Mask", "nodes": ["retouch.mask"], "parent": "face" }
    ]);
    t["retired_subworkspaces"] = json!(["old"]);
    assert_eq!(schema::check(&schema::template(), &t), Vec::<String>::new());
    let read = parse_template(&t.to_string()).unwrap();
    assert_eq!(
        read.subworkspaces[1],
        SubWorkspace {
            id: "mask".into(),
            name: "Mask".into(),
            nodes: nodes(&["retouch.mask"]),
            parent: Some("face".into()),
        }
    );
    assert_eq!(serde_json::to_value(&read).unwrap(), t);
    // In a bundle, written and loaded back.
    let mut bundle = portrait();
    bundle.template = read.clone();
    assert!(bundle.validate(&catalogue()).problems.is_empty());
    let dir = temp("round-trip");
    bundle.save(&dir.join("b")).unwrap();
    assert_eq!(Bundle::load(&dir.join("b")).unwrap().template, read);
    // A field a sub-workspace does not have is refused by both.
    t["subworkspaces"][0]["colour"] = json!(1);
    assert!(!schema::check(&schema::template(), &t).is_empty());
    assert!(parse_template(&t.to_string()).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_checks_name_what_is_wrong() {
    let problems = |list: Json, retired: Json| {
        let mut b = portrait();
        b.template.subworkspaces = serde_json::from_value(list).unwrap();
        b.template.retired_subworkspaces = serde_json::from_value(retired).unwrap();
        b.validate(&catalogue()).problems.join("\n")
    };
    let p = problems(
        json!([{ "id": "a", "name": "A", "nodes": ["nowhere"] }]),
        json!([]),
    );
    assert!(p.contains("no node `nowhere`"), "{p}");
    let p = problems(
        json!([{ "id": "a", "name": "A", "nodes": ["retouch.nothing"] }]),
        json!([]),
    );
    assert!(p.contains("has no node `nothing`"), "{p}");
    let p = problems(
        json!([{ "id": "a", "name": "A", "nodes": ["exposure.inner"] }]),
        json!([]),
    );
    assert!(p.contains("not an opened composite"), "{p}");
    let p = problems(
        json!([{ "id": "a", "name": "A", "nodes": ["exposure"] }]),
        json!(["a"]),
    );
    assert!(p.contains("retired"), "{p}");
    let p = problems(
        json!([
            { "id": "a", "name": "A", "nodes": ["exposure"] },
            { "id": "a", "name": "B", "nodes": ["retouch"] }
        ]),
        json!([]),
    );
    assert!(p.contains("a second sub-workspace `a`"), "{p}");
    let p = problems(
        json!([
            { "id": "a", "name": "A", "nodes": ["exposure", "retouch"] },
            { "id": "b", "name": "B", "nodes": ["retouch.mask"] }
        ]),
        json!([]),
    );
    assert!(p.contains("share nodes"), "{p}");
    let p = problems(
        json!([
            { "id": "a", "name": "A", "nodes": ["retouch"] },
            { "id": "b", "name": "B", "nodes": ["exposure"], "parent": "a" }
        ]),
        json!([]),
    );
    assert!(p.contains("not in its parent"), "{p}");
    let p = problems(
        json!([{ "id": "a", "name": "A", "nodes": [], "parent": "z" }]),
        json!([]),
    );
    assert!(p.contains("no parent `z`"), "{p}");
    let p = problems(
        json!([
            { "id": "a", "name": "A", "nodes": [], "parent": "b" },
            { "id": "b", "name": "B", "nodes": [], "parent": "a" }
        ]),
        json!([]),
    );
    assert!(p.contains("cycle"), "{p}");
    let p = problems(
        json!([{ "id": "Not An Id", "name": " ", "nodes": [] }]),
        json!([]),
    );
    assert!(
        p.contains("is not an id") && p.contains("name empty"),
        "{p}"
    );
}

#[test]
fn delegating_nests_in_the_innermost_scope_and_adopts_the_ones_inside() {
    let s = state();
    let (s, face) = s
        .delegated("Face", &nodes(&["exposure", "retouch"]))
        .unwrap();
    assert_eq!(face["id"], json!("face"));
    assert_eq!(face["parent"], Json::Null);
    let (s, mask) = s.delegated("Mask", &nodes(&["retouch.mask"])).unwrap();
    assert_eq!(mask["parent"], json!("face"));
    // A scope over the composite, between the two: the mask moves in.
    let (s, retouch) = s.delegated("Retouch", &nodes(&["retouch"])).unwrap();
    assert_eq!(retouch["parent"], json!("face"));
    assert_eq!(retouch["adopted"], json!(["mask"]));
    let t = &s.bundle.template;
    assert_eq!(
        t.subworkspace("mask").unwrap().parent.as_deref(),
        Some("retouch")
    );
    assert!(
        s.problems(&catalogue()).is_empty(),
        "{:?}",
        s.problems(&catalogue())
    );
    // What the next part reads: a node's innermost scope, a scope's
    // nodes and its path from the top.
    assert_eq!(t.scope_of("retouch.mask").unwrap().id, "mask");
    assert_eq!(t.scope_of("retouch.blend").unwrap().id, "retouch");
    assert_eq!(t.scope_of("exposure").unwrap().id, "face");
    assert!(t.scope_of("source").is_none());
    assert_eq!(t.subworkspace_nodes("retouch").unwrap(), ["retouch"]);
    assert!(t.in_subworkspace("retouch.feather", "face"));
    assert!(!t.in_subworkspace("exposure", "retouch"));
    let path: Vec<&str> = t
        .subworkspace_path("mask")
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(path, ["face", "retouch", "mask"]);
    let top: Vec<&str> = t
        .subworkspace_children(None)
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(top, ["face"]);
    // Nodes crossing a scope's edge are refused, as are unknown nodes,
    // an empty name and no nodes.
    let (s2, _) = state()
        .delegated("Mask", &nodes(&["retouch.mask"]))
        .unwrap();
    let (_, both) = s2
        .delegated("Both", &nodes(&["retouch.mask", "retouch.feather"]))
        .unwrap();
    assert_eq!(both["adopted"], json!(["mask"]));
    let crossing = s
        .delegated("Cross", &nodes(&["retouch.mask", "exposure"]))
        .unwrap_err();
    assert!(crossing.contains("cross"), "{crossing}");
    assert!(s.delegated("X", &nodes(&["nowhere"])).is_err());
    assert!(s.delegated(" ", &nodes(&["exposure"])).is_err());
    assert!(s.delegated("X", &[]).is_err());
    assert!(s.delegated("X", &nodes(&["exposure", "exposure"])).is_err());
}

#[test]
fn deleting_moves_the_nested_ones_up_and_an_id_is_never_given_again() {
    let (s, _) = state()
        .delegated("Face", &nodes(&["exposure", "retouch"]))
        .unwrap();
    let (s, _) = s.delegated("Mask", &nodes(&["retouch.mask"])).unwrap();
    let (s, gone) = s.subworkspace_deleted("face").unwrap();
    assert_eq!(gone["moved"], json!(["mask"]));
    let t = &s.bundle.template;
    assert_eq!(t.subworkspace("mask").unwrap().parent, None);
    assert_eq!(t.retired_subworkspaces, ["face"]);
    // The nodes stay in the network.
    assert_eq!(
        t.network.nodes.len(),
        portrait().template.network.nodes.len()
    );
    assert!(s.subworkspace_deleted("face").is_err());
    // The same name again: a new id.
    let (s, again) = s.delegated("Face", &nodes(&["exposure"])).unwrap();
    assert_eq!(again["id"], json!("face-2"));
    // Deleted through an edit that leaves it out: retired too.
    let list = serde_json::to_value(&s.bundle.template.subworkspaces).unwrap();
    let kept: Vec<Json> = list
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["id"] != json!("face-2"))
        .cloned()
        .collect();
    let s = s.edited(&json!({ "subworkspaces": kept })).unwrap();
    assert_eq!(ids(&s), ["mask"]);
    assert_eq!(s.bundle.template.retired_subworkspaces, ["face", "face-2"]);
    // Given back by hand, a retired id is a problem.
    let back = s
        .edited(&json!({ "subworkspaces": [
            { "id": "face", "name": "Face", "nodes": ["exposure"] }
        ] }))
        .unwrap();
    assert!(
        back.problems(&catalogue()).join(" ").contains("retired"),
        "{:?}",
        back.problems(&catalogue())
    );
    assert_eq!(
        s.bundle.template.free_subworkspace_id("Face"),
        "face-3",
        "retired ids stay taken"
    );
}

#[test]
fn a_node_taken_out_of_the_network_leaves_every_scope() {
    let (s, _) = state()
        .delegated("Face", &nodes(&["exposure", "retouch"]))
        .unwrap();
    let (s, _) = s.delegated("Mask", &nodes(&["retouch.mask"])).unwrap();
    let mut net = s.bundle.template.network.clone();
    net.nodes.retain(|n| n.name != "retouch");
    net.connections
        .retain(|c| c.from != "retouch" && c.to != "retouch");
    net.view.node = "exposure".into();
    net.view.port = "output".into();
    net.presets.clear();
    let s = s.edited(&json!({ "network": net })).unwrap();
    let t = &s.bundle.template;
    assert_eq!(t.subworkspace_nodes("face").unwrap(), ["exposure"]);
    // The scope stays, empty, until it is deleted.
    assert!(t.subworkspace_nodes("mask").unwrap().is_empty());
    assert!(
        s.problems(&catalogue()).is_empty(),
        "{:?}",
        s.problems(&catalogue())
    );
    // A composite closed: its inner nodes leave too.
    let (s, _) = state()
        .delegated("Mask", &nodes(&["retouch.mask"]))
        .unwrap();
    let mut net = s.bundle.template.network.clone();
    net.nodes.iter_mut().for_each(|n| n.open = false);
    let s = s.edited(&json!({ "network": net })).unwrap();
    assert!(
        s.bundle
            .template
            .subworkspace_nodes("mask")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn delegate_and_delete_are_steps_with_dry_runs_and_undo() {
    let root = temp("steps");
    let config = root.join("config");
    let (dir, files) = made(&root, "portrait");
    let before = files.load().unwrap();
    let args = |extra: Json| {
        let mut a = json!({ "workspace": dir });
        for (k, v) in extra.as_object().unwrap() {
            a[k] = v.clone();
        }
        a
    };
    // A dry run answers what it would do and writes nothing.
    let dry = tools::delegate(
        &config,
        &args(json!({ "name": "Skin", "nodes": ["retouch"], "dry_run": true })),
        &catalogue(),
    )
    .unwrap();
    assert_eq!(dry["id"], json!("skin"));
    assert_eq!(dry["dry_run"], json!(true));
    assert_eq!(dry["problems"], json!([]));
    assert_eq!(files.load().unwrap(), before);
    // Applied: one step, written.
    tools::delegate(
        &config,
        &args(json!({ "name": "Skin", "nodes": ["retouch"] })),
        &catalogue(),
    )
    .unwrap();
    let delegated = files.load().unwrap();
    assert_eq!(ids(&delegated), ["skin"]);
    // Refused: nothing written.
    assert!(
        tools::delegate(
            &config,
            &args(json!({ "name": "Bad", "nodes": ["nowhere"] })),
            &catalogue(),
        )
        .is_err()
    );
    assert_eq!(files.load().unwrap(), delegated);
    // Deleted: a dry run first, then one step.
    let dry = tools::delete_subworkspace(
        &config,
        &args(json!({ "id": "skin", "dry_run": true })),
        &catalogue(),
    )
    .unwrap();
    assert_eq!(dry["name"], json!("Skin"));
    assert_eq!(files.load().unwrap(), delegated);
    tools::delete_subworkspace(&config, &args(json!({ "id": "skin" })), &catalogue()).unwrap();
    let deleted = files.load().unwrap();
    assert!(ids(&deleted).is_empty());
    assert_eq!(deleted.bundle.template.retired_subworkspaces, ["skin"]);
    // Each one undone, in turn.
    assert_eq!(
        tools::undo(&config, &args(json!({}))).unwrap()["undone"],
        json!("delete a sub-workspace")
    );
    assert_eq!(files.load().unwrap(), delegated);
    assert_eq!(
        tools::undo(&config, &args(json!({}))).unwrap()["undone"],
        json!("delegate nodes")
    );
    assert_eq!(files.load().unwrap(), before);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_reset_keeps_the_ids_retired() {
    let (s, _) = state().delegated("Skin", &nodes(&["retouch"])).unwrap();
    let reset = s.reset(&portrait(), &catalogue()).unwrap();
    assert!(ids(&reset).is_empty());
    assert_eq!(reset.bundle.template.retired_subworkspaces, ["skin"]);
    let (_, again) = reset.delegated("Skin", &nodes(&["retouch"])).unwrap();
    assert_eq!(again["id"], json!("skin-2"));
}

#[test]
fn an_import_copies_sub_workspaces_with_new_ids() {
    // The source: a sub-workspace `skin` with one nested in it.
    let (source, _) = state()
        .delegated("Skin", &nodes(&["exposure", "retouch"]))
        .unwrap();
    let (source, _) = source.delegated("Mask", &nodes(&["retouch.mask"])).unwrap();
    // The target has a `skin` of its own.
    let (target, _) = state().delegated("Skin", &nodes(&["exposure"])).unwrap();
    let picks = BTreeMap::from([("view".to_string(), Side::Ours)]);
    let merged = workspace::import(
        &target,
        &source.bundle,
        &[Part::Network],
        &picks,
        &catalogue(),
    )
    .unwrap();
    let t = &merged.state.bundle.template;
    assert_eq!(ids(&merged.state), ["skin", "skin-2", "mask"]);
    let copy = t.subworkspace("skin-2").unwrap();
    assert_eq!(copy.name, "Skin");
    assert_eq!(
        copy.nodes,
        nodes(&["portrait_retouch.exposure", "portrait_retouch.retouch"])
    );
    assert_eq!(copy.parent, None);
    let mask = t.subworkspace("mask").unwrap();
    assert_eq!(mask.nodes, nodes(&["portrait_retouch.retouch.mask"]));
    assert_eq!(mask.parent.as_deref(), Some("skin-2"));
    // The target's own is untouched, and the copy is checked as any.
    assert_eq!(t.subworkspace("skin").unwrap().nodes, nodes(&["exposure"]));
    assert!(
        merged.state.problems(&catalogue()).is_empty(),
        "{:?}",
        merged.state.problems(&catalogue())
    );
    let delegated: Vec<&Json> = merged
        .changes
        .iter()
        .filter(|c| c["change"] == json!("delegated"))
        .collect();
    assert_eq!(delegated.len(), 2);
}

#[test]
fn a_desktop_merge_moves_sub_workspaces_with_the_network() {
    let base = state();
    let (theirs, _) = base.delegated("Skin", &nodes(&["retouch"])).unwrap();
    let ours = base.edited(&json!({ "name": "Faces" })).unwrap();
    let merged = workspace::merge(&base, &ours, &theirs).unwrap();
    assert_eq!(merged.taken, ["network"]);
    assert_eq!(ids(&merged.state), ["skin"]);
    assert_eq!(merged.state.meta.name, "Faces");
}
