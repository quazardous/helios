//! The Log panel: what the desktop reports to read at leisure, newest
//! last: each import's outcome, and the darktable edits it could not keep
//! whole (items not applied, modes substituted, sidecars not read). A
//! toast says an import ended; the details wait here.
//!
//! Each entry keeps its message, not its text, so it reads in the
//! interface's language even after the language changes. Entries go to
//! the log file too (`log`, target `helios`).

use egui::Ui;
use helios_core::Message;

use crate::dialogs::Level;

/// The panel's id.
pub const PANEL: &str = "helios.log";

/// How many entries the panel keeps; the oldest go first.
pub const KEPT: usize = 1000;

/// One entry.
#[derive(Clone, Debug)]
pub struct Entry {
    pub level: Level,
    pub message: Message,
}

/// The entries, oldest first.
#[derive(Debug, Default)]
pub struct Log {
    entries: Vec<Entry>,
}

impl Log {
    /// Adds an entry, and writes it to the log file in English.
    pub fn push(&mut self, level: Level, message: Message) {
        match level {
            Level::Info => log::info!(target: "helios", "{}", message.english),
            Level::Warning => log::warn!(target: "helios", "{}", message.english),
            Level::Error => log::error!(target: "helios", "{}", message.english),
        }
        if self.entries.len() == KEPT {
            self.entries.remove(0);
        }
        self.entries.push(Entry { level, message });
    }

    /// The entries, oldest first.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The panel: the entries, newest at the bottom, warnings and errors
    /// in the theme's colours.
    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let text = |m: &Message| crate::i18n::text(&ctx, m.clone());
        if self.entries.is_empty() {
            ui.weak(text(&Message::new(
                "gui-log-empty",
                "Nothing to report yet",
            )));
            return;
        }
        let mut clear = false;
        ui.horizontal(|ui| {
            if ui
                .button(text(&Message::new("gui-log-clear", "Clear")))
                .clicked()
            {
                clear = true;
            }
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for e in &self.entries {
                    let line = text(&e.message);
                    let colour = match e.level {
                        Level::Info => None,
                        Level::Warning => Some(ui.visuals().warn_fg_color),
                        Level::Error => Some(ui.visuals().error_fg_color),
                    };
                    let rich = egui::RichText::new(line);
                    let rich = match colour {
                        Some(c) => rich.color(c),
                        None => rich,
                    };
                    ui.add(egui::Label::new(rich).wrap());
                }
            });
        if clear {
            self.entries.clear();
        }
    }
}
