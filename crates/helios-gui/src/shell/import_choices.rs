//! The import window's Choices column (`docs/design/import-window.md`),
//! top to bottom:
//!
//! - **Copy into the Library**, or **Add where they are**: a card's photos
//!   are copied unless the person switches (a card is formatted later), a
//!   folder's as last chosen;
//! - **Into**: the folder copies go to (egui-pop's folder chooser; the
//!   Library folder when none is chosen) and its layout by capture date,
//!   with an example path from the first photo ticked;
//! - **Name**: the rename pattern, a preset or a custom one, with an
//!   example name from the same photo;
//! - **Duplicates**: skip, keep or ask;
//! - **Read darktable edits**;
//! - **More**, folded: metadata and time offsets, to come.
//!
//! Into and Name apply to a copy only: "Add where they are" hides them.
//! The choices reach the import as `library.importFiles`'s arguments
//! ([`Choices::args`]) and are kept, as the last used, per library
//! ([`Shell::remember_import_choices_in`]). A preset is a named
//! [`Choices`], kept beside them (`import-presets.json`): copy or add
//! for folders included, a card still copied unless switched.

use std::path::{Path, PathBuf};

use helios_core::Message;
use helios_library::import::{Duplicates, Layout};
use helios_library::rename::{Naming, Pattern};
use helios_library::signals::{self, Signals};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Shell;
use super::import_window::ImportWindow;

/// The rename presets' patterns, in the order shown.
const NAMES: [&str; 4] = [
    "{stem}",
    "{date}_{stem}",
    "{date}-{seq}",
    "{date}_{time}_{camera}",
];

/// What an import does with the photos, kept between runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choices {
    /// A folder's photos copied too (a card's always are, unless the
    /// person switches).
    pub copy_folders: bool,
    /// Where copies go; the Library folder when `None`.
    pub into: Option<PathBuf>,
    pub layout: Layout,
    /// The rename pattern (`{stem}`: the original name).
    pub rename: String,
    pub duplicates: Duplicates,
    pub darktable: bool,
}

impl Default for Choices {
    fn default() -> Self {
        Choices {
            copy_folders: false,
            into: None,
            layout: Layout::Day,
            rename: "{stem}".into(),
            duplicates: Duplicates::Skip,
            darktable: true,
        }
    }
}

impl Choices {
    /// `library.importFiles`'s arguments for `paths`, copied or not.
    pub fn args(&self, paths: &[PathBuf], copy: bool) -> Value {
        let mut args = json!({
            "paths": paths,
            "copy": copy,
            "duplicates": self.duplicates.as_str(),
            "darktable": self.darktable,
        });
        if copy {
            args["layout"] = json!(self.layout.as_str());
            args["rename"] = json!(self.rename);
            if let Some(into) = &self.into {
                args["into"] = json!(into);
            }
        }
        args
    }
}

/// The photo the examples are made from: its main file, when it was
/// taken and by which camera.
pub(super) struct Sample {
    pub path: PathBuf,
    pub time: Option<chrono::NaiveDateTime>,
    pub camera: Option<String>,
}

/// The examples of a sample: its folder (`2026/2026-11-02/`) and its
/// name (`20261102_IMG_0001.JPG`); the name is an error when the
/// pattern is not one.
pub(super) fn examples(choices: &Choices, sample: &Sample) -> (String, Result<String, String>) {
    let signals = Signals {
        taken: sample.time.map(|t| t.and_utc().timestamp() as f64),
        model: sample.camera.clone(),
        frame: signals::name_frame(&sample.path),
        ..Signals::default()
    };
    let folder = choices.layout.folder(signals.date());
    let folder = folder.components().fold(String::new(), |mut out, c| {
        out.push_str(&c.as_os_str().to_string_lossy());
        out.push('/');
        out
    });
    let stem = sample
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image");
    let ext = sample.path.extension().and_then(|e| e.to_str());
    let name = Pattern::parse(&choices.rename).map(|p| {
        let name = p.name(&Naming {
            stem,
            signals: &signals,
            seq: 1,
        });
        match ext {
            Some(ext) => format!("{name}.{ext}"),
            None => name,
        }
    });
    (folder, name)
}

