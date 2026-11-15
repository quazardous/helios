//! The gallery's "Helios UI" page: every preset of `helios_ui`, in each
//! of its states, side by side. It is the reference for how Helios's
//! screens look, and the place UI bugs are reported from.

use egui_pop::egui::{self, Ui};
use helios_ui::{Action, Empty, Loading, Search, Section};

/// The page's state: the search fields' queries and the last preset
/// clicked.
#[derive(Debug, Default)]
pub struct PresetsPage {
    queries: [String; 3],
    /// What the last click asked for, said under the page's title.
    pub last: Option<String>,
}

impl PresetsPage {
    pub fn ui(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| self.content(ui));
    }

    /// The presets, family by family.
    pub fn content(&mut self, ui: &mut Ui) {
        helios_ui::section(
            ui,
            &Section::new("Helios UI").description(
                "Every control Helios's screens use, chosen by intent, in its states.",
            ),
        );
        if let Some(last) = &self.last {
            ui.label(format!("Clicked: {last}"));
        }

        helios_ui::section(
            ui,
            &Section::new("Search").description("One field per scope: magnifier, hint, clear."),
        );
        for (search, query) in Search::ALL.into_iter().zip(self.queries.iter_mut()) {
            helios_ui::search(ui, query, search);
        }

        helios_ui::section(
            ui,
            &Section::new("Actions").description("Primary, secondary and danger."),
        );
        ui.horizontal(|ui| {
            for kind in Action::ALL {
                if helios_ui::action(ui, kind, kind.name()).clicked() {
                    self.last = Some(format!("action {}", kind.name()));
                }
            }
        });

        helios_ui::section(
            ui,
            &Section::new("Loading").description("Skeletons in the content's shape."),
        );
        helios_ui::loading(ui, Loading::Text(2));
        helios_ui::loading(ui, Loading::Rows(2));

        helios_ui::section(
            ui,
            &Section::new("Restrictions").description("What limits a view, removed by its button."),
        );
        if helios_ui::restriction(ui, "Album: Trip") {
            self.last = Some("restriction removed".to_string());
        }

        helios_ui::section(
            ui,
            &Section::new("Empty and failed").description("Why a place is empty, and what to do."),
        );
        ui.columns(3, |cols| {
            for (i, case) in Empty::ALL.into_iter().enumerate() {
                if helios_ui::empty(&mut cols[i % 3], case) {
                    self.last = Some(format!("empty {}", case.name()));
                }
            }
            if helios_ui::failed(
                &mut cols[2],
                "Could not read the library",
                "The disk did not answer.",
            ) {
                self.last = Some("failed retry".to_string());
            }
        });
    }
}
