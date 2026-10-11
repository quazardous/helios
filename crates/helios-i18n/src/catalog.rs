//! Looking text up: one Fluent bundle per locale, a fallback chain, and
//! the English written in the code at its end.

use std::collections::BTreeMap;

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource};
use fluent_syntax::ast::Entry;
use unic_langid::LanguageIdentifier;

use crate::ids::{MessageId, owned_by};
use crate::locale::{self, ENGLISH, PSEUDO};

type Bundle = FluentBundle<FluentResource>;

mod builtin {
    include!(concat!(env!("OUT_DIR"), "/builtin.rs"));
}

/// The translations Helios ships, as `(locale, file, source)`: every
/// `i18n/<locale>/*.ftl` of the repository but English's, embedded at
/// build time.
pub fn builtin_catalogs() -> &'static [(&'static str, &'static str, &'static str)] {
    builtin::BUILTIN
}

/// Text in the current locale.
///
/// A lookup names a message by its stable id ([`crate::ids`]) and gives
/// the English text: the current locale is tried, then its parents
/// (`fr-CA`, then `fr`), then that English, so a missing translation shows
/// English, never an id. English catalogs are not consulted: the English
/// is the code's, and `i18n/en` is generated from it for translators.
pub struct I18n {
    bundles: BTreeMap<String, Bundle>,
    locale: String,
    chain: Vec<String>,
}

impl I18n {
    /// No catalog, in `locale`.
    pub fn new(locale: &str) -> Self {
        let mut i18n = Self {
            bundles: BTreeMap::new(),
            locale: String::new(),
            chain: Vec::new(),
        };
        i18n.set_locale(locale);
        i18n
    }

    /// English only: what tests pin.
    pub fn english() -> Self {
        Self::new(ENGLISH)
    }

    /// Helios's own catalogs ([`builtin_catalogs`]), in `locale`.
    pub fn builtin(locale: &str) -> Self {
        let mut i18n = Self::new(locale);
        for (tag, file, source) in builtin_catalogs() {
            i18n.add_catalog(tag, source)
                .unwrap_or_else(|e| panic!("i18n/{tag}/{file}: {e}"));
        }
        i18n
    }

    /// Switches the locale; the next lookups use it. A tag naming no
    /// language selects English.
    pub fn set_locale(&mut self, tag: &str) {
        self.locale = locale::normalize(tag).unwrap_or_else(|| ENGLISH.into());
        self.chain = locale::chain(&self.locale);
    }

    /// The current locale's tag.
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// The locales that have a catalog.
    pub fn locales(&self) -> impl Iterator<Item = &str> {
        self.bundles.keys().map(String::as_str)
    }

    /// Adds Fluent source to a locale's catalog. Refused, with nothing
    /// added, when it does not parse or defines a message already there.
    pub fn add_catalog(&mut self, tag: &str, source: &str) -> Result<(), String> {
        self.add(tag, source, None)
    }

    /// Adds a plugin's (or a module's) Fluent source to a locale's catalog,
    /// at run time. Every message must belong to `owner` (see
    /// [`owned_by`]): a plugin translates its own commands, panels and
    /// parameters, never another's.
    pub fn add_plugin_catalog(
        &mut self,
        owner: &str,
        tag: &str,
        source: &str,
    ) -> Result<(), String> {
        self.add(tag, source, Some(owner))
    }

