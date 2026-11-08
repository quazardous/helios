//! The virtualised grid: scrolling 10,000 images keeps the thumbnails'
//! textures under their budget, asks the cells in view first, and reads
//! the cells scrolled back in again from the disk cache. The pool and its
//! cache are faked: a request is answered at once, a miss "read from the
//! file" and stored, a hit served by the cache. No timing is asserted.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use egui::{Event, Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase, vec2};
use helios_gui::library::Entry;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::Value;

const IMAGES: usize = 10_000;
/// Room for about 95 textures of 256 × 170.
const BUDGET: u64 = 16 * 1024 * 1024;

/// The fake cache: what is stored, and how each image was served.
#[derive(Default)]
struct FakeCache {
    stored: HashSet<String>,
    /// Read from the file (a miss), by id.
    reads: HashMap<String, usize>,
    /// Served by the cache, by id.
    hits: HashMap<String, usize>,
    /// Every request, in order, with the step it came in.
    asked: Vec<(usize, String)>,
    step: usize,
}

fn id(i: usize) -> String {
    format!("{i:016x}")
}

fn index(id: &str) -> usize {
    usize::from_str_radix(id, 16).unwrap()
}

fn wheel(dy: f32) -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, dy),
        phase: TouchPhase::Move,
        source: MouseWheelSource::Wheel,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn scrolling_ten_thousand_images_keeps_the_textures_in_budget() {
    let entries: Vec<Entry> = (0..IMAGES)
        .map(|i| Entry {
            id: id(i),
            name: format!("img{i}.jpg"),
            ..Entry::default()
        })
        .collect();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries);
    let cache = Rc::new(RefCell::new(FakeCache::default()));
    {
        let mut library = shell.library_mut();
        library.set_texture_budget(BUDGET);
        let sink = library.thumbnails();
        let fake = cache.clone();
        library.set_on_view(move |ids| {
            let mut c = fake.borrow_mut();
            for id in ids {
                let step = c.step;
                c.asked.push((step, id.clone()));
                if c.stored.insert(id.clone()) {
                    *c.reads.entry(id.clone()).or_default() += 1;
                } else {
                    *c.hits.entry(id.clone()).or_default() += 1;
                }
                let thumb = helios_thumbs::Rgb8 {
                    width: 256,
                    height: 170,
                    pixels: vec![(index(id) % 200) as u8; 256 * 170 * 3],
                };
                assert!(sink.send_srgb(id.clone(), thumb));
            }
        });
    }
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    h.state_mut()
        .run("helios.vault.focus", Value::Null, Origin::User)
        .expect("the Library panel");
    for _ in 0..4 {
        h.step();
    }
    let first_screen = h.state().library().in_view();
    assert!(!first_screen.is_empty(), "cells in view");
    {
        let s = h.state();
        let library = s.library();
        for k in &first_screen {
            assert!(library.has_thumbnail(k), "{k} shown on the first screen");
        }
    }
    let rect = h
        .state()
        .target_rect("helios.library.grid")
        .expect("the grid's rect");
    h.event(Event::PointerMoved(rect.center()));
    h.step();

    let check = |h: &egui_kittest::Harness<'_, Shell>, step: usize, cache: &FakeCache| {
        let s = h.state();
        let library = s.library();
        let (count, bytes) = library.thumbnail_textures();
        assert!(
            bytes <= BUDGET,
            "step {step}: {count} textures, {bytes} bytes over the budget"
        );
        let in_view = library.in_view();
        // The cells in view asked before the margin past them.
        let last = in_view.iter().map(|k| index(k)).max().unwrap_or(0);
        let now: Vec<&String> = cache
            .asked
            .iter()
            .filter(|(st, _)| *st == step)
            .map(|(_, k)| k)
            .collect();
        let first_margin = now.iter().position(|k| index(k) > last);
        if let Some(m) = first_margin {
            for (p, k) in now.iter().enumerate() {
                if in_view.contains(k) {
                    assert!(p < m, "step {step}: {k} in view asked after the margin");
                }
            }
        }
    };

    // Down, far: the first screen's textures are freed.
    let mut step = 0;
    let mut deepest = 0;
    // A screen passed on the way down (the first screen stays drawn by
    // the filmstrip, around the cursor).
    let mut passed: Vec<String> = Vec::new();
    for n in 0..80 {
        step += 1;
        cache.borrow_mut().step = step;
        h.event(wheel(-400.0));
        h.step();
        check(&h, step, &cache.borrow());
        if n == 20 {
            passed = h.state().library().in_view();
        }
        deepest = h
            .state()
            .library()
            .in_view()
            .iter()
            .map(|k| index(k))
            .fold(deepest, usize::max);
    }
    assert!(deepest > 200, "the grid scrolled far (reached {deepest})");
    {
        let s = h.state();
        let library = s.library();
        assert!(!passed.is_empty());
        let freed = passed.iter().filter(|k| !library.has_thumbnail(k)).count();
        assert_eq!(freed, passed.len(), "the screen passed was freed");
    }
    // Back up: the screen passed comes back, from the cache.
    for _ in 0..200 {
        step += 1;
        cache.borrow_mut().step = step;
        h.event(wheel(400.0));
        h.step();
        check(&h, step, &cache.borrow());
        let back = h.state().library().in_view().contains(&passed[0]);
        if back {
            break;
        }
    }
    for _ in 0..3 {
        step += 1;
        cache.borrow_mut().step = step;
        h.step();
    }
    let s = h.state();
    let library = s.library();
    let c = cache.borrow();
    for k in library.in_view() {
        assert!(library.has_thumbnail(&k), "{k} shown again");
    }
    let reloaded: Vec<&String> = passed
        .iter()
        .filter(|k| library.in_view().contains(k))
        .filter(|k| c.hits.get(*k).copied().unwrap_or(0) > 0)
        .collect();
    assert!(
        !reloaded.is_empty(),
        "cells scrolled back in reloaded from the cache"
    );
    for k in &passed {
        assert_eq!(
            c.reads.get(k).copied().unwrap_or(0),
            1,
            "{k} read from its file once"
        );
    }
}
