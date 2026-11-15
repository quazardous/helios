//! The keymap at run time: commands Helios registers after start-up are
//! declared to egui-pop on their own, and a profile's keys are a whole
//! keymap document, loaded and exported by egui-pop.

use egui_pop::model::ids::{CommandId, OwnerId};
use egui_pop::model::keymap::Layer;
use helios_gui::{Shell, helios_panels};
use helios_shell::commands::{CommandSpec, Origin};
use serde_json::json;

#[test]
fn a_command_registered_later_is_declared_without_reconnecting() {
    let mut shell = Shell::new(helios_panels(), None);
    let before = shell.app().handle().plugins();
    shell
        .register(
            CommandSpec::new("test.later", "Say later", "Test"),
            |_, _| Ok(json!("later")),
        )
        .unwrap();
    let app = shell.app().handle().clone();
    let id = CommandId::from_static("test.later");
    // Known to egui-pop at once, owned by Helios's commands, run by
    // Helios's registry.
    let owner = app
        .commands()
        .into_iter()
        .find(|(c, _)| c.id == id)
        .unwrap()
        .1;
    assert_eq!(owner, OwnerId::from_static("helios.commands"));
    assert_eq!(
        shell.run("test.later", json!(null), Origin::User).unwrap(),
        json!("later")
    );
    assert_eq!(app.plugins(), before, "no plugin was reconnected");
    // Twice is refused, by Helios's registry.
    let again = shell.register(CommandSpec::new("test.later", "Again", "Test"), |_, _| {
        Ok(json!(null))
    });
    assert!(again.is_err());
}

#[test]
fn a_profile_s_keys_load_as_a_whole_keymap_with_their_clauses() {
    let dir = std::env::temp_dir().join(format!("helios-keymap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("cull.helios-profile"),
        "helios_profile = 1\nname = \"cull\"\n\
         [[keymap]]\nkeys = \"x\"\ncommand = \"vault.reject\"\n\
         when = \"focusedPanel == 'helios.filmstrip'\"\n\
         [[keymap]]\nkeys = \"\"\ncommand = \"-vault.next\"\n",
    )
    .unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_profiles_dir(dir.clone());
    shell.use_profile("cull").unwrap();
    let app = shell.app().handle().clone();
    let user = app.export_keymap(Layer::User);
    let x = user
        .bindings
        .iter()
        .find(|b| b.command.as_deref() == Some("vault.reject"))
        .unwrap();
    assert_eq!(x.when, "focusedPanel == 'helios.filmstrip'");
    // The default key the profile takes away is gone.
    let next = CommandId::from_static("vault.next");
    assert!(app.keys_for(&next).is_empty(), "{:?}", app.keys_for(&next));
    assert!(
        shell
            .notes
            .iter()
            .any(|n| n.contains("key bindings changed")),
        "{:?}",
        shell.notes
    );
    // Exported again, the profile keeps both, the clause included.
    let keymap = shell.profile().keymap.unwrap();
    assert!(
        keymap
            .iter()
            .any(|b| b.command == "vault.reject" && !b.when.is_empty())
    );
    assert!(keymap.iter().any(|b| b.command == "-vault.next"));
    std::fs::remove_dir_all(&dir).unwrap();
}
