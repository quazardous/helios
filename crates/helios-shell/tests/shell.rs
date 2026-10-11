//! The shell's logic, headless: commands, keys, panels, layouts.

use helios_shell::commands::{CommandSpec, Origin, Registry};
use helios_shell::keymap::{Keymap, Keys, Layer, Resolution};
use helios_shell::layout::{Auxiliary, Layout, Node};
use helios_shell::panels::{Area, PanelSpec, Panels};
use helios_shell::when::Context;
use serde_json::{Value, json};

#[test]
fn commands_run_by_id_from_any_origin_and_are_recorded() {
    let mut r = Registry::new();
    r.register(
        CommandSpec::new("culling.rate", "Rate", "Culling").when("gridFocus"),
        |args, _| Ok(args["stars"].clone()),
    )
    .unwrap();
    assert!(
        r.register(CommandSpec::new("culling.rate", "Again", "x"), |_, _| Ok(
            Value::Null
        ))
        .is_err()
    );
    let grid = Context::new().flag("gridFocus", true);
    assert_eq!(
        r.run("culling.rate", json!({"stars": 3}), Origin::User, &grid),
        Ok(json!(3))
    );
    let agent = Origin::Agent("session-1".into());
    let err = r
        .run("culling.rate", json!({}), agent.clone(), &Context::new())
        .unwrap_err();
    assert!(err.contains("does not apply"), "{err}");
    assert!(r.run("nope", Value::Null, agent.clone(), &grid).is_err());
    assert_eq!(r.history().len(), 2, "unknown commands are not runs");
    assert_eq!(r.history()[1].origin, agent);
    assert_eq!(r.search("rate cull", &grid).len(), 1);
    assert!(
        r.search("rate", &Context::new()).is_empty(),
        "not offered where it does not apply"
    );
}

fn keys(k: &str) -> Keys {
    Keys::parse(k).unwrap()
}

#[test]
fn layers_contexts_and_removal_decide_what_a_key_does() {
    let mut k = Keymap::new();
    k.bind(
        "Ctrl+G",
        "image.group",
        "gridFocus",
        Layer::Default,
        "framework",
    )
    .unwrap();
    k.bind(
        "Ctrl+G",
        "graph.group",
        "graphFocus",
        Layer::Default,
        "framework",
    )
    .unwrap();
    k.bind("Ctrl+G", "gamut.check", "", Layer::Suggestion, "darkroom")
        .unwrap();
    k.bind(
        "E",
        "exposure.focus",
        "!textInputFocus",
        Layer::Suggestion,
        "exposure",
    )
    .unwrap();
    let graph = Context::new().flag("graphFocus", true);
    let grid = Context::new().flag("gridFocus", true);
    let elsewhere = Context::new();
    // The context picks among defaults; a suggestion only fills the gap.
    assert_eq!(
        k.resolve(&keys("Ctrl+G"), &graph),
        Resolution::Run("graph.group".into())
    );
    assert_eq!(
        k.resolve(&keys("Ctrl+G"), &grid),
        Resolution::Run("image.group".into())
    );
    assert_eq!(
        k.resolve(&keys("Ctrl+G"), &elsewhere),
        Resolution::Run("gamut.check".into())
    );
    // A single letter never fires while typing.
    let typing = Context::new().flag("textInputFocus", true);
    assert_eq!(k.resolve(&keys("E"), &typing), Resolution::Nothing);
    // The user rebinds, and removes a default for good.
    k.bind("Ctrl+G", "my.macro", "", Layer::User, "user")
        .unwrap();
    assert_eq!(
        k.resolve(&keys("Ctrl+G"), &graph),
        Resolution::Run("my.macro".into())
    );
    k.bind("Ctrl+G", "-image.group", "", Layer::User, "user")
        .unwrap();
    assert!(k.keys_for("image.group").is_empty());
    // Conflicts are listed, losing suggestions included.
    let lost: Vec<String> = k
        .conflicts()
        .iter()
        .map(|(lose, _)| lose.command.clone())
        .collect();
    assert!(lost.contains(&"gamut.check".to_string()), "{lost:?}");
}

