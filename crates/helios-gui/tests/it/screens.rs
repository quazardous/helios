//! The screens Helios's event loop tells egui-pop: the main window kept
//! on a monitor now gone (X11, Windows) opens on a screen left, as
//! eframe creates it (`shell::place_main_window`'s hook); where no
//! position was kept (Wayland) nothing moves.

use egui::{ViewportBuilder, vec2};
use egui_pop::api::window::{self, Screen};
use helios_gui::shell::place_main_window;

/// A laptop's panel and, at its right, a monitor, named as winit names
/// them.
fn panel() -> Screen {
    Screen::from_physical(Some("eDP-1".into()), (0, 0), (2560, 1600), 2.0)
}

fn monitor() -> Screen {
    Screen::from_physical(Some("DP-1".into()), (2560, 0), (1920, 1080), 1.0)
}

/// The builder eframe creates the main window with, from `viewport`, the
/// screens told being `screens`.
fn created(viewport: ViewportBuilder, screens: Vec<Screen>) -> ViewportBuilder {
    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    place_main_window(&mut options, None);
    window::set_screens(screens);
    let vb = match options.window_builder.take() {
        Some(hook) => hook(options.viewport.clone()),
        None => options.viewport.clone(),
    };
    window::set_screens(Vec::new());
    vb
}

#[test]
fn the_main_window_kept_on_a_monitor_now_gone_opens_on_a_screen_left() {
    let kept = ViewportBuilder::default()
        .with_inner_size([1000.0, 700.0])
        .with_position(monitor().rect.min + vec2(100.0, 50.0));
    let vb = created(kept.clone(), vec![panel()]);
    let p = vb.position.expect("a position");
    assert!(panel().rect.contains(p), "{p:?} is on the panel");
    assert_eq!(vb.inner_size, Some(vec2(1000.0, 700.0)));
    // The monitor plugged in again: the window opens where it was.
    let vb = created(kept.clone(), vec![panel(), monitor()]);
    assert_eq!(vb.position, kept.position);
}

#[test]
fn where_no_position_was_kept_nothing_moves() {
    let kept = ViewportBuilder::default().with_inner_size([1000.0, 700.0]);
    let vb = created(kept, vec![panel()]);
    assert_eq!(vb.position, None, "the system places it");
    assert_eq!(vb.inner_size, Some(vec2(1000.0, 700.0)));
}

/// A monitor unplugged while Helios runs is said on egui-pop's bus, by
/// the name winit gives it (`screen.removed`), once the event loop tells
/// the screens left; plugged in again, `screen.added`. Helios's loop is
/// the one place that reads the monitors (`event_loop::tell_screens`).
#[test]
fn a_monitor_unplugged_is_said_on_the_bus_by_its_name() {
    use std::sync::{Arc, Mutex};

    use egui_pop::api::bus::bus_contract::{ListenOpts, Point as _};
    use egui_pop::api::bus::{ScreenAdded, ScreenRemoved};
    use helios_gui::{Shell, headless, helios_panels};

    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let bus = h.state().app().handle().bus();
    let said = heard.clone();
    bus.listen_local::<ScreenRemoved>(
        "test.removed",
        ListenOpts::new().owner("test").last(),
        move |c, ()| {
            said.lock()
                .unwrap()
                .push(format!("{} {:?}", ScreenRemoved::ID, c.name));
            Ok(())
        },
    )
    .unwrap();
    let said = heard.clone();
    bus.listen_local::<ScreenAdded>(
        "test.added",
        ListenOpts::new().owner("test").last(),
        move |c, ()| {
            said.lock()
                .unwrap()
                .push(format!("{} {:?}", ScreenAdded::ID, c.name));
            Ok(())
        },
    )
    .unwrap();
    window::set_screens(vec![panel(), monitor()]);
    h.run();
    assert!(
        heard.lock().unwrap().is_empty(),
        "the screens first told are not said"
    );
    window::set_screens(vec![panel()]);
    h.run();
    window::set_screens(vec![panel(), monitor()]);
    h.run();
    window::set_screens(Vec::new());
    assert_eq!(
        *heard.lock().unwrap(),
        vec![
            "pop.screen.removed Some(\"DP-1\")".to_string(),
            "pop.screen.added Some(\"DP-1\")".to_string(),
        ]
    );
}

/// What the loop's slow re-read of the monitors does
/// (`event_loop::ScreenWatch`): winit has no hot-plug event, so a monitor
/// unplugged with no event on a Helios window is heard at the next
/// re-read, within the period; a re-read that finds the same screens
/// asks for no repaint (idle stays idle).
#[test]
fn a_monitor_unplugged_with_no_window_event_is_heard_within_the_period() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use egui_pop::api::bus::ScreenRemoved;
    use egui_pop::api::bus::bus_contract::{ListenOpts, Point as _};
    use helios_gui::event_loop::ScreenWatch;
    use helios_gui::{Shell, headless, helios_panels};

    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let said = heard.clone();
    h.state()
        .app()
        .handle()
        .bus()
        .listen_local::<ScreenRemoved>(
            "test.removed",
            ListenOpts::new().owner("test").last(),
            move |c, ()| {
                said.lock()
                    .unwrap()
                    .push(format!("{} {:?}", ScreenRemoved::ID, c.name));
                Ok(())
            },
        )
        .unwrap();
    window::set_screens(vec![panel(), monitor()]);
    h.run();
    // Repaints asked from now on, counted.
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    h.ctx.set_request_repaint_callback(move |_| {
        count.fetch_add(1, Ordering::Relaxed);
    });

    let mut watch = ScreenWatch::new(Some(h.ctx.clone()));
    let t0 = Instant::now();
    let at = watch.next_wake(t0).expect("two screens: watched");
    let period = at - t0;
    assert!(
        (Duration::from_secs(2)..=Duration::from_secs(5)).contains(&period),
        "{period:?}"
    );
    assert!(
        !watch.wake(t0, || panic!("the monitors read before the period")),
        "nothing before the period"
    );
    // The same screens: nothing told, no repaint.
    assert!(!watch.wake(at, || vec![panel(), monitor()]));
    assert_eq!(
        asked.load(Ordering::Relaxed),
        0,
        "no repaint when nothing changed"
    );
    // The monitor unplugged, no window event: heard at the next re-read.
    let at = watch.next_wake(at).expect("still watched");
    assert!(watch.wake(at, || vec![panel()]));
    assert!(
        asked.load(Ordering::Relaxed) > 0,
        "a repaint, for egui-pop to say it"
    );
    h.run();
    window::set_screens(Vec::new());
    assert_eq!(
        *heard.lock().unwrap(),
        vec!["pop.screen.removed Some(\"DP-1\")".to_string()]
    );
}

/// The re-read runs only while it may matter: a Helios window focused, or
/// more than one screen (a window may sit on one that can go). One screen
/// and no focus: the loop sleeps until an event.
#[test]
fn the_monitors_are_re_read_only_focused_or_with_several_screens() {
    use std::time::Instant;

    use helios_gui::event_loop::ScreenWatch;

    let id = winit::window::WindowId::dummy();
    let now = Instant::now();
    let mut watch = ScreenWatch::new(None);
    window::set_screens(vec![panel()]);
    assert_eq!(watch.next_wake(now), None, "one screen, unfocused");
    watch.focused(id, true);
    assert!(watch.next_wake(now).is_some(), "focused");
    watch.focused(id, false);
    assert_eq!(watch.next_wake(now), None, "focus lost");
    window::set_screens(vec![panel(), monitor()]);
    assert!(watch.next_wake(now).is_some(), "two screens");
    window::set_screens(Vec::new());
}
