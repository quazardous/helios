//! The display surfaces a photo can be shown on, and where a double
//! click sends it.
//!
//! Two kinds of surface show a photo: the **Image** (the darkroom's
//! viewport, `helios.viewport`) and **Compare** (the loupe and survey,
//! `helios.compare`, a comparison of one being the loupe). Compare opens
//! as many times as wanted: each instance (`helios.compare#<n>`, "Compare
//! 2") holds a comparison of its own, kept with the layout. Showing a
//! photo is one bus request, [`SHOW_COMMAND`] (`photo.show {photo,
//! surface?, origin?, from?}`): a double click or Enter in a gallery, the
//! photo menu's "Show in ▸" and an agent all go through it, so they do
//! the same.
//!
//! Without a `surface`, the request follows the double-click strategy the
//! user picked ([`STRATEGY_SETTING`]): the Studio's primary surface (the
//! Image, as before), the surface focused last, the surface in the same
//! window as the gallery, a new Compare each time, or ask each time. When more than one surface is
//! open, the one a double click would reach wears a small target mark.
//!
//! The conventions are those of catalogue apps with a loupe beside a
//! darkroom (a second display, "open in" menus): one place decides where
//! a photo goes, and the destination is shown.
//!
//! Zen mode (View ▸ Zen mode, `Ctrl+K Z`) shows the panel that matters
//! alone: egui-pop asks the application on `pop.zen.panel` when no panel
//! is named and the focused one is not a view, and Helios answers with
//! the surface a double click reaches ([`zen_choice`]), a Compare while
//! culling.

use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::bus::ZenPanel;
use egui_pop::api::bus::bus_contract::{Bus, ListenOpts, Point as _};
use egui_pop::model::ids::PanelId;
use egui_pop::model::layout::Layout;

/// Shows a photo on a display surface (`photo`: its id; `surface`: the
/// surface's panel, [`NEW`] or [`NEW_WINDOW`], else the double-click
/// strategy chooses).
pub const SHOW_COMMAND: &str = "photo.show";

/// Where a double click (or Enter) in a gallery shows the photo.
pub const STRATEGY_SETTING: &str = "library.double_click";

/// The Image surface: the darkroom's viewport.
pub const IMAGE: &str = "helios.viewport";

/// The Compare surface: the loupe (a comparison of one) and survey. A
/// panel declared with several instances: `helios.compare#<n>` is
/// another Compare, with a comparison of its own.
pub const COMPARE: &str = "helios.compare";

/// `photo.show`'s `surface` for a new Compare, docked.
pub const NEW: &str = "new";

/// `photo.show`'s `surface` for a new Compare in a window of its own.
pub const NEW_WINDOW: &str = "new_window";

/// The surfaces declared, the primary first.
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
    /// A new Compare each time.
    AlwaysNew,
    /// A menu of the surfaces each time.
    Ask,
}

/// The strategies' setting words, in [`Strategy`]'s order.
pub const STRATEGIES: &[&str] = &[
    "primary",
    "last_focused",
    "same_window",
    "always_new",
    "ask",
];

impl Strategy {
    /// The strategy a setting word names (an unknown one: the primary).
    pub fn parse(word: &str) -> Strategy {
        match word {
            "last_focused" => Strategy::LastFocused,
            "same_window" => Strategy::SameWindow,
            "always_new" => Strategy::AlwaysNew,
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
            Strategy::AlwaysNew => STRATEGIES[3],
            Strategy::Ask => STRATEGIES[4],
        }
    }
}

/// Where a request without a surface goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// A surface open (or the Image).
    Surface(String),
    /// A new Compare.
    New,
    /// A question.
    Ask,
}

/// Whether `id` is a Compare: the panel declared or one of its
/// instances (`helios.compare#<n>`).
pub fn is_compare(id: &str) -> bool {
    PanelId::new(id).is_ok_and(|p| p.kind().as_str() == COMPARE)
}

/// A surface by its panel id, if it is one: the Image, Compare or a
/// Compare instance.
pub fn surface(id: &str) -> Option<String> {
    (id == IMAGE || is_compare(id)).then(|| id.to_string())
}

/// The surfaces the layout shows (docked or in a window), the Image
/// first, then each Compare in its number's order.
pub fn open(layout: &Layout) -> Vec<String> {
    let shown = |p: &PanelId| layout.shows(p);
    let image = PanelId::from_static(IMAGE);
    let compare = PanelId::from_static(COMPARE);
    let mut out = Vec::new();
    if shown(&image) {
        out.push(IMAGE.to_string());
    }
    let mut compares = layout.instances_of(&compare);
    if !compares.contains(&compare) {
        compares.insert(0, compare);
    }
    out.extend(
        compares
            .into_iter()
            .filter(|p| shown(p))
            .map(|p| p.as_str().to_string()),
    );
    out
}

