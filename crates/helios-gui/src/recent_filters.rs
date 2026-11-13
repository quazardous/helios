//! Recent filters (`docs/design/library.md`, L6): the filters last used
//! in the Library, newest first, deduplicated and capped, and the ones
//! the user pinned, kept per library in the vault (a library document,
//! `library/filters`, key `history`) so they survive a restart. The
//! switcher (`library.filter.switch`, Ctrl+Shift+K) lists the pinned
//! ones, the recent ones and the smart albums in one filtered list;
//! choosing one applies it to the grid (`library.filter.apply`), as
//! darktable's collection history does.

use egui_pop::views::library::filter::Filter;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Opens the switcher over the filter bar.
pub const SWITCH_COMMAND: &str = "library.filter.switch";
/// Lists the pinned and recent filters and the smart albums.
pub const RECENT_COMMAND: &str = "library.filter.recent";
/// Applies a filter (`query` typed as in the search field, or `filter`).
pub const APPLY_COMMAND: &str = "library.filter.apply";
/// Pins a filter (`query` or `filter`, else the filter in force), or
/// unpins it (`pinned: false`).
pub const PIN_COMMAND: &str = "library.filter.pin";

/// The vault's library namespace and key the history is kept under.
pub const NS: &str = "filters";
pub const KEY: &str = "history";

/// How many recent filters are kept (darktable keeps ten).
pub const CAP: usize = 10;

/// How long a typed filter rests before it counts as used, in seconds.
pub const TYPED_REST: f64 = 2.0;

/// The pinned and recent filters of a library.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterHistory {
    /// Newest first, none of them pinned, at most [`CAP`].
    pub recent: Vec<Filter>,
    /// In the order pinned.
    pub pinned: Vec<Filter>,
}

impl FilterHistory {
    /// Read from its vault document; a document it cannot read is an
    /// empty history.
    pub fn of(doc: Option<&Value>) -> FilterHistory {
        doc.and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_default()
    }

    /// Its vault document.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// `filter` used: first of the recent ones, once; an empty filter or
    /// a pinned one is not listed. Whether the history changed.
    pub fn record(&mut self, filter: &Filter) -> bool {
        if filter.is_empty() || self.pinned.contains(filter) {
            return false;
        }
        if self.recent.first() == Some(filter) {
            return false;
        }
        self.recent.retain(|f| f != filter);
        self.recent.insert(0, filter.clone());
        self.recent.truncate(CAP);
        true
    }

    /// Pins `filter` (out of the recent ones), or unpins it (back first
    /// of the recent ones). Whether the history changed.
    pub fn pin(&mut self, filter: &Filter, pinned: bool) -> bool {
        if filter.is_empty() {
            return false;
        }
        if pinned {
            if self.pinned.contains(filter) {
                return false;
            }
            self.recent.retain(|f| f != filter);
            self.pinned.push(filter.clone());
            true
        } else {
            let before = self.pinned.len();
            self.pinned.retain(|f| f != filter);
            before != self.pinned.len() && {
                self.record(filter);
                true
            }
        }
    }

    /// Recorded before the history was read: `earlier` (oldest first)
    /// put on top of this one, read from the vault.
    pub fn merged(mut self, earlier: &[Filter]) -> FilterHistory {
        for f in earlier {
            self.record(f);
        }
        self
    }
}

/// A filter written as it is typed in the search field: `rating>=3
/// label:red flag:pick tag:beach date:2024-06-01..2024-06-30`, then its
/// words (Helios's own terms among them: `collection:`, `cull:`…).
pub fn text(filter: &Filter) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(r) = filter.min_rating {
        parts.push(format!("rating>={r}"));
    }
    parts.extend(filter.labels.iter().map(|l| format!("label:{}", l.name())));
    parts.extend(filter.flags.iter().map(|f| format!("flag:{}", f.name())));
    parts.extend(
        filter
            .tags
            .iter()
            .map(|t| format!("tag:{}", crate::library::quoted(t))),
    );
    match (&filter.from, &filter.to) {
        (Some(a), Some(b)) => parts.push(format!("date:{a}..{b}")),
        (Some(a), None) => parts.push(format!("from:{a}")),
        (None, Some(b)) => parts.push(format!("to:{b}")),
        (None, None) => {}
    }
    if !filter.text.is_empty() {
        parts.push(filter.text.clone());
    }
    parts.join(" ")
}

