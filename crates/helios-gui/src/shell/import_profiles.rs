//! The import window's Include and Apply During Import sections, among
//! the choices every source shares (`docs/design/import-window.md`), as
//! Lightroom's import keeps a filter and a metadata preset with its other
//! import choices:
//!
//! - **Include**: which files of the source are imported, its kinds
//!   (raw, JPEG, video, other), name patterns, a day range, a size range
//!   and "New photos only". A filter can be saved under a name ("Save
//!   filter as…") and picked again on any source; the named filters are
//!   kept beside the presets (`import-filters.json`).
//! - **Apply During Import**: keywords, a rating and a colour label every
//!   photo imported is given.
//!
//! Both are part of the source's [`Choices`], so they are kept on a saved
//! source (a card by its volume), reach every import of it as
//! `library.importFiles`'s `filter` and `apply`, manual or automatic, and
//! are recorded in the import history with the other choices.

use std::path::Path;

use helios_core::Message;
use helios_library::import_profile::{Apply, Filter, Kind, LABELS};

use super::Shell;
use super::import_sources::SourceKey;
use super::import_window::ImportWindow;

/// The named filters' file, beside the presets'.
pub(super) const FILE: &str = "import-filters.json";

/// What is being typed in the sections: the name patterns and keywords
/// as one line each (kept while their parse is the choices'), and the
/// name a filter is saved as.
#[derive(Default)]
pub(crate) struct Editing {
    names: String,
    keywords: String,
    saving: Option<String>,
}

/// The sections' words.
pub(crate) struct ProfileWords {
    include: String,
    filter: String,
    all: String,
    kinds: [String; 4],
    names: String,
    names_help: String,
    from: String,
    to: String,
    day_help: String,
    at_least: String,
    at_most: String,
    new_only: String,
    save_as: String,
    save: String,
    delete: String,
    apply: String,
    keywords: String,
    keywords_help: String,
    rating: String,
    none: String,
    label: String,
    labels: [String; 5],
}

impl ProfileWords {
    pub(super) fn new(t: &dyn Fn(Message) -> String) -> Self {
        ProfileWords {
            include: t(Message::new("gui-import-include", "Include")),
            filter: t(Message::new("gui-import-include-filter", "Filter")),
            all: t(Message::new("gui-import-include-all", "All photos")),
            kinds: [
                t(Message::new("gui-import-include-raw", "Raw")),
                t(Message::new("gui-import-include-jpeg", "JPEG")),
                t(Message::new("gui-import-include-video", "Video")),
                t(Message::new("gui-import-include-other", "Other")),
            ],
            names: t(Message::new("gui-import-include-names", "Names")),
            names_help: t(Message::new(
                "gui-import-include-names-help",
                "File name patterns, comma-separated: * any text, ? one character",
            )),
            from: t(Message::new("gui-import-include-from", "From")),
            to: t(Message::new("gui-import-include-to", "To")),
            day_help: t(Message::new(
                "gui-import-include-day-help",
                "A day as YYYY-MM-DD, by the file's date",
            )),
            at_least: t(Message::new("gui-import-include-at-least", "At least")),
            at_most: t(Message::new("gui-import-include-at-most", "At most")),
            new_only: t(Message::new(
                "gui-import-include-new-only",
                "New photos only",
            )),
            save_as: t(Message::new(
                "gui-import-include-save-as",
                "Save filter as…",
            )),
            save: t(Message::new("gui-import-include-save", "Save")),
            delete: t(Message::new("gui-import-include-delete", "Delete filter")),
            apply: t(Message::new("gui-import-apply", "Apply During Import")),
            keywords: t(Message::new("gui-import-apply-keywords", "Keywords")),
            keywords_help: t(Message::new(
                "gui-import-apply-keywords-help",
                "Added to every photo imported, comma-separated",
            )),
            rating: t(Message::new("gui-import-apply-rating", "Rating")),
            none: t(Message::new("gui-import-apply-none", "None")),
            label: t(Message::new("gui-import-apply-label", "Label")),
            labels: [
                t(Message::new("gui-import-apply-red", "Red")),
                t(Message::new("gui-import-apply-yellow", "Yellow")),
                t(Message::new("gui-import-apply-green", "Green")),
                t(Message::new("gui-import-apply-blue", "Blue")),
                t(Message::new("gui-import-apply-purple", "Purple")),
            ],
        }
    }
}

