//! The Duplicates panel: the exact duplicates an import found (a file
//! whose content the library already has, or that came earlier in the same
//! import), each with what the import's policy did with it (see
//! `docs/library.md`, "Exact duplicates").
//!
//! - `skip`: left out, listed as skipped;
//! - `keep`: imported as one more copy of the same image;
//! - `ask`: left out and waiting: the panel asks, file by file, whether to
//!   keep it too or skip it. Keeping imports that file again with the
//!   `keep` policy (and the import's own placement and renaming), as a
//!   `library.import` the host runs ([`Duplicates::take_imports`]).
//!
//! Nothing is ever deleted. The host sends each import's lines here, from
//! the thread the import runs on ([`Duplicates::sink`]).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::Ui;
use helios_core::Message;
use serde_json::{Value, json};

/// The panel's id.
pub const PANEL: &str = "helios.duplicates";

/// What became of a duplicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    /// Left out by the `skip` policy.
    Skipped,
    /// Imported as one more copy, by the `keep` policy.
    Located,
    /// Left out by the `ask` policy, waiting for an answer.
    Asked,
    /// Kept after being asked: imported again as one more copy.
    Kept,
    /// Skipped after being asked.
    Declined,
}

/// A duplicate an import found.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub path: PathBuf,
    /// The image's id (hex).
    pub id: String,
    pub fate: Fate,
    /// Whether the library had it (`library`) or an earlier file of the
    /// same import (`source`).
    pub of: String,
    /// Where the library has it, or the earlier file.
    pub copies: Vec<PathBuf>,
    /// The import's arguments (`library.import`'s), to import it again.
    pub import: Value,
}

/// The duplicates found so far, and the imports the answers ask for.
pub struct Duplicates {
    found: Vec<Found>,
    imports: Vec<Value>,
    sender: Sender<(Value, Value)>,
    receiver: Receiver<(Value, Value)>,
}

impl Default for Duplicates {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Duplicates {
            found: Vec::new(),
            imports: Vec::new(),
            sender,
            receiver,
        }
    }
}

impl Duplicates {
    /// Where an import's lines are sent, with its arguments, from any
    /// thread.
    pub fn sink(&self) -> Sender<(Value, Value)> {
        self.sender.clone()
    }

    /// Takes in the lines sent; how many duplicates now wait for an
    /// answer among them.
    pub fn receive(&mut self) -> usize {
        let mut asked = 0;
        while let Ok((import, line)) = self.receiver.try_recv() {
            if self.add(&import, &line) == Some(Fate::Asked) {
                asked += 1;
            }
        }
        asked
    }

