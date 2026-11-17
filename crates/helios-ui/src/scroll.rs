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
    /// The body of a dialog (a tree of rules): no higher than a share of
    /// the window, so the dialog's own buttons below it stay in view.
    Dialog,
    /// A form in a side panel's section (the Library's metadata): a
    /// share of the height left, so the grid and the sections below keep
    /// their room; as wide as the panel, whatever its fields' width.
    Form,
}

impl Scroll {
    pub const ALL: [Scroll; 5] = [
        Scroll::Region,
        Scroll::Tail,
        Scroll::Section,
        Scroll::Dialog,
        Scroll::Form,
    ];

    /// How many lines of text a tail shows.
    const TAIL_LINES: f32 = 10.0;

    /// How high a section's list grows before it scrolls, in points.
    const SECTION_HEIGHT: f32 = 200.0;

    /// How much of the window's height a dialog's body grows to before
    /// it scrolls.
    const DIALOG_SHARE: f32 = 0.6;

    /// How much of the height left a form's section grows to before it
    /// scrolls, and the bounds of that height, in points.
    const FORM_SHARE: f32 = 0.4;
    const FORM_HEIGHT: std::ops::RangeInclusive<f32> = 120.0..=320.0;
}

/// `add` in a region scrolling up and down: egui-pop's scroll area (the
/// kit's bars, the keys). `id` keeps its position.
///
/// Need (layout): a scrolling region.
/// Need (layout): the tail of a console, the newest line kept in view.
/// Need (layout): a list in a side panel's section, a few rows high.
/// Need (layout): a dialog's body, its buttons kept in view below it.
/// Need (layout): a form in a side panel's section, a share of its height.
pub fn scroll<R>(ui: &mut Ui, id: &str, kind: Scroll, add: impl FnOnce(&mut Ui) -> R) -> R {
    let area = ScrollArea::vertical().id_salt(id);
    let area = match kind {
        Scroll::Region => area,
        Scroll::Tail => area
            .max_height(Scroll::TAIL_LINES * ui.text_style_height(&TextStyle::Monospace))
            .stick_to_bottom(true),
        Scroll::Section => area.max_height(Scroll::SECTION_HEIGHT),
        Scroll::Dialog => area.max_height(ui.ctx().content_rect().height() * Scroll::DIALOG_SHARE),
        Scroll::Form => area
            .max_height(
                (ui.available_height() * Scroll::FORM_SHARE)
                    .clamp(*Scroll::FORM_HEIGHT.start(), *Scroll::FORM_HEIGHT.end()),
            )
            .auto_shrink([false, true]),
    };
    ui.scroll_area(area, add).inner
}
