//! Buttons by their weight in a screen: the one primary action, the
//! secondary ones, a destructive one.

use egui_pop::core::ui::{Tone, UiPop as _};
use egui_pop::egui::{Response, Ui};

/// What an action weighs in its screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// The action the screen is for ("Add the source", "Import"): one per
    /// screen or dialog, in the accent.
    Primary,
    /// Any other action ("Cancel", "Back"), the plain button.
    Secondary,
    /// An action that deletes or cannot be undone, in the danger tone.
    Danger,
}

impl Action {
    pub const ALL: [Action; 3] = [Action::Primary, Action::Secondary, Action::Danger];

    /// Its tone in the theme's kit (`button.tone.*`).
    pub fn tone(self) -> Tone {
        match self {
            Action::Primary => Tone::Accent,
            Action::Secondary => Tone::Neutral,
            Action::Danger => Tone::Danger,
        }
    }

    /// Its name, for the gallery and agents.
    pub fn name(self) -> &'static str {
        match self {
            Action::Primary => "primary",
            Action::Secondary => "secondary",
            Action::Danger => "danger",
        }
    }
}

/// A button for `kind` of action, labelled `label`: egui-pop's button in
/// the action's tone.
///
/// Need (action): a button by tone (primary, danger, quiet).
/// Need (action): the screen's primary or secondary action.
pub fn action(ui: &mut Ui, kind: Action, label: &str) -> Response {
    ui.button_tone(kind.tone(), label)
}
