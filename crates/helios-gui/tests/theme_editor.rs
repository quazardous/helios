//! egui-pop's theme editor in Helios: a page of the settings window under
//! Appearance, editing the theme in use (Helios's dark and light pair),
//! its edits taken as the theme's overrides.

use egui::{Color32, Key, Modifiers, vec2};
use egui_kittest::kittest::Queryable;
use egui_pop::api::theme_editor::THEME_EDITOR;
use helios_gui::theme::{Extra, Tokens};
use helios_gui::{Shell, headless, helios_panels, theme_editor};
use helios_shell::commands::Origin;
use serde_json::json;

#[test]
fn the_theme_editor_page_edits_a_token_of_the_theme_in_use() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1600.0, 1000.0));
    h.run();
    let app = h.state().app().handle().clone();
    let page = app
        .settings_pages()
        .into_iter()
        .find(|p| p.id == theme_editor::PAGE)
        .expect("the theme editor's page is mounted");
    assert_eq!(page.parent.as_deref(), Some(theme_editor::SECTION));

    h.state_mut()
        .run(
            "app.settings.page",
            json!({ "page": theme_editor::PAGE }),
            Origin::User,
        )
        .unwrap();
    h.run();
    h.run();
    assert!(egui_pop::api::settings_window::is_open(&app));
    // Helios's dark and light themes are a pair: the editor offers the
    // variant to edit.
    assert!(h.query_by_label("Variant edited").is_some());

    // A Helios token, typed in its hex field.
    app.service(&THEME_EDITOR)
        .unwrap()
        .select("viewport.surround");
    h.run();
    let field = h.get_by_label("Hex of viewport.surround");
    field.scroll_to_me();
    field.focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    h.get_by_label("Hex of viewport.surround")
        .type_text("#405060");
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();

    // Installed at once: the theme's overrides, what Helios draws with
    // and what a profile saves.
    assert_eq!(
        app.theme().spec().overrides.get("viewport.surround"),
        Some(&json!("#405060"))
    );
    assert_eq!(
        Extra::of(&Tokens::of(&h.ctx)).viewport_surround,
        Color32::from_rgb(0x40, 0x50, 0x60)
    );
    let saved = h.state().profile().theme.unwrap();
    assert_eq!(saved.name, "helios-dark");
    assert_eq!(
        saved.overrides.get("viewport.surround"),
        Some(&json!("#405060"))
    );
}
