//! Helios's translations, over egui-pop's translation plugin
//! (`egui-pop-i18n`: catalogues, fallback chains, the language setting,
//! the extractor and the checks).
//!
//! What stays here is Helios's own:
//!
//! - [`CATALOGUES`]: the repository's `i18n/<locale>/*.ftl`, built in,
//!   given to `I18nPlugin::catalogues` by the desktop;
//! - [`ids`]: the message ids of what Helios declares (commands, panels,
//!   module parameters), derived from stable ids, never from labels;
//! - [`extract`]: the English catalogue (`i18n/en`) generated from those
//!   declarations and from the messages the code builds
//!   (`cargo xtask i18n extract`);
//! - [`plugins`]: an external plugin's own catalogues, held to its keys;
//! - [`message`]: a [`Message`] (a key, its arguments, its English) in the
//!   catalogues' language.
//!
//! Catalogues live in `i18n/<locale>/` as one file per area
//! (`commands.ftl`, `panels.ftl`, `modules.ftl`, `interface.ftl`).
//! `i18n/en` is generated and never edited; the other locales are
//! translations of it.

pub mod extract;
pub mod ids;
pub mod plugins;

pub use egui_pop_i18n::{Catalogue, FluentValue, I18n};
pub use extract::Declarations;
pub use helios_core::{Arg, Message};
pub use ids::MessageId;

mod builtin {
    include!(concat!(env!("OUT_DIR"), "/builtin.rs"));
}

/// Helios's catalogues, every locale's, built in.
pub static CATALOGUES: &[Catalogue] = builtin::BUILTIN;

/// The environment variable naming the language, before the system's,
/// when the language setting is `system`.
pub const ENV: &str = "HELIOS_LANG";

/// A message in the language of `i18n`: its key looked up with its
/// arguments (a message argument translated first, a list's items each),
/// else its English. Text without a key is shown as it is.
pub fn message(i18n: &I18n, m: &Message) -> String {
    if !m.is_keyed() {
        return m.english.clone();
    }
    let values: Vec<(&str, FluentValue<'static>)> = m
        .args
        .iter()
        .map(|(name, arg)| (name.as_str(), value(i18n, arg)))
        .collect();
    i18n.get(&m.key, &values)
        .unwrap_or_else(|| m.english.clone())
}

fn value(i18n: &I18n, arg: &Arg) -> FluentValue<'static> {
    match arg {
        Arg::Number(n) => FluentValue::from(*n),
        Arg::Text(t) => FluentValue::from(t.clone()),
        Arg::Message(m) => FluentValue::from(message(i18n, m)),
        Arg::List(l) => FluentValue::from(
            l.items
                .iter()
                .map(|a| match value(i18n, a) {
                    FluentValue::String(s) => s.into_owned(),
                    FluentValue::Number(n) => {
                        egui_pop_i18n::format::number(&i18n.locale(), n.value, None)
                    }
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
                .join(&l.separator),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_catalogues_load_beside_egui_pops() {
        let i = I18n::with_egui_pop("fr");
        i.add_catalogues(CATALOGUES).unwrap();
        assert!(i.locales().iter().any(|l| l == "fr"));
        assert_eq!(i.t(&ids::panel_title("helios.graph").key(), &[]), "Graphe");
    }

    #[test]
    fn a_message_is_shown_in_the_language_else_in_english() {
        let i = I18n::new("fr");
        i.add(
            "fr",
            "t-fell = { $module } : { $why }, { NUMBER($n, maximumFractionDigits: 1) } essais\nt-why = mémoire pleine\n",
        )
        .unwrap();
        // Keys through variables: these are not the catalogue's.
        let (fell, why, other) = ("t-fell", "t-why", "t-other");
        let m = Message::new(fell, "{ $module }: { $why }, { $n } tries")
            .with("module", "exposure")
            .with("why", Message::new(why, "out of memory"))
            .with("n", 2.5);
        assert_eq!(m.english, "exposure: out of memory, 2.5 tries");
        assert_eq!(message(&i, &m), "exposure : mémoire pleine, 2,5 essais");
        let missing = Message::new(other, "only { $x }").with("x", "English");
        assert_eq!(message(&i, &missing), "only English");
        assert_eq!(message(&i, &Message::plain("as is")), "as is");
    }

    #[test]
    fn an_import_summary_reads_in_french_with_its_reasons_and_plural() {
        let i = I18n::with_egui_pop("fr");
        i.add_catalogues(CATALOGUES).unwrap();
        // As `helios_xmp::Report::summary` builds it; keys through
        // variables, the extractor reads the code's.
        let (summary, group, unknown) = (
            "xmp-not-applied",
            "xmp-not-applied-group",
            "xmp-reason-unknown-module",
        );
        let groups = |n: usize| {
            Arg::list(
                (0..n).map(|k| {
                    Message::new(group, "{ $labels } ({ $reason })")
                        .with("labels", format!("m{k} v1"))
                        .with("reason", Message::new(unknown, "no Helios module"))
                }),
                "; ",
            )
        };
        let m = |n: usize| {
            Message::new(summary, "{ $count } of { $items } …")
                .with("count", n)
                .with("items", 4_usize)
                .with("groups", groups(n))
        };
        assert_eq!(
            message(&i, &m(2)),
            "2 éléments d’historique sur 4 non appliqués, conservés pour plus tard : \
             m0 v1 (pas de module Helios); m1 v1 (pas de module Helios)"
        );
        assert!(message(&i, &m(1)).starts_with("1 élément d’historique sur 4 non appliqué,"));
    }

    #[test]
    fn the_repository_catalogues_have_no_fault() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../i18n");
        let catalogs = egui_pop_i18n::check::Catalogs::read(&dir).unwrap();
        let r = egui_pop_i18n::check::check(&catalogs);
        assert!(r.faults.is_empty(), "{r}");
    }
}
