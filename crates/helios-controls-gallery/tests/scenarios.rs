//! The bench's headless scenarios S1 to S6 (`docs/design/control-contexts.md`,
//! section 5), on the scenarios' bench ([`Bench::load_sample`]): each
//! measures what an agent can (steps, pointer travel from AccessKit
//! bounds, shift on a context change, reachability, packing at several
//! widths and densities, an encoder bank), checks the invariants, and
//! writes its numbers to a text report (`reports/<scenario>.txt`, in the
//! bench box's results, else in the test's temporary folder) so runs can
//! be compared. The committed copies are in `tests/reports`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use egui_hud::card::fits;
use egui_hud::flow::Cause;
use egui_hud::resolve::{self, Layouts, Values, Visibility};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_pop::egui::accesskit::Role;
use egui_pop::egui::{Event, Modifiers, PointerButton, Pos2, Rect, Vec2, pos2, vec2};
use helios_controls::render::{Change, Edit};
use helios_controls::{CardPlacement, Surface};
use helios_controls_gallery::bench::{Bench, BenchMode};
use helios_controls_gallery::bench_ui::BenchPage;
use serde_json::json;

/// Writes a scenario's report and prints it.
fn report(name: &str, text: &str) {
    println!("{text}");
    let out = std::path::Path::new("/out");
    let dir = if out.is_dir() {
        out.join("reports")
    } else {
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("reports")
    };
    std::fs::create_dir_all(&dir).expect("the reports' folder");
    std::fs::write(dir.join(format!("{name}.txt")), text).expect("the report is written");
}

fn sample() -> Bench {
    let mut b = Bench::default();
    b.load_sample();
    b
}

fn harness(page: BenchPage) -> Harness<'static, BenchPage> {
    Harness::builder()
        .with_size(vec2(1200.0, 640.0))
        .build_ui_state(|ui, p: &mut BenchPage| p.ui(ui), page)
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// A pointer drag from `from` to `to`, in small moves, released there.
fn drag(h: &mut Harness<'_, BenchPage>, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(button(from, true));
    h.step();
    for i in 1..=20 {
        let t = i as f32 / 20.0;
        h.event(Event::PointerMoved(from + (to - from) * t));
        h.step();
    }
    h.event(button(to, false));
    h.step();
    h.event(Event::PointerGone);
    h.run();
}

/// The bench's top left corner, from its width handle: where the hand
/// starts.
fn bench_origin(h: &Harness<'_, BenchPage>) -> Pos2 {
    let handle = h.get_by_role(Role::Splitter).rect();
    pos2(handle.left() - 2.0 - h.state().bench.width, handle.top())
}

/// One move of the hand: its distance, the target's width, and Fitts's
/// index of difficulty (Shannon: log2(D/W + 1)).
#[derive(Clone, Copy, Debug)]
struct Move {
    distance: f32,
    width: f32,
}

impl Move {
    fn between(from: Pos2, to: Rect) -> Self {
        Self {
            distance: from.distance(to.center()),
            width: to.width().min(to.height()).max(1.0),
        }
    }

    fn index(self) -> f32 {
        (self.distance / self.width + 1.0).log2()
    }
}

fn moves_line(moves: &[Move]) -> String {
    let total: f32 = moves.iter().map(|m| m.distance).sum();
    let ids: Vec<String> = moves
        .iter()
        .map(|m| {
            format!(
                "{:.0}pt/{:.0}pt D/W {:.1} ID {:.2}",
                m.distance,
                m.width,
                m.distance / m.width,
                m.index()
            )
        })
        .collect();
    format!("reach {total:.0} pt [{}]", ids.join("; "))
}

