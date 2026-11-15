//! A choice among a few named options, shown closed: a drop-down.

use egui_pop::core::ui::UiPop as _;
use egui_pop::core::widgets;
use egui_pop::egui::{Response, Ui};

/// A drop-down over `current`, one row per option (its value and the
/// words shown for it): egui-pop's combo box and selectable rows. `name`
/// is its accessible name, what the choice is about ("Kind shown"); `id`
/// keeps its open state. The closed box's response, changed when another
/// option was picked.
///
/// Need (input): a drop-down choice (combo box).
/// Need (input): a filter picked from a short list of options.
pub fn choice<V: PartialEq + Clone>(
    ui: &mut Ui,
    id: &str,
    name: &str,
    current: &mut V,
    options: &[(V, String)],
) -> Response {
    let shown = options
        .iter()
        .find(|(value, _)| value == current)
        .map_or("", |(_, label)| label.as_str());
    let before = current.clone();
    let combo = widgets::combo_box(id).selected_text(shown);
    let mut response = ui
        .combo_box(combo, |ui| {
            for (value, label) in options {
                widgets::selectable_value(ui, current, value.clone(), label.as_str());
            }
        })
        .response;
    // The box says what it chooses, not only what is chosen.
    let name = name.to_string();
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_label(name));
    if *current != before {
        response.mark_changed();
    }
    response
}
