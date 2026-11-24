//! A plugin's tool on the image (`[ui] rack`, its `tools`): the
//! gphoto2-camera example's focus point, a point handle on its own
//! `focus-x`, `focus-y` values, declared as `egui_hud` handle data and
//! drawn by the host over the open photo while its `focus-show` switch
//! is on. A drag is one undo step in the plugins' scope, told to the
//! plugin once (`ui/changed`); Escape gives a drag up with no step and
//! nothing told; the undo puts the point back and is told too. A handle
//! of a kind this Helios does not know is left out with its reason on
//! the plugin's Settings section, the rest of the tool kept.
//!
//! A tool on the host's own parameters (`"module": "mask_circle"`) shows
//! on the photo's circle only once the plugin holds `edit.controls`,
//! declared and granted; until then its refusal is on the plugin's
//! section. Its drag is one undo step on the photo's own line. Shift
//! snaps a plugin's handle as it snaps a mask's.

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

/// The camera with a tool on the photo's circle masks (`spot`, their
/// centre), and `edit.controls` declared when `declares`.
fn spotter(dir: &Path, declares: bool) -> PathBuf {
    let plugin = camera(dir, |d| {
        d["tools"].as_array_mut().unwrap().push(json!({
            "name": "spot",
            "label": "io-github-quazardous-helios-gphoto2-camera-focus",
            "module": "mask_circle",
            "handles": {"format": "egui_hud-handles/1", "handles": [
                {"name": "centre", "kind": "point", "look": "move"}
            ]},
        }));
    });
    if declares {
        let m = plugin.join("helios-plugin.toml");
        let text = std::fs::read_to_string(&m).unwrap()
            + "\n[[capabilities]]\nname = \"edit.controls\"\nreason = \"Places circle masks on the focus point\"\n";
        std::fs::write(m, text).unwrap();
    }
    plugin
}

/// A photo open in the viewport, the camera plugin enabled.
fn shell(broker: &Arc<Broker>) -> Harness<'static, Shell> {
    shell_with(broker, false)
}

/// The same, a circle mask `circle` in the photo's graph when `circle`.
fn shell_with(broker: &Arc<Broker>, circle: bool) -> Harness<'static, Shell> {
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
    if circle {
        g.add_module(
            "circle",
            ModuleRef::new(helios_modules::masks::MaskCircle),
            &Params::new()
                .with("x", Value::Float(600.0))
                .with("y", Value::Float(450.0))
                .with("radius", Value::Float(100.0))
                .with("feather", Value::Float(20.0)),
        )
        .unwrap();
    }
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

/// Where the spot tool's handle on node `node` is on screen, if shown.
fn spot(h: &Harness<'_, Shell>, node: &str) -> Option<Pos2> {
    let v = h.state().viewport();
    let id = format!("plugin-tool:{CAMERA}/spot/{node}");
    let o = v.plugin_tools().overlays().iter().find(|o| o.id == id)?;
    let k = o.handles.first()?;
    Some(v.state().mapping()?.image_to_screen(k.at))
}

fn button(h: &mut Harness<'_, Shell>, pos: Pos2, pressed: bool, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    });
}

/// Presses at `from` and moves to `to`; the button stays down.
fn drag_to(h: &mut Harness<'_, Shell>, from: Pos2, to: Pos2) {
    drag_with(h, from, to, Modifiers::NONE);
}

/// The same, `modifiers` held.
fn drag_with(h: &mut Harness<'_, Shell>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.input_mut()
        .events
        .push(Event::ModifiersChanged(modifiers));
    h.hover_at(from);
    h.step();
    button(h, from, true, modifiers);
    h.step();
    for i in 1..=5 {
        h.input_mut()
            .events
            .push(Event::ModifiersChanged(modifiers));
        h.hover_at(from + (to - from) * (i as f32 / 5.0));
        h.step();
    }
}