/// Sets a number on the bench by a pointer drag, then by a typed value
/// when the drag did not land on it: the steps it took, the value the
/// drag landed on, how far the drag went, and the key nudges a person
/// would have needed instead of typing.
fn set_number(
    h: &mut Harness<'_, BenchPage>,
    key: &str,
    label: &str,
    target: f64,
) -> (u32, f64, f32, u64) {
    let rect = h.get_by_role_and_label(Role::Slider, label).rect();
    let (control, now, variant, representation) = {
        let b = &h.state().bench;
        let piece = b
            .composition
            .pieces()
            .into_iter()
            .find(|(p, _)| p.key == key)
            .expect("on the bench")
            .0
            .clone();
        let p = h
            .state()
            .last
            .as_ref()
            .and_then(|p| p.get(key))
            .cloned()
            .expect("placed");
        (
            b.registry.get(&piece.control).cloned().expect("registered"),
            b.value(key).and_then(|v| v.as_f64()).unwrap_or(0.0),
            piece.variant.unwrap_or_else(|| "slider".into()),
            p.representation,
        )
    };
    let n = control.value.number().cloned().expect("a number");
    let scrubs =
        variant == "scrub" || representation == Some(helios_controls::Representation::Simplified);
    let from = rect.center();
    let to = if scrubs {
        // A scrub adds the drag's points times the control's sensitivity.
        let dx = (target - now) / control.feel.sensitivity;
        from + vec2(dx as f32, 0.0)
    } else {
        // A slider takes the value under the pointer, across its soft
        // range.
        let t = ((target - n.soft.0) / (n.soft.1 - n.soft.0)) as f32;
        pos2(rect.left() + rect.width() * t, rect.center().y)
    };
    drag(h, from, to);
    let landed = h
        .state()
        .bench
        .value(key)
        .and_then(|v| v.as_f64())
        .unwrap_or(now);
    let mut steps = 1;
    let residual = (target - landed).abs();
    let nudges = (residual / n.step).round() as u64;
    if residual > n.step / 2.0 {
        // The typed value: one step.
        h.state_mut().bench.apply(vec![Edit::Number {
            key: key.to_owned(),
            change: Change {
                to: target,
                cause: Cause::Typed,
                done: true,
                cancelled: false,
            },
        }]);
        h.run();
        steps += 1;
    }
    let got = h
        .state()
        .bench
        .value(key)
        .and_then(|v| v.as_f64())
        .unwrap_or(f64::NAN);
    assert!(
        (got - target).abs() <= n.step / 2.0,
        "{key}: {got} for {target}"
    );
    (steps, landed, from.distance(to), nudges)
}

/// S1: exposure to +0.70 EV, then a gradient's feather to 40 px, in each
/// form and variant: the steps (input actions) and the pointer's travel.
#[test]
fn s1_steps_and_reach_per_form_and_variant() {
    let mut text = String::from(
        "S1: exposure to +0.70 EV, then feather to 40 px (bench 480 pt, comfortable)\n\
         steps: a drag, plus a typed value when the drag did not land on the value\n\
         reach: from the bench's top left to each control's centre (AccessKit bounds)\n\n",
    );
    for (variant, rep) in [
        ("slider", "full"),
        ("slider", "compact"),
        ("slider", "simplified"),
        ("scrub", "full"),
        ("scrub", "simplified"),
    ] {
        let mut b = sample();
        b.run(
            "controls.bench.part",
            &json!({"key": "exposure", "variant": variant, "representation": rep}),
        )
        .unwrap();
        b.run(
            "controls.bench.part",
            &json!({"key": "feather", "representation": rep}),
        )
        .unwrap();
        let mut h = harness(BenchPage::new(b));
        h.run();
        let start = bench_origin(&h);
        let exposure = h.get_by_role_and_label(Role::Slider, "Exposure").rect();
        let feather = h.get_by_role_and_label(Role::Slider, "Feather").rect();
        let moves = [
            Move::between(start, exposure),
            Move::between(exposure.center(), feather),
        ];
        let (s1, l1, d1, n1) = set_number(&mut h, "exposure", "Exposure", 0.70);
        let (s2, l2, d2, n2) = set_number(&mut h, "feather", "Feather", 40.0);
        assert_eq!(h.state().bench.readout("exposure"), "+0.70 EV");
        assert_eq!(h.state().bench.readout("feather"), "40 px");
        let _ = writeln!(
            text,
            "{variant}/{rep}: steps {} (exposure {s1}: drag {d1:.0} pt landed {l1:.2}, or {n1} key nudges; \
             feather {s2}: drag {d2:.0} pt landed {l2:.0}, or {n2} key nudges); {}; exposure {:.0}x{:.0} pt, feather {:.0}x{:.0} pt",
            s1 + s2,
            moves_line(&moves),
            exposure.width(),
            exposure.height(),
            feather.width(),
            feather.height(),
        );
    }
    report("s1", &text);
}

