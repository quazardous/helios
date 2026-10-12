//! The shell drawn headless: layout round trips, keys, the palette,
//! detached panels, and pictures of named parts.

use egui::{Key, Modifiers, vec2};
use egui_kittest::kittest::Queryable;
use helios_gui::dock::{from_dock, to_dock};
use helios_gui::snapshot::Recorder;
use helios_gui::{Shell, headless, helios_panels, name_rect};
use helios_shell::commands::Origin;
use helios_shell::layout::Node;
use helios_shell::panels::Area;
use serde_json::json;

fn tabs(p: &[&str], active: usize) -> Node {
    Node::Tabs {
        panels: p.iter().map(std::string::ToString::to_string).collect(),
        active,
    }
}

#[test]
fn layout_nodes_round_trip_through_the_dock() {
    let node = Node::Split {
        vertical: false,
        ratio: 0.3,
        a: Box::new(tabs(&["a", "b"], 1)),
        b: Box::new(Node::Split {
            vertical: true,
            ratio: 0.6,
            a: Box::new(tabs(&["c"], 0)),
            b: Box::new(tabs(&["d", "e"], 0)),
        }),
    };
    assert_eq!(from_dock(&to_dock(&node)), Some(node));
}

#[test]
fn draws_every_area_and_pictures_a_panel() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    // The parameters panel, with nothing shown.
    h.get_by_label("No parameters: nothing is shown yet");
    // The image viewport, empty.
    h.get_by_label("No image");
    // The Vault panel and the filmstrip show the (empty) library.
    h.get_by_label("No images: import a folder into the vault");
    h.get_by_label("No images");
    let window = headless::snapshot(&mut h, "window").unwrap();
    assert_eq!(window.dimensions(), (1280, 800));
    let vault = headless::snapshot(&mut h, "helios.vault").unwrap();
    let r = h.state().target_rect("helios.vault").unwrap();
    assert_eq!(
        vault.dimensions(),
        (r.width().round() as u32, r.height().round() as u32)
    );
    assert!(vault.width() > 100 && vault.width() < 640, "{r:?}");
    assert!(headless::snapshot(&mut h, "nothing.here").is_err());
}

#[test]
fn a_named_widget_can_be_pictured() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_content("helios.params", |ui| {
        let r = ui.button("Exposure").rect;
        name_rect(ui.ctx(), "node/1/exposure", r);
    });
    let mut h = headless::harness(shell, vec2(1024.0, 700.0));
    let img = headless::snapshot(&mut h, "node/1/exposure").unwrap();
    assert!(img.width() > 20 && img.height() > 10);
}

#[test]
fn keys_open_the_palette_which_runs_a_command() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1024.0, 700.0));
    h.run();
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::P);
    h.run();
    assert!(h.state().palette_open());
    h.get_by_role(egui::accesskit::Role::TextInput)
        .type_text("hide queue");
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert!(!h.state().palette_open());
    let placed = h.state().layout();
    assert!(!placed.placed().contains(&"helios.queue"));
    let history = h.state().history();
    let last = history.last().unwrap();
    assert_eq!(last.id, "helios.queue.toggle");
    assert_eq!(last.origin, Origin::User);
}

#[test]
fn commands_move_panels_and_the_layout_follows() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1024.0, 700.0));
    h.run();
    let agent = Origin::Agent("test".into());
    let s = h.state_mut();
    s.run("helios.log.detach", json!(null), agent.clone())
        .unwrap();
    s.run("helios.scopes.toggle", json!(null), agent.clone())
        .unwrap();
    h.run();
    let layout = h.state().layout();
    assert_eq!(layout.auxiliary.len(), 1);
    assert_eq!(layout.auxiliary[0].panel, "helios.log");
    assert!(!layout.placed().contains(&"helios.scopes"));
    // The detached panel is still drawn (embedded headless) and pictured.
    assert!(headless::snapshot(&mut h, "helios.log").is_ok());

    let s = h.state_mut();
    s.run("helios.log.dock", json!(null), agent.clone())
        .unwrap();
    s.run("helios.scopes.focus", json!(null), agent).unwrap();
    h.run();
    let layout = h.state().layout();
    assert!(layout.auxiliary.is_empty());
    let side = layout
        .areas
        .iter()
        .find(|(a, _)| *a == Area::SecondarySideBar)
        .unwrap();
    assert!(
        matches!(&side.1, Node::Tabs { panels, .. } if panels.contains(&"helios.scopes".to_string()))
    );
    assert_eq!(h.state().focused().as_deref(), Some("helios.scopes"));
}

