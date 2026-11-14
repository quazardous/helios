//! The import profile editor (`docs/design/import-window.md`): "Manage
//! profiles…" in the Detail column's profile menu opens it in the import
//! window's body, as the "Add a source…" wizard does.
//!
//! - **The list**, on the left: Everyday first, then the person's
//!   profiles; New, Duplicate, Rename and Delete (Everyday is neither
//!   renamed nor deleted), and the connectors the selected profile is
//!   the default of (each connector the registry offers, by its label).
//! - **The editor**, on the right: the selected profile's choices grouped
//!   as Copy, Naming, Filter, Apply and Advanced, with a live example of
//!   a file's folder and name (the first photo ticked, else a made-up
//!   one).
//!
//! A change is saved on the profile at once, and every source pointing
//! at it takes it ([`ImportWindow::link_sources`]). While the editor is
//! open the window's choices are the selected profile's, so the widgets
//! of the Choices column ([`super::import_choices`],
//! [`super::import_profiles`]) edit it; closing it gives the window back
//! the choices of the source shown.

use std::path::{Path, PathBuf};

use egui_pop::core::overlay::DialogLayout;
use helios_core::Message;
use helios_library::import::Duplicates;

use super::Shell;
use super::import_choices::{self, ChoiceWords, Choices, Sample};
use super::import_profile_store::ImportProfile;
use super::import_sources::{SavedSource, SourceKey};
use super::import_window::ImportWindow;

/// The editor's state, while it is open.
#[derive(Debug)]
pub(crate) struct Manager {
    /// The profile edited.
    selected: String,
    /// Its name, while it is typed (Rename).
    renaming: Option<String>,
}

impl ImportWindow {
    /// The profile the Detail column shows: the source shown's, else its
    /// connector's default, else the window's.
    pub(super) fn profile_in_use(&self) -> String {
        if let Some(key) = &self.sources.showing {
            if let Some(id) = self.sources.get(key).and_then(|s| s.profile.clone()) {
                return id;
            }
            if let Some(id) = self
                .connector_known(key)
                .and_then(|c| self.profiles.defaults.get(&c))
            {
                return id.clone();
            }
        }
        self.profiles.current.clone()
    }

    /// The profile menu's pick: the source shown points at `id`, else the
    /// window uses it.
    pub(super) fn use_profile(&mut self, id: &str) {
        if self.profiles.get(id).is_none() {
            return;
        }
        let choices = self.profiles.choices(id);
        if let Some(key) = self.sources.showing.clone() {
            let connector = self.connector_of(&key);
            self.sources
                .set_profile(&key, &connector, id, choices.clone());
        } else {
            self.profiles.current = id.to_string();
            self.profiles.save();
        }
        self.choices = choices;
        self.copy = None;
        self.custom = false;
    }

    /// Opens the editor on the profile in use.
    pub(super) fn open_manager(&mut self) {
        let selected = self.profile_in_use();
        self.sources.window = self.window_choices().clone();
        self.choices = self.profiles.choices(&selected);
        self.custom = false;
        self.manager = Some(Manager {
            selected,
            renaming: None,
        });
    }

    /// Closes the editor: the window's choices are the source shown's
    /// again.
    fn close_manager(&mut self) {
        self.keep_profile_edits();
        self.manager = None;
        self.sources.window = self.profiles.choices(&self.profiles.current);
        self.choices = match self.sources.showing.clone() {
            Some(key) => self
                .sources
                .own(&key)
                .cloned()
                .unwrap_or_else(|| self.kind_default(&key)),
            None => self.sources.window.clone(),
        };
        self.custom = false;
    }

    /// Edits the profile `id` instead.
    fn select_profile(&mut self, id: &str) {
        self.keep_profile_edits();
        if let Some(m) = &mut self.manager {
            m.selected = id.to_string();
            m.renaming = None;
        }
        self.choices = self.profiles.choices(id);
        self.custom = false;
    }

    /// The choices edited, saved on the selected profile when changed.
    pub(super) fn keep_profile_edits(&mut self) {
        let Some(m) = &self.manager else { return };
        if self
            .profiles
            .set_choices(&m.selected.clone(), self.choices.clone())
        {
            self.profiles.save();
        }
    }

