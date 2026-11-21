//! A plugin's declared controls (`[ui] rack`): the gphoto2-camera
//! example's capture controls (ISO, a choice, and the shutter speed, a
//! number) shown in its section of Settings > Plugins, drawn by the host,
//! its values in the store at `plugin/<id>/<key>`. A drag on its slider is
//! one undo step, in the plugins' history scope; the plugin is told each
//! committed change once (`ui/changed`), the undo too, and nothing for a
//! drag given up with Escape. A part naming an unknown control is left out
//! with its reason, and the rest of the card stays.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::{Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_gui::{Shell, headless, helios_panels};
use helios_plugin::{Broker, Policy, Scope};
use helios_shell::commands::Origin;
use helios_values::HistoryScope;
use serde_json::{Value as Json, json};

const CAMERA: &str = "io.github.quazardous.helios.gphoto2-camera";
const SECTION: &str = "plugins/io-github-quazardous-helios-gphoto2-camera";
const SHUTTER: &str = "plugin/io.github.quazardous.helios.gphoto2-camera/shutter";

/// Stands in for the plugin's program: answers `device.list` with no
/// camera, and writes each notification it is sent, but `initialized`, to
/// `told`, one JSON per line.
const STUB: &str = r#"
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        if m.get("method") == "exit":
            break
        if m.get("method") != "initialized":
            with open("told", "a") as f:
                f.write(json.dumps(m) + "\n")
        continue
    r = {"jsonrpc": "2.0", "id": m["id"]}
    method = m.get("method")
    if method == "initialize":
        r["result"] = {"name": "stub"}
    elif method == "device.list":
        r["result"] = {"devices": []}
    else:
        r["result"] = None
    sys.stdout.write(json.dumps(r) + "\n")
    sys.stdout.flush()
"#;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-plugin-controls-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The gphoto2-camera example's folder as it ships (manifest, controls,
/// catalogues), its program swapped for [`STUB`]; `controls` edits its
/// declaration.
fn camera(dir: &Path, controls: impl Fn(&mut Json)) -> PathBuf {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/examples/gphoto2-camera");
    let to = dir.join("gphoto2-camera");
    for file in ["i18n/en/plugin.ftl", "i18n/fr/plugin.ftl"] {
        std::fs::create_dir_all(to.join(file).parent().unwrap()).unwrap();
        std::fs::copy(from.join(file), to.join(file)).unwrap();
    }
    let mut decl: Json =
        serde_json::from_str(&std::fs::read_to_string(from.join("ui/controls.json")).unwrap())
            .unwrap();
    controls(&mut decl);
    std::fs::create_dir_all(to.join("ui")).unwrap();
    std::fs::write(to.join("ui/controls.json"), decl.to_string()).unwrap();
    let manifest = std::fs::read_to_string(from.join("helios-plugin.toml"))
        .unwrap()
        .replace(
            r#"command = ["gphoto2-camera"]"#,
            r#"command = ["python3", "stub.py"]"#,
        );
    std::fs::write(to.join("helios-plugin.toml"), manifest).unwrap();
    std::fs::write(to.join("stub.py"), STUB).unwrap();
    to
}

fn shell(broker: &Arc<Broker>) -> Harness<'static, Shell> {
    let mut shell = Shell::builder(helios_panels(), None)
        .plugins(broker)
        .build();
    shell.set_plugins(broker.clone());
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    h.run();
    h.state_mut()
        .run("admin.open", json!({ "path": SECTION }), Origin::User)
        .unwrap();
    h.run();
    h.run();
    h
}

