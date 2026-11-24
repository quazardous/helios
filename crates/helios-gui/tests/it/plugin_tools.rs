//! A plugin's tool on the image (`[ui] rack`, its `tools`): the
//! gphoto2-camera example's focus point, a point handle on its own
//! `focus-x`, `focus-y` values, declared as `egui_hud` handle data and
//! drawn by the host over the open photo while its `focus-show` switch
//! is on. A drag is one undo step in the plugins' scope, told to the
//! plugin once (`ui/changed`); Escape gives a drag up with no step and
//! nothing told; the undo puts the point back and is told too. A handle
//! of a kind this Helios does not know is left out with its reason on
//! the plugin's Settings section, the rest of the tool kept.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::{Shell, headless, helios_panels};
use helios_plugin::{Broker, Policy, Scope};
use helios_shell::commands::Origin;
use helios_values::{Change, HistoryScope, Set};
use serde_json::{Value as Json, json};

const CAMERA: &str = "io.github.quazardous.helios.gphoto2-camera";
const SHOW: &str = "plugin/io.github.quazardous.helios.gphoto2-camera/focus-show";
const X: &str = "plugin/io.github.quazardous.helios.gphoto2-camera/focus-x";
const Y: &str = "plugin/io.github.quazardous.helios.gphoto2-camera/focus-y";

/// Stands in for the plugin's program: answers its requests, and writes
/// each notification it is sent to `told`, one JSON per line.
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
    let dir =
        std::env::temp_dir().join(format!("helios-plugin-tools-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The gphoto2-camera example's folder as it ships, its program swapped
/// for [`STUB`]; `edit` changes its declaration.
fn camera(dir: &Path, edit: impl Fn(&mut Json)) -> PathBuf {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/examples/gphoto2-camera");
    let to = dir.join("gphoto2-camera");
    for file in ["i18n/en/plugin.ftl", "i18n/fr/plugin.ftl"] {
        std::fs::create_dir_all(to.join(file).parent().unwrap()).unwrap();
        std::fs::copy(from.join(file), to.join(file)).unwrap();
    }
    let mut decl: Json =
        serde_json::from_str(&std::fs::read_to_string(from.join("ui/controls.json")).unwrap())
            .unwrap();
    edit(&mut decl);
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

const W: u32 = 1200;
const H: u32 = 900;

/// A photo open in the viewport, the camera plugin enabled.
fn shell(broker: &Arc<Broker>) -> Harness<'static, Shell> {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, |x, y, c| match c {
            0..=2 => 0.05 + 0.3 * (x + y) as f32 / (W + H) as f32,
            _ => 1.0,
        }),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_modules::exposure::Exposure),
            &Params::new().with("exposure", Value::Float(0.0)),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    let mut shell = Shell::builder(helios_panels(), None)
        .plugins(broker)
        .build();
    shell.set_plugins(broker.clone());
    shell.set_view_source(g, exp, "output").unwrap();
    egui_pop::api::palettes::set_hidden(shell.app().handle(), true);
    let mut h = headless::harness_at(shell, vec2(1280.0, 800.0), 1.0);
    h.step();
    h.step();
    h
}

fn f(h: &Harness<'_, Shell>, path: &str) -> f64 {
    let s = h.state().values().lock().unwrap();
    s.get(path).unwrap().value.as_f64().unwrap()
}

fn plugin_steps(h: &Harness<'_, Shell>) -> usize {
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo_steps_of(HistoryScope::Plugin)
        .len()
}

/// Where the focus handle is on screen, if the tool is shown.
fn handle(h: &Harness<'_, Shell>) -> Option<Pos2> {
    let v = h.state().viewport();
    let o = v.plugin_tools().overlays().first()?;
    let k = o.handles.first()?;
    Some(v.state().mapping()?.image_to_screen(k.at))
}

