//! The display surfaces a photo can be shown on, and where a double
//! click sends it.
//!
//! Two surfaces show a photo today: the **Image** (the darkroom's
//! viewport, `helios.viewport`) and **Compare** (the loupe and survey,
//! `helios.compare`, a comparison of one being the loupe). Showing a
//! photo is one bus request, [`SHOW_COMMAND`] (`photo.show {photo,
//! surface?, origin?, from?}`): a double click or Enter in a gallery, the
//! photo menu's "Show in ▸" and an agent all go through it, so they do
//! the same.
//!
//! Without a `surface`, the request follows the double-click strategy the
//! user picked ([`STRATEGY_SETTING`]): the Studio's primary surface (the
//! Image, as before), the surface focused last, the surface in the same
//! window as the gallery, or ask each time. When more than one surface is
//! open, the one a double click would reach wears a small target mark.
//!
//! The conventions are those of catalogue apps with a loupe beside a
//! darkroom (a second display, "open in" menus): one place decides where
//! a photo goes, and the destination is shown.

use egui_pop::model::ids::PanelId;
use egui_pop::model::layout::Layout;

/// Shows a photo on a display surface (`photo`: its id; `surface`: the
/// surface's panel, else the double-click strategy chooses).
pub const SHOW_COMMAND: &str = "photo.show";

/// Where a double click (or Enter) in a gallery shows the photo.
pub const STRATEGY_SETTING: &str = "library.double_click";

/// The Image surface: the darkroom's viewport.
pub const IMAGE: &str = "helios.viewport";

/// The Compare surface: the loupe (a comparison of one) and survey.
pub const COMPARE: &str = "helios.compare";

/// The surfaces, the primary first.
pub const SURFACES: &[&str] = &[IMAGE, COMPARE];

/// How a double click chooses its surface ([`STRATEGY_SETTING`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// The Studio's primary surface, the Image (as before surfaces).
    Primary,
    /// The surface that had the focus last.
    LastFocused,
    /// The surface in the same window as the gallery clicked.
    SameWindow,
    /// A menu of the surfaces each time.
    Ask,
}

/// The strategies' setting words, in [`Strategy`]'s order.
pub const STRATEGIES: &[&str] = &["primary", "last_focused", "same_window", "ask"];

impl Strategy {
    /// The strategy a setting word names (an unknown one: the primary).
    pub fn parse(word: &str) -> Strategy {
        match word {
            "last_focused" => Strategy::LastFocused,
            "same_window" => Strategy::SameWindow,
            "ask" => Strategy::Ask,
            _ => Strategy::Primary,
        }
    }

    /// Its setting word.
    pub fn word(self) -> &'static str {
        match self {
            Strategy::Primary => STRATEGIES[0],
            Strategy::LastFocused => STRATEGIES[1],
            Strategy::SameWindow => STRATEGIES[2],
            Strategy::Ask => STRATEGIES[3],
        }
    }
}

/// Where a request without a surface goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Surface(&'static str),
    Ask,
}

/// A surface by its panel id, if it is one.
pub fn surface(id: &str) -> Option<&'static str> {
    SURFACES.iter().copied().find(|s| *s == id)
}

/// The surfaces the layout shows (docked or in a window), in order.
pub fn open(layout: &Layout) -> Vec<&'static str> {
    SURFACES
        .iter()
        .copied()
        .filter(|s| PanelId::new(*s).is_ok_and(|p| layout.shows(&p)))
        .collect()
}

/// The window a panel is in: the first panel of its own window, `None`
/// for the main window. A gallery instance (`helios.vault#2`) is looked
/// up as itself.
pub fn window_of(layout: &Layout, panel: &str) -> Option<String> {
    let id = PanelId::new(panel).ok()?;
    layout.window_of(&id).map(|w| w.panel.as_str().to_string())
}

/// What the surfaces know between frames: the one focused last.
#[derive(Clone, Debug, Default)]
pub struct Surfaces {
    last_focused: Option<&'static str>,
}

impl Surfaces {
    /// The focused panel this frame: a surface is remembered.
    pub fn follow(&mut self, focused: Option<&str>) {
        if let Some(s) = focused.and_then(surface) {
            self.last_focused = Some(s);
        }
    }

    /// The surface focused last, if any.
    pub fn last_focused(&self) -> Option<&'static str> {
        self.last_focused
    }

    /// Where a request without a surface goes, by `strategy`, from the
    /// panel `from` (the gallery clicked; none: the main window).
    pub fn route(&self, strategy: Strategy, layout: &Layout, from: Option<&str>) -> Route {
        let open = open(layout);
        match strategy {
            Strategy::Primary => Route::Surface(IMAGE),
            Strategy::LastFocused => Route::Surface(
                self.last_focused
                    .filter(|s| open.contains(s))
                    .unwrap_or(IMAGE),
            ),
            Strategy::SameWindow => {
                let window = from.and_then(|f| window_of(layout, f));
                let same = open
                    .iter()
                    .copied()
                    .find(|s| window_of(layout, s) == window);
                Route::Surface(same.unwrap_or(IMAGE))
            }
            Strategy::Ask => Route::Ask,
        }
    }

    /// The surface wearing the target mark: the one a double click from
    /// the main window reaches, when more than one surface is open.
    pub fn target(
        &self,
        strategy: Strategy,
        layout: &Layout,
        from: Option<&str>,
    ) -> Option<&'static str> {
        if open(layout).len() < 2 {
            return None;
        }
        match self.route(strategy, layout, from) {
            Route::Surface(s) => Some(s),
            Route::Ask => None,
        }
    }
}

/// What the photo menu's "Show in ▸" and the target mark read of the
/// surfaces, set by the shell each frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    /// The open surfaces: each panel and whether it is in a window of
    /// its own.
    pub open: Vec<(&'static str, bool)>,
    /// The surface a double click reaches, marked when two or more are
    /// open.
    pub target: Option<&'static str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategies_round_trip_through_their_words() {
        for w in STRATEGIES {
            assert_eq!(Strategy::parse(w).word(), *w);
        }
        assert_eq!(Strategy::parse("nonsense"), Strategy::Primary);
    }

    #[test]
    fn a_surface_is_known_by_its_panel() {
        assert_eq!(surface("helios.compare"), Some(COMPARE));
        assert_eq!(surface("helios.vault"), None);
    }
}