/// S2: the gradient's profile, folded at detail `essential`, set to
/// sigmoid: by "More" (through the bench's pointer targets), by search
/// and by a key; and the invariant that it is one action away.
#[test]
fn s2_a_folded_part_is_one_action_away() {
    let b = sample();
    b.set_context(&json!({"detail": "essential"})).unwrap();
    let mut h = harness(BenchPage::new(b));
    h.run();
    let placement = h.state().last.clone().expect("resolved");
    resolve::reachable(&placement).unwrap();
    let more = placement.more.as_ref().expect("a More row");
    assert!(more.names("profile"));
    let folded: Vec<&str> = more.folded.iter().map(|m| m.key.as_str()).collect();

    // By "More": open it, show the profile, open its list, pick sigmoid.
    let start = bench_origin(&h);
    let band = h
        .get_all_by_role(Role::Button)
        .find(|n| {
            n.accesskit_node()
                .label()
                .is_some_and(|l| l.starts_with("More"))
        })
        .expect("the More row")
        .rect();
    h.get_all_by_role(Role::Button)
        .find(|n| {
            n.accesskit_node()
                .label()
                .is_some_and(|l| l.starts_with("More"))
        })
        .unwrap()
        .click();
    h.run();
    let show = h.get_by_label("Show Profile").rect();
    h.get_by_label("Show Profile").click();
    h.run();
    assert!(
        h.state()
            .last
            .as_ref()
            .unwrap()
            .get("profile")
            .unwrap()
            .visibility
            .is_shown()
    );
    // The profile's list: the bench's one drop-down (the context bar's
    // are above the bench).
    let on_bench = |n: &egui_kittest::Node<'_>| n.rect().top() > start.y;
    let combo = h
        .get_all_by_role(Role::ComboBox)
        .find(on_bench)
        .expect("the profile's list")
        .rect();
    h.get_all_by_role(Role::ComboBox)
        .find(on_bench)
        .unwrap()
        .click();
    h.run();
    let option = h.get_by_label("Sigmoid").rect();
    h.get_by_label("Sigmoid").click();
    h.run();
    assert_eq!(h.state().bench.readout("profile"), "Sigmoid");
    let moves = [
        Move::between(start, band),
        Move::between(band.center(), show),
        Move::between(show.center(), combo),
        Move::between(combo.center(), option),
    ];
    // Changed, it stays shown once the moment ends (a task change).
    h.state()
        .bench
        .set_context(&json!({"task": "retouch"}))
        .unwrap();
    h.state()
        .bench
        .set_context(&json!({"task": "develop"}))
        .unwrap();
    h.run();
    let after = h.state().last.clone().unwrap();
    assert!(after.get("profile").unwrap().visibility.is_shown());
    resolve::reachable(&after).unwrap();

    // By search: `controls.find` lists it, folded; by a key:
    // `controls.reveal_all`. Counted on the model: the bench has no
    // palette.
    let found = resolve::find(&placement, "profile");
    assert_eq!(found.len(), 1);
    assert!(matches!(found[0].visibility, Visibility::Folded { .. }));
    let text = format!(
        "S2: profile to sigmoid at detail essential (folded: {})\n\
         by More: 4 steps (open More, Show Profile, open the list, Sigmoid); {}\n\
         by search: 5 steps (open the search, type, choose, open the list, Sigmoid), counted on the model\n\
         by a key: 3 steps (reveal all, open the list, Sigmoid), counted on the model\n\
         invariant: reachable before and after; More names it ({}); changed, it stays shown after a task change\n",
        folded.join(", "),
        moves_line(&moves),
        more.title(),
    );
    report("s2", &text);
}

