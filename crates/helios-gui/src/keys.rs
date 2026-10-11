//! egui key events as the keymap's chords.

use egui::{Key, Modifiers};
use helios_shell::keymap::Chord;

/// The keymap's name for a key: punctuation by its symbol, the rest by
/// its lowercase name (`a`, `3`, `f1`, `space`, `left`).
pub fn key_name(key: Key) -> String {
    let symbol = key.symbol_or_name();
    if symbol.len() == 1 && !symbol.chars().all(char::is_alphanumeric) {
        symbol.to_string()
    } else {
        key.name().to_lowercase()
    }
}

pub fn chord(key: Key, modifiers: Modifiers) -> Chord {
    Chord {
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
        meta: modifiers.mac_cmd,
        key: key_name(key),
    }
}