/// Patterns or keywords typed as one line.
fn line_of(items: &[String]) -> String {
    items.join(", ")
}

/// A line typed back to patterns or keywords.
fn items_of(line: &str) -> Vec<String> {
    Apply::parse_keywords(line)
}

/// A size in megabytes, 0 for none.
fn megabytes(bytes: Option<u64>) -> u64 {
    bytes.map_or(0, |b| b / 1_000_000)
}

fn bytes(mb: u64) -> Option<u64> {
    (mb > 0).then(|| mb * 1_000_000)
}

/// Include: the filter of the selected source.
pub(super) fn include_ui(ui: &mut egui::Ui, w: &mut ImportWindow, words: &ProfileWords) {
    egui::CollapsingHeader::new(&words.include)
        .id_salt("helios.import.include")
        .default_open(!w.choices.filter.is_all())
        .show(ui, |ui| {
            // The named filters, picked as a whole.
            ui.horizontal_wrapped(|ui| {
                ui.label(&words.filter);
                if ui
                    .selectable_label(w.choices.filter.is_all(), &words.all)
                    .clicked()
                {
                    w.choices.filter = Filter::default();
                }
                let picked = w
                    .filters
                    .iter()
                    .find(|(_, f)| **f == w.choices.filter)
                    .map(|(n, _)| n.clone());
                let names: Vec<String> = w.filters.keys().cloned().collect();
                for name in names {
                    if ui
                        .selectable_label(picked.as_ref() == Some(&name), &name)
                        .clicked()
                    {
                        w.use_filter(&name);
                    }
                }
            });
            let f = &mut w.choices.filter;
            ui.horizontal_wrapped(|ui| {
                for (kind, label) in Kind::ALL.into_iter().zip(&words.kinds) {
                    let mut on = f.kinds.contains(&kind);
                    if ui.checkbox(&mut on, label).changed() {
                        f.kinds.retain(|k| *k != kind);
                        if on {
                            f.kinds.push(kind);
                            f.kinds.sort();
                        }
                    }
                }
            });
            let e = &mut w.profile_edit;
            if items_of(&e.names) != f.names {
                e.names = line_of(&f.names);
            }
            ui.horizontal(|ui| {
                ui.label(&words.names);
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut e.names)
                            .hint_text("IMG_*, *.CR3")
                            .desired_width(f32::INFINITY),
                    )
                    .on_hover_text(&words.names_help)
                    .changed()
                {
                    f.names = items_of(&e.names);
                }
            });
            ui.horizontal(|ui| {
                for (label, day) in [(&words.from, &mut f.from), (&words.to, &mut f.to)] {
                    ui.label(label);
                    let mut text = day.clone().unwrap_or_default();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut text)
                                .hint_text("YYYY-MM-DD")
                                .desired_width(84.0),
                        )
                        .on_hover_text(&words.day_help)
                        .changed()
                    {
                        *day = Some(text.trim().to_string()).filter(|t| !t.is_empty());
                    }
                }
            });
            ui.horizontal(|ui| {
                for (label, size) in [
                    (&words.at_least, &mut f.min_bytes),
                    (&words.at_most, &mut f.max_bytes),
                ] {
                    ui.label(label);
                    let mut mb = megabytes(*size);
                    if ui
                        .add(egui::DragValue::new(&mut mb).suffix(" MB"))
                        .changed()
                    {
                        *size = bytes(mb);
                    }
                }
            });
            ui.checkbox(&mut f.new_only, &words.new_only);
            save_ui(ui, w, words);
        });
}

/// "Save filter as…", its name typed in place, and "Delete filter" on a
/// named one.
fn save_ui(ui: &mut egui::Ui, w: &mut ImportWindow, words: &ProfileWords) {
    let named = w
        .filters
        .iter()
        .find(|(_, f)| **f == w.choices.filter)
        .map(|(n, _)| n.clone());
    ui.horizontal_wrapped(|ui| match w.profile_edit.saving.take() {
        Some(mut name) => {
            let typed = ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0));
            let enter = typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button(&words.save).clicked() || enter) && !name.trim().is_empty() {
                w.save_filter(name.trim());
            } else if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                w.profile_edit.saving = Some(name);
            }
        }
        None => {
            if !w.choices.filter.is_all() && ui.small_button(&words.save_as).clicked() {
                w.profile_edit.saving = Some(named.clone().unwrap_or_default());
            }
            if let Some(name) = &named
                && ui.small_button(&words.delete).clicked()
            {
                w.delete_filter(name);
            }
        }
    });
}