/// The window a panel is in: the first panel of its own window, `None`
/// for the main window. A gallery instance (`helios.vault#2`) is looked
/// up as itself.
pub fn window_of(layout: &Layout, panel: &str) -> Option<String> {
    let id = PanelId::new(panel).ok()?;
    layout.window_of(&id).map(|w| w.panel.as_str().to_string())
}

/// What the surfaces know between frames: the one focused last, and the
/// Compare focused last in each window.
#[derive(Clone, Debug, Default)]
pub struct Surfaces {
    last_focused: Option<String>,
    /// Per window (`None`: the main window), the Compare that had the
    /// focus last there.
    compare_in_window: Vec<(Option<String>, String)>,
}

impl Surfaces {
    /// The focused panel this frame: a surface is remembered.
    pub fn follow(&mut self, focused: Option<&str>) {
        if let Some(s) = focused.and_then(surface)
            && self.last_focused.as_ref() != Some(&s)
        {
            self.last_focused = Some(s);
        }
    }

    /// The surface focused last, if any.
    pub fn last_focused(&self) -> Option<&str> {
        self.last_focused.as_deref()
    }

    /// Keeps `compare`, focused, as the Compare focused last in `window`
    /// (its window's first panel; `None`: the main window).
    pub fn follow_compare(&mut self, compare: &str, window: Option<String>) {
        if !is_compare(compare) {
            return;
        }
        self.compare_in_window.retain(|(w, _)| *w != window);
        self.compare_in_window.push((window, compare.to_string()));
    }

    /// The Compare a `compare.*` command without a `surface` acts on (the
    /// arrows, Delete, sync): the focused Compare, else the one focused
    /// last in `window` (the window the user works in) while still
    /// `open`, else the first, [`COMPARE`]. The rule is egui-pop's for
    /// Ctrl+Z (the focused view's history, else the one focused last in
    /// the window).
    pub fn active_compare(
        &self,
        focused: Option<&str>,
        window: Option<&str>,
        open: &[String],
    ) -> String {
        let is_open = |c: &str| c == COMPARE || open.iter().any(|o| o == c);
        if let Some(f) = focused.filter(|f| is_compare(f) && is_open(f)) {
            return f.to_string();
        }
        self.compare_in_window
            .iter()
            .find(|(w, _)| w.as_deref() == window)
            .map(|(_, c)| c.clone())
            .filter(|c| is_open(c))
            .unwrap_or_else(|| COMPARE.to_string())
    }

    /// Where a request without a surface goes, by `strategy`, from the
    /// panel `from` (the gallery clicked; none: the main window).
    pub fn route(&self, strategy: Strategy, layout: &Layout, from: Option<&str>) -> Route {
        let open = open(layout);
        let image = || Route::Surface(IMAGE.to_string());
        match strategy {
            Strategy::Primary => image(),
            Strategy::LastFocused => self
                .last_focused
                .clone()
                .filter(|s| open.contains(s))
                .map_or_else(image, Route::Surface),
            Strategy::SameWindow => {
                let window = from.and_then(|f| window_of(layout, f));
                open.into_iter()
                    .find(|s| window_of(layout, s) == window)
                    .map_or_else(image, Route::Surface)
            }
            Strategy::AlwaysNew => Route::New,
            Strategy::Ask => Route::Ask,
        }
    }

    /// The surface wearing the target mark: the one a double click from
    /// the main window reaches, when more than one surface is open (none
    /// for a new one each time, or ask).
    pub fn target(
        &self,
        strategy: Strategy,
        layout: &Layout,
        from: Option<&str>,
    ) -> Option<String> {
        if open(layout).len() < 2 {
            return None;
        }
        match self.route(strategy, layout, from) {
            Route::Surface(s) => Some(s),
            Route::New | Route::Ask => None,
        }
    }
}