/// Each shown control's rect, by key, in bench points.
fn rects(p: &CardPlacement) -> BTreeMap<String, egui_hud::card::Rect> {
    p.controls()
        .filter(|c| c.visibility.is_shown())
        .filter_map(|c| Some((c.key.clone(), c.rect?)))
        .collect()
}

/// How far the controls shown before a change moved: the centres'
/// displacement weighted by area, the largest, and how many left.
fn shift(
    before: &BTreeMap<String, egui_hud::card::Rect>,
    after: &BTreeMap<String, egui_hud::card::Rect>,
) -> (f32, f32, usize) {
    let centre = |r: &egui_hud::card::Rect| Vec2::new(r.x + r.w / 2.0, r.y + r.h / 2.0);
    let (mut weighted, mut area, mut largest, mut gone) = (0.0, 0.0, 0.0_f32, 0);
    for (k, r) in before {
        let a = r.w * r.h;
        area += a;
        match after.get(k) {
            Some(n) => {
                let d = (centre(n) - centre(r)).length();
                weighted += d * a;
                largest = largest.max(d);
            }
            None => gone += 1,
        }
    }
    (
        if area > 0.0 { weighted / area } else { 0.0 },
        largest,
        gone,
    )
}

/// S3: develop, retouch, develop with the pointer over the bench: nothing
/// moves until it leaves (anti-jump); then the shift. With the rules off,
/// nothing moves.
#[test]
fn s3_a_task_switch_waits_for_the_pointer_and_shifts() {
    let mut text = String::from(
        "S3: task develop -> retouch -> develop, the pointer over the bench\n\
         shift: the centres' displacement weighted by area (pt), the largest (pt), controls folded\n\n",
    );
    for rules in [true, false] {
        let mut b = sample();
        if !rules {
            b.composition.rules.clear();
        }
        let mut h = harness(BenchPage::new(b));
        h.run();
        let under = h
            .get_by_role_and_label(Role::Slider, "Feather")
            .rect()
            .center();
        h.event(Event::PointerMoved(under));
        h.run();
        let develop = rects(h.state().last.as_ref().unwrap());

        h.state()
            .bench
            .set_context(&json!({"task": "retouch"}))
            .unwrap();
        h.run();
        let held = h.state().last.clone().unwrap();
        let (held_shift, _, held_gone) = shift(&develop, &rects(&held));
        assert_eq!(held_shift, 0.0, "nothing moves under the pointer");
        assert_eq!(held_gone, 0);
        assert_eq!(held.pending, rules, "a change waits only when there is one");

        h.event(Event::PointerMoved(pos2(4.0, 630.0)));
        h.run();
        let retouch = rects(h.state().last.as_ref().unwrap());
        let (s_out, max_out, gone_out) = shift(&develop, &retouch);
        resolve::reachable(h.state().last.as_ref().unwrap()).unwrap();

        h.state()
            .bench
            .set_context(&json!({"task": "develop"}))
            .unwrap();
        h.run();
        let back = rects(h.state().last.as_ref().unwrap());
        let (s_back, max_back, gone_back) = shift(&retouch, &back);
        let (s_round, _, _) = shift(&develop, &back);
        if rules {
            assert!(
                gone_out == 2 && s_out > 0.0,
                "contrast and black level fold"
            );
        } else {
            assert_eq!((s_out, gone_out), (0.0, 0));
        }
        assert_eq!(s_round, 0.0, "back in develop, back in place");
        let _ = writeln!(
            text,
            "rules {}: under the pointer {held_shift:.1} (pending {}); once it leaves {s_out:.1}, largest {max_out:.1}, folded {gone_out}; \
             back to develop {s_back:.1}, largest {max_back:.1}, folded {gone_back}; round trip {s_round:.1}",
            if rules { "on" } else { "off" },
            held.pending,
        );
    }
    report("s3", &text);
}