#[test]
fn chords_wait_for_their_second_key_and_keys_are_logical() {
    let mut k = Keymap::new();
    k.bind(
        "Ctrl+K Ctrl+S",
        "keys.editor",
        "",
        Layer::Default,
        "framework",
    )
    .unwrap();
    k.bind("3", "culling.rate.3", "", Layer::Default, "framework")
        .unwrap();
    let cx = Context::new();
    assert_eq!(k.resolve(&keys("Ctrl+K"), &cx), Resolution::Pending);
    assert_eq!(
        k.resolve(&keys("Ctrl+K Ctrl+S"), &cx),
        Resolution::Run("keys.editor".into())
    );
    assert_eq!(k.resolve(&keys("Ctrl+K Ctrl+X"), &cx), Resolution::Nothing);
    assert_eq!(
        k.resolve(&keys("Numpad3"), &cx),
        Resolution::Run("culling.rate.3".into())
    );
    assert_eq!(keys("ctrl+shift+p").to_string(), "Ctrl+Shift+P");
    assert!(Keys::parse("Hyper+P").is_err());
}

fn panels() -> Panels {
    let mut p = Panels::new();
    for (id, area) in [
        ("helios.viewport", Area::Editors),
        ("helios.graph", Area::BottomPanel),
        ("helios.params", Area::SecondarySideBar),
        ("helios.vault", Area::PrimarySideBar),
    ] {
        p.declare(PanelSpec {
            id: id.into(),
            title: id.trim_start_matches("helios.").into(),
            area,
            areas: vec![],
            commands: vec![],
        })
        .unwrap();
    }
    p
}

#[test]
fn a_layout_round_trips_and_is_repaired_against_panels_and_screens() {
    let panels = panels();
    assert_eq!(
        panels.shell_commands().len(),
        8,
        "focus and toggle per panel"
    );
    let mut layout = Layout::default_for(&panels);
    assert_eq!(layout.placed().len(), 4);
    // The user detaches the graph to a second screen.
    for (_, node) in &mut layout.areas {
        if let Node::Tabs { panels, .. } = node {
            panels.retain(|p| p != "helios.graph");
        }
    }
    layout
        .areas
        .retain(|(_, n)| !matches!(n, Node::Tabs { panels, .. } if panels.is_empty()));
    layout.auxiliary.push(Auxiliary {
        panel: "helios.graph".into(),
        screen: "HDMI-1".into(),
        rect: [0.0, 0.0, 800.0, 600.0],
    });
    let saved = Layout::from_json(&layout.to_json()).unwrap();
    assert_eq!(saved, layout);
    // Same screens: unchanged.
    let (same, notes) = saved.clone().repair(&panels, &["eDP-1", "HDMI-1"]);
    assert_eq!((same, notes.len()), (layout.clone(), 0));
    // The second screen is gone, a panel disappeared, a new one appeared.
    let mut next = Panels::new();
    for p in panels.all().iter().filter(|p| p.id != "helios.vault") {
        next.declare(p.clone()).unwrap();
    }
    next.declare(PanelSpec {
        id: "helios.queue".into(),
        title: "queue".into(),
        area: Area::BottomPanel,
        areas: vec![],
        commands: vec![],
    })
    .unwrap();
    let (fixed, notes) = saved.repair(&next, &["eDP-1"]);
    let placed = fixed.placed();
    assert!(placed.contains(&"helios.graph") && placed.contains(&"helios.queue"));
    assert!(!placed.contains(&"helios.vault"));
    assert!(fixed.auxiliary.is_empty());
    assert_eq!(notes.len(), 3, "{notes:?}");
}