/// Apply During Import: what every photo of the selected source is given.
pub(super) fn apply_ui(ui: &mut egui::Ui, w: &mut ImportWindow, words: &ProfileWords) {
    egui::CollapsingHeader::new(&words.apply)
        .id_salt("helios.import.apply")
        .default_open(!w.choices.apply.is_empty())
        .show(ui, |ui| {
            let a = &mut w.choices.apply;
            let e = &mut w.profile_edit;
            if items_of(&e.keywords) != a.keywords {
                e.keywords = line_of(&a.keywords);
            }
            ui.horizontal(|ui| {
                ui.label(&words.keywords);
                if ui
                    .add(egui::TextEdit::singleline(&mut e.keywords).desired_width(f32::INFINITY))
                    .on_hover_text(&words.keywords_help)
                    .changed()
                {
                    a.keywords = items_of(&e.keywords);
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(&words.rating);
                ui.selectable_value(&mut a.rating, None, &words.none);
                for stars in 1..=5u8 {
                    ui.selectable_value(&mut a.rating, Some(stars), "★".repeat(stars.into()));
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(&words.label);
                ui.selectable_value(&mut a.label, None, &words.none);
                for (label, said) in LABELS.into_iter().zip(&words.labels) {
                    ui.selectable_value(&mut a.label, Some(label.to_string()), said);
                }
            });
        });
}

impl ImportWindow {
    /// The selected source takes the named filter `name`.
    pub(super) fn use_filter(&mut self, name: &str) {
        if let Some(filter) = self.filters.get(name) {
            self.choices.filter = filter.clone();
        }
    }

    /// Saves the selected source's filter as `name`, replacing one of
    /// that name.
    pub(super) fn save_filter(&mut self, name: &str) {
        self.filters
            .insert(name.to_string(), self.choices.filter.clone());
        self.save_filters();
    }

    /// Deletes the named filter `name`; the sources keep theirs.
    pub(super) fn delete_filter(&mut self, name: &str) {
        if self.filters.remove(name).is_some() {
            self.save_filters();
        }
    }

    fn save_filters(&self) {
        let Some(file) = &self.filters_file else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = serde_json::to_string_pretty(&self.filters).unwrap_or_default();
        if let Err(e) = std::fs::write(file, text) {
            log::warn!("import filters: {}: {e}", file.display());
        }
    }
}

impl Shell {
    /// The named import filters, by name.
    pub fn import_filters(&self) -> Vec<(String, Filter)> {
        self.import
            .get()
            .filters
            .iter()
            .map(|(n, f)| (n.clone(), f.clone()))
            .collect()
    }

    /// Saves `filter` as the named import filter `name`, as "Save filter
    /// as…" does.
    pub fn save_import_filter(&mut self, name: &str, filter: Filter) {
        let mut w = self.import.get_mut();
        w.filters.insert(name.to_string(), filter);
        w.save_filters();
    }

    /// The import source of the folder or card at `path` takes the named
    /// filter `name`, as picking it in its Include section does: kept on
    /// the source.
    ///
    /// # Errors
    /// No filter of that name.
    pub fn use_import_filter_on(&mut self, path: &Path, name: &str) -> Result<(), String> {
        let mut guard = self.import.get_mut();
        let w = &mut *guard;
        let filter = w
            .filters
            .get(name)
            .cloned()
            .ok_or_else(|| format!("no import filter named `{name}`"))?;
        set_source_choices(w, path, |c| c.filter = filter);
        Ok(())
    }

    /// The import source of the folder or card at `path` gives every photo
    /// it imports `apply`, kept on the source.
    pub fn set_import_apply_on(&mut self, path: &Path, apply: Apply) {
        let mut guard = self.import.get_mut();
        set_source_choices(&mut guard, path, |c| c.apply = apply);
    }
}

/// The choices of the source of the folder or card at `path`, changed by
/// `change` and kept on it.
fn set_source_choices(
    w: &mut ImportWindow,
    path: &Path,
    change: impl FnOnce(&mut super::import_choices::Choices),
) {
    let key = SourceKey::Folder(path.to_path_buf());
    let mut choices = w.choices_for(path);
    change(&mut choices);
    let connector = w.connector_of(&key);
    w.sources.set(&key, &connector, Some(choices.clone()));
    if w.sources.showing.as_ref() == Some(&key) {
        w.choices = choices;
    }
}
