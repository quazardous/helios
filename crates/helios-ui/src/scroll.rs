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
    /// A list in a section of a side panel (the Library's folders,
    /// albums, imports): no higher than a few rows, so the sections
    /// below it stay in reach.
    Section,
}

impl Scroll {
    pub const ALL: [Scroll; 3] = [Scroll::Region, Scroll::Tail, Scroll::Section];

    /// How many lines of text a tail shows.
    const TAIL_LINES: f32 = 10.0;

    /// How high a section's list grows before it scrolls, in points.
    const SECTION_HEIGHT: f32 = 200.0;
}

/// `add` in a region scrolling up and down: egui-pop's scroll area (the
/// kit's bars, the keys). `id` keeps its position.
///
/// Need (layout): a scrolling region.
/// Need (layout): the tail of a console, the newest line kept in view.
/// Need (layout): a list in a side panel's section, a few rows high.
pub fn scroll<R>(ui: &mut Ui, id: &str, kind: Scroll, add: impl FnOnce(&mut Ui) -> R) -> R {
    let area = ScrollArea::vertical().id_salt(id);
    let area = match kind {
        Scroll::Region => area,
        Scroll::Tail => area
            .max_height(Scroll::TAIL_LINES * ui.text_style_height(&TextStyle::Monospace))
            .stick_to_bottom(true),
        Scroll::Section => area.max_height(Scroll::SECTION_HEIGHT),
    };
    ui.scroll_area(area, add).inner
}
