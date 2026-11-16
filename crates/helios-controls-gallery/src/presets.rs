//! The gallery's "Helios UI" page: every preset of `helios_ui`, in each
//! of its states, side by side. It is the reference for how Helios's
//! screens look, and the place UI bugs are reported from.

use egui_pop::core::ui::{Tone, UiPop as _};
use egui_pop::egui::{self, Ui};
use helios_ui::{Action, Empty, Kind, Late, Loading, Progress, Scroll, Search, Section, Value};

/// The page's state: the search fields' queries, the choice's value and
/// the last preset clicked.
#[derive(Debug, Default)]
pub struct PresetsPage {
    queries: [String; 3],
    /// The choice's value: an index into [`KINDS`].
    kind: usize,
    /// What the last click asked for, said under the page's title.
    pub last: Option<String>,
}

/// The choice's options.
const KINDS: [&str; 3] = ["All kinds", "Imports", "Exports"];

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

        ui.horizontal(|ui| {
            ui.label("Import 12 photos");
            for kind in Action::ALL {
                let label = format!("{} in a row", kind.name());
                if helios_ui::row_action(ui, kind, &label).clicked() {
                    self.last = Some(format!("row action {}", kind.name()));
                }
            }
        });

        helios_ui::section(
            ui,
            &Section::new("Choices").description("A drop-down named by what it chooses."),
        );
        let options: Vec<(usize, String)> = KINDS
            .iter()
            .enumerate()
            .map(|(i, k)| (i, (*k).to_string()))
            .collect();
        if helios_ui::choice(
            ui,
            "gallery.presets.kind",
            "Kind shown",
            &mut self.kind,
            &options,
        )
        .changed()
        {
            self.last = Some(format!("choice {}", KINDS[self.kind]));
        }

        helios_ui::section(
            ui,
            &Section::new("Details").description("Names and their values, in two columns."),
        );
        let fields: Vec<(String, String)> = [
            ("Kind", "Import"),
            ("Started", "12 Oct 2026, 18:42"),
            ("Source", "Card EOS_DIGITAL"),
        ]
        .iter()
        .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
        .collect();
        helios_ui::details(ui, "gallery.presets.details", &fields);

        helios_ui::section(
            ui,
            &Section::new("Progress").description(
                "A bar when the work counts, a spinner when it does not; in a row, or wide.",
            ),
        );
        ui.horizontal(|ui| {
            helios_ui::progress(ui, Progress::Row, "Import in a row", Some(0.29));
            helios_ui::progress(ui, Progress::Row, "Preview in a row", None);
        });
        helios_ui::progress(ui, Progress::Block, "Export wide", Some(0.64));

        helios_ui::section(
            ui,
            &Section::new("Scrolling")
                .description("A region, and a console's tail: the newest line in view."),
        );
        ui.columns(2, |cols| {
            cols[0].set_max_height(90.0);
            helios_ui::scroll(
                &mut cols[0],
                "gallery.presets.region",
                Scroll::Region,
                |ui| {
                    for i in 1..=12 {
                        ui.text_role(egui_pop::core::ui::TextRole::Body, format!("Photo {i:02}"));
                    }
                },
            );
            helios_ui::scroll(&mut cols[1], "gallery.presets.tail", Scroll::Tail, |ui| {
                for i in 1..=24 {
                    ui.code_text(format!("18:42:{i:02}  read IMG_{i:04}.CR3"));
                }
                ui.text_tone(Tone::Danger, "18:43:00  failed: IMG_0025.CR3, disk full");
            });
        });

        helios_ui::section(
            ui,
            &Section::new("Loading").description("Skeletons in the content's shape."),
        );
        helios_ui::loading(ui, Loading::Text(2));
        helios_ui::loading(ui, Loading::Rows(2));

        helios_ui::section(
            ui,
            &Section::new("Late values")
                .description("Pending, fresh, refreshing and failed, in the room of their kind."),
        );
        let values: [(&str, Kind, Late<&str>); 4] = [
            ("Photos", Kind::Count, Late::Pending),
            ("Free space", Kind::Size, Late::Fresh("412.7 GB")),
            ("Last backup", Kind::Date, Late::Refreshing("12 Oct 2026")),
            (
                "Archive size",
                Kind::Size,
                Late::Failed("The disk did not answer.".into()),
            ),
        ];
        for (name, kind, late) in values {
            ui.horizontal(|ui| {
                ui.label(format!("{name}:"));
                if helios_ui::value(ui, Value::new(name, kind, late)) {
                    self.last = Some(format!("value {name} retry"));
                }
            });
        }

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
