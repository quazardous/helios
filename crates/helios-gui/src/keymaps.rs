//! Keymap packages (see `docs/gui.md`, "Keyboard shortcuts"): a set of
//! keys a user adds over the default map in one step, and takes away the
//! same way.
//!
//! A package is a keymap document (`keymaps/*.json`: keys, a command,
//! `-command` to remove one, `null` to take the keys from the layers
//! below, a `when`), loaded into the user's layer before the user's own
//! keys, which keep winning. Its clauses may name the default map's
//! scopes (`${library}`, `${culling}`, `${image}`, `${darkroom}`,
//! `${viewport}`), so a package follows them. What a package leaves out
//! of the keymap it imitates is listed with the reason (`left_out`).

use egui_pop::model::keymap::{DocBinding, Keys};
use serde::Deserialize;

use crate::keys::{culling, darkroom, image, library, viewport};

/// A key of the imitated keymap a package leaves out, and why.
#[derive(Clone, Debug, Deserialize)]
pub struct LeftOut {
    pub keys: String,
    pub what: String,
    pub reason: String,
}

/// A keymap package (see the module's notes).
#[derive(Clone, Debug, Deserialize)]
pub struct Package {
    pub name: String,
    pub title: String,
    pub keymap: Vec<DocBinding>,
    #[serde(default)]
    pub left_out: Vec<LeftOut>,
}

/// The packages Helios ships.
const PACKAGES: [&str; 1] = [include_str!("../keymaps/darktable-classic.json")];

impl Package {
    /// Its bindings, the default map's scopes put in their clauses.
    pub fn bindings(&self) -> Vec<DocBinding> {
        let scopes = [
            ("${library}", library()),
            ("${culling}", culling()),
            ("${image}", image()),
            ("${darkroom}", darkroom()),
            ("${viewport}", viewport()),
        ];
        self.keymap
            .iter()
            .map(|b| {
                let mut b = b.clone();
                for (name, clause) in &scopes {
                    b.when = b.when.replace(name, &format!("({clause})"));
                }
                b
            })
            .collect()
    }
}

/// Every package Helios ships.
pub fn packages() -> Vec<Package> {
    PACKAGES
        .iter()
        .map(|text| serde_json::from_str(text).expect("Helios's keymap packages parse"))
        .collect()
}

/// A package by name.
pub fn package(name: &str) -> Option<Package> {
    packages().into_iter().find(|p| p.name == name)
}

/// Whether two bindings are the same: same keys (as parsed), command
/// and clause.
pub fn same(a: &DocBinding, b: &DocBinding) -> bool {
    let keys = |k: &str| Keys::parse(k).ok();
    let same_keys = match (keys(&a.keys), keys(&b.keys)) {
        (Some(x), Some(y)) => x == y,
        _ => a.keys.trim() == b.keys.trim(),
    };
    same_keys && a.command == b.command && a.when == b.when
}
