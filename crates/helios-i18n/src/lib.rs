//! Helios's translations, over Fluent (`fluent-bundle`).
//!
//! - [`ids`]: message ids derived mechanically from stable ids (a
//!   command's, a panel's, a module parameter's), never from labels; the
//!   scheme is documented there.
//! - [`catalog`]: [`I18n`], one bundle per locale; a lookup gives the
//!   stable id and the English text, and falls back along `fr-CA` → `fr` →
//!   that English. Plugins add their own catalogs at run time under their
//!   id; the locale switches at run time; `x-pseudo` shows accented,
//!   longer English.
//! - [`locale`]: which locale to use (`HELIOS_LANG`, then the setting, then
//!   the system's), and the fallback chain.
//! - [`extract`]: the English catalog generated from the declarations
//!   (`cargo xtask i18n extract`).
//! - [`check`]: coverage, missing, orphan and changed keys per locale, and
//!   the faults that fail a build (`cargo xtask i18n check`).
//!
//! Catalogs live in the repository's `i18n/<locale>/` as one file per area
//! (`commands.ftl`, `panels.ftl`, `modules.ftl`). `i18n/en` is generated
//! and never edited; the other locales are translations of it, embedded in
//! the build.

pub mod catalog;
pub mod check;
pub mod extract;
pub mod ids;
pub mod locale;

pub use catalog::I18n;
pub use extract::Declarations;
pub use fluent_bundle::{FluentArgs, FluentValue};
pub use ids::MessageId;