    /// Every source takes its profile's choices; a source with choices of
    /// its own and no profile (kept before profiles, or set by a command)
    /// points at the profile that has them, made when there is none.
    pub(super) fn link_sources(&mut self) {
        let mut stored = false;
        let mut made = false;
        let profiles = &mut self.profiles;
        for s in &mut self.sources.list {
            let known = s.profile.as_deref().and_then(|id| profiles.get(id));
            match (known, &s.choices) {
                (Some(p), _) => {
                    if s.choices.as_ref() != Some(&p.choices) {
                        s.choices = Some(p.choices.clone());
                        stored = true;
                    }
                }
                (None, Some(c)) => {
                    let id = profiles.find(c).unwrap_or_else(|| {
                        made = true;
                        profiles.add(&profile_name(s), c.clone())
                    });
                    s.profile = Some(id);
                    stored = true;
                }
                (None, None) => {
                    if s.profile.take().is_some() {
                        stored = true;
                    }
                }
            }
        }
        if made {
            self.profiles.save();
        }
        if stored {
            self.sources.store();
            // The column shows the source's choices as they are now.
            if self.manager.is_none()
                && let Some(key) = &self.sources.showing
                && let Some(own) = self.sources.own(key)
            {
                self.choices = own.clone();
            }
        }
    }