/// What the Choices column asked for this frame.
pub(super) enum Asked {
    /// The folder chooser, for Into.
    Into,
}

/// The Choices column's words.
pub(super) struct ChoiceWords {
    pub heading: String,
    pub copy: String,
    pub add: String,
    pub card_note: String,
    pub into: String,
    pub library_folder: String,
    pub change: String,
    pub layouts: [String; 4],
    pub folder_example: Option<String>,
    pub name: String,
    pub names: Vec<(String, String)>,
    pub custom: String,
    pub name_example: Option<String>,
    pub bad_name: Option<String>,
    pub no_sample: String,
    pub duplicates: String,
    pub skip: String,
    pub keep: String,
    pub ask: String,
    pub darktable: String,
    pub darktable_help: String,
    pub more: String,
    pub more_note: String,
}

impl ImportWindow {
    /// Whether the import copies: as the person switched it, else a
    /// card's photos copied, a folder's as last chosen.
    pub(super) fn copies(&self) -> bool {
        self.copy.unwrap_or_else(|| {
            self.ticked.iter().any(|p| self.is_card(p)) || self.choices.copy_folders
        })
    }

    /// The first photo ticked, the examples' sample.
    pub(super) fn sample(&mut self) -> Option<Sample> {
        let c = &mut self.contents;
        let first = c
            .shown()
            .into_iter()
            .find(|&i| c.cells[i].ticked && c.importable(i))?;
        let cell = &c.cells[first];
        Some(Sample {
            path: cell.raw.clone().or_else(|| cell.picture.clone())?,
            time: cell.time(),
            camera: cell.camera.clone(),
        })
    }

    /// The preset the choices are, when they are one.
    pub(super) fn preset_of_choices(&self) -> Option<String> {
        self.presets
            .iter()
            .find(|(_, c)| **c == self.choices)
            .map(|(name, _)| name.clone())
    }

    fn save_presets(&self) {
        let Some(file) = &self.presets_file else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = serde_json::to_string_pretty(&self.presets).unwrap_or_default();
        if let Err(e) = std::fs::write(file, text) {
            log::warn!("import presets: {}: {e}", file.display());
        }
    }

    /// Keeps the choices, as the last used, where the desktop said.
    pub(super) fn save_choices(&self) {
        let Some(file) = &self.choices_file else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = serde_json::to_string_pretty(&self.choices).unwrap_or_default();
        if let Err(e) = std::fs::write(file, text) {
            log::warn!("import choices: {}: {e}", file.display());
        }
    }
}

impl Shell {
    /// Keeps the import window's choices, as the last used, in `file`
    /// (JSON, one per library), read now.
    /// The presets are kept beside it, in `import-presets.json`.
    pub fn remember_import_choices_in(&mut self, file: PathBuf) {
        let read = |f: &Path| std::fs::read_to_string(f).ok();
        let mut w = self.import.get_mut();
        w.choices = read(&file)
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let presets = file.with_file_name("import-presets.json");
        w.presets = read(&presets)
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        w.presets_file = Some(presets);
        w.choices_file = Some(file);
    }

    /// The import presets' names, in order.
    pub fn import_presets(&self) -> Vec<String> {
        self.import.get().presets.keys().cloned().collect()
    }

    /// Sets the Choices column to a preset's (`None`: Default): copy or
    /// add for folders included, a card still copied unless switched.
    pub fn apply_import_preset(&mut self, name: Option<&str>) {
        self.import.get_mut().apply_preset(name);
    }

    /// Saves the choices as the preset `name`, replacing one of that name.
    /// The copy or add shown is kept for folders when no card is ticked.
    pub fn save_import_preset(&mut self, name: &str) {
        self.import.get_mut().save_preset(name);
    }

    /// Deletes the preset `name`; the choices stay as they are.
    pub fn delete_import_preset(&mut self, name: &str) {
        self.import.get_mut().delete_preset(name);
    }

