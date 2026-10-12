//! The desktop shell's logic, kept apart from any GUI toolkit so it is
//! tested fast and headless (see `docs/gui.md`):
//!
//! - [`when`]: the context expressions that say where a command applies
//!   (`graphFocus && !textInputFocus`);
//! - [`commands`]: every action is a named command, run the same way from a
//!   menu, a key, the palette, an agent or a test;
//! - [`panels`]: panels are declared, the shell places them in its areas;
//! - [`layout`]: a workspace's arrangement (areas, editor groups, tabs,
//!   auxiliary windows), saved and restored.
//!
//! Keys and menus are declared to egui-pop, the UI library the desktop is
//! built on, which resolves keys and draws menus from its own model.

pub mod commands;
pub mod layout;
pub mod panels;
pub mod when;