    /// The editor, in the window's body; whether Into's folder chooser is
    /// asked.
    pub(super) fn manager_ui(&mut self, ui: &mut egui::Ui, words: &ChoiceWords) -> bool {
        let Some(texts) = self.texts.clone() else {
            return false;
        };
        let t = |m: Message| texts(m);
        let mut into = false;
        let mut done = false;
        let layout = DialogLayout::new(ui, "helios.import.profiles").fill(true);
        let height = layout.body_height(ui);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        let mut body = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("helios.import.profiles.body")
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        body.set_clip_rect(rect.intersect(body.clip_rect()));
        body.heading(t(Message::new(
            "gui-import-profiles-title",
            "Import profiles",
        )));
        let list_width = (rect.width() * 0.3).clamp(160.0, 260.0);
        body.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(list_width);
                self.list_ui(ui, &t);
            });
            ui.separator();
            ui.vertical(|ui| {
                egui::ScrollArea::vertical()
                    .id_salt("helios.import.profiles.editor")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        into = self.editor_ui(ui, words, &t);
                    });
            });
        });
        layout.footer(ui, |ui| {
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(t(Message::new("gui-import-profiles-done", "Done")))
                    .clicked()
                {
                    done = true;
                }
            });
        });
        if done {
            self.close_manager();
        }
        into
    }

    /// The profiles, and what is done to the selected one.
    fn list_ui(&mut self, ui: &mut egui::Ui, t: &dyn Fn(Message) -> String) {
        let Some(selected) = self.manager.as_ref().map(|m| m.selected.clone()) else {
            return;
        };
        let mut pick = None;
        for p in self.profiles.list() {
            if ui.selectable_label(p.id == selected, &p.name).clicked() {
                pick = Some(p.id.clone());
            }
        }
        ui.separator();
        let built_in = self
            .profiles
            .get(&selected)
            .is_some_and(ImportProfile::built_in);
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(t(Message::new("gui-import-profiles-new", "New")))
                .clicked()
            {
                let name = t(Message::new("gui-import-profiles-new-name", "New profile"));
                pick = Some(self.profiles.add(&name, Choices::default()));
                self.profiles.save();
            }
            if ui
                .button(t(Message::new(
                    "gui-import-profiles-duplicate",
                    "Duplicate",
                )))
                .clicked()
            {
                self.keep_profile_edits();
                let name = self
                    .profiles
                    .get(&selected)
                    .map_or_else(String::new, |p| p.name.clone());
                let name = t(
                    Message::new("gui-import-profiles-copy-of", "{ $name } copy")
                        .with("name", name),
                );
                pick = Some(self.profiles.add(&name, self.choices.clone()));
                self.profiles.save();
            }
            if ui
                .add_enabled(
                    !built_in,
                    egui::Button::new(t(Message::new("gui-import-profiles-rename", "Rename"))),
                )
                .clicked()
                && let Some(m) = &mut self.manager
            {
                m.renaming = self.profiles.get(&selected).map(|p| p.name.clone());
            }
            if ui
                .add_enabled(
                    !built_in,
                    egui::Button::new(t(Message::new("gui-import-profiles-delete", "Delete"))),
                )
                .clicked()
                && self.profiles.remove(&selected)
            {
                self.profiles.save();
                // Its sources take their kind's default again.
                for s in &mut self.sources.list {
                    if s.profile.as_deref() == Some(selected.as_str()) {
                        s.profile = None;
                        s.choices = None;
                    }
                }
                self.sources.store();
                self.manager = None;
                self.open_manager_on(super::import_profile_store::EVERYDAY);
            }
        });
        if let Some(name) = self.manager.as_mut().and_then(|m| m.renaming.as_mut()) {
            let edit = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let name = name.clone();
            if (enter
                || ui
                    .button(t(Message::new("gui-import-profiles-save-name", "Save")))
                    .clicked())
                && !name.trim().is_empty()
            {
                if self.profiles.rename(&selected, &name) {
                    self.profiles.save();
                }
                if let Some(m) = &mut self.manager {
                    m.renaming = None;
                }
            }
        }
        ui.separator();
        ui.label(t(Message::new(
            "gui-import-profiles-default-for",
            "Default for",
        )));
        // Each connector, by its label: none is special here.
        let texts = self.texts.clone();
        let connectors: Vec<(String, String)> = self
            .connectors
            .manifests()
            .into_iter()
            .map(|m| {
                let label = super::import_accounts::translated(texts.as_ref(), &m.label);
                (m.id, label)
            })
            .collect();
        for (connector, label) in connectors {
            let mut on = self.profiles.defaults.get(&connector) == Some(&selected);
            if ui.checkbox(&mut on, label).changed() {
                if on {
                    self.profiles.defaults.insert(connector, selected.clone());
                } else {
                    self.profiles.defaults.remove(&connector);
                }
                self.profiles.save();
            }
        }
        if let Some(id) = pick {
            self.select_profile(&id);
        }
    }

    /// Opens the editor on the profile `id`.
    fn open_manager_on(&mut self, id: &str) {
        self.choices = self.profiles.choices(id);
        self.custom = false;
        self.manager = Some(Manager {
            selected: id.to_string(),
            renaming: None,
        });
    }

    /// The selected profile's choices, grouped; whether Into's folder
    /// chooser is asked.
    fn editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        words: &ChoiceWords,
        t: &dyn Fn(Message) -> String,
    ) -> bool {
        let mut asked = None;
        // The live example: the first photo ticked, else a made-up one.
        let sample = self.sample().unwrap_or_else(|| Sample {
            path: PathBuf::from("IMG_0001.CR3"),
            time: chrono::Local::now().naive_local().into(),
            camera: Some("Camera".into()),
        });
        let (folder, name) = import_choices::examples(&self.choices, &sample);
        let into = self.choices.into.as_deref().map_or_else(
            || t(super::import_profile_store::library_name()),
            folder_name,
        );
        let example = match name {
            Ok(name) => format!("{into}/{folder}{name}"),
            Err(e) => e,
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong(t(Message::new("gui-import-profiles-example", "Example")));
            ui.monospace(example);
        });
        ui.separator();
        ui.strong(t(Message::new("gui-import-profiles-copy", "Copy")));
        ui.weak(t(Message::new(
            "gui-import-profiles-copy-help",
            "A source that cannot be added where it is, or that is wiped after import, is always copied",
        )));
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(&mut self.choices.copy_folders, true, &words.copy);
            ui.radio_value(&mut self.choices.copy_folders, false, &words.add);
        });
        ui.separator();
        ui.strong(t(Message::new("gui-import-profiles-naming", "Naming")));
        import_choices::copy_choices_ui(ui, self, words, &mut asked);
        ui.separator();
        ui.strong(t(Message::new("gui-import-profiles-filter", "Filter")));
        super::import_profiles::include_ui(ui, self, &words.profile);
        ui.separator();
        ui.strong(t(Message::new("gui-import-profiles-apply", "Apply")));
        super::import_profiles::apply_ui(ui, self, &words.profile);
        ui.separator();
        ui.strong(t(Message::new("gui-import-profiles-advanced", "Advanced")));
        ui.horizontal_wrapped(|ui| {
            ui.label(&words.duplicates);
            let d = &mut self.choices.duplicates;
            ui.radio_value(d, Duplicates::Skip, &words.skip);
            ui.radio_value(d, Duplicates::Keep, &words.keep);
            ui.radio_value(d, Duplicates::Ask, &words.ask);
        });
        ui.checkbox(&mut self.choices.darktable, &words.darktable)
            .on_hover_text(&words.darktable_help);
        if self.camera_looks {
            ui.checkbox(&mut self.choices.camera_look, &words.camera_look)
                .on_hover_text(&words.camera_look_help);
        }
        ui.checkbox(
            &mut self.choices.stack_pairs,
            t(Message::new(
                "gui-import-profiles-stack-pairs",
                "Stack RAW+JPEG pairs",
            )),
        )
        .on_hover_text(t(Message::new(
            "gui-import-profiles-stack-pairs-help",
            "A raw and a JPEG of the same shot are shown as one stack, the raw on top",
        )));
        asked.is_some()
    }
}