#[test]
fn a_closed_panel_stays_closed_when_the_layout_is_repaired() {
    let mut layout = Layout::default_for(&panels());
    let first = layout.placed()[0].to_string();
    for (_, node) in &mut layout.areas {
        if let Node::Tabs { panels, .. } = node {
            panels.retain(|p| *p != first);
        }
    }
    layout
        .areas
        .retain(|(_, n)| !matches!(n, Node::Tabs { panels, .. } if panels.is_empty()));
    layout.hidden.push(first.clone());
    let (repaired, notes) = Layout::from_json(&layout.to_json())
        .unwrap()
        .repair(&panels(), &["primary"]);
    assert!(!repaired.placed().contains(&first.as_str()), "{notes:?}");
    assert_eq!(repaired.hidden, [first]);
}

#[test]
fn a_panel_found_where_it_may_not_be_goes_back_to_its_default_area() {
    let mut p = Panels::new();
    p.declare(PanelSpec {
        id: "helios.viewport".into(),
        title: "Image".into(),
        area: Area::Editors,
        areas: vec![Area::Editors],
        commands: vec![],
    })
    .unwrap();
    p.declare(PanelSpec {
        id: "helios.log".into(),
        title: "Log".into(),
        area: Area::BottomPanel,
        areas: vec![],
        commands: vec![],
    })
    .unwrap();
    assert!(p.get("helios.log").unwrap().allows(Area::Editors));
    assert!(
        !p.get("helios.viewport")
            .unwrap()
            .allows(Area::PrimarySideBar)
    );
    let saved = Layout {
        areas: vec![(
            Area::PrimarySideBar,
            Node::Tabs {
                panels: vec!["helios.viewport".into(), "helios.log".into()],
                active: 0,
            },
        )],
        auxiliary: vec![],
        hidden: vec![],
    };
    let (fixed, notes) = saved.repair(&p, &[]);
    assert_eq!(
        fixed.areas,
        [
            (
                Area::Editors,
                Node::Tabs {
                    panels: vec!["helios.viewport".into()],
                    active: 0
                }
            ),
            (
                Area::PrimarySideBar,
                Node::Tabs {
                    panels: vec!["helios.log".into()],
                    active: 0
                }
            ),
        ]
    );
    assert_eq!(
        notes,
        ["`helios.viewport` cannot be in the primary side bar: moved to its default area"]
    );
}

#[test]
fn menus_are_declared_grouped_and_follow_the_context() {
    use helios_shell::menus::{Item, MenuItem, Menus, Shown};
    let mut m = Menus::new();
    m.contribute(MenuItem::command("system", "9_quit", 0, "window.close"))
        .unwrap();
    m.contribute(MenuItem::command("system", "1_open", 2, "workspace.recent"))
        .unwrap();
    m.contribute(MenuItem::command("system", "1_open", 1, "workspace.open"))
        .unwrap();
    m.contribute(
        MenuItem::command("system", "1_open", 3, "palette.only").when("paletteVisible", false),
    )
    .unwrap();
    m.contribute(
        MenuItem::command("system", "5_view", 0, "workbench.fullscreen")
            .when("paletteVisible", true)
            .checked("fullScreen"),
    )
    .unwrap();
    assert!(
        m.contribute(MenuItem::command("system", "x", 0, "y").when("a &&", true))
            .is_err()
    );
    let id = |s: &Shown| match s {
        Shown::Item {
            item: Item::Command { id, .. },
            ..
        } => id.clone(),
        Shown::Separator => "-".into(),
        _ => "?".into(),
    };
    let cx = Context::new().flag("fullScreen", true);
    let shown = m.show("system", &cx);
    let ids: Vec<String> = shown.iter().map(id).collect();
    // The palette-only item is hidden; full screen greyed but ticked.
    assert_eq!(
        ids,
        [
            "workspace.open",
            "workspace.recent",
            "-",
            "workbench.fullscreen",
            "-",
            "window.close"
        ]
    );
    assert!(matches!(
        shown[3],
        Shown::Item {
            enabled: false,
            checked: true,
            ..
        }
    ));
    assert_eq!(m.ids(), ["system"]);
}
