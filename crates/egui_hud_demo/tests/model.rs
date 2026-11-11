//! The demo's model, headless: the catalogue loads, every preset loads
//! and keeps its controls reachable, the gain steps down from a fader to
//! a knob to a scrub field as the bench narrows, the folding preset
//! gathers parts under "More (n)", values undo and redo, and a bench
//! round-trips as text.

use egui_hud::Representation;
use egui_hud::bench::BenchMode;
use egui_hud::flow::Cause;
use egui_hud::render::{Change, Drawn, Edit};
use egui_hud::slot::{Fallback, Orientation};
use egui_hud_demo::{Demo, GAIN, METER, MUTE, SILENCE, bench, kit, signal};
use egui_kittest::Harness;

#[test]
fn the_catalogue_loads_and_every_preset_resolves_with_every_part_reachable() {
    let kit = kit().unwrap();
    for id in [GAIN, "pan", METER, "intensity", "temperature"] {
        assert!(kit.registry.get(id).is_some(), "{id} registered");
    }
    // A plain toggle or choice is the generic control, as a parameter.
    for id in ["mute", "tag", "power", "effect"] {
        assert!(kit.registry.get(id).is_none(), "{id} not registered");
        assert!(kit.parameters.iter().any(|p| p.name == id), "{id} offered");
    }
    assert!(kit.presets.len() >= 5);
    let mut b = bench().unwrap();
    let shelf: Vec<String> = b.shelf.groups("").into_iter().map(|(g, _)| g).collect();
    assert_eq!(shelf, ["Light", "Mixer channel", "Miscellaneous"]);
    assert!(b.shelf.get(MUTE).is_some());
    for p in kit.presets.clone() {
        b.load_preset(&p.name)
            .unwrap_or_else(|e| panic!("{}: {e}", p.name));
        if b.mode.boxed() {
            let (placement, _) = b.resolve().unwrap();
            egui_hud::resolve::reachable(&placement).unwrap_or_else(|e| panic!("{}: {e}", p.name));
        }
    }
}

#[test]
fn the_gain_steps_down_from_fader_to_knob_to_scrub_as_the_bench_narrows() {
    let mut b = bench().unwrap();
    let mut look = |preset: &str| {
        b.load_preset(preset).unwrap();
        let (p, _) = b.resolve().unwrap();
        let g = p.get(GAIN).unwrap().clone();
        assert!(g.visibility.is_shown(), "{preset}");
        (g.representation, g.orientation)
    };
    assert_eq!(
        look("Wide: fader"),
        (Some(Representation::Full), Some(Orientation::Vertical))
    );
    assert_eq!(look("Narrow: knob").0, Some(Representation::Compact));
    assert_eq!(
        look("Narrowest: scrub field").0,
        Some(Representation::Simplified)
    );
    // The same bench, resized by its width handle: back to a fader.
    b.set_width(480.0);
    let (p, _) = b.resolve().unwrap();
    assert_eq!(
        p.get(GAIN).unwrap().representation,
        Some(Representation::Full)
    );
}

#[test]
fn the_folding_preset_gathers_parts_under_more_with_their_reasons() {
    let mut b = bench().unwrap();
    b.load_preset("Folding: More").unwrap();
    let (p, _) = b.resolve().unwrap();
    let more = p.more.clone().expect("a More entry");
    assert!(more.count >= 2, "{more:?}");
    let why = b.explain(&p);
    let pan = why.iter().find(|e| e.key == "pan").unwrap();
    assert_eq!(pan.visibility, "folded");
    assert_eq!(pan.decided_by, "the detail level, against its tier");
    assert_eq!(
        why.iter().find(|e| e.key == GAIN).unwrap().visibility,
        "shown"
    );
}

#[test]
fn a_change_is_one_undo_step_and_undoes() {
    let mut b = bench().unwrap();
    let change = |to, done| Change {
        to,
        cause: Cause::Drag,
        done,
        cancelled: false,
    };
    b.apply(vec![
        Edit::Number {
            key: GAIN.into(),
            change: change(-3.0, false),
        },
        Edit::Number {
            key: GAIN.into(),
            change: change(-6.0, true),
        },
    ]);
    assert_eq!(b.readout(GAIN), "-6.0 dB");
    assert_eq!(b.store.undo_steps().len(), 1);
    assert!(b.undo());
    assert_eq!(b.readout(GAIN), "0.0 dB");
    assert!(b.redo());
    assert_eq!(b.readout(GAIN), "-6.0 dB");
    // The meter is an indicator: the flow refuses to write it.
    let refused = b.nudge(METER, 1, "keys").unwrap();
    assert!(refused.contains("indicator"), "{refused}");
    assert_eq!(b.readout(METER), "-60.0 dB");
    assert_eq!(b.store.undo_steps().len(), 1);
}

#[test]
fn a_bench_round_trips_as_text_and_bad_text_changes_nothing() {
    let mut d = Demo::new();
    assert!(d.error.is_none());
    d.page.bench.load_preset("Light on a toolbar").unwrap();
    d.page.save_text();
    let saved = d.page.text.clone();
    d.page.bench.load_preset("Mixer channel").unwrap();
    d.page.text = "{ not a bench".into();
    d.page.load_text();
    assert_eq!(d.page.bench.mode, BenchMode::Panel);
    d.page.text = saved;
    d.page.load_text();
    assert_eq!(d.page.bench.mode, BenchMode::Toolbar);
    assert!(d.page.bench.composition.holds("colour"));
}

#[test]
fn the_meter_follows_the_gain_and_falls_silent_when_muted() {
    assert_eq!(signal(0.0, true, 1.0), SILENCE);
    assert!(signal(0.0, false, 0.4) > signal(-40.0, false, 0.4));
    let d = Demo::new();
    assert!(d.level_at(0.4) > SILENCE);
}

#[test]
fn the_page_draws_headless_with_its_fader_meter_and_fallbacks() {
    let mut h = Harness::builder()
        .with_size(egui_pop::egui::vec2(1280.0, 900.0))
        .build_ui_state(|ui, d: &mut Demo| d.ui(ui), Demo::new());
    h.run_steps(3);
    let drawn = &h.state().page.drawn;
    assert_eq!(drawn.get(GAIN), Some(&Drawn::Integrated), "{drawn:?}");
    assert_eq!(drawn.get(METER), Some(&Drawn::Integrated));
    assert_eq!(drawn.get("eq"), Some(&Drawn::Integrated));
    h.state_mut()
        .page
        .bench
        .load_preset("Narrow: knob")
        .unwrap();
    h.run_steps(3);
    assert_eq!(
        h.state().page.drawn.get(GAIN),
        Some(&Drawn::Fallback(Fallback::Variant("knob".into())))
    );
}
