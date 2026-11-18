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

use egui_pop::views::library::compare::{CompareState, PANE_LIMIT, PaneView};

/// How many photos a comparison holds at most: as many as egui-pop's
/// Compare surface lays out (a 4 by 4 survey).
pub const MAX_PANES: usize = PANE_LIMIT;

/// Compares photos side by side, in the Compare place (`images`, else
/// the selection): Lightroom's C. Without `images` while comparing, it
/// leaves Compare (a toggle).
pub const COMMAND: &str = "library.compare";

/// Leaves Compare: back to the place it was entered from (the grid), as
/// Escape, the Compare view's "Back to grid", the Compare command again
/// (a toggle) and the Compare panel closed do.
pub const LEAVE_COMMAND: &str = "compare.leave";

/// Adds photos to the open comparison (`images`, else the gallery's
/// selection), after those compared or before the one at `at`; at most
/// [`MAX_PANES`] in all.
pub const ADD_COMMAND: &str = "compare.add";

/// Takes photos out of the comparison (`images`, else the active one);
/// the last one out leaves Compare.
pub const REMOVE_COMMAND: &str = "compare.remove";

/// Compares exactly these photos (`images`, one to [`MAX_PANES`]),
/// entering Compare when it is not shown; never a toggle.
pub const SET_COMMAND: &str = "compare.set";

/// Empties the comparison and leaves Compare.
pub const CLEAR_COMMAND: &str = "compare.clear";

/// The comparison's photos, the active one, the best and the sync;
/// with `image`, makes that compared photo active first.
pub const ACTIVE_COMMAND: &str = "compare.active";

/// The comparison of one focus. It never writes a gallery's selection
/// or cursor: its active photo is its own.
#[derive(Clone, Debug)]
pub struct Comparison {
    /// The photos compared, in order. Empty: the selection is compared
    /// (one to [`MAX_PANES`] photos), as in the Library's Compare tab.
    photos: Vec<String>,
    /// The active photo's place among those shown.
    active: usize,
    /// The photo marked best.
    best: Option<String>,
    /// Zoom and pan follow each other across the photos.
    pub synced: bool,
    /// The zoom and pan of each photo, or the shared one when synced.
    pub(crate) view: CompareState,
    /// The gallery's selection when last seen (none: not yet, for these
    /// photos), so only a new one makes a photo active.
    seen: Option<Vec<String>>,
}

