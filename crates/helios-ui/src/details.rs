//! What is known about one thing (a job, a photo, a disk): names and
//! their values, in two columns.

use egui_pop::core::ui::{Grid, TextRole, Track, UiPop as _};
use egui_pop::egui::Ui;

/// A details grid: one line per `(name, value)`, the names in a column
/// as wide as the widest, the values taking the rest: egui-pop's grid,
/// its header cells and body text. `id` tells two grids of a screen
/// apart.
///
/// Need (data): a details grid, names and their values.
/// Need (data): the properties of one item, in two columns.
pub fn details(ui: &mut Ui, id: &str, rows: &[(String, String)]) {
    ui.grid(Grid::new(id, [Track::CONTENT, Track::fr(1.0)]), |g| {
        for (name, value) in rows {
            g.header(name.as_str());
            g.cell(|ui| ui.text_role(TextRole::Body, value.as_str()));
        }
    });
}
