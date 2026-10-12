//! The shell at every scale egui-pop's test kit knows: the layout stays
//! in points, the pictures follow the physical pixels, the interface zoom
//! is egui-pop's, a `hidpi` profile variant follows the screen, and a
//! detached panel's window follows its own screen's scale.

use egui::{Key, Modifiers, vec2};
use egui_kittest::kittest::Queryable;
use egui_pop_test::at_scales;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

fn framed() -> Shell {
    let mut shell = Shell::new(helios_panels(), None);
    shell.draw_frame(true);
    shell.set_title("photos");
    shell
}

#[test]
fn the_frame_and_the_areas_keep_their_place_in_points_at_every_scale() {
    let mut at_one = None;
    at_scales(|s| {
        let mut h = headless::harness_at(framed(), vec2(1280.0, 800.0), s);
        h.run();
        let title = h.get_by_label("photos — Helios").rect();
        let vault = h.state().target_rect("helios.vault").unwrap();
        let params = h.state().target_rect("helios.params").unwrap();
        let here = (title, vault, params);
        let one = *at_one.get_or_insert(here);
        let near = |a: egui::Rect, b: egui::Rect| (a.min - b.min).length() < 1.0;
        assert!(
            near(here.1, one.1) && near(here.2, one.2),
            "{here:?} {one:?}"
        );
        assert!((here.0.center().y - one.0.center().y).abs() < 1.0);
        // Pictures are in physical pixels.
        let window = headless::snapshot(&mut h, "window").unwrap();
        assert_eq!(
            window.dimensions(),
            ((1280.0 * s).round() as u32, (800.0 * s).round() as u32)
        );
    });
}

#[test]
fn the_interface_zoom_is_egui_pop_s_and_its_change_is_journaled() {
    use egui_pop::api::zoom;
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1024.0, 700.0));
    h.run();
    // egui's own zoom keys are off: Ctrl and `=` leave the interface.
    h.key_press_modifiers(Modifiers::CTRL, Key::Equals);
    h.run();
    assert!((h.ctx.zoom_factor() - 1.0).abs() < 1e-6);
    // egui-pop's keys move it a step, in every window.
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::ALT, Key::Equals);
    h.run();
    let app = h.state().app().handle().clone();
    assert!((zoom::current(&app) - 1.1).abs() < 1e-6);
    assert!((h.ctx.zoom_factor() - 1.1).abs() < 1e-6);
    assert!((h.state().pixels_per_point() - 1.1).abs() < 1e-6);
    let changed: Vec<_> = app
        .events_since(0)
        .into_iter()
        .filter(|e| e.name == "scale.changed")
        .collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::ALT, Key::Num0);
    h.run();
    assert!((h.ctx.zoom_factor() - 1.0).abs() < 1e-6);
}