/// The filter an agent names: `filter` (an object, as listed) or
/// `query` (typed as in the search field); none when neither is given.
pub fn filter_arg(args: &Value) -> Result<Option<Filter>, String> {
    if let Some(f) = args.get("filter").filter(|f| !f.is_null()) {
        return serde_json::from_value(f.clone())
            .map(Some)
            .map_err(|e| format!("`filter`: {e}"));
    }
    match args.get("query") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(q)) => Ok(Some(Filter::parse(q))),
        Some(other) => Err(format!("`query`: {other} is not text")),
    }
}

/// What the switcher lists, in order: the pinned filters, the recent
/// ones, then the smart albums (`id`, `name`), each as its filter (a
/// smart album's is its `collection:` term).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub filter: Filter,
    pub kind: Kind,
    /// What it shows: the query, or a smart album's name.
    pub label: String,
}

/// Where an entry of the switcher comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pinned,
    Recent,
    Smart,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Pinned => "pinned",
            Kind::Recent => "recent",
            Kind::Smart => "smart",
        }
    }
}

/// The switcher's entries (see [`Entry`]); a smart album already listed
/// as a pinned or recent filter is not listed again.
pub fn entries(history: &FilterHistory, smart: &[(String, String)]) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for (list, kind) in [
        (&history.pinned, Kind::Pinned),
        (&history.recent, Kind::Recent),
    ] {
        out.extend(list.iter().map(|f| Entry {
            filter: f.clone(),
            kind,
            label: text(f),
        }));
    }
    for (id, name) in smart {
        let filter = Filter {
            text: format!("collection:{}", crate::library::quoted(id)),
            ..Filter::default()
        };
        if out.iter().any(|e| e.filter == filter) {
            continue;
        }
        out.push(Entry {
            filter,
            kind: Kind::Smart,
            label: name.clone(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(q: &str) -> Filter {
        Filter::parse(q)
    }

    #[test]
    fn recording_dedupes_puts_first_and_caps() {
        let mut h = FilterHistory::default();
        assert!(h.record(&f("rating>=3")));
        assert!(h.record(&f("label:red")));
        assert!(!h.record(&f("label:red")), "already first");
        assert!(h.record(&f("rating>=3")));
        assert_eq!(h.recent, [f("rating>=3"), f("label:red")]);
        assert!(!h.record(&Filter::default()), "nothing filtered");
        for n in 0..20 {
            h.record(&f(&format!("word{n}")));
        }
        assert_eq!(h.recent.len(), CAP);
        assert_eq!(h.recent[0], f("word19"));
    }

    #[test]
    fn pinned_ones_leave_the_recent_list_and_come_back_unpinned() {
        let mut h = FilterHistory::default();
        h.record(&f("a"));
        h.record(&f("b"));
        assert!(h.pin(&f("a"), true));
        assert!(!h.pin(&f("a"), true));
        assert_eq!(h.recent, [f("b")]);
        assert!(!h.record(&f("a")), "a pinned filter is not recent");
        assert!(h.pin(&f("a"), false));
        assert_eq!(h.recent, [f("a"), f("b")]);
        assert!(h.pinned.is_empty());
        let back = FilterHistory::of(Some(&h.to_json()));
        assert_eq!(back, h);
        assert_eq!(
            FilterHistory::of(Some(&serde_json::json!(3))),
            FilterHistory::default()
        );
    }

    #[test]
    fn a_filter_reads_back_from_its_text() {
        for q in [
            "rating>=3 label:red flag:pick tag:beach date:2024-06-01..2024-06-30 sea camera:\"Z 8\"",
            "from:2024-06-01 collection:quick",
            "cull:sharp",
        ] {
            assert_eq!(Filter::parse(&text(&f(q))), f(q), "{q}");
        }
    }

    #[test]
    fn the_switcher_lists_pinned_recent_then_smart_albums() {
        let mut h = FilterHistory::default();
        h.record(&f("collection:best"));
        h.record(&f("rating>=4"));
        h.pin(&f("label:red"), true);
        let e = entries(
            &h,
            &[
                ("best".into(), "Best".into()),
                ("late".into(), "Late".into()),
            ],
        );
        let kinds: Vec<(&str, Kind)> = e.iter().map(|e| (e.label.as_str(), e.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("label:red", Kind::Pinned),
                ("rating>=4", Kind::Recent),
                ("collection:best", Kind::Recent),
                ("Late", Kind::Smart),
            ]
        );
        assert_eq!(
            filter_arg(&serde_json::json!({ "query": "rating>=4" })).unwrap(),
            Some(f("rating>=4"))
        );
        let listed = serde_json::to_value(&e[0].filter).unwrap();
        assert_eq!(
            filter_arg(&serde_json::json!({ "filter": listed })).unwrap(),
            Some(f("label:red"))
        );
        assert_eq!(filter_arg(&serde_json::json!({})).unwrap(), None);
    }
}
