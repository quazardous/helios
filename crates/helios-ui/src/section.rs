//! Section headers: one level for a section of a page, a title and an
//! optional description, the theme's section spacing before it.

use egui_pop::core::ui::{Space, UiPop as _};
use egui_pop::egui::{Response, Ui};

/// A section's header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Section {
    title: String,
    description: Option<String>,
}

impl Section {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            description: None,
        }
    }

    /// A sentence under the title: what the section holds.
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    pub fn title(&self) -> &str {
        &self.title
    }
}

/// A section's header: the theme's section space, its title in the
/// section text role, its description in the muted one. The title's
/// response.
pub fn section(ui: &mut Ui, section: &Section) -> Response {
    ui.space_role(Space::Section);
    let title = ui.section_title(&section.title);
    if let Some(text) = &section.description {
        ui.muted(text);
    }
    ui.space_role(Space::Inline);
    title
}