#[test]
fn records_frames_at_a_fixed_rate() {
    let dir = std::env::temp_dir().join(format!("helios-gui-rec-{}", std::process::id()));
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(640.0, 480.0));
    h.run();
    let mut rec = Recorder::new(dir.join("clip.mp4"), 10).pngs();
    headless::record(&mut h, "helios.viewport", 3, &mut rec).unwrap();
    assert_eq!(rec.frames(), 3);
    let out = rec.finish().unwrap();
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 3);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn f11_toggles_full_screen_and_the_frame_has_its_buttons() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_title("photos");
    shell.draw_frame(true);
    let mut h = headless::harness(shell, vec2(1024.0, 700.0));
    h.run();
    h.get_by_label("photos");
    // F11 is egui-pop's full screen (its window plugin).
    h.key_press(Key::F11);
    h.run();
    let history = h.state().history();
    let last = history.last().unwrap();
    assert_eq!(last.id, "app.fullscreen");
    assert!(last.result.is_ok());
    let s = h.state_mut();
    for id in ["window.minimize", "window.maximize", "window.close"] {
        s.run(id, json!(null), Origin::User).unwrap();
    }
}

#[test]
fn themes_switch_from_the_palette_and_export_to_a_token_file() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(800.0, 500.0));
    h.run();
    let dark_fill = h.ctx.global_style().visuals.panel_fill;
    let s = h.state_mut();
    s.run("theme.helios-light", json!(null), Origin::User)
        .unwrap();
    h.run();
    let light_fill = h.ctx.global_style().visuals.panel_fill;
    assert_ne!(dark_fill, light_fill);
    assert!(!h.ctx.global_style().visuals.dark_mode);
    assert_eq!(h.state().theme_name(), "helios-light");

    let path =
        std::env::temp_dir().join(format!("helios-theme-{}.tokens.json", std::process::id()));
    let s = h.state_mut();
    s.run("theme.export", json!({ "path": path }), Origin::User)
        .unwrap();
    h.run();
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(file["$name"], "helios-light");
    assert_eq!(file["surface"]["panel"]["$type"], "color");
    std::fs::remove_file(&path).unwrap();

    // An unknown theme falls back to the dark one and says so.
    let s = h.state_mut();
    s.run("theme.select", json!({ "name": "nope" }), Origin::User)
        .unwrap();
    h.run();
    assert!(
        h.state()
            .notes
            .iter()
            .any(|n| n.contains("no theme `nope`"))
    );
    assert_eq!(h.ctx.global_style().visuals.panel_fill, dark_fill);
}