/// The variant a shown control lays out.
fn below_minimum(b: &Bench, p: &CardPlacement) -> Vec<String> {
    let mut bad = Vec::new();
    for c in p.controls().filter(|c| c.visibility.is_shown()) {
        let control = c
            .control
            .as_deref()
            .and_then(|id| b.registry.get(id))
            .unwrap();
        let v = control
            .variants
            .iter()
            .find(|v| Some(&v.id) == c.variant.as_ref())
            .unwrap();
        let (Some(rep), Some(slot)) = (c.representation, c.slot) else {
            bad.push(format!("{} has no slot", c.key));
            continue;
        };
        let units = match c.orientation {
            Some(helios_controls::Orientation::Vertical) => {
                helios_controls::Slot::new(slot.units.height, slot.units.width)
            }
            _ => slot.units,
        };
        if !v
            .form(rep)
            .is_some_and(|f| fits(&f.policy, units) || fits(&f.policy, slot.units))
        {
            bad.push(format!(
                "{} in {} below its {rep:?} form",
                c.key, slot.units
            ));
        }
    }
    bad
}

/// S4: the bench at 260, 340 and 480 points, at each density: every
/// control reachable, none below its minimum, the logical order kept;
/// snapshots at the three widths; reachability for every combination.
#[test]
fn s4_widths_and_densities() {
    let mut text = String::from(
        "S4: the bench at 260, 340, 480 pt, each density (panel)\n\
         width/density: columns, space, shown/folded, card height\n\n",
    );
    for width in [260.0, 340.0, 480.0] {
        for density in ["comfortable", "compact", "touch"] {
            let mut b = sample();
            b.set_context(&json!({"density": density})).unwrap();
            b.set_width(width);
            let (p, _) = b.resolve().unwrap();
            resolve::reachable(&p).unwrap();
            let bad = below_minimum(&b, &p);
            assert!(bad.is_empty(), "{width} {density}: {bad:?}");
            let order: Vec<&str> = p.controls().map(|c| c.key.as_str()).collect();
            assert_eq!(order, b.composition.keys(), "logical order kept");
            let shown = p.controls().filter(|c| c.visibility.is_shown()).count();
            let height = p
                .layout
                .as_ref()
                .map_or(0.0, egui_hud::card::CardLayout::height);
            let keys = b.keys();
            let _ = writeln!(
                text,
                "{width:.0}/{density}: {:.2} columns, {:?}, {shown} shown / {} folded, {height:.0} pt high",
                keys.card_box.columns(),
                keys.space,
                p.more.as_ref().map_or(0, |m| m.count),
            );
        }
    }

    // In the drawing: the AccessKit order of the controls is the logical
    // order, and a snapshot per width.
    let mut results = egui_kittest::SnapshotResults::new();
    for width in [260.0_f32, 340.0, 480.0] {
        let mut b = sample();
        b.set_width(width);
        let mut h = Harness::builder()
            .with_size(vec2(width + 24.0, 420.0))
            .build_ui_state(|ui, p: &mut BenchPage| p.bench_zone(ui), BenchPage::new(b));
        h.run();
        let labels = ["Exposure", "Black level", "Contrast", "Feather", "Angle"];
        let seen: Vec<String> = h
            .query_all_by(|n| {
                n.role() == Role::Slider && n.label().is_some_and(|l| labels.contains(&l.as_str()))
            })
            .map(|n| n.accesskit_node().label().unwrap_or_default())
            .collect();
        assert_eq!(seen, labels, "{width}: AccessKit order");
        snapshot_into(&mut h, &format!("s4-{width:.0}"), &mut results);
    }

    // Reachability for every combination of the slow keys, the image
    // flags and the density, on each boxed surface, with the values at
    // their defaults and all changed.
    let b = sample();
    let (card, _) = b.composition.card();
    let changed: Values = [
        ("exposure", json!(1.0)),
        ("black", json!(0.02)),
        ("contrast", json!(0.3)),
        ("feather", json!(60.0)),
        ("angle", json!(45.0)),
        ("profile", json!("Sigmoid")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let combos = 4 * 3 * 3 * 8 * 3;
    for surface in [
        Surface::Panel,
        Surface::Dock,
        Surface::Toolbar,
        Surface::Float,
    ] {
        for (name, values) in [
            ("defaults", Values::new()),
            ("all changed", changed.clone()),
        ] {
            resolve::reachable_everywhere(
                &b.registry,
                &card,
                surface,
                &Layouts::default(),
                &values,
            )
            .unwrap_or_else(|e| panic!("{surface:?}, {name}: {e}"));
        }
    }
    let _ = writeln!(
        text,
        "\nreachability: {combos} combinations (task x space x detail x image x density) x 4 surfaces x 2 value sets: all reachable\n\
         snapshots: s4-260, s4-340, s4-480 (comfortable)\n\
         web build: not run headless here (the same code on a canvas)"
    );
    report("s4", &text);
    results.unwrap();
}

/// S5: a raw-only part changed, then a picture, then raw again: changed,
/// it is never folded, and says why.
#[test]
fn s5_a_changed_part_is_never_folded() {
    let mut b = sample();
    let picture = json!({"image": {"raw": false, "edited": false, "masked": false}});
    let raw = json!({"image": {"raw": true, "edited": false, "masked": false}});
    b.set_context(&picture).unwrap();
    let (p, _) = b.resolve().unwrap();
    let untouched = helios_controls_gallery::bench::explain(p.get("black").unwrap(), 2.0);
    assert_eq!(untouched.visibility, "not applicable");

    b.set_context(&raw).unwrap();
    b.nudge("black", 20, "pointer").unwrap();
    b.set_context(&picture).unwrap();
    let (p, _) = b.resolve().unwrap();
    let changed = helios_controls_gallery::bench::explain(p.get("black").unwrap(), 2.0);
    assert_eq!(changed.visibility, "shown");
    assert!(changed.changed);
    assert!(
        changed.notes.iter().any(|n| n.contains("Raw only")),
        "{:?}",
        changed.notes
    );
    assert_eq!(changed.decided_by, "a changed value is never folded");
    resolve::reachable(&p).unwrap();
    b.set_context(&raw).unwrap();
    let (p, _) = b.resolve().unwrap();
    let back = helios_controls_gallery::bench::explain(p.get("black").unwrap(), 2.0);
    assert_eq!(back.visibility, "shown");
    let text = format!(
        "S5: black level (raw only) on a picture\n\
         untouched: {} ({}; by {})\n\
         changed to {} then a picture: {} ({}; notes: {})\n\
         raw again: {} ({})\n",
        untouched.visibility,
        untouched.why,
        untouched.decided_by,
        b.readout("black"),
        changed.visibility,
        changed.decided_by,
        changed.notes.join(" | "),
        back.visibility,
        back.why,
    );
    report("s5", &text);
}

/// S6: eight encoders on a bank over the docked bench, a task switch:
/// the bank following the card against a fixed bank. There is no device
/// abstraction yet (slice 19), so no MIDI: the sweep is synthetic
/// nudges through the value flow, from the `midi` device.
#[test]
fn s6_an_encoder_bank_across_a_task_switch() {
    const ENCODERS: usize = 8;
    let numbers = |b: &Bench| -> Vec<String> {
        b.composition
            .keys()
            .into_iter()
            .filter(|k| matches!(b.value(k), Some(v) if v.as_f64().is_some()))
            .map(str::to_owned)
            .collect()
    };
    // The bank following the card: the shown numbers, in reading order.
    let follow = |b: &mut Bench| -> Vec<String> {
        let (p, _) = b.resolve().unwrap();
        let mut shown: Vec<(usize, String)> = p
            .controls()
            .filter(|c| c.visibility.is_shown())
            .filter(|c| matches!(b.value(&c.key), Some(v) if v.as_f64().is_some()))
            .map(|c| (c.visual.unwrap_or(usize::MAX), c.key.clone()))
            .collect();
        shown.sort();
        shown.into_iter().take(ENCODERS).map(|(_, k)| k).collect()
    };
    let names = |bank: &[String]| -> String {
        (0..ENCODERS)
            .map(|i| format!("{}:{}", i + 1, bank.get(i).map_or("-", String::as_str)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut text = String::from(
        "S6: eight encoders over the docked bench, task develop -> retouch\n\
         no device abstraction yet (slice 19): no MIDI; a synthetic sweep of `nudge` intents through the value flow, device `midi`\n\n",
    );
    for strategy in ["follows the card", "fixed"] {
        let mut b = sample();
        b.set_mode(BenchMode::Dock);
        let fixed: Vec<String> = numbers(&b).into_iter().take(ENCODERS).collect();
        let before = if strategy == "fixed" {
            fixed.clone()
        } else {
            follow(&mut b)
        };
        b.set_context(&json!({"task": "retouch"})).unwrap();
        let after = if strategy == "fixed" {
            fixed.clone()
        } else {
            follow(&mut b)
        };
        let (p, _) = b.resolve().unwrap();
        let remapped = (0..ENCODERS)
            .filter(|&i| before.get(i) != after.get(i))
            .count();
        let hidden: Vec<&String> = after
            .iter()
            .filter(|k| !p.get(k).unwrap().visibility.is_shown())
            .collect();
        // The sweep: each encoder a notch up, one undo step each.
        let steps_before = b.store.undo_steps().len();
        for k in &after {
            assert_eq!(
                b.nudge(k, 1, "midi").unwrap(),
                "committed: one undo step",
                "{k}"
            );
        }
        assert_eq!(b.store.undo_steps().len() - steps_before, after.len());
        // A notch on a folded control changes it: it is shown again.
        let (swept, _) = b.resolve().unwrap();
        let unfolded = hidden
            .iter()
            .filter(|k| swept.get(k).unwrap().visibility.is_shown())
            .count();
        resolve::reachable(&swept).unwrap();
        if strategy == "fixed" {
            assert_eq!(
                hidden.len(),
                2,
                "contrast and black level folded under their encoders"
            );
        } else {
            assert!(hidden.is_empty());
        }
        let _ = writeln!(
            text,
            "{strategy}:\n  develop  {}\n  retouch  {}\n  encoders remapped {remapped}, bound to a folded control {}, unfolded by the sweep {unfolded}, undo steps {}",
            names(&before),
            names(&after),
            hidden.len(),
            after.len(),
        );
    }
    report("s6", &text);
}

/// The harness's snapshot, compared with `tests/snapshots/<name>.png`;
/// a new one is written to the bench box's results (as in `gallery.rs`).
fn snapshot_into<S>(
    h: &mut Harness<'_, S>,
    name: &str,
    results: &mut egui_kittest::SnapshotResults,
) {
    let committed = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.png"));
    let out = std::path::Path::new("/out");
    if !committed.exists() && std::env::var_os("UPDATE_SNAPSHOTS").is_none() && out.is_dir() {
        let image = h.render().expect("the bench renders");
        let dir = out.join("snapshots");
        std::fs::create_dir_all(&dir).unwrap();
        image.save(dir.join(format!("{name}.png"))).unwrap();
        eprintln!("new snapshot {name}: written to /out/snapshots, to commit");
        return;
    }
    results.add(h.try_snapshot(name));
}
