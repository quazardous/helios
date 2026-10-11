//! The desktop shell's logic, kept apart from any GUI toolkit so it is
//! tested fast and headless (see `docs/gui.md`):
//!
//! - [`when`]: the context expressions that say where a command or a key
//!   applies (`graphFocus && !textInputFocus`);
//! - [`commands`]: every action is a named command, run the same way from a
//!   menu, a key, the palette, an agent or a test;
//! - [`keymap`]: keys bound to commands in layers (user > workspace >
//!   framework defaults > component suggestions); the framework alone
//!   resolves keys, a default can be removed, conflicts are listed;
//! - [`menus`]: menus are declared by id, their items commands;
//! - [`panels`]: panels are declared, the shell places them in its areas;
//! - [`layout`]: a workspace's arrangement (areas, editor groups, tabs,
//!   auxiliary windows), saved and restored.

pub mod commands;
pub mod keymap;
pub mod layout;
pub mod menus;
pub mod panels;
pub mod when;
