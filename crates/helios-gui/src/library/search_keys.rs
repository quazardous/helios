//! The Library's filter keys, declared rather than hard-coded
//! (`docs/design/library-search.md`, sections 3 and 4).
//!
//! A filter key (`rating`, `camera`, `backup`…) is a [`SearchKey`]: its
//! name, its label, the values it takes, the vault facet that counts them
//! and how a picked value becomes a term of the search text. The built-in
//! keys use this contract, and a module adds its own with
//! [`SearchKeys::register`] (a culling module's `sharpness`, an
//! extractor's kept key): the facets bar, and later the filter button,
//! the suggestions and the advanced rows, list whatever is registered.
//!
//! This is a typed registry for now: `docs/design/hooks.md` makes search
//! keys the hook point `library.search.keys`, and that kernel is not
//! built yet. A declaration is plain data plus `fn` pointers (no state,
//! `Send + Sync`), each with its owner, so the same declarations are
//! collected through that hook point once it exists. Executing a key
//! stays the vault's rules: nothing here runs per photo.

use helios_core::Message;

/// The values a key takes.
#[derive(Clone, Debug, PartialEq)]
pub enum Values {
    /// One of a known list (`flag:pick`).
    Finite(Vec<String>),
    /// Any text (`camera:"Z 8"`), suggested from the values seen.
    Open,
    /// A number of stars, compared (`rating>=3`).
    Stars,
    /// A capture day or a range of days (`date:2024-06-01..2024-06-30`).
    Date,
}

/// How a picked value is written in the search text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Term {
    /// `key:value`, the value quoted when it holds a space.
    Is,
    /// `key>=value`: this value or more.
    AtLeast,
}

/// How a value is shown in the facets bar and on a chip.
#[derive(Clone, Copy, Debug)]
pub enum ValueText {
    /// As it is stored.
    Plain,
    /// A number of stars (`3★`).
    Stars,
    /// In words, translated (`cull:soft` reads "soft").
    Words(fn(&str) -> Message),
}

/// One filter key, as its owner declares it.
#[derive(Clone, Debug)]
pub struct SearchKey {
    /// What the search text types before the colon (`camera`).
    pub key: String,
    /// Who declared it: `helios` for the built-in keys, else the module
    /// or plugin id.
    pub owner: String,
    /// Its name in the facets bar and the filter menu.
    pub label: Message,
    pub values: Values,
    /// The vault facet counting its values, when the vault counts them.
    pub facet: Option<String>,
    pub term: Term,
    pub value_text: ValueText,
}

impl SearchKey {
    /// A key taking any text, written `key:value`, with no facet.
    pub fn new(owner: &str, key: &str, label: Message) -> Self {
        SearchKey {
            key: key.into(),
            owner: owner.into(),
            label,
            values: Values::Open,
            facet: None,
            term: Term::Is,
            value_text: ValueText::Plain,
        }
    }

    /// Its values counted by this vault facet.
    pub fn facet(mut self, facet: &str) -> Self {
        self.facet = Some(facet.into());
        self
    }

    pub fn values(mut self, values: Values) -> Self {
        self.values = values;
        self
    }

    /// One of these values.
    pub fn finite(self, values: &[&str]) -> Self {
        self.values(Values::Finite(
            values.iter().map(|v| v.to_string()).collect(),
        ))
    }

    pub fn term(mut self, term: Term) -> Self {
        self.term = term;
        self
    }

    pub fn value_text(mut self, text: ValueText) -> Self {
        self.value_text = text;
        self
    }

    /// The search text's term taking `value` (`label:red`,
    /// `camera:"Z 8"`, `rating>=3`).
    pub fn term_of(&self, value: &str) -> String {
        match self.term {
            Term::Is => format!("{}:{}", self.key, super::quoted(value)),
            Term::AtLeast => format!("{}>={value}", self.key),
        }
    }

    /// A value as the facets bar and a chip show it.
    pub fn shown(&self, ctx: &egui_pop::egui::Context, value: &str) -> String {
        match self.value_text {
            ValueText::Plain => value.to_string(),
            ValueText::Stars => format!("{value}★"),
            ValueText::Words(words) => crate::i18n::text(ctx, words(value)),
        }
    }
}

/// Every filter key the Library knows, in the order menus list them.
#[derive(Clone, Debug)]
pub struct SearchKeys(Vec<SearchKey>);

/// The built-in keys' owner.
pub const HELIOS: &str = "helios";

impl Default for SearchKeys {
    fn default() -> Self {
        Self::builtin()
    }
}