#[test]
fn a_hidpi_variant_follows_the_screen_not_the_interface_zoom() {
    let dir = std::env::temp_dir().join(format!("helios-hidpi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("work.helios-profile"),
        "helios_profile = 1\nname = \"work\"\n\
         [theme]\nname = \"helios-dark\"\n\
         [on.hidpi.theme]\nname = \"helios-light\"\n",
    )
    .unwrap();
    let shell = |dir: &std::path::Path| {
        let mut s = Shell::new(helios_panels(), None);
        s.set_profiles_dir(dir.to_path_buf());
        s.use_profile("work").unwrap();
        s
    };
    // A dense screen: the variant applies from the first frame.
    let mut h = headless::harness_at(shell(&dir), vec2(800.0, 500.0), 2.0);
    h.run();
    assert_eq!(h.state().theme_name(), "helios-light");
    // A plain screen zoomed to 200 %: still a plain screen.
    let mut h = headless::harness(shell(&dir), vec2(800.0, 500.0));
    h.run();
    let s = h.state_mut();
    s.run(
        egui_pop::api::zoom::CMD_ZOOM_IN.as_str(),
        json!(null),
        Origin::User,
    )
    .unwrap();
    h.run();
    h.run();
    assert!(h.state().pixels_per_point() > 1.0);
    assert_eq!(h.state().theme_name(), "helios-dark");
    // Moved to a dense screen: applied again.
    egui_pop_test::set_native_scale(&mut h, 2.0);
    h.run();
    h.run();
    assert_eq!(h.state().theme_name(), "helios-light");
    std::fs::remove_dir_all(&dir).unwrap();
}

const W: u32 = 1200;
const H: u32 = 900;

/// A ramp through an exposure and a mask: the viewport, its drawn shapes
/// and a Parameters panel.
fn masked() -> Shell {
    use helios_core::{Graph, Image, ModuleRef, Params, Region};
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
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    let mask = g
        .add_module(
            "mask",
            ModuleRef::new(helios_module_masks::MaskCombine),
            &Params::new(),
        )
        .unwrap();
    let blend = g
        .add_module(
            "blend",
            ModuleRef::new(helios_module_masks::MaskBlend),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", blend, "input").unwrap();
    g.connect(exp, "output", blend, "processed").unwrap();
    g.connect(mask, "mask", blend, "mask").unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, blend, "output").unwrap();
    shell
}

/// The stroke widths drawn, in points (texts' and invisible ones left out).
fn stroke_widths(shapes: &[egui::epaint::ClippedShape]) -> Vec<f32> {
    use egui::Shape;
    fn walk(s: &Shape, out: &mut Vec<f32>) {
        let w = match s {
            Shape::Vec(v) => {
                for s in v {
                    walk(s, out);
                }
                return;
            }
            Shape::Circle(c) => c.stroke,
            Shape::Ellipse(e) => e.stroke,
            Shape::LineSegment { stroke, .. } => *stroke,
            Shape::Rect(r) => r.stroke,
            Shape::Path(p) => egui::Stroke::new(
                p.stroke.width,
                match p.stroke.color {
                    egui::epaint::ColorMode::Solid(c) => c,
                    egui::epaint::ColorMode::UV(_) => egui::Color32::WHITE,
                },
            ),
            _ => return,
        };
        if w.width > 0.0 && w.color.a() > 0 {
            out.push(w.width);
        }
    }
    let mut out = Vec::new();
    for c in shapes {
        walk(&c.shape, &mut out);
    }
    out
}

/// The widths that are not whole physical pixels at `ppp`.
fn blurred(shapes: &[egui::epaint::ClippedShape], ppp: f32) -> Vec<f32> {
    let mut out: Vec<f32> = stroke_widths(shapes)
        .into_iter()
        .filter(|w| ((w * ppp) - (w * ppp).round()).abs() > 1e-3)
        .collect();
    out.sort_by(f32::total_cmp);
    out.dedup();
    out
}

#[test]
fn a_detached_panel_follows_its_own_screen_and_the_main_window_its_own() {
    let (mut h, windows) = headless::harness_with_windows(masked(), vec2(1280.0, 800.0), 1.0);
    h.step();
    for (command, args) in [
        ("helios.params.focus", json!(null)),
        ("mask.add.circle", json!(null)),
        ("mask.add.ellipse", json!(null)),
        ("helios.viewport.detach", json!(null)),
    ] {
        h.state_mut().run(command, args, Origin::User).unwrap();
        h.step();
    }
    windows.set_scale("helios.viewport", 2.0);
    for _ in 0..20 {
        h.step();
        let stats = h.state().viewport_stats();
        assert!(stats.wait_idle(std::time::Duration::from_mins(1)));
        h.step();
        if h.state().viewport().state().tiles_complete() {
            break;
        }
    }
    let drawn = windows
        .drawn("helios.viewport")
        .expect("a window of its own");
    // Each window its own scale.
    assert_eq!(drawn.pixels_per_point, 2.0);
    assert_eq!(h.ctx.pixels_per_point(), 1.0);
    assert_eq!(h.state().pixels_per_point(), 1.0);
    assert_ne!(
        h.state().target_window("viewport.display"),
        egui::ViewportId::ROOT
    );
    // The viewport maps through its window's scale: fitted, the whole
    // image fills the display there (not a quarter of it).
    let m = h.state().viewport().state().mapping().unwrap();
    assert_eq!(m.ppp, 2.0);
    let r = h.state().target_rect("viewport.display").unwrap();
    let (a, b) = (
        m.image_to_screen([0.0, 0.0]),
        m.image_to_screen([f64::from(W), f64::from(H)]),
    );
    let (iw, ih) = (b.x - a.x, b.y - a.y);
    assert!(
        (iw - r.width()).abs() < 1.0 || (ih - r.height()).abs() < 1.0,
        "{iw}x{ih} in {r:?}"
    );
    // Whole-pixel strokes in each window, the ellipse's handles included.
    assert!(blurred(&drawn.shapes, 2.0).is_empty());
    assert!(blurred(&h.output().shapes, 1.0).is_empty());
    // Pictures in each window's own pixels; the image is drawn in the
    // detached one.
    let aux = windows.render(&h.ctx, "helios.viewport").unwrap();
    let size = drawn.size * 2.0;
    assert_eq!(
        aux.dimensions(),
        (size.x.round() as u32, size.y.round() as u32)
    );
    let c = r.center() * 2.0;
    let px = aux.get_pixel(c.x as u32, c.y as u32).0;
    assert!(px[3] == 255 && px[..3] != [0, 0, 0], "{px:?}");
    let main = h.render().unwrap();
    assert_eq!(main.dimensions(), (1280, 800));
    // 100 % is one image pixel on one pixel of the window's own screen.
    h.state_mut()
        .run("viewport.zoom.100", json!(null), Origin::User)
        .unwrap();
    h.step();
    h.step();
    let m = h.state().viewport().state().mapping().unwrap();
    let one = m.image_to_screen([1.0, 0.0]).x - m.image_to_screen([0.0, 0.0]).x;
    assert!((one - 0.5).abs() < 1e-4, "an image pixel is {one} points");
    // Moved to the other screens: Helios's own strokes (the shapes not
    // selected, drawn faint) stay whole at every scale.
    // upstream: egui-pop's overlay draws the selected shape and its
    // handles at 1.5 and 3 points whatever the scale, blurred at 1.25 and
    // 1.5 (and the handles' 1.5 at 1); they are left out here until it
    // snaps them.
    for s in egui_pop_test::SCALES {
        windows.set_scale("helios.viewport", s);
        h.step();
        h.step();
        let d = windows.drawn("helios.viewport").unwrap();
        assert_eq!(d.pixels_per_point, s);
        let own: Vec<f32> = blurred(&d.shapes, s)
            .into_iter()
            .filter(|w| ![1.5, 3.0].contains(w))
            .collect();
        assert!(own.is_empty(), "{own:?} at {s}");
    }
}
