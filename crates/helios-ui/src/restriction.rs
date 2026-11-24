//! What limits a view: an album, a folder, a filter, shown where the
//! view is so it never reads as "nothing here".

use egui_pop::egui::{Color32, TextStyle, Ui};
use egui_pop::views::library::tags::TagChip;

/// One thing limiting what a view shows ("Album: Trip"), as a chip the
/// user removes to lift it: egui-pop's removable tag chip, its remove
/// button named "Remove {text}". Whether it was removed.
///
/// Need (state): what restricts a list or a gallery, removable.
///
/// A text wider than the line (a folder's long path in a side bar) is
/// cut in its middle ("Folder: C:\Users\…\raw"), the whole text in its
/// tooltip and its remove button's name: the chip, and its remove button
/// at its end, stay within the line instead of stretching it.
pub fn restriction(ui: &mut Ui, text: &str) -> bool {
    // The chip's padding and remove button, around its text.
    let chrome = 3.0 * ui.spacing().interact_size.y;
    let room = (ui.max_rect().width() - chrome).max(0.0);
    let shown = fit_middle(ui, text, room);
    let chip = TagChip::new(shown.as_str())
        .removable(true)
        .remove_label(format!("Remove {text}"))
        .show(ui);
    if shown != text {
        chip.response.on_hover_text(text);
    }
    chip.removed
}

/// `text`, or its start and end around an ellipsis, as wide as `room` at
/// most in the body font (its start and end kept: what it is and where).
fn fit_middle(ui: &Ui, text: &str, room: f32) -> String {
    let font = TextStyle::Body.resolve(ui.style());
    let width = |s: &str| {
        ui.painter()
            .layout_no_wrap(s.to_owned(), font.clone(), Color32::PLACEHOLDER)
            .size()
            .x
    };
    if width(text) <= room {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    // The longest cut that fits, half of it from the start, half from
    // the end.
    let (mut lo, mut hi) = (0, chars.len());
    let cut = |keep: usize| {
        let head = keep.div_ceil(2);
        let tail = keep - head;
        let mut s: String = chars[..head].iter().collect();
        s.push('…');
        s.extend(&chars[chars.len() - tail..]);
        s
    };
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if width(&cut(mid)) <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}
