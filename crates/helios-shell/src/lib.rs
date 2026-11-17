//! The desktop shell's logic, kept apart from any GUI toolkit so it is
//! tested fast and headless (see `docs/gui.md`):
//!
//! - [`when`]: the context expressions that say where a command applies
//!   (`graphFocus && !textInputFocus`);
//! - [`commands`]: every action is a named command, run the same way from a
//!   menu, a key, the palette, an agent or a test;
//! - [`policy`]: who may run a command (its origin) and whether its dry
//!   run is honoured, decided in one place before the command runs.
//!
//! Keys, menus, panels and their layout are declared to egui-pop, the UI
//! library the desktop is built on, which resolves keys, draws menus and
//! places panels (its workbench) from its own model.

pub mod commands;
pub mod policy;
pub mod when;
