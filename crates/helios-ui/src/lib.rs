//! Helios's controls, chosen by intent.
//!
//! **Rule: Helios makes no widgets.** This crate holds intent presets
//! only: each function says what a control is for (the Library's search,
//! "no photos yet", a primary action, a value still loading) and maps it
//! to an egui-pop widget and the theme's tokens. It has no drawing code
//! of its own: no painter, no exact-size allocation, no `Widget` impl. A
//! control egui-pop lacks is added to egui-pop first, then preset here.
//!
//! Screens name the intent, never a width, a colour or a hint string, so
//! a search field or an empty state looks the same in every module, and
//! the global style is set in one place (the theme, through egui-pop).
//!
//! ```
//! use egui_pop::egui;
//! use helios_ui::{Action, Empty, Search, Section, Value};
//!
//! fn library(ui: &mut egui::Ui, query: &mut String, photos: usize, size: &Value<String>) {
//!     helios_ui::section(ui, &Section::new("Library").description("Every photo Helios knows."));
//!     helios_ui::search(ui, query, Search::Library);
//!     if photos == 0 && helios_ui::empty(ui, Empty::NoPhotos) {
//!         // run the import command
//!     }
//!     helios_ui::value(ui, "Size on disk", size);
//!     if helios_ui::action(ui, Action::Primary, "Import…").clicked() {
//!         // run the import command
//!     }
//! }
//! ```

mod action;
mod notify;
mod search;
mod section;
mod state;
mod value;

pub use action::{Action, action};
pub use notify::{Notice, notify};
pub use search::{Scope, Search, search};
pub use section::{Section, section};
pub use state::{Empty, Loading, empty, failed, loading};
pub use value::{Value, value};