    /// The import window's choices.
    pub fn import_choices(&self) -> Choices {
        self.import.get().choices.clone()
    }

    /// Sets the import window's choices.
    pub fn set_import_choices(&mut self, choices: Choices) {
        self.import.get_mut().choices = choices;
    }

    /// Whether the import window copies the photos.
    pub fn import_copies(&self) -> bool {
        self.import.get().copies()
    }
}

impl ImportWindow {
    /// Sets the choices to a preset's (`None`: Default).
    pub(super) fn apply_preset(&mut self, name: Option<&str>) {
        let choices = match name {
            Some(name) => match self.presets.get(name) {
                Some(c) => c.clone(),
                None => return,
            },
            None => Choices::default(),
        };
        self.choices = choices;
        self.copy = None;
        self.custom = false;
    }

    /// Saves the choices as the preset `name`.
    pub(super) fn save_preset(&mut self, name: &str) {
        let mut choices = self.choices.clone();
        if !self.ticked.iter().any(|p| self.is_card(p)) {
            choices.copy_folders = self.copies();
        }
        self.choices.copy_folders = choices.copy_folders;
        self.presets.insert(name.to_string(), choices);
        self.save_presets();
    }

    /// Deletes the preset `name`.
    pub(super) fn delete_preset(&mut self, name: &str) {
        if self.presets.remove(name).is_some() {
            self.save_presets();
        }
    }

    pub(super) fn choice_words(
        &self,
        t: &dyn Fn(Message) -> String,
        sample: Option<&Sample>,
    ) -> ChoiceWords {
        let w = self;
        let (folder_example, name_example, bad_name) = match sample {
            Some(s) => {
                let (folder, name) = examples(&w.choices, s);
                let eg = |example: String| {
                    t(
                        Message::new("gui-import-window-example", "e.g. { $example }")
                            .with("example", example),
                    )
                };
                match name {
                    Ok(name) => (Some(eg(folder)), Some(eg(name)), None),
                    Err(e) => (Some(eg(folder)), None, Some(e)),
                }
            }
            None => (None, None, Pattern::parse(&w.choices.rename).err()),
        };
        ChoiceWords {
            heading: t(Message::new("gui-import-window-choices", "Choices")),
            copy: t(Message::new(
                "gui-import-window-copy",
                "Copy into the Library",
            )),
            add: t(Message::new("gui-import-window-add", "Add where they are")),
            card_note: t(Message::new(
                "gui-import-window-card-note",
                "A card's photos are copied: the card is formatted later, the copies stay.",
            )),
            into: t(Message::new("gui-import-window-into", "Into")),
            library_folder: t(Message::new(
                "gui-import-window-library-folder",
                "The Library folder",
            )),
            change: t(Message::new(
                "gui-import-window-change-into",
                "Choose where copies go",
            )),
            layouts: [
                t(Message::new("gui-import-window-layout-day", "By day")),
                t(Message::new("gui-import-window-layout-month", "By month")),
                t(Message::new("gui-import-window-layout-year", "By year")),
                t(Message::new("gui-import-window-layout-flat", "One folder")),
            ],
            folder_example,
            name: t(Message::new("gui-import-window-name", "Name")),
            names: NAMES
                .iter()
                .map(|p| (*p).to_string())
                .zip([
                    t(Message::new(
                        "gui-import-window-name-original",
                        "Original name",
                    )),
                    t(Message::new(
                        "gui-import-window-name-date-stem",
                        "Date and name",
                    )),
                    t(Message::new(
                        "gui-import-window-name-date-seq",
                        "Date and number",
                    )),
                    t(Message::new(
                        "gui-import-window-name-date-camera",
                        "Date, time and camera",
                    )),
                ])
                .collect(),
            custom: t(Message::new("gui-import-window-name-custom", "Custom")),
            name_example,
            bad_name,
            no_sample: t(Message::new(
                "gui-import-window-no-sample",
                "Tick a photo to see an example",
            )),
            duplicates: t(Message::new("gui-import-window-duplicates", "Duplicates")),
            skip: t(Message::new("gui-import-window-duplicates-skip", "Skip")),
            keep: t(Message::new("gui-import-window-duplicates-keep", "Keep")),
            ask: t(Message::new("gui-import-window-duplicates-ask", "Ask")),
            darktable: t(Message::new(
                "gui-import-window-darktable",
                "Read darktable edits",
            )),
            darktable_help: t(Message::new(
                "gui-import-window-darktable-help",
                "Each .xmp sidecar, duplicates included, becomes a version of its photo",
            )),
            more: t(Message::new("gui-import-window-more", "More")),
            more_note: t(Message::new(
                "gui-import-window-more-note",
                "Metadata templates and time offsets come later.",
            )),
        }
    }
}

