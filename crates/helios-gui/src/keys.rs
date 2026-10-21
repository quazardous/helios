//! Helios's keyboard map (see `docs/gui.md`, "Keyboard shortcuts"): the
//! default layer of egui-pop's keymap, every binding scoped by a `when`
//! clause; [`crate::keymaps`] adds packages over it.

use crate::contexts::{CULLING, DARKROOM, ORIENTATION};

/// While the library has the focus (its grid, the filmstrip or Compare),
/// anywhere but the darkroom: as in darktable's lighttable. egui-pop
/// leaves plain keys to a text field being typed in.
pub fn library() -> String {
    format!(
        "(focusedPanel == 'helios.vault' || focusedPanel == 'helios.filmstrip' || focusedPanel == 'helios.compare') && place != '{DARKROOM}'"
    )
}

/// While culling (a mode of the context stack): the culling keys work
/// wherever the focus is, the image's included.
pub fn culling() -> String {
    format!("mode.{ORIENTATION} == '{CULLING}'")
}

/// While the image has the focus, not culling (whose keys are the
/// vault's there): the mask tools.
pub fn image() -> String {
    format!("focusedPanel == 'helios.viewport' && mode.{ORIENTATION} != '{CULLING}'")
}

/// In the darkroom.
pub fn darkroom() -> String {
    format!("place == '{DARKROOM}'")
}

/// In the darkroom, or wherever the image has the focus: its zoom.
pub fn viewport() -> String {
    format!("focusedPanel == 'helios.viewport' || place == '{DARKROOM}'")
}

/// The default map: keys, command, `when`. darktable's keys where they
/// fit the credo, Lightroom's culling keys beside them.
pub fn defaults() -> Vec<(String, String, String)> {
    let mut keys: Vec<(String, String, String)> = Vec::new();
    let mut bind = |k: &str, command: &str, when: &str| {
        keys.push((k.into(), command.into(), when.into()));
    };
    let (library, culling, image, viewport) = (library(), culling(), image(), viewport());
    // Culling, in the library and while culling: digits rate (top row
    // and keypad alike), R toggles the reject (darktable), P, X and U
    // flag (Lightroom), F2 to F5 and 6 to 9 label. F1 stays the palette's
    // (egui-pop's): its red is 6, or F1 with the darktable package.
    let mut culling_keys: Vec<(String, String)> = (0..=5)
        .map(|n| (n.to_string(), format!("vault.rate.{n}")))
        .collect();
    for (k, command) in [
        ("R", "vault.reject"),
        ("P", "vault.flag.pick"),
        ("X", "vault.flag.reject"),
        ("U", "vault.flag.none"),
        ("F2", "vault.label.yellow"),
        ("F3", "vault.label.green"),
        ("F4", "vault.label.blue"),
        ("F5", "vault.label.purple"),
        ("6", "vault.label.red"),
        ("7", "vault.label.yellow"),
        ("8", "vault.label.green"),
        ("9", "vault.label.blue"),
        ("Right", "vault.next"),
        ("Left", "vault.previous"),
        ("S", "vault.stack.toggle"),
    ] {
        culling_keys.push((k.into(), command.into()));
    }
    for (k, command) in &culling_keys {
        for when in [&library, &culling] {
            bind(k, command, when);
        }
    }
    // Next and previous image while culling; in the grid Space selects.
    bind("Space", "vault.next", &culling);
    bind("Backspace", "vault.previous", &culling);
    // Drawing masks, while the image has the focus. Escape and Enter
    // bind only while a path is drawn: otherwise Escape is the
    // viewport's (it cancels a drag).
    for (k, command) in [
        ("C", "mask.add.circle"),
        ("E", "mask.add.ellipse"),
        ("G", "mask.add.gradient"),
        ("P", "mask.add.path"),
        // The mask display, as darktable's mask key.
        ("M", crate::mask_display::COMMAND),
    ] {
        bind(k, command, &image);
    }
    bind(
        "Enter",
        "mask.path.close",
        &format!("{image} && maskTool == 'path'"),
    );
    // While a path is drawn, mask editing is a pushed mode: Escape pops
    // it (egui-pop's `nav.back`), which stops the path; this binding
    // serves a path drawn without the stack. In zen mode Escape leaves
    // it (egui-pop's).
    bind(
        "Escape",
        "mask.cancel",
        &format!("{image} && maskTool == 'path' && !workbenchZen && !modePushed"),
    );
    bind(
        "Delete",
        "mask.point.remove",
        &format!("{image} && maskPoint"),
    );
    // The image's zoom, as in darktable: 100 %, fill, fit.
    for (k, command) in [
        ("Alt+1", "viewport.zoom.100"),
        ("Alt+2", "viewport.zoom.fill"),
        ("Alt+3", "viewport.zoom.fit"),
        ("Ctrl+Equals", "viewport.zoom.in"),
        ("Ctrl+Plus", "viewport.zoom.in"),
        ("Ctrl+Minus", "viewport.zoom.out"),
    ] {
        bind(k, command, &viewport);
    }
    // Navigation, everywhere (egui-pop adds the palette, settings, back,
    // full screen, undo and quit).
    bind("Ctrl+P", crate::quick_open::COMMAND, "");
    // Zen mode (egui-pop's, no default key): the image alone, as in
    // VS Code; Escape leaves it.
    bind("Ctrl+K Z", egui_pop::api::workbench::CMD_ZEN.as_str(), "");
    bind("Ctrl+K Ctrl+S", "workbench.keybindings", "");
    bind("Ctrl+B", "view.startSideBar.toggle", "");
    bind("Ctrl+Alt+B", "view.endSideBar.toggle", "");
    bind("Ctrl+J", "view.bottomPanel.toggle", "");
    keys
}