impl SearchKeys {
    /// The built-in keys: what the search field and the facets bar know
    /// today, facets first in the bar's order.
    pub fn builtin() -> Self {
        let key =
            |key: &str, id: &str, label: &str| SearchKey::new(HELIOS, key, Message::new(id, label));
        SearchKeys(vec![
            key("rating", "gui-library-facet-rating", "Rating")
                .facet("rating")
                .values(Values::Stars)
                .term(Term::AtLeast)
                .value_text(ValueText::Stars),
            key("label", "gui-library-facet-label", "Label")
                .facet("label")
                .finite(&["red", "yellow", "green", "blue", "purple"]),
            key("flag", "gui-library-facet-flag", "Flag")
                .facet("flag")
                .finite(&["pick", "reject", "none"]),
            key("camera", "gui-library-facet-camera", "Camera").facet("camera"),
            key("lens", "gui-library-facet-lens", "Lens").facet("lens"),
            key("type", "gui-library-facet-type", "Type").facet("file_type"),
            key("presence", "gui-library-facet-presence", "Presence")
                .facet("presence")
                .finite(&["present", "offline", "missing"]),
            key("backup", "gui-library-facet-backup", "Backup")
                .facet("backup")
                .finite(&["met", "short", "unverified", "none"]),
            key("cull", "gui-library-facet-cull", "Culling")
                .facet("cull")
                .finite(&helios_vault::cull::STATES)
                .value_text(ValueText::Words(super::cull_value)),
            key("tag", "gui-library-key-tag", "Tag"),
            key("keyword", "gui-library-key-keyword", "Keyword"),
            key("date", "gui-library-key-date", "Capture date").values(Values::Date),
            key("folder", "gui-library-folders", "Folders"),
            key("collection", "gui-library-albums", "Albums"),
            key("import", "gui-library-imports", "Imports"),
        ])
    }

    /// Adds a module's key; a key already declared is refused, whoever
    /// declared it (a module never redefines another's key).
    pub fn register(&mut self, key: SearchKey) -> Result<(), String> {
        if let Some(had) = self.get(&key.key) {
            return Err(format!(
                "the search key `{}` is already declared by {}",
                key.key, had.owner
            ));
        }
        if key.key.is_empty() || !key.key.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(format!(
                "a search key is letters, digits and `_`, not `{}`",
                key.key
            ));
        }
        self.0.push(key);
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&SearchKey> {
        self.0.iter().find(|k| k.key == key)
    }

    pub fn iter(&self) -> impl Iterator<Item = &SearchKey> {
        self.0.iter()
    }

    /// The keys a vault facet counts, in order: the facets bar's lines.
    pub fn faceted(&self) -> impl Iterator<Item = (&SearchKey, &str)> {
        self.0
            .iter()
            .filter_map(|k| k.facet.as_deref().map(|f| (k, f)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_facets_keep_the_bar_s_order_and_terms() {
        let keys = SearchKeys::builtin();
        let facets: Vec<&str> = keys.faceted().map(|(_, f)| f).collect();
        assert_eq!(
            facets,
            [
                "rating",
                "label",
                "flag",
                "camera",
                "lens",
                "file_type",
                "presence",
                "backup",
                "cull"
            ]
        );
        // The terms the facets bar wrote before the keys were declared.
        let term = |k: &str, v: &str| keys.get(k).unwrap().term_of(v);
        assert_eq!(term("rating", "3"), "rating>=3");
        assert_eq!(term("type", "cr3"), "type:cr3");
        assert_eq!(term("camera", "Z 8"), "camera:\"Z 8\"");
        assert_eq!(term("label", "red"), "label:red");
        for (k, f) in keys.faceted() {
            assert_eq!(k.owner, HELIOS);
            assert!(f == "file_type" || k.key == f, "{} counted by {f}", k.key);
        }
    }

    #[test]
    fn a_module_adds_a_key_and_never_redefines_one() {
        let mut keys = SearchKeys::builtin();
        let sharp = SearchKey::new(
            "culling",
            "sharpness",
            Message::new("test-sharpness", "Sharpness"),
        )
        .facet("sharpness")
        .finite(&["high", "low"]);
        keys.register(sharp).unwrap();
        let (last, facet) = keys.faceted().last().unwrap();
        assert_eq!((last.owner.as_str(), facet), ("culling", "sharpness"));
        assert_eq!(last.term_of("high"), "sharpness:high");

        let again = SearchKey::new("other", "camera", Message::new("test-cam", "Cam"));
        let err = keys.register(again).unwrap_err();
        assert!(err.contains("helios"), "{err}");
        let bad = SearchKey::new("other", "a b", Message::new("test-ab", "AB"));
        assert!(keys.register(bad).is_err());
    }
}