fn release(h: &mut Harness<'_, Shell>, at: Pos2) {
    button(h, at, false, Modifiers::NONE);
    h.step();
    h.input_mut()
        .events
        .push(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

/// The plugin's Settings section, as the accessibility tree says it.
fn section(h: &mut Harness<'_, Shell>) -> String {
    h.state_mut()
        .run(
            "admin.open",
            json!({ "path": "plugins/io-github-quazardous-helios-gphoto2-camera" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    h.run();
    format!("{:#?}", h.root())
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
    let page = section(&mut h);
    assert!(
        page.contains("handle `spiral`: unknown kind `spiral`"),
        "{page}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tool_on_the_photo_s_circle_moves_it_only_once_edit_controls_is_granted() {
    let dir = temp("host");
    let plugin = spotter(&dir, true);
    let mut broker = Broker::new(Policy::default());
    broker.add(&plugin, Scope::User).unwrap();
    let broker = Arc::new(broker);
    let mut h = shell_with(&broker, true);

    // Not granted: not shown.
    assert!(spot(&h, "circle").is_none(), "hidden until granted");

    // Granted: on the circle, at its centre.
    broker.grant(CAMERA, "edit.controls").unwrap();
    h.step();
    h.step();
    let from = spot(&h, "circle").expect("shown once granted");
    let (x0, y0) = (f(&h, "node/circle/x"), f(&h, "node/circle/y"));
    let image = h.state().values().lock().unwrap().undo_steps().len();

    // A drag moves the photo's circle: one step on the photo's line,
    // none on the plugins'.
    let to = from + vec2(50.0, 30.0);
    drag_to(&mut h, from, to);
    release(&mut h, to);
    let (x1, y1) = (f(&h, "node/circle/x"), f(&h, "node/circle/y"));
    assert!(x1 > x0 && y1 > y0, "moved: {x1}, {y1}");
    assert_eq!(
        h.state().values().lock().unwrap().undo_steps().len(),
        image + 1,
        "one step on the photo's line"
    );
    assert_eq!(plugin_steps(&h), 0, "nothing on the plugins' line");

    // The photo's undo puts it back.
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    h.step();
    assert_eq!((f(&h, "node/circle/x"), f(&h, "node/circle/y")), (x0, y0));

    // Revoked: gone at once.
    broker.revoke(CAMERA, "edit.controls").unwrap();
    h.step();
    h.step();
    assert!(spot(&h, "circle").is_none(), "hidden once revoked");
    // Said on the plugin's section.
    let page = section(&mut h);
    assert!(
        page.contains("tool `spot` on `mask_circle`")
            && page.contains("needs the capability `edit.controls`, not granted"),
        "{page}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tool_on_the_host_s_parameters_without_edit_controls_is_refused() {
    let dir = temp("undeclared");
    let plugin = spotter(&dir, false);
    let mut broker = Broker::new(Policy::default());
    broker.add(&plugin, Scope::User).unwrap();
    let broker = Arc::new(broker);
    assert!(broker.grant(CAMERA, "edit.controls").is_err());
    let mut h = shell_with(&broker, true);
    assert!(spot(&h, "circle").is_none());
    let page = section(&mut h);
    assert!(
        page.contains("does not declare the capability `edit.controls`"),
        "{page}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shift_holds_a_plugin_s_point_to_the_axes_as_a_mask_s() {
    let dir = temp("shift");
    let plugin = camera(&dir, |_| {});
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
    let from = handle(&h).expect("shown");
    let (x0, y0) = (f(&h, X), f(&h, Y));
    // Mostly across, a little down: Shift keeps it on the row.
    let to = from + vec2(80.0, 9.0);
    drag_with(&mut h, from, to, Modifiers::SHIFT);
    release(&mut h, to);
    let (x1, y1) = (f(&h, X), f(&h, Y));
    assert!(x1 > x0, "moved across: {x1}");
    assert!((y1 - y0).abs() < 1e-6, "held on its row: {y0} -> {y1}");
    let _ = std::fs::remove_dir_all(&dir);
}