/// The `ui/changed` notifications the plugin got, in order.
fn told(dir: &Path) -> Vec<Json> {
    std::fs::read_to_string(dir.join("told"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Json>(l).ok())
        .filter(|m| m["method"] == "ui/changed")
        .collect()
}

/// Runs passes until the plugin got `n` notifications, at most 10 s,
/// then a little longer to see none more comes.
fn told_n(h: &mut Harness<'_, Shell>, dir: &Path, n: usize) -> Vec<Json> {
    let end = Instant::now() + Duration::from_secs(10);
    while told(dir).len() < n && Instant::now() < end {
        h.run();
        std::thread::sleep(Duration::from_millis(20));
    }
    for _ in 0..10 {
        h.run();
        std::thread::sleep(Duration::from_millis(20));
    }
    let t = told(dir);
    assert_eq!(t.len(), n, "{t:#?}");
    t
}

fn press(h: &Harness<'_, Shell>, at: Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

fn shutter(h: &Harness<'_, Shell>) -> f64 {
    h.state()
        .values()
        .lock()
        .unwrap()
        .get(SHUTTER)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

fn plugin_steps(h: &Harness<'_, Shell>) -> usize {
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo_steps_of(HistoryScope::Plugin)
        .len()
}

/// Presses on the shutter's slider and moves right in `moves` frames.
fn drag_start(h: &mut Harness<'_, Shell>, moves: usize) -> Pos2 {
    let from = h
        .get_by_role_and_label(Role::Slider, "Shutter speed")
        .rect()
        .center();
    h.hover_at(from);
    h.run();
    press(h, from, true);
    h.run();
    for k in 1..=moves {
        h.hover_at(from + vec2(15.0 * k as f32, 0.0));
        h.run();
    }
    from + vec2(15.0 * moves as f32, 0.0)
}

#[test]
fn the_camera_s_controls_are_drawn_by_the_host_and_told_once_per_change() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 is not installed: skipped");
        return;
    }
    let dir = temp("camera");
    let plugin = camera(&dir, |_| {});
    let mut broker = Broker::new(Policy::default());
    broker.add(&plugin, Scope::User).unwrap();
    let broker = Arc::new(broker);
    broker.grant(CAMERA, "device.usb").unwrap();
    broker.grant(CAMERA, "process.spawn").unwrap();
    let mut h = shell(&broker);

    // Its card: the ISO menu and the shutter's slider, by their labels.
    for shown in ["Capture", "ISO", "Shutter speed"] {
        assert!(
            h.query_all_by_label_contains(shown).next().is_some(),
            "{shown} on the page\n{:#?}",
            h.root()
        );
    }
    let start = shutter(&h);
    assert_eq!(start, 0.008);
    // Running, it was handed its values at `initialize`.
    broker.device_list().unwrap();

    // A drag: one undo step, in the plugins' scope; told once.
    let to = drag_start(&mut h, 4);
    assert!(told(&plugin).is_empty(), "nothing told during the drag");
    press(&h, to, false);
    h.run();
    let moved = shutter(&h);
    assert!(moved > start, "the drag moved it: {moved}");
    assert_eq!(plugin_steps(&h), 1, "one step for the whole drag");
    let t = told_n(&mut h, &plugin, 1);
    assert_eq!(t[0]["params"]["paths"], json!([SHUTTER]));
    assert_eq!(t[0]["params"]["values"]["shutter"], json!(moved));
    assert_eq!(t[0]["params"]["cause"], "edit");

    // A drag given up (Escape, which the rack turns into the store's
    // cancel, as `params::escape_gives_a_drag_up_without_a_trace` shows):
    // its frames and its cancel tell nothing. Pressed inside the Settings
    // app, Escape is that app's own (it dismisses the page), so the
    // gesture is driven here as the rack drives it.
    {
        let values = h.state().values().clone();
        let mut s = values.lock().unwrap();
        let g = s.begin_gesture(Origin::User, "drag shutter");
        for x in [0.5, 1.0] {
            s.set(
                helios_values::Set::new(
                    SHUTTER,
                    helios_values::Change::To(helios_core::Value::Float(x)),
                    Origin::User,
                )
                .gesture(g),
            )
            .unwrap();
        }
        s.cancel(g);
    }
    assert_eq!(shutter(&h), moved, "put back");
    assert_eq!(plugin_steps(&h), 1, "no step");
    told_n(&mut h, &plugin, 1);

    // The undo: told once, the value before the drag.
    h.state()
        .values()
        .lock()
        .unwrap()
        .in_history(HistoryScope::Plugin, |s| s.undo(&Origin::User))
        .unwrap();
    let t = told_n(&mut h, &plugin, 2);
    assert_eq!(t[1]["params"]["values"]["shutter"], json!(start));
    assert_eq!(t[1]["params"]["cause"], "undo");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unknown_control_is_left_out_and_the_rest_of_the_card_stays() {
    let dir = temp("unknown");
    let plugin = camera(&dir, |d| {
        d["case"]["items"][0]["part"]["control"] = json!("hologram");
    });
    let mut broker = Broker::new(Policy::default());
    broker.add(&plugin, Scope::User).unwrap();
    let h = shell(&Arc::new(broker));
    h.get_by_role_and_label(Role::Slider, "Shutter speed");
    let page = format!("{:#?}", h.root());
    for said in ["not shown", "part `iso`: unknown control `hologram`"] {
        assert!(page.contains(said), "{said}\n{page}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