    /// Takes in one line of an import (`library.import`'s arguments, and
    /// the line the import gave for a file): kept when it is a duplicate.
    pub fn add(&mut self, import: &Value, line: &Value) -> Option<Fate> {
        let duplicate = line.get("duplicate")?;
        let fate = match line["status"].as_str()? {
            "skipped" => Fate::Skipped,
            "located" => Fate::Located,
            "asked" => Fate::Asked,
            _ => return None,
        };
        let path = PathBuf::from(line["path"].as_str()?);
        let of = duplicate["of"].as_str().unwrap_or("library").to_string();
        let copies = match of.as_str() {
            "source" => duplicate["first"]
                .as_str()
                .map(PathBuf::from)
                .into_iter()
                .collect(),
            _ => duplicate["at"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.as_str().map(PathBuf::from))
                        .collect()
                })
                .unwrap_or_default(),
        };
        // The same file again (an import run twice): its newest fate.
        self.found.retain(|f| f.path != path);
        self.found.push(Found {
            path,
            id: line["id"].as_str().unwrap_or_default().to_string(),
            fate,
            of,
            copies,
            import: import.clone(),
        });
        Some(fate)
    }

    /// The duplicates found, oldest first.
    pub fn found(&self) -> &[Found] {
        &self.found
    }

    /// How many wait for an answer.
    pub fn waiting(&self) -> usize {
        self.found.iter().filter(|f| f.fate == Fate::Asked).count()
    }

    fn asked(&mut self, index: usize) -> Result<&mut Found, String> {
        self.found
            .get_mut(index)
            .filter(|f| f.fate == Fate::Asked)
            .ok_or_else(|| format!("duplicate {index} waits for no answer"))
    }

    /// Keeps a duplicate waiting: it is imported again, as one more copy
    /// of its image, placed and named as its import said.
    pub fn keep(&mut self, index: usize) -> Result<(), String> {
        let found = self.asked(index)?;
        found.fate = Fate::Kept;
        let mut args = json!({ "path": found.path, "duplicates": "keep" });
        for key in ["copy", "into", "rename"] {
            if let Some(v) = found.import.get(key) {
                args[key] = v.clone();
            }
        }
        self.imports.push(args);
        Ok(())
    }

    /// Skips a duplicate waiting: it stays out of the library, untouched.
    pub fn skip(&mut self, index: usize) -> Result<(), String> {
        self.asked(index)?.fate = Fate::Declined;
        Ok(())
    }

    /// Answers every duplicate waiting the same way.
    pub fn answer_all(&mut self, keep: bool) {
        for i in 0..self.found.len() {
            if self.found[i].fate == Fate::Asked {
                let _ = if keep { self.keep(i) } else { self.skip(i) };
            }
        }
    }

    /// Forgets the duplicates no longer waiting.
    pub fn clear_answered(&mut self) {
        self.found.retain(|f| f.fate == Fate::Asked);
    }

    /// The imports the answers ask for (`library.import`'s arguments),
    /// for the host to run.
    pub fn take_imports(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.imports)
    }

    /// The panel: each duplicate with what became of it, a choice for each
    /// one waiting.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.receive();
        let ctx = ui.ctx().clone();
        let text = |m: Message| crate::i18n::text(&ctx, m);
        if self.found.is_empty() {
            ui.weak(text(Message::new(
                "gui-duplicates-none",
                "No duplicates found at import",
            )));
            return;
        }
        let waiting = self.waiting();
        let mut all = None;
        let mut clear = false;
        ui.horizontal(|ui| {
            if waiting > 0 {
                ui.strong(text(
                    Message::new(
                        "gui-duplicates-waiting",
                        "{ $count } duplicates wait for an answer",
                    )
                    .with("count", waiting),
                ));
                if ui
                    .button(text(Message::new("gui-duplicates-keep-all", "Keep all")))
                    .clicked()
                {
                    all = Some(true);
                }
                if ui
                    .button(text(Message::new("gui-duplicates-skip-all", "Skip all")))
                    .clicked()
                {
                    all = Some(false);
                }
            }
            if ui
                .button(text(Message::new(
                    "gui-duplicates-clear",
                    "Clear the answered",
                )))
                .clicked()
            {
                clear = true;
            }
        });
        let mut answers = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("helios.duplicates.scroll")
            .show(ui, |ui| {
                for (i, f) in self.found.iter().enumerate() {
                    ui.separator();
                    let name = f.path.file_name().map_or_else(
                        || f.path.display().to_string(),
                        |n| n.to_string_lossy().into(),
                    );
                    ui.label(&name).on_hover_text(f.path.display().to_string());
                    let copies = f
                        .copies
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    ui.weak(if f.of == "source" {
                        text(
                            Message::new(
                                "gui-duplicates-of-source",
                                "The same file as { $first }, earlier in this import",
                            )
                            .with("first", copies),
                        )
                    } else {
                        text(
                            Message::new(
                                "gui-duplicates-of-library",
                                "Already in the library: { $at }",
                            )
                            .with("at", copies),
                        )
                    });
                    match f.fate {
                        Fate::Asked => {
                            ui.horizontal(|ui| {
                                ui.label(text(Message::new(
                                    "gui-duplicates-asked",
                                    "Keep it as one more copy, or skip it?",
                                )));
                                let keep =
                                    ui.button(text(Message::new("gui-duplicates-keep", "Keep")));
                                crate::name_rect(
                                    ui.ctx(),
                                    &format!("helios.duplicates.{i}.keep"),
                                    keep.rect,
                                );
                                if keep.clicked() {
                                    answers.push((i, true));
                                }
                                let skip =
                                    ui.button(text(Message::new("gui-duplicates-skip", "Skip")));
                                crate::name_rect(
                                    ui.ctx(),
                                    &format!("helios.duplicates.{i}.skip"),
                                    skip.rect,
                                );
                                if skip.clicked() {
                                    answers.push((i, false));
                                }
                            });
                        }
                        Fate::Skipped => {
                            ui.weak(text(Message::new(
                                "gui-duplicates-skipped",
                                "Skipped, as the import's policy said",
                            )));
                        }
                        Fate::Located => {
                            ui.weak(text(Message::new(
                                "gui-duplicates-located",
                                "Kept as one more copy, as the import's policy said",
                            )));
                        }
                        Fate::Kept => {
                            ui.weak(text(Message::new(
                                "gui-duplicates-kept",
                                "Kept as one more copy",
                            )));
                        }
                        Fate::Declined => {
                            ui.weak(text(Message::new("gui-duplicates-declined", "Skipped")));
                        }
                    }
                }
            });
        for (i, keep) in answers {
            let _ = if keep { self.keep(i) } else { self.skip(i) };
        }
        if let Some(keep) = all {
            self.answer_all(keep);
        }
        if clear {
            self.clear_answered();
        }
    }
}