    fn add(&mut self, tag: &str, source: &str, owner: Option<&str>) -> Result<(), String> {
        let tag = locale::normalize(tag)
            .filter(|t| t != PSEUDO)
            .ok_or_else(|| format!("`{tag}` names no language"))?;
        let resource = FluentResource::try_new(source.to_string()).map_err(|(_, errors)| {
            let errors: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
            format!("does not parse: {}", errors.join("; "))
        })?;
        let bundle = self.bundles.entry(tag.clone()).or_insert_with(|| {
            let id: LanguageIdentifier = tag.parse().expect("a normalized tag parses");
            let mut b = FluentBundle::new_concurrent(vec![id]);
            b.set_use_isolating(false);
            b
        });
        for entry in resource.entries() {
            if let Entry::Message(m) = entry {
                let id = m.id.name;
                if let Some(owner) = owner.filter(|o| !owned_by(o, id)) {
                    return Err(format!("`{id}` does not belong to `{owner}`"));
                }
                if bundle.has_message(id) {
                    return Err(format!("`{id}` is already defined for {tag}"));
                }
            }
        }
        bundle.add_resource(resource).map_err(|errors| {
            let errors: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
            errors.join("; ")
        })
    }

    /// The text of `id` in the current locale, else in its parents, else
    /// `english`. With `args`, `english` is itself read as a Fluent pattern
    /// (`{ $count -> [one] … *[other] … }`); without, it is shown as is.
    pub fn text(&self, id: &MessageId, english: &str, args: Option<&FluentArgs>) -> String {
        let found = self
            .chain
            .iter()
            .filter(|tag| tag.as_str() != ENGLISH)
            .filter_map(|tag| self.bundles.get(tag))
            .find_map(|b| format(b, id, args));
        let text = found.unwrap_or_else(|| english_text(english, args));
        if self.locale == PSEUDO {
            fluent_pseudo::transform_dom(&text, false, true, true).into_owned()
        } else {
            text
        }
    }

    /// Whether `id` has a translation along the current chain.
    pub fn translated(&self, id: &MessageId) -> bool {
        self.chain
            .iter()
            .filter(|tag| tag.as_str() != ENGLISH)
            .filter_map(|tag| self.bundles.get(tag))
            .any(|b| format(b, id, None).is_some())
    }
}

/// `id` formatted by one bundle, if it has it.
fn format(bundle: &Bundle, id: &MessageId, args: Option<&FluentArgs>) -> Option<String> {
    let message = bundle.get_message(&id.id)?;
    let pattern = match &id.attr {
        Some(a) => message.get_attribute(a)?.value(),
        None => message.value()?,
    };
    let mut errors = Vec::new();
    Some(
        bundle
            .format_pattern(pattern, args, &mut errors)
            .into_owned(),
    )
}

