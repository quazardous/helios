//! What scrolls: a region of a screen, or the tail of what a job wrote.

use egui_pop::core::ui::UiPop as _;
use egui_pop::egui::{ScrollArea, TextStyle, Ui};

/// What a scrolling region holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scroll {
    /// A list or a detail filling its place.
    Region,
    /// Lines that keep coming (a console): a few lines high, the newest
    /// kept in view.
    Tail,
}

impl Scroll {
    pub const ALL: [Scroll; 2] = [Scroll::Region, Scroll::Tail];

    /// How many lines of text a tail shows.
    const TAIL_LINES: f32 = 10.0;
}

/// `add` in a region scrolling up and down: egui-pop's scroll area (the
/// kit's bars, the keys). `id` keeps its position.
///
/// Need (layout): a scrolling region.
/// Need (layout): the tail of a console, the newest line kept in view.
pub fn scroll<R>(ui: &mut Ui, id: &str, kind: Scroll, add: impl FnOnce(&mut Ui) -> R) -> R {
    let area = ScrollArea::vertical().id_salt(id);
    let area = match kind {
        Scroll::Region => area,
        Scroll::Tail => area
            .max_height(Scroll::TAIL_LINES * ui.text_style_height(&TextStyle::Monospace))
            .stick_to_bottom(true),
    };
    ui.scroll_area(area, add).inner
}