/// The panel zen mode shows when none is named and the focused panel is
/// not a view: while culling, a Compare (the one focused last, else the
/// first open); otherwise the surface a double click reaches by
/// `strategy`, when it is open. None (a new Compare each time, or ask):
/// egui-pop shows the centre's active tab.
pub fn zen_choice(
    surfaces: &Surfaces,
    strategy: Strategy,
    layout: &Layout,
    culling: bool,
) -> Option<String> {
    let open = open(layout);
    if culling {
        let last = surfaces
            .last_focused()
            .filter(|s| is_compare(s) && open.iter().any(|o| o == s));
        if let Some(s) = last.or_else(|| open.iter().map(String::as_str).find(|s| is_compare(s))) {
            return Some(s.to_string());
        }
    }
    match surfaces.route(strategy, layout, None) {
        Route::Surface(s) if open.contains(&s) => Some(s),
        _ => None,
    }
}

/// Helios's listener of egui-pop's `pop.zen.panel`.
pub const ZEN_LISTENER: &str = "helios.zen";

/// Registers [`ZEN_LISTENER`] on `bus`: it answers with `choice`, the
/// panel [`zen_choice`] gave on the shell's last frame. One there already
/// is replaced.
///
/// # Errors
/// The bus refused it.
pub fn listen_zen(bus: &dyn Bus, choice: Arc<Mutex<Option<String>>>) -> Result<(), String> {
    bus.unlisten(&ZenPanel::ID, ZEN_LISTENER);
    bus.listen::<ZenPanel>(
        ZEN_LISTENER,
        ListenOpts::new().owner(ZEN_LISTENER),
        move |(), value| {
            let chosen = choice
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if chosen.is_some() {
                *value = chosen;
            }
            Ok(())
        },
    )
    .map_err(|e| e.to_string())
}

/// Helios's listener of egui-pop's `pop.command.label`.
pub const ZEN_LABEL_LISTENER: &str = "helios.zen.label";

/// Registers [`ZEN_LABEL_LISTENER`] on `bus`: it answers `workbench.zen`'s
/// title and tooltip with `label`, the [`crate::plugins::zen_label`] the
/// shell found on its last frame ("Zen: Compare"), so the View menu's
/// entry and the shortcut's tooltip name the panel zen shows. Other
/// commands keep their text. One there already is replaced.
///
/// # Errors
/// The bus refused it.
pub fn listen_zen_label(bus: &dyn Bus, label: Arc<Mutex<Option<String>>>) -> Result<(), String> {
    use egui_pop::api::bus::CommandLabel;
    use egui_pop::api::workbench::CMD_ZEN;
    bus.unlisten(&CommandLabel::ID, ZEN_LABEL_LISTENER);
    bus.listen::<CommandLabel>(
        ZEN_LABEL_LISTENER,
        ListenOpts::new().owner(ZEN_LABEL_LISTENER),
        move |command, text| {
            if *command != CMD_ZEN {
                return Ok(());
            }
            let said = label.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if let Some(l) = said {
                *text = text.clone().title(l.clone()).tooltip(l);
            }
            Ok(())
        },
    )
    .map_err(|e| e.to_string())
}