impl Default for Comparison {
    fn default() -> Self {
        Comparison {
            photos: Vec::new(),
            active: 0,
            best: None,
            synced: true,
            view: CompareState::new(),
            seen: None,
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
        self.seen = None;
    }

    /// The list once `ids` are added before the photo at `at` (none, or
    /// past the end: after the photos compared), without the ones
    /// already there, at most [`MAX_PANES`]: `(list, added)`.
    pub fn with_added(&self, ids: &[String], at: Option<usize>) -> (Vec<String>, Vec<String>) {
        let mut list = self.photos.clone();
        let mut at = at.unwrap_or(list.len()).min(list.len());
        let mut added = Vec::new();
        for id in ids {
            if list.len() >= MAX_PANES {
                break;
            }
            if !list.contains(id) && !added.contains(id) {
                list.insert(at, id.clone());
                at += 1;
                added.push(id.clone());
            }
        }
        (list, added)
    }

    /// The list once `id` moved to `to` (its place once moved), or
    /// `None` when it is not compared.
    pub fn with_moved(&self, id: &str, to: usize) -> Option<Vec<String>> {
        let mut list = self.photos.clone();
        let from = list.iter().position(|p| p == id)?;
        let photo = list.remove(from);
        list.insert(to.min(list.len()), photo);
        Some(list)
    }

    /// Edits the list in place (photos added or taken out): the active
    /// photo stays active while it is compared (else the one taking its
    /// place), the best stays while compared, and each photo keeps its
    /// own zoom and pan wherever it goes in the list.
    pub fn edit(&mut self, photos: Vec<String>) {
        let active = self.photos.get(self.active).cloned();
        let at = self.active;
        let mut photos = photos;
        photos.truncate(MAX_PANES);
        self.follow_views(&photos);
        self.photos = photos;
        self.active = active
            .and_then(|a| self.photos.iter().position(|p| *p == a))
            .unwrap_or_else(|| at.min(self.photos.len().saturating_sub(1)));
        self.best = self.best.take().filter(|b| self.photos.contains(b));
        self.seen = None;
    }

    /// The views kept in step with the list going from the photos
    /// compared to `next`: those taken out drop theirs, the others carry
    /// theirs to their new place, those added start at fit.
    fn follow_views(&mut self, next: &[String]) {
        let mut list = self.photos.clone();
        for i in (0..list.len()).rev() {
            if !next.contains(&list[i]) {
                list.remove(i);
                self.view.removed(i);
            }
        }
        for (to, id) in next.iter().enumerate() {
            match list.iter().position(|p| p == id) {
                Some(from) if from != to => {
                    let photo = list.remove(from);
                    list.insert(to, photo);
                    self.view.moved(from, to);
                }
                Some(_) => {}
                None => {
                    list.insert(to.min(list.len()), id.clone());
                    self.view.inserted(to);
                }
            }
        }
    }

    /// Whether the gallery's `selection` changed since last seen (not on
    /// the first sight of it): a photo of the comparison selected
    /// elsewhere then becomes active, while the arrows keep moving the
    /// comparison's own active photo, the selection untouched.
    pub fn selection_changed(&mut self, selection: &[String]) -> bool {
        let changed = self.seen.as_deref().is_some_and(|s| s != selection);
        if self.seen.as_deref() != Some(selection) {
            self.seen = Some(selection.to_vec());
        }
        changed
    }

    /// The photos shown: the chosen ones, else the selection's one to
    /// [`MAX_PANES`] (none otherwise). One photo is a loupe.
    pub fn shown(&self, selection: &[String]) -> Vec<String> {
        if !self.photos.is_empty() {
            return self.photos.clone();
        }
        if (1..=MAX_PANES).contains(&selection.len()) {
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
        assert_eq!(c.shown(&ids(&["a"])), ids(&["a"]), "one photo is a loupe");
        assert!(c.shown(&[]).is_empty(), "none: the surface's drop hint");
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
        let many: Vec<String> = (0..MAX_PANES + 2).map(|i| format!("p{i}")).collect();
        c.set_photos(many);
        assert_eq!(c.photos().len(), MAX_PANES);
        assert_eq!(c.best(), None);
    }

    #[test]
    fn photos_added_and_taken_out_keep_the_active_one_and_the_best() {
        let mut c = Comparison::default();
        c.set_photos(ids(&["a", "b"]));
        c.set_active(1);
        c.set_best(Some("a".into()));
        let (list, added) = c.with_added(&ids(&["b", "c", "d"]), None);
        assert_eq!(list, ids(&["a", "b", "c", "d"]));
        assert_eq!(added, ids(&["c", "d"]), "b was already compared");
        let (front, _) = c.with_added(&ids(&["x", "y"]), Some(1));
        assert_eq!(
            front,
            ids(&["a", "x", "y", "b"]),
            "dropped before the pane at 1"
        );
        c.edit(list);
        let shown = c.shown(&[]);
        assert_eq!(c.active(&shown).map(String::as_str), Some("b"));
        // The active photo taken out: the next one takes its place.
        c.edit(ids(&["a", "c", "d"]));
        let shown = c.shown(&[]);
        assert_eq!(c.active(&shown).map(String::as_str), Some("c"));
        assert_eq!(c.best(), Some("a"));
        // A move keeps the active photo and its place in the list.
        let moved = c.with_moved("d", 0).expect("d is compared");
        assert_eq!(moved, ids(&["d", "a", "c"]));
        c.edit(moved);
        let shown = c.shown(&[]);
        assert_eq!(c.active(&shown).map(String::as_str), Some("c"));
        assert_eq!(c.with_moved("z", 0), None);
        // The best taken out, down to one photo: still a list.
        c.edit(ids(&["d"]));
        assert_eq!(
            (c.photos(), c.best(), c.active_index(1)),
            (&ids(&["d"])[..], None, 0)
        );
    }
}