/// The code's English, with its arguments placed.
fn english_text(english: &str, args: Option<&FluentArgs>) -> String {
    let Some(args) = args else {
        return english.to_string();
    };
    let source = format!("m = {}", english.replace('\n', "\n    "));
    let Ok(resource) = FluentResource::try_new(source) else {
        return english.to_string();
    };
    let mut bundle: Bundle =
        FluentBundle::new_concurrent(vec![ENGLISH.parse().expect("English is a language")]);
    bundle.set_use_isolating(false);
    if bundle.add_resource(resource).is_err() {
        return english.to_string();
    }
    format(&bundle, &MessageId::new("m"), Some(args)).unwrap_or_else(|| english.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{command_title, panel_title};

    const FR: &str = r#"
panel-helios-graph =
    .title = Graphe
images-selected =
    { $count ->
        [one] { $count } image sélectionnée
       *[other] { $count } images sélectionnées
    }
"#;

    const FR_CA: &str = "panel-helios-graph =\n    .title = Graphe (CA)\n";

    fn i18n() -> I18n {
        let mut i = I18n::new("fr-CA");
        i.add_catalog("fr", FR).unwrap();
        i.add_catalog("fr-CA", FR_CA).unwrap();
        i
    }

    fn count(n: i64) -> FluentArgs<'static> {
        let mut a = FluentArgs::new();
        a.set("count", n);
        a
    }

    #[test]
    fn the_chain_falls_back_to_the_parent_then_to_the_code_english() {
        let mut i = i18n();
        assert_eq!(
            i.text(&panel_title("helios.graph"), "Graph", None),
            "Graphe (CA)"
        );
        i.set_locale("fr");
        assert_eq!(
            i.text(&panel_title("helios.graph"), "Graph", None),
            "Graphe"
        );
        let palette = command_title("workbench.palette");
        assert_eq!(
            i.text(&palette, "Show all commands", None),
            "Show all commands"
        );
        assert!(!i.translated(&palette));
        i.set_locale("de");
        assert_eq!(i.text(&panel_title("helios.graph"), "Graph", None), "Graph");
    }

    #[test]
    fn switching_the_locale_at_run_time() {
        let mut i = i18n();
        i.set_locale("en");
        assert_eq!(i.locale(), "en");
        assert_eq!(i.text(&panel_title("helios.graph"), "Graph", None), "Graph");
        i.set_locale("fr_FR.UTF-8");
        assert_eq!(i.locale(), "fr-FR");
        assert_eq!(
            i.text(&panel_title("helios.graph"), "Graph", None),
            "Graphe"
        );
    }

    #[test]
    fn french_plurals_are_singular_for_zero_and_one() {
        let i = i18n();
        let id = MessageId::new("images-selected");
        let english = "{ $count ->\n    [one] { $count } image selected\n   *[other] { $count } images selected\n}";
        assert_eq!(
            i.text(&id, english, Some(&count(0))),
            "0 image sélectionnée"
        );
        assert_eq!(
            i.text(&id, english, Some(&count(1))),
            "1 image sélectionnée"
        );
        assert_eq!(
            i.text(&id, english, Some(&count(2))),
            "2 images sélectionnées"
        );
        let en = I18n::english();
        assert_eq!(en.text(&id, english, Some(&count(0))), "0 images selected");
        assert_eq!(en.text(&id, english, Some(&count(1))), "1 image selected");
    }

    #[test]
    fn a_plugin_adds_its_catalog_under_its_prefix() {
        let mut i = i18n();
        i.add_plugin_catalog(
            "acme.denoise",
            "fr",
            "command-acme-denoise-run =\n    .title = Débruiter\nacme-denoise-done = Fini\n",
        )
        .unwrap();
        let run = command_title("acme.denoise.run");
        assert_eq!(i.text(&run, "Denoise", None), "Débruiter");
        let theirs = "panel-helios-graph =\n    .title = Pirate\n";
        let e = i
            .add_plugin_catalog("acme.denoise", "fr", theirs)
            .unwrap_err();
        assert!(e.contains("does not belong"), "{e}");
        let again = "acme-denoise-done = Encore\n";
        let e = i
            .add_plugin_catalog("acme.denoise", "fr", again)
            .unwrap_err();
        assert!(e.contains("already defined"), "{e}");
        assert!(i.add_catalog("fr", "broken = {").is_err());
    }

    #[test]
    fn no_isolation_marks_around_arguments() {
        let mut i = I18n::new("fr");
        i.add_catalog("fr", "hello = Bonjour { $name } !\n")
            .unwrap();
        let mut a = FluentArgs::new();
        a.set("name", "Ada");
        assert_eq!(
            i.text(&MessageId::new("hello"), "Hello { $name }!", Some(&a)),
            "Bonjour Ada !"
        );
    }

    #[test]
    fn the_pseudo_locale_accents_and_lengthens() {
        let mut i = i18n();
        i.set_locale("x-pseudo");
        let english = "Show all commands";
        let pseudo = i.text(&command_title("workbench.palette"), english, None);
        assert_ne!(pseudo, english);
        assert!(!pseudo.is_ascii());
        let ratio = pseudo.chars().count() as f64 / english.chars().count() as f64;
        assert!(ratio > 1.2, "{pseudo}: {ratio}");
        // The pseudo-locale reads English, never a translation.
        let graph = i.text(&panel_title("helios.graph"), "Graph", None);
        assert!(!graph.contains("Graphe"), "{graph}");
    }

    #[test]
    fn the_builtin_catalogs_load() {
        let i = I18n::builtin("fr");
        assert!(i.locales().any(|l| l == "fr"));
        assert!(!i.locales().any(|l| l == "en"));
    }
}