/// What the photo menu's "Show in ▸" and the target mark read of the
/// surfaces, set by the shell each frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    /// The open surfaces: each panel and whether it is in a window of
    /// its own.
    pub open: Vec<(String, bool)>,
    /// The surface a double click reaches, marked when two or more are
    /// open.
    pub target: Option<String>,
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

    /// The Image docked in the main window; Compare in a window of its
    /// own, with a second gallery.
    fn two_windows() -> Layout {
        use egui_pop::model::layout::{Auxiliary, Node};
        use egui_pop::model::panels::Area;
        let id = |s: &str| PanelId::new(s).unwrap();
        let mut layout = Layout::default();
        layout.areas.push((Area::Center, Node::tab(id(IMAGE))));
        layout
            .areas
            .push((Area::StartSide, Node::tab(id("helios.vault"))));
        layout.auxiliary.push(Auxiliary::window(
            id(COMPARE),
            "main",
            [0.0, 0.0, 800.0, 600.0],
        ));
        layout
            .join_window(&id("helios.vault#2"), &id(COMPARE))
            .unwrap();
        layout
    }

    #[test]
    fn each_strategy_routes_by_its_rule() {
        let layout = two_windows();
        assert_eq!(open(&layout), [IMAGE, COMPARE]);
        let surf = |s: &str| Route::Surface(s.to_string());
        let mut s = Surfaces::default();
        let route = |s: &Surfaces, k, from| s.route(k, &layout, from);
        // Primary: the Image, wherever the click.
        assert_eq!(
            route(&s, Strategy::Primary, Some("helios.vault#2")),
            surf(IMAGE)
        );
        // Same window: the surface beside the gallery clicked.
        assert_eq!(
            route(&s, Strategy::SameWindow, Some("helios.vault")),
            surf(IMAGE)
        );
        assert_eq!(
            route(&s, Strategy::SameWindow, Some("helios.vault#2")),
            surf(COMPARE)
        );
        // Last focused: the primary until a surface had the focus.
        assert_eq!(route(&s, Strategy::LastFocused, None), surf(IMAGE));
        s.follow(Some(COMPARE));
        s.follow(Some("helios.vault"));
        assert_eq!(route(&s, Strategy::LastFocused, None), surf(COMPARE));
        assert_eq!(
            s.target(Strategy::LastFocused, &layout, None),
            Some(COMPARE.to_string())
        );
        // Always new: a new Compare, and no target mark.
        assert_eq!(route(&s, Strategy::AlwaysNew, None), Route::New);
        assert_eq!(s.target(Strategy::AlwaysNew, &layout, None), None);
        // Ask: a question, and no target mark.
        assert_eq!(route(&s, Strategy::Ask, None), Route::Ask);
        assert_eq!(s.target(Strategy::Ask, &layout, None), None);
        // One surface open: no mark.
        let mut one = layout.clone();
        one.auxiliary.clear();
        assert_eq!(s.target(Strategy::Primary, &one, None), None);
    }

    #[test]
    fn compare_instances_are_surfaces_of_their_window() {
        let mut layout = two_windows();
        let id = |s: &str| PanelId::new(s).unwrap();
        layout
            .join_window(&id("helios.compare#2"), &id(COMPARE))
            .unwrap();
        assert_eq!(open(&layout), [IMAGE, COMPARE, "helios.compare#2"]);
        let mut s = Surfaces::default();
        s.follow(Some("helios.compare#2"));
        assert_eq!(
            s.route(Strategy::LastFocused, &layout, None),
            Route::Surface("helios.compare#2".into())
        );
    }

    #[test]
    fn a_command_goes_to_the_focused_compare_else_the_windows_last() {
        let open: Vec<String> = [COMPARE, "helios.compare#2", "helios.compare#3"]
            .map(String::from)
            .into();
        let mut s = Surfaces::default();
        // Nothing focused yet: the first.
        assert_eq!(s.active_compare(None, None, &open), COMPARE);
        // The focused Compare.
        assert_eq!(
            s.active_compare(Some("helios.compare#2"), None, &open),
            "helios.compare#2"
        );
        // The focus elsewhere: the Compare focused last in that window.
        s.follow_compare("helios.compare#2", None);
        s.follow_compare("helios.compare#3", Some("helios.compare#3".into()));
        s.follow_compare("helios.vault", None);
        assert_eq!(
            s.active_compare(Some("helios.vault"), None, &open),
            "helios.compare#2"
        );
        assert_eq!(
            s.active_compare(None, Some("helios.compare#3"), &open),
            "helios.compare#3"
        );
        // Closed: back to the first.
        assert_eq!(s.active_compare(None, None, &open[..1]), COMPARE);
    }

    #[test]
    fn zen_picks_a_compare_while_culling_else_the_double_click_surface() {
        let mut layout = two_windows();
        let id = |s: &str| PanelId::new(s).unwrap();
        layout
            .join_window(&id("helios.compare#2"), &id(COMPARE))
            .unwrap();
        let mut s = Surfaces::default();
        // Not culling: where a double click goes.
        assert_eq!(
            zen_choice(&s, Strategy::Primary, &layout, false).as_deref(),
            Some(IMAGE)
        );
        assert_eq!(zen_choice(&s, Strategy::Ask, &layout, false), None);
        // Culling: the first Compare, then the one focused last.
        assert_eq!(
            zen_choice(&s, Strategy::Primary, &layout, true).as_deref(),
            Some(COMPARE)
        );
        s.follow(Some("helios.compare#2"));
        assert_eq!(
            zen_choice(&s, Strategy::Primary, &layout, true).as_deref(),
            Some("helios.compare#2")
        );
        // No Compare open while culling: the double-click surface.
        let mut image_only = layout.clone();
        image_only.auxiliary.clear();
        assert_eq!(
            zen_choice(&s, Strategy::Primary, &image_only, true).as_deref(),
            Some(IMAGE)
        );
    }

    #[test]
    fn a_surface_is_known_by_its_panel() {
        assert_eq!(surface("helios.compare").as_deref(), Some(COMPARE));
        assert_eq!(
            surface("helios.compare#3").as_deref(),
            Some("helios.compare#3")
        );
        assert!(is_compare("helios.compare#3"));
        assert!(!is_compare(IMAGE));
        assert_eq!(surface("helios.vault"), None);
    }
}