#[test]
fn a_profile_exports_imports_with_a_diff_and_is_used_by_name() {
    let dir = std::env::temp_dir().join(format!("helios-profiles-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_profiles_dir(dir.clone());
    shell
        .bind_user_keys("ctrl+shift+e", "theme.helios-light")
        .unwrap();
    let mut h = headless::harness(shell, vec2(800.0, 500.0));
    h.run();
    let s = h.state_mut();
    s.run("theme.helios-high-contrast", json!(null), Origin::User)
        .unwrap();
    s.run("helios.log.toggle", json!(null), Origin::User)
        .unwrap();
    h.run();
    // Saved as the "work" profile.
    let file = dir.join("work.helios-profile");
    let s = h.state_mut();
    s.run("profile.export", json!({ "path": file }), Origin::User)
        .unwrap();
    h.run();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("Mod+shift+e"), "{text}");
    assert!(text.contains("helios-high-contrast"), "{text}");

    // Elsewhere: another theme, then the profile is read and shown first.
    let mut other = Shell::new(helios_panels(), None);
    other.set_profiles_dir(dir.clone());
    let mut h2 = headless::harness(other, vec2(800.0, 500.0));
    h2.run();
    let s = h2.state_mut();
    s.run("profile.import", json!({ "path": file }), Origin::User)
        .unwrap();
    h2.run();
    let notes = h2.state().notes.join("\n");
    assert!(
        notes.contains("helios-dark → helios-high-contrast"),
        "{notes}"
    );
    assert_eq!(
        h2.state().theme_name(),
        "helios-dark",
        "nothing applied yet"
    );
    let s = h2.state_mut();
    s.run("profile.import.apply", json!(null), Origin::User)
        .unwrap();
    h2.run();
    assert_eq!(h2.state().theme_name(), "helios-high-contrast");
    assert!(!h2.state().layout().placed().contains(&"helios.log"));

    // By name, as a workspace does.
    let s = h2.state_mut();
    s.run("theme.helios-light", json!(null), Origin::User)
        .unwrap();
    s.run("profile.use", json!({ "name": "work" }), Origin::User)
        .unwrap();
    h2.run();
    assert_eq!(h2.state().theme_name(), "helios-high-contrast");
    assert_eq!(h2.state().profile_name(), "work");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_shell_applies_a_profile_resolved_for_this_machine() {
    use helios_profile::{Machine, System};
    let dir = std::env::temp_dir().join(format!("helios-variants-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("work.helios-profile"),
        "helios_profile = 1\nname = \"work\"\n\
         [theme]\nname = \"helios-dark\"\n\
         [on.machine.laptop.theme]\nname = \"helios-light\"\n\
         [on.machine.studio.theme]\nname = \"helios-high-contrast\"\n",
    )
    .unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_profiles_dir(dir.clone());
    shell.set_machine(Machine {
        system: System::current(),
        name: Some("laptop".into()),
        hidpi: false,
    });
    let mut h = headless::harness(shell, vec2(800.0, 500.0));
    h.run();
    let dark_fill = h.ctx.global_style().visuals.panel_fill;
    let s = h.state_mut();
    s.run("profile.use", json!({ "name": "work" }), Origin::User)
        .unwrap();
    h.run();
    assert_eq!(h.state().theme_name(), "helios-light");
    assert_ne!(h.ctx.global_style().visuals.panel_fill, dark_fill);
    let resolved = h
        .state_mut()
        .run("profile.resolved", json!(null), Origin::User)
        .unwrap();
    assert_eq!(resolved["machine"]["name"], "laptop");
    assert_eq!(
        resolved["from"],
        json!([{ "part": "theme", "key": "theme", "selector": "machine.laptop" }])
    );
    let notes = h.state().notes.join("\n");
    assert!(notes.contains("Theme theme from machine.laptop"), "{notes}");

    // Exported, the variants travel; this machine's view flattens them.
    let (all, mine) = (
        dir.join("all.helios-profile"),
        dir.join("mine.helios-profile"),
    );
    let s = h.state_mut();
    s.run("profile.export", json!({ "path": all }), Origin::User)
        .unwrap();
    s.run(
        "profile.export",
        json!({ "path": mine, "this_machine": true }),
        Origin::User,
    )
    .unwrap();
    h.run();
    let all = std::fs::read_to_string(&all).unwrap();
    assert!(all.contains("name = \"helios-dark\""), "{all}");
    assert!(all.contains("[on.machine.studio.theme]"), "{all}");
    let mine = std::fs::read_to_string(&mine).unwrap();
    assert!(mine.contains("name = \"helios-light\""), "{mine}");
    assert!(!mine.contains("[on."), "{mine}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn scopes_draw_the_image_they_are_given_in_each_mode() {
    use helios_gui::scopes::{Mode, picture};
    // A dark left half and a bright right half.
    let image = helios_core::Image::from_fn(helios_core::Region::sized(64, 32), 4, |x, _, c| {
        if c == 3 {
            1.0
        } else if x < 32 {
            0.05
        } else {
            0.8
        }
    });
    let hist = picture(&image, Mode::Histogram, false);
    let wave = picture(&image, Mode::Waveform, false);
    assert_ne!(hist.pixels, wave.pixels);
    // In the waveform, the left columns light low and the right ones high.
    let lit_rows = |x: usize| -> Vec<usize> {
        (0..wave.size[1])
            .filter(|y| wave.pixels[y * wave.size[0] + x].r() > 40)
            .collect()
    };
    let (left, right) = (lit_rows(10), lit_rows(200));
    assert!(left.iter().min() > right.iter().max(), "{left:?} {right:?}");

    let mut shell = Shell::new(helios_panels(), None);
    shell.set_scope_image(image);
    let mut h = headless::harness(shell, vec2(1024.0, 700.0));
    h.run();
    let s = h.state_mut();
    s.run("helios.scopes.focus", json!(null), Origin::User)
        .unwrap();
    s.run("scopes.parade", json!(null), Origin::User).unwrap();
    h.run();
    let shot = headless::snapshot(&mut h, "helios.scopes.picture").unwrap();
    assert!(shot.width() > 100, "{}", shot.width());
}

#[test]
fn the_library_rates_and_labels_what_is_selected_and_hands_the_changes_over() {
    use helios_gui::library::{Change, Entry, Op};
    let entries: Vec<Entry> = ["a", "b", "c"]
        .iter()
        .map(|id| Entry {
            id: id.to_string(),
            name: format!("{id}.raf"),
            ..Entry::default()
        })
        .collect();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries.clone());
    // A thumbnail arrives from another thread.
    let sink = shell.library_thumbnails();
    std::thread::spawn(move || {
        let image =
            helios_core::Image::from_fn(helios_core::Region::sized(600, 400), 4, |_, _, _| 0.2);
        sink.send(("b".into(), image)).unwrap();
    })
    .join()
    .unwrap();
    let mut h = headless::harness(shell, vec2(1024.0, 700.0));
    h.run();
    let s = h.state_mut();
    s.run("helios.filmstrip.focus", json!(null), Origin::User)
        .unwrap();
    s.run("vault.next", json!(null), Origin::User).unwrap();
    s.run("vault.next", json!(null), Origin::User).unwrap();
    h.run();
    // Keys as in darktable's lighttable, while the library has the focus.
    h.key_press(Key::Num3);
    h.key_press(Key::R);
    h.run();
    let s = h.state_mut();
    s.run("vault.label.blue", json!(null), Origin::User)
        .unwrap();
    s.run("vault.previous", json!(null), Origin::User).unwrap();
    s.run("vault.rate", json!({ "stars": 5 }), Origin::User)
        .unwrap();
    h.run();
    let s = h.state_mut();
    assert_eq!(s.library().selected, Some(0));
    // Rejected, with its stars kept beside the reject.
    assert_eq!(s.library().entries[1].rating, 3);
    assert!(s.library().entries[1].rejected);
    assert_eq!(s.library().entries[1].labels, ["blue"]);
    // What the user did, as commands: the last set of each field, and the
    // label added rather than the whole set.
    let changes = s.take_library_changes();
    let change = |image: &str, field: &str, op| Change {
        image: image.into(),
        field: field.into(),
        op,
    };
    assert_eq!(
        changes,
        [
            change("b", "rating", Op::Set(Some(json!(3)))),
            change("b", "rejected", Op::Set(Some(json!(true)))),
            change("b", "labels", Op::Add("blue".into())),
            change("a", "rating", Op::Set(Some(json!(5)))),
        ]
    );
    assert!(s.take_library_changes().is_empty());
    // A new list keeps the selection on its image.
    let mut reordered = entries;
    reordered.reverse();
    s.set_library(reordered);
    assert_eq!(s.library().selected, Some(2));
    h.run();
    let shot = headless::snapshot(&mut h, "helios.library.strip.1").unwrap();
    assert!(shot.width() > 30, "{}", shot.width());
}

#[test]
fn the_shell_runs_the_value_commands_with_their_origin() {
    use helios_values::Descriptor;
    let mut shell = Shell::new(helios_panels(), None);
    shell
        .values
        .lock()
        .unwrap()
        .declare("view/zoom", Descriptor::float("zoom", 0.1, 8.0, 1.0));
    let a = shell
        .run(
            "param.set",
            json!({ "path": "view/zoom", "step": 10 }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    assert!((a["value"].as_f64().unwrap() - 1.79).abs() < 1e-9, "{a}");
    assert_eq!(
        shell.history().last().unwrap().origin,
        Origin::Agent("a1".into())
    );
}

/// Drags a panel's tab (just above its body) to a point, over a few frames.
fn drag_tab(h: &mut egui_kittest::Harness<'_, Shell>, panel: &str, to: egui::Pos2) {
    let body = h.state().target_rect(panel).unwrap();
    let from = egui::pos2(body.left() + 24.0, body.top() - 12.0);
    h.hover_at(from);
    h.run();
    h.drag_at(from);
    h.run();
    for k in 1..=8 {
        h.hover_at(from + (to - from) * (k as f32 / 8.0));
        h.run();
    }
    h.drop_at(to);
    h.run();
    h.run();
}

fn area_of(shell: &Shell, panel: &str) -> Option<Area> {
    shell
        .layout()
        .areas
        .iter()
        .find(|(_, n)| {
            serde_json::to_string(n)
                .unwrap()
                .contains(&format!("\"{panel}\""))
        })
        .map(|(a, _)| *a)
}

#[test]
fn a_tab_dragged_to_another_area_moves_there_and_stays_after_a_restart() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    assert_eq!(
        area_of(h.state(), "helios.vault"),
        Some(Area::PrimarySideBar)
    );
    let target = h.state().target_rect("helios.params").unwrap().center();
    drag_tab(&mut h, "helios.vault", target);
    assert_eq!(
        area_of(h.state(), "helios.vault"),
        Some(Area::SecondarySideBar),
        "{:?}",
        h.state().notes
    );
    let saved = h.state().layout();
    let again = Shell::new(helios_panels(), Some(saved.clone()));
    assert_eq!(again.layout(), saved);
}

#[test]
fn a_tab_that_may_not_go_there_goes_back_and_says_why() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let target = h.state().target_rect("helios.vault").unwrap().center();
    // Shown while dragging, before the drop.
    let body = h.state().target_rect("helios.viewport").unwrap();
    let from = egui::pos2(body.left() + 24.0, body.top() - 12.0);
    h.hover_at(from);
    h.run();
    h.drag_at(from);
    h.run();
    for k in 1..=8 {
        h.hover_at(from + (target - from) * (k as f32 / 8.0));
        h.run();
    }
    assert!(
        h.state().target_rect("helios.drop").is_ok(),
        "a drop hint is shown"
    );
    h.drop_at(target);
    h.run();
    h.run();
    assert_eq!(area_of(h.state(), "helios.viewport"), Some(Area::Editors));
    assert_eq!(
        h.state().notes.last().map(String::as_str),
        Some("Image stays out of the primary side bar")
    );
}

#[test]
fn move_to_does_what_a_drag_does_under_the_same_rules() {
    let mut shell = Shell::new(helios_panels(), None);
    shell
        .run(
            "helios.log.moveTo",
            json!({ "area": "editors" }),
            Origin::User,
        )
        .unwrap();
    shell
        .run(
            "helios.graph.moveTo",
            json!({ "area": "bottom_panel" }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    assert_eq!(area_of(h.state(), "helios.log"), Some(Area::Editors));
    assert_eq!(area_of(h.state(), "helios.graph"), Some(Area::Editors));
    assert_eq!(
        h.state().notes.last().map(String::as_str),
        Some("Graph stays out of the bottom panel")
    );
}

#[test]
fn helios_s_declarations_sit_cleanly_in_egui_pop() {
    use egui_pop::model::ids::CommandId;
    let shell = Shell::new(helios_panels(), None);
    // No two commands claim the same keys; every binding runs a command.
    egui_pop_test::assert_keymap_clean(shell.app());
    let app = shell.app().handle();
    let diagnostics = app.diagnostics();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    // Helios's commands are egui-pop's, with Helios's keys.
    let rate = app
        .command(&CommandId::from_static("vault.rate.3"))
        .unwrap();
    assert_eq!(rate.category, "Vault");
    let keys = app.keys_for(&CommandId::from_static("workbench.systemMenu"));
    assert_eq!(keys, ["Alt+Space"]);
}
