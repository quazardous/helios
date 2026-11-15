//! What limits a view: an album, a folder, a filter, shown where the
//! view is so it never reads as "nothing here".

use egui_pop::egui::Ui;
use egui_pop::views::library::tags::TagChip;

/// One thing limiting what a view shows ("Album: Trip"), as a chip the
/// user removes to lift it: egui-pop's removable tag chip, its remove
/// button named "Remove {text}". Whether it was removed.
///
/// Need (state): what restricts a list or a gallery, removable.
pub fn restriction(ui: &mut Ui, text: &str) -> bool {
    TagChip::new(text)
        .removable(true)
        .remove_label(format!("Remove {text}"))
        .show(ui)
        .removed
}
