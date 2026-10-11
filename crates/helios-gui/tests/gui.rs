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
        panels: p.iter().map(|s| s.to_string()).collect(),
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
    for title in ["Image", "Vault", "Parameters", "Queue"] {
        h.get_by_label(title);
    }
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
    let last = h.state().registry.history().last().unwrap();
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
    assert_eq!(h.state().focused(), Some("helios.scopes"));
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
    h.key_press(Key::F11);
    h.run();
    let last = h.state().registry.history().last().unwrap();
    assert_eq!(last.id, "workbench.fullscreen");
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
    assert_eq!(h.state().theme().name, "helios-light");

    let path =
        std::env::temp_dir().join(format!("helios-theme-{}.tokens.json", std::process::id()));
    let s = h.state_mut();
    s.run("theme.export", json!({ "path": path }), Origin::User)
        .unwrap();
    h.run();
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(file["$name"], "helios-light");
    assert_eq!(file["bg"]["panel"]["$type"], "color");
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
        .keymap
        .bind(
            "ctrl+shift+e",
            "theme.helios-light",
            "",
            helios_shell::keymap::Layer::User,
            "user",
        )
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
        h2.state().theme().name,
        "helios-dark",
        "nothing applied yet"
    );
    let s = h2.state_mut();
    s.run("profile.import.apply", json!(null), Origin::User)
        .unwrap();
    h2.run();
    assert_eq!(h2.state().theme().name, "helios-high-contrast");
    assert!(!h2.state().layout().placed().contains(&"helios.log"));

    // By name, as a workspace does.
    let s = h2.state_mut();
    s.run("theme.helios-light", json!(null), Origin::User)
        .unwrap();
    s.run("profile.use", json!({ "name": "work" }), Origin::User)
        .unwrap();
    h2.run();
    assert_eq!(h2.state().theme().name, "helios-high-contrast");
    assert_eq!(h2.state().profile_name(), "work");
    std::fs::remove_dir_all(&dir).unwrap();
}