fn folder_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The Choices column.
pub(super) fn choices_ui(
    ui: &mut egui::Ui,
    w: &mut ImportWindow,
    words: &ChoiceWords,
) -> Option<Asked> {
    let mut asked = None;
    ui.heading(&words.heading);
    egui::ScrollArea::vertical()
        .id_salt("helios.import.choices")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut copy = w.copies();
            let before = copy;
            ui.radio_value(&mut copy, true, &words.copy);
            ui.radio_value(&mut copy, false, &words.add);
            if copy != before {
                w.copy = Some(copy);
            }
            if w.ticked.iter().any(|p| w.is_card(p)) {
                ui.add(egui::Label::new(egui::RichText::new(&words.card_note).weak()).wrap());
            }
            if copy {
                ui.separator();
                copy_choices_ui(ui, w, words, &mut asked);
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(&words.duplicates);
                let d = &mut w.choices.duplicates;
                ui.radio_value(d, Duplicates::Skip, &words.skip);
                ui.radio_value(d, Duplicates::Keep, &words.keep);
                ui.radio_value(d, Duplicates::Ask, &words.ask);
            });
            ui.checkbox(&mut w.choices.darktable, &words.darktable)
                .on_hover_text(&words.darktable_help);
            egui::CollapsingHeader::new(&words.more)
                .id_salt("helios.import.more")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&words.more_note).weak()).wrap());
                });
        });
    asked
}

/// Into and Name: a copy's.
fn copy_choices_ui(
    ui: &mut egui::Ui,
    w: &mut ImportWindow,
    words: &ChoiceWords,
    asked: &mut Option<Asked>,
) {
    let c = &mut w.choices;
    ui.horizontal(|ui| {
        ui.strong(&words.into);
        let shown = c
            .into
            .as_deref()
            .map_or_else(|| words.library_folder.clone(), folder_name);
        let button = ui.button(format!("{shown} …")).on_hover_text(&words.change);
        if button.clicked() {
            *asked = Some(Asked::Into);
        }
    });
    ui.horizontal_wrapped(|ui| {
        for (layout, label) in Layout::ALL.into_iter().zip(&words.layouts) {
            ui.selectable_value(&mut c.layout, layout, label);
        }
    });
    match &words.folder_example {
        Some(example) => ui.weak(example),
        None => ui.weak(&words.no_sample),
    };
    ui.add_space(4.0);
    ui.strong(&words.name);
    let preset = words.names.iter().position(|(p, _)| *p == c.rename);
    ui.horizontal_wrapped(|ui| {
        for (pattern, label) in &words.names {
            if ui
                .selectable_label(c.rename == *pattern && !w.custom, label)
                .clicked()
            {
                c.rename.clone_from(pattern);
                w.custom = false;
            }
        }
        if ui
            .selectable_label(w.custom || preset.is_none(), &words.custom)
            .clicked()
        {
            w.custom = true;
        }
    });
    if w.custom || preset.is_none() {
        ui.add(
            egui::TextEdit::singleline(&mut c.rename)
                .hint_text("{date}_{stem}")
                .desired_width(f32::INFINITY),
        )
        .on_hover_text("{date} {time} {year} {month} {day} {camera} {maker} {frame} {stem} {seq}");
    }
    if let Some(e) = &words.bad_name {
        ui.colored_label(ui.visuals().error_fg_color, e);
    } else if let Some(example) = &words.name_example {
        ui.weak(example);
    }
}