fn button(h: &mut Harness<'_, Shell>, pos: Pos2, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

/// Presses at `from` and moves to `to`; the button stays down.
fn drag_to(h: &mut Harness<'_, Shell>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.step();
    button(h, from, true);
    h.step();
    for i in 1..=5 {
        h.hover_at(from + (to - from) * (i as f32 / 5.0));
        h.step();
    }
}

fn release(h: &mut Harness<'_, Shell>, at: Pos2) {
    button(h, at, false);
    h.step();
    h.step();
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

/// Runs frames until the plugin got `n` notifications, at most 10 s,
/// then a little longer to see none more comes.
fn told_n(h: &mut Harness<'_, Shell>, dir: &Path, n: usize) -> Vec<Json> {
    let end = Instant::now() + Duration::from_secs(10);
    while told(dir).len() < n && Instant::now() < end {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    for _ in 0..10 {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    let t = told(dir);
    assert_eq!(t.len(), n, "{t:#?}");
    t
}

#[test]
fn the_camera_s_focus_point_is_dragged_on_the_image_as_one_step_told_once() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 is not installed: skipped");
        return;
    }
    let dir = temp("focus");
    let plugin = camera(&dir, |_| {});
    let mut broker = Broker::new(Policy::default());
    broker.add(&plugin, Scope::User).unwrap();
    let broker = Arc::new(broker);
    broker.grant(CAMERA, "device.usb").unwrap();
    broker.grant(CAMERA, "process.spawn").unwrap();
    let mut h = shell(&broker);
    broker.device_list().unwrap();

    // Its switch off: nothing over the photo.
    assert!(handle(&h).is_none(), "hidden while `focus-show` is off");
    h.state()
        .values()
        .lock()
        .unwrap()
        .set(Set::new(SHOW, Change::To(Value::Bool(true)), Origin::User))
        .unwrap();
    h.step();
    h.step();
    let from = handle(&h).expect("shown once `focus-show` is on");
    told_n(&mut h, &plugin, 1);
    let (x0, y0) = (f(&h, X), f(&h, Y));
    assert_eq!((x0, y0), (500.0, 500.0));

    // A drag: both values move, one undo step in the plugins' scope,
    // the plugin told once, after the drag only.
    let to = from + vec2(60.0, -40.0);
    drag_to(&mut h, from, to);
    assert_ne!(f(&h, X), x0, "live while dragging");
    assert_eq!(told(&plugin).len(), 1, "nothing told during the drag");
    release(&mut h, to);
    let (x1, y1) = (f(&h, X), f(&h, Y));
    assert!(x1 > x0 && y1 < y0, "moved: {x1}, {y1}");
    assert_eq!(plugin_steps(&h), 2, "the switch, then the drag");
    assert!(
        h.state().values().lock().unwrap().undo_steps().is_empty(),
        "nothing on the image's line"
    );
    let t = told_n(&mut h, &plugin, 2);
    assert_eq!(t[1]["params"]["paths"], json!([X, Y]));
    assert_eq!(t[1]["params"]["values"]["focus-x"], json!(x1));
    assert_eq!(t[1]["params"]["cause"], "edit");

    // Escape mid-drag: back where it was, no step, nothing told.
    let from = handle(&h).unwrap();
    drag_to(&mut h, from, from + vec2(-80.0, 0.0));
    assert_ne!(f(&h, X), x1);
    h.key_press(Key::Escape);
    h.step();
    release(&mut h, from + vec2(-80.0, 0.0));
    assert_eq!((f(&h, X), f(&h, Y)), (x1, y1));
    assert_eq!(plugin_steps(&h), 2);
    told_n(&mut h, &plugin, 2);

    // The undo: the point back, told as such.
    h.state()
        .values()
        .lock()
        .unwrap()
        .in_history(HistoryScope::Plugin, |s| s.undo(&Origin::User))
        .unwrap();
    h.step();
    assert_eq!((f(&h, X), f(&h, Y)), (x0, y0));
    let t = told_n(&mut h, &plugin, 3);
    assert_eq!(t[2]["params"]["cause"], "undo");
    assert_eq!(t[2]["params"]["values"]["focus-x"], json!(x0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unknown_handle_kind_is_left_out_and_said_on_the_plugin_s_section() {
    let dir = temp("unknown");
    let plugin = camera(&dir, |d| {
        let handles = d["tools"][0]["handles"]["handles"].as_array_mut().unwrap();
        handles.push(json!({"name": "spiral", "kind": "spiral", "turns": 2}));
    });
    let mut broker = Broker::new(Policy::default());
    broker.add(&plugin, Scope::User).unwrap();
    let broker = Arc::new(broker);
    let mut h = shell(&broker);
    h.state()
        .values()
        .lock()
        .unwrap()
        .set(Set::new(SHOW, Change::To(Value::Bool(true)), Origin::User))
        .unwrap();
    h.step();
    h.step();
    // The point is kept; the spiral is not drawn.
    let kept = h.state().viewport().plugin_tools().overlays()[0]
        .handles
        .len();
    assert_eq!(kept, 1);
    // Said on the plugin's Settings section.
    h.state_mut()
        .run(
            "admin.open",
            json!({ "path": "plugins/io-github-quazardous-helios-gphoto2-camera" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    h.run();
    let page = format!("{:#?}", h.root());
    assert!(
        page.contains("handle `spiral`: unknown kind `spiral`"),
        "{page}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
