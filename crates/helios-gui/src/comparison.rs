//! The Compare focus's state: which photos are compared, the active one,
//! the one marked best, and the zoom and pan, synchronised or not.
//!
//! It belongs to the focus (see docs/design/windows.md), not to the
//! library view, so the comparison can become a window of its own: the
//! focus keeps it, the Compare panel draws it
//! ([`crate::library::Library::compare_ui`]), and the Compare place on
//! the context stack ([`crate::contexts::COMPARISON`]) names its photos,
//! so a workspace reopens on the same comparison.
//!
//! The conventions are Lightroom's Survey and Compare views and
//! darktable's culling: the photos side by side, one of them active
//! (outlined), the arrow keys moving the active photo, the culling keys
//! marking it, zoom and pan synchronised by default.

use egui_pop::views::library::compare::{CompareState, MAX_PANES, PaneView};

/// Compares photos side by side, in the Compare place (`images`, else
/// the selection): Lightroom's C.
pub const COMMAND: &str = "library.compare";

/// The comparison of one focus.
#[derive(Clone, Debug)]
pub struct Comparison {
    /// The photos compared, in order. Empty: the selection is compared
    /// (two to [`MAX_PANES`] photos), as in the Library's Compare tab.
    photos: Vec<String>,
    /// The active photo's place among those shown.
    active: usize,
    /// The photo marked best.
    best: Option<String>,
    /// Zoom and pan follow each other across the photos.
    pub synced: bool,
    /// The zoom and pan of each photo, or the shared one when synced.
    pub(crate) view: CompareState,
}

impl Default for Comparison {
    fn default() -> Self {
        Comparison {
            photos: Vec::new(),
            active: 0,
            best: None,
            synced: true,
            view: CompareState::new(),
        }
    }
}

impl Comparison {
    /// The photos chosen to compare (empty: the selection is).
    pub fn photos(&self) -> &[String] {
        &self.photos
    }

    /// Compares these photos (at most [`MAX_PANES`]), the first active,
    /// every view back to fit; the same photos keep the active one, the
    /// best and the views.
    pub fn set_photos(&mut self, photos: Vec<String>) {
        let mut photos = photos;
        photos.truncate(MAX_PANES);
        if photos == self.photos {
            return;
        }
        self.photos = photos;
        self.active = 0;
        self.best = self.best.take().filter(|b| self.photos.contains(b));
        self.view.reset();
    }

    /// The photos shown: the chosen ones, else the selection's two to
    /// [`MAX_PANES`] (none otherwise).
    pub fn shown(&self, selection: &[String]) -> Vec<String> {
        if !self.photos.is_empty() {
            return self.photos.clone();
        }
        if (2..=MAX_PANES).contains(&selection.len()) {
            selection.to_vec()
        } else {
            Vec::new()
        }
    }

    /// The active photo's place among `count` shown.
    pub fn active_index(&self, count: usize) -> usize {
        self.active.min(count.saturating_sub(1))
    }

    /// The active photo among those shown.
    pub fn active<'a>(&self, shown: &'a [String]) -> Option<&'a String> {
        shown.get(self.active_index(shown.len()))
    }

    /// Makes the photo at `index` active.
    pub fn set_active(&mut self, index: usize) {
        self.active = index;
    }

    /// Moves the active photo by `step` among `count` shown, wrapping
    /// around as Lightroom's Survey does.
    pub fn step(&mut self, step: i64, count: usize) {
        if count == 0 {
            return;
        }
        let n = count as i64;
        let at = self.active_index(count) as i64;
        self.active = (at + step).rem_euclid(n) as usize;
    }

    /// The photo marked best.
    pub fn best(&self) -> Option<&str> {
        self.best.as_deref()
    }

    /// Marks a photo best (or none).
    pub fn set_best(&mut self, id: Option<String>) {
        self.best = id;
    }

    /// Photo `i`'s zoom and pan (the shared one when synced).
    pub fn view(&self, i: usize) -> PaneView {
        self.view.view(i, self.synced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_chosen_photos_win_over_the_selection_and_the_arrows_wrap() {
        let mut c = Comparison::default();
        assert!(c.synced, "synchronised by default");
        assert!(
            c.shown(&ids(&["a"])).is_empty(),
            "one photo is no comparison"
        );
        assert_eq!(c.shown(&ids(&["a", "b"])), ids(&["a", "b"]));
        c.set_photos(ids(&["a", "b", "c"]));
        assert_eq!(c.shown(&ids(&["x", "y"])), ids(&["a", "b", "c"]));
        let shown = c.shown(&[]);
        assert_eq!(c.active(&shown).map(String::as_str), Some("a"));
        c.step(-1, 3);
        assert_eq!(c.active(&shown).map(String::as_str), Some("c"));
        c.step(1, 3);
        c.step(1, 3);
        assert_eq!(c.active(&shown).map(String::as_str), Some("b"));
        c.set_best(Some("b".into()));
        // The same photos keep the active one and the best; others reset.
        c.set_photos(ids(&["a", "b", "c"]));
        assert_eq!(c.active_index(3), 1);
        c.set_photos(ids(&["b", "d"]));
        assert_eq!((c.active_index(2), c.best()), (0, Some("b")));
        c.set_photos(ids(&["d", "e", "f", "g", "h"]));
        assert_eq!(c.photos().len(), MAX_PANES);
        assert_eq!(c.best(), None);
    }
}