/// The name a profile made for a source takes: the source's.
fn profile_name(s: &SavedSource) -> String {
    s.name
        .clone()
        .or_else(|| s.label.clone())
        .or_else(|| {
            s.path
                .as_deref()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
        })
        .or_else(|| s.account.clone())
        .unwrap_or_else(|| "Source".into())
}

fn folder_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl Shell {
    /// The import profiles, Everyday first.
    pub fn import_profiles(&self) -> Vec<ImportProfile> {
        self.import.get().profiles.list().to_vec()
    }

    /// The import profile the import window's Detail column shows: the
    /// source shown's, else the window's.
    pub fn import_profile_in_use(&self) -> String {
        self.import.get().profile_in_use()
    }

    /// Picks the import profile `id` in the profile menu: the source
    /// shown points at it, else the window uses it. Whether it is one.
    pub fn use_import_profile(&mut self, id: &str) -> bool {
        let mut w = self.import.get_mut();
        if w.profiles.get(id).is_none() {
            return false;
        }
        w.use_profile(id);
        true
    }

    /// The import source of the folder or card at `path` points at the
    /// profile `id`.
    ///
    /// # Errors
    /// No profile of that id.
    pub fn use_import_profile_on(&mut self, path: &Path, id: &str) -> Result<(), String> {
        let mut guard = self.import.get_mut();
        let w = &mut *guard;
        if w.profiles.get(id).is_none() {
            return Err(format!("no import profile `{id}`"));
        }
        let key = SourceKey::Folder(path.to_path_buf());
        let connector = w.connector_of(&key);
        let choices = w.profiles.choices(id);
        w.sources.set_profile(&key, &connector, id, choices.clone());
        if w.sources.showing.as_ref() == Some(&key) {
            w.choices = choices;
        }
        Ok(())
    }

    /// Adds `profile`, or replaces the one of its id (an empty id: made
    /// of its name), as the profile editor does; every source pointing
    /// at it takes it. Its id.
    pub fn put_import_profile(&mut self, profile: ImportProfile) -> String {
        let mut w = self.import.get_mut();
        let id = w.profiles.put(profile);
        w.profiles.save();
        w.link_sources();
        if w.sources.showing.is_none() && w.manager.is_none() {
            w.choices = w.profiles.choices(&w.profiles.current);
        }
        id
    }

    /// Removes the import profile `id` (never Everyday); its sources take
    /// their kind's default again. Whether it was one.
    pub fn remove_import_profile(&mut self, id: &str) -> bool {
        let mut w = self.import.get_mut();
        if !w.profiles.remove(id) {
            return false;
        }
        w.profiles.save();
        for s in &mut w.sources.list {
            if s.profile.as_deref() == Some(id) {
                s.profile = None;
                s.choices = None;
            }
        }
        w.sources.store();
        true
    }

    /// Makes `id` the default profile of the sources of the connector
    /// `connector` (`None`: the window's).
    pub fn set_import_profile_default(&mut self, connector: &str, id: Option<&str>) {
        let mut w = self.import.get_mut();
        match id {
            Some(id) if w.profiles.get(id).is_some() => {
                w.profiles
                    .defaults
                    .insert(connector.to_string(), id.to_string());
            }
            _ => {
                w.profiles.defaults.remove(connector);
            }
        }
        w.profiles.save();
    }

    /// Opens the import profile editor in the import window, as "Manage
    /// profiles…" does.
    pub fn open_import_profiles(&mut self) {
        self.import.get_mut().open_manager();
    }

    /// Whether the import profile editor is open.
    pub fn import_profiles_open(&self) -> bool {
        self.import.get().manager.is_some()
    }
}
