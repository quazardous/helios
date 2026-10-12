//! The engine as egui-pop's tile source: tiles rendered a union at a time,
//! an edit rendering again, the readout in working-space values. Drawn on
//! the CPU path (no GPU here): what is tested is what the source delivers.

use std::sync::Arc;
use std::time::Duration;

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::viewport::source::Readout;
use egui_pop::viewport::{View, Viewport, ViewportState};
use helios_core::{Bus, Engine, Graph, Image, Region};
use helios_gui::engine_source::{EngineSource, Shown, Stats};

const W: u32 = 1200;
const H: u32 = 900;

fn ramp(gain: f32) -> Arc<Graph> {
    let mut g = Graph::new();
    g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, move |x, y, c| match c {
            0 => gain * (0.05 + 0.4 * x as f32 / W as f32),
            1 => gain * (0.1 + 0.2 * y as f32 / H as f32),
            2 => 0.15,
            _ => 1.0,
        }),
    );
    Arc::new(g)
}

struct Scene {
    state: ViewportState,
    view: View,
    source: EngineSource,
    stats: Arc<Stats>,
    readout: Option<Readout>,
}

fn harness(view: View) -> Harness<'static, Scene> {
    let mut state = ViewportState::new();
    state.set_settle(0.0);
    let stats = Arc::<Stats>::default();
    let engine = Arc::new(Engine::new(Arc::new(Bus::new())));
    let source = EngineSource::new("test-viewport", engine, state.sink(), stats.clone());
    let g = ramp(1.0);
    let node = g.find("source").unwrap();
    source.show(Shown::new(g, node, "image").unwrap());
    Harness::builder()
        .with_size(vec2(600.0, 400.0))
        .build_ui_state(
            |ui, s: &mut Scene| {
                let out = Viewport::new("t")
                    .source(&s.source)
                    .show(ui, &mut s.state, &mut s.view);
                if out.readout.is_some() {
                    s.readout = out.readout;
                }
            },
            Scene {
                state,
                view,
                source,
                stats,
                readout: None,
            },
        )
}

/// Steps frames until every tile wanted is on screen, waiting after each
/// for the render thread to answer what the frame asked.
fn settle(h: &mut Harness<'_, Scene>) {
    for _ in 0..40 {
        h.step();
        assert!(
            h.state().stats.wait_idle(Duration::from_mins(1)),
            "the render thread never answered"
        );
        h.step();
        if h.state().state.tiles_complete() {
            return;
        }
    }
    panic!("the tiles never all came: {:?}", h.state().stats);
}

#[test]
fn neighbouring_tiles_are_rendered_as_one_union() {
    let mut h = harness(View::actual([600.0, 450.0]));
    settle(&mut h);
    let s = &h.state().stats;
    let (renders, tiles) = (Stats::get(&s.renders), Stats::get(&s.tiles));
    // The proxy's tiles and the exact tiles, each set one render or a few.
    assert!(tiles >= 6, "{s:?}");
    assert!(renders * 3 <= tiles, "{renders} renders for {tiles} tiles");
    assert_eq!(Stats::get(&s.failed), 0);
}

#[test]
fn showing_another_graph_renders_again() {
    let mut h = harness(View::fit());
    settle(&mut h);
    let before = Stats::get(&h.state().stats.tiles);
    let g = ramp(2.0);
    let node = g.find("source").unwrap();
    h.state().source.show(Shown::new(g, node, "image").unwrap());
    settle(&mut h);
    assert!(Stats::get(&h.state().stats.tiles) > before);
}

#[test]
fn the_readout_gives_working_space_values() {
    let mut h = harness(View::actual([600.0, 450.0]));
    settle(&mut h);
    let centre = h.state().state.mapping().unwrap().rect.center();
    h.hover_at(centre);
    for _ in 0..10 {
        h.step();
        h.state().stats.wait_idle(Duration::from_mins(1));
        if h.state().readout.is_some() {
            break;
        }
    }
    let r = h.state().readout.clone().expect("a readout");
    let [x, y] = r.pixel;
    assert!((x - 600).abs() <= 1 && (y - 450).abs() <= 1, "{r:?}");
    let want = [
        0.05 + 0.4 * x as f32 / W as f32,
        0.1 + 0.2 * y as f32 / H as f32,
        0.15,
    ];
    let names: Vec<&str> = r.values.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["R", "G", "B"]);
    for (i, (_, v)) in r.values.iter().enumerate() {
        assert!((v - want[i]).abs() < 1e-6, "{r:?} vs {want:?}");
    }
}
