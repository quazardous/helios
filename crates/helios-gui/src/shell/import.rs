//! Importing photos from the desktop, always within reach (see
//! `docs/library.md`, "Importing from the desktop"):
//!
//! - **Where from.** File > Import… (Ctrl+Shift+I) and the empty
//!   library's "Import photos…" open the import window
//!   (`library.importWindow`, `super::import_window`); files and folders
//!   dropped on the window, or pasted, open it with them as its sources.
//!   `library.importSource` and `library.importFiles` stay for scripts and
//!   plugins: run without their arguments, the import dialog asks.
//! - **Where to.** As the workspace's import source says. Without one,
//!   the import dialog asks: referenced in place (the default) or copied
//!   into the library folder, and "Remember for this workspace" makes the
//!   answer the workspace's import source (a `folder` source, one undo
//!   step), so the next import does not ask.
//! - **darktable edits.** Each photo's darktable sidecars become versions
//!   of it (`helios_library::darktable`), on by default; the dialog has
//!   the checkbox, and an import source may turn it off.
//! - **Importer plugins.** File > Import from lists an entry per
//!   importer a plugin declares (`library.importWith`): it asks for the
//!   file or folder the importer reads, then queues an import whose
//!   `importer` the host runs through `crate::importers::run`; what the
//!   plugin found, left out and warned about goes to the Log panel.
//! - **What it did.** The host runs the import as a job and sends its
//!   lines here ([`Shell::import_sink`]): duplicates go to the Duplicates
//!   panel, the darktable edits not kept whole to the Log panel, and the
//!   end to a short toast.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use egui::Context as Egui;
use egui_pop::api::toasts::{TOASTS, Toast};
use helios_core::Message;
use helios_shell::commands::Origin;
use helios_template::{ImportDesc, LibraryDesc};
use helios_values::Descriptor;
use serde_json::{Value, json};

use super::Shell;
use crate::dialogs::{Field, Form, Level, Notice};

/// The paths of an import's arguments: `path`, or `paths`.
fn paths_of(args: &Value) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = args
        .get("paths")
        .and_then(Value::as_array)
        .map(|p| {
            p.iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    paths.extend(
        args.get("path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
    );
    paths
}

fn boolean(key: &str, label: String, description: String, value: bool) -> Field {
    let mut descriptor = Descriptor::bool(&label, value);
    descriptor.description = description;
    Field {
        key: key.into(),
        descriptor,
        value: helios_core::Value::Bool(value),
    }
}

impl Shell {
    /// Where an import's lines go, with its arguments (`library.import`'s),
    /// from any thread: a line per file as `helios_library::import` gives
    /// it, then `{ "ended": report }` or `{ "failed": error }`.
    pub fn import_sink(&self) -> Sender<(Value, Value)> {
        self.core.import_lines.0.clone()
    }

    /// Whether an import can be queued: not without a library open, the
    /// error then saying so (rather than the host's missing command).
    pub(super) fn can_import(&self) -> Result<(), String> {
        match &self.core.no_library {
            None => Ok(()),
            Some(why) => Err(self.text(why.clone())),
        }
    }

    /// The Log panel's entries, to read.
    pub fn log(&self) -> std::cell::Ref<'_, crate::log_panel::Log> {
        self.core.log.borrow()
    }

    /// `library.importSource`: a card or folder (`path`) imported as the
    /// workspace's import source says, or as the arguments say (`copy`,
    /// `darktable`, `remember`); without a path, the import dialog asks
    /// for it, with the choices the workspace leaves open.
    pub(super) fn import_from_source(
        &mut self,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        self.can_import()?;
        let Some(path) = args
            .get("path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            let mut fields = vec![Field {
                key: "path".into(),
                descriptor: Descriptor::path(
                    &self.text(Message::new("gui-import-path", "card or folder")),
                    helios_values::PathKind::Dir,
                    &[],
                ),
                value: helios_core::Value::Text(String::new()),
            }];
            fields.extend(self.import_fields());
            self.form(Form {
                title: self.text(Message::new("gui-import-form-title", "Import photos")),
                fields,
                verb: self.text(Message::new("gui-import-verb", "Import")),
                command: "library.importSource".into(),
            });
            return Ok(Value::Null);
        };
        self.import_now(json!({ "path": path }), args, origin)
    }

    /// `library.importFiles`: files and folders (`paths`) imported as one
    /// import. With `ask` (a drop on the window), a workspace without an
    /// import source asks where they go first, unless `copy` says it.
    pub(super) fn import_files(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        self.can_import()?;
        let paths = paths_of(&json!({ "paths": args["paths"] }));
        if paths.is_empty() {
            return Err("`paths` is missing".into());
        }
        let asks =
            args["ask"] == true && self.focus.import_source.is_none() && args.get("copy").is_none();
        if asks {
            let title = self.text(
                Message::new("gui-import-dropped-title", "Import { $count } items")
                    .with("count", paths.len()),
            );
            let form = Form {
                title,
                fields: self.import_fields(),
                verb: self.text(Message::new("gui-import-verb", "Import")),
                command: "library.importFiles".into(),
            };
            self.form_with(form, json!({ "paths": paths }))?;
            return Ok(Value::Null);
        }
        self.import_now(json!({ "paths": paths }), args, origin)
    }

    /// `library.importWith`: an importer plugin's source (`source`)
    /// imported as the import dialog or the workspace's import source
    /// says; without a source, the dialog asks for it, a folder or a file
    /// as the importer reads.
    pub(super) fn import_with(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        let text = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        };
        self.can_import()?;
        let (plugin, importer) = (
            text("plugin").ok_or("`plugin` is missing")?,
            text("importer").ok_or("`importer` is missing")?,
        );
        let entry = self
            .core
            .lists
            .borrow()
            .importers
            .iter()
            .find(|e| e.plugin == plugin && e.importer == importer)
            .cloned()
            .ok_or_else(|| format!("no importer `{importer}` of plugin `{plugin}` is installed"))?;
        let Some(source) = text("source") else {
            let (label, kind) = match entry.kind {
                helios_plugin::manifest::SourceKind::Folder => (
                    Message::new("gui-importer-folder", "folder"),
                    helios_values::PathKind::Dir,
                ),
                helios_plugin::manifest::SourceKind::File => (
                    Message::new("gui-importer-file", "file"),
                    helios_values::PathKind::File,
                ),
            };
            let extensions: Vec<&str> = entry.extensions.iter().map(String::as_str).collect();
            let mut fields = vec![Field {
                key: "source".into(),
                descriptor: Descriptor::path(&self.text(label), kind, &extensions),
                value: helios_core::Value::Text(String::new()),
            }];
            // An import source is a folder of the workspace's: nothing to
            // remember of an importer's.
            fields.extend(
                self.import_fields()
                    .into_iter()
                    .filter(|f| f.key != "remember"),
            );
            let title = self.text(
                Message::new("gui-importer-form-title", "Import from { $importer }")
                    .with("importer", entry.message()),
            );
            let form = Form {
                title,
                fields,
                verb: self.text(Message::new("gui-import-verb", "Import")),
                command: crate::importers::COMMAND.into(),
            };
            self.form_with(form, json!({ "plugin": plugin, "importer": importer }))?;
            return Ok(Value::Null);
        };
        let target = json!({
            "importer": { "plugin": plugin, "importer": importer, "source": source },
        });
        let mut choices = args.clone();
        if let Some(o) = choices.as_object_mut() {
            o.remove("remember");
        }
        self.import_now(target, &choices, origin)
    }

    /// The import dialog's choices: where the photos go and whether to
    /// remember it, when the workspace has no import source (remembered
    /// only in a workspace); darktable edits always.
    fn import_fields(&self) -> Vec<Field> {
        let mut fields = Vec::new();
        let asks = self.focus.import_source.is_none();
        if asks {
            fields.push(boolean(
                "copy",
                self.text(Message::new(
                    "gui-import-copy",
                    "Copy the photos into the library folder",
                )),
                self.text(Message::new(
                    "gui-import-copy-help",
                    "Off: the photos stay where they are, referenced in place",
                )),
                false,
            ));
        }
        fields.push(boolean(
            "darktable",
            self.text(Message::new(
                "gui-import-darktable",
                "Read darktable edits (.xmp sidecars)",
            )),
            self.text(Message::new(
                "gui-import-darktable-help",
                "Each sidecar, duplicates included, becomes a version of its photo",
            )),
            self.focus
                .import_source
                .as_ref()
                .is_none_or(|s| s.darktable),
        ));
        if asks && self.focus.edit.is_some() {
            fields.push(boolean(
                "remember",
                self.text(Message::new(
                    "gui-import-remember",
                    "Remember for this workspace",
                )),
                self.text(Message::new(
                    "gui-import-remember-help",
                    "The workspace's import source: the next import does not ask",
                )),
                false,
            ));
        }
        fields
    }

    /// Queues an import of `target` (`path` or `paths`) with the choices of
    /// `args` (`copy`, `into`, `layout`, `rename`, `duplicates`,
    /// `darktable`), remembered first when asked. Without a library open,
    /// nothing is queued: the error says so.
    fn import_now(
        &mut self,
        mut target: Value,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        self.can_import()?;
        let copy = args.get("copy").and_then(Value::as_bool);
        let darktable = args
            .get("darktable")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                self.focus
                    .import_source
                    .as_ref()
                    .is_none_or(|s| s.darktable)
            });
        if args["remember"] == true && self.focus.import_source.is_none() {
            self.remember_import(copy.unwrap_or(false), darktable, origin)?;
        }
        if !darktable {
            target["darktable"] = json!(false);
        }
        // Run later as the user: who asked is said, for the history.
        target["origin"] = json!(crate::persist::who(origin));
        Ok(self.queue_import(target, copy, args))
    }

    /// Makes the import dialog's answer the workspace's import source:
    /// a folder, placed as answered, duplicates skipped (one undo step).
    fn remember_import(
        &mut self,
        copy: bool,
        darktable: bool,
        origin: &Origin,
    ) -> Result<(), String> {
        let state = self
            .workspace_state()
            .ok_or_else(|| String::from("no workspace open: an import source is a workspace's"))?;
        let mut library: LibraryDesc = state.bundle.template.library.unwrap_or_default();
        library.import = Some(ImportDesc {
            source: "folder".into(),
            placement: if copy { "copy" } else { "reference" }.into(),
            rename: None,
            duplicates: "skip".into(),
            suggest_stacks: false,
            darktable,
        });
        self.edit_workspace(&json!({ "library": library }), origin)?;
        Ok(())
    }

    /// Queues an import (`library.import`'s arguments) for the host,
    /// placed, renamed and with duplicates handled as `choices` say
    /// (`into`, `layout`, `rename`, `duplicates`), else as the
    /// workspace's import source says; `copy` overrides its placement (a
    /// reference is never renamed nor filed).
    pub(super) fn queue_import(
        &mut self,
        mut args: Value,
        copy: Option<bool>,
        choices: &Value,
    ) -> Value {
        let source = self.focus.import_source.as_ref();
        let given = |key: &str| {
            choices
                .get(key)
                .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                .cloned()
        };
        if copy.unwrap_or_else(|| source.is_some_and(|s| s.placement == "copy")) {
            args["copy"] = json!(true);
            for key in ["into", "layout"] {
                if let Some(value) = given(key) {
                    args[key] = value;
                }
            }
            if let Some(rename) =
                given("rename").or_else(|| source.and_then(|s| s.rename.as_ref()).map(|r| json!(r)))
            {
                args["rename"] = rename;
            }
        }
        if let Some(duplicates) =
            given("duplicates").or_else(|| source.map(|s| json!(s.duplicates)))
        {
            args["duplicates"] = duplicates;
        }
        self.core.imports.push(args.clone());
        json!({ "queued": args })
    }

    /// Takes in the lines imports sent: duplicates to their panel, the
    /// darktable edits not kept whole to the Log panel, each import's end
    /// said. How many duplicates now wait for an answer among them.
    pub(super) fn receive_imports(&mut self) -> usize {
        let mut asked = self.core.duplicates.borrow_mut().receive();
        while let Ok((import, line)) = self.core.import_lines.1.try_recv() {
            if line.get("ended").is_some() || line.get("failed").is_some() {
                // The photos it brought are selected once the Library
                // shows them.
                let ids = std::mem::take(&mut self.focus.imported_ids);
                if line.get("ended").is_some() && !ids.is_empty() {
                    self.focus.select_imported = ids;
                }
                self.import_ended(&line);
                continue;
            }
            if let Some(id) = line["id"].as_str()
                && matches!(
                    line["status"].as_str(),
                    Some("added" | "located" | "unchanged")
                )
                && !self.focus.imported_ids.iter().any(|i| i == id)
            {
                self.focus.imported_ids.push(id.to_string());
            }
            if let Some(scan) = line.get("scanned") {
                self.log_scan(scan);
                continue;
            }
            if self.core.duplicates.borrow_mut().add(&import, &line)
                == Some(crate::duplicates::Fate::Asked)
            {
                asked += 1;
            }
            self.log_edits(&line);
        }
        asked
    }

    /// The photos the last import brought, selected as soon as the
    /// Library holds them.
    pub(super) fn select_imported_photos(&mut self) {
        if self.focus.select_imported.is_empty() {
            return;
        }
        let mut library = self.focus.library.borrow_mut();
        if self
            .focus
            .select_imported
            .iter()
            .any(|id| library.holds(id))
        {
            library.select_ids(&self.focus.select_imported);
            self.focus.select_imported.clear();
        }
    }

    /// What an importer plugin found, left out and warned about: in the
    /// Log panel, under the importer's label.
    fn log_scan(&mut self, scan: &Value) {
        let (plugin, importer) = (
            scan["plugin"].as_str().unwrap_or_default(),
            scan["importer"].as_str().unwrap_or_default(),
        );
        let label = self
            .core
            .lists
            .borrow()
            .importers
            .iter()
            .find(|e| e.plugin == plugin && e.importer == importer)
            .map_or_else(
                || Message::new("", importer),
                crate::importers::Entry::message,
            );
        let mut log = self.core.log.borrow_mut();
        let found = scan["images"].as_array().map_or(0, Vec::len);
        let refused = scan["refused"].as_array().cloned().unwrap_or_default();
        log.push(
            Level::Info,
            Message::new(
                "gui-importer-scanned",
                "{ $importer }: { $found } photos found in { $source }, { $refused } left out",
            )
            .with("importer", label.clone())
            .with("found", found)
            .with("source", scan["source"].as_str().unwrap_or_default())
            .with("refused", refused.len()),
        );
        for r in &refused {
            log.push(
                Level::Warning,
                Message::new(
                    "gui-importer-refused",
                    "{ $importer }: { $path } left out: { $why }",
                )
                .with("importer", label.clone())
                .with("path", r["path"].as_str().unwrap_or_default())
                .with("why", r["why"].as_str().unwrap_or_default()),
            );
        }
        for w in scan["warnings"].as_array().into_iter().flatten() {
            log.push(
                Level::Warning,
                Message::new("gui-importer-warning", "{ $importer }: { $warning }")
                    .with("importer", label.clone())
                    .with("warning", w.as_str().unwrap_or_default()),
            );
        }
    }

    /// One file's darktable sidecars kept with reservations, or not kept:
    /// in the Log panel.
    fn log_edits(&mut self, line: &Value) {
        let mut log = self.core.log.borrow_mut();
        for edit in line["darktable"].as_array().into_iter().flatten() {
            let sidecar = edit["path"].as_str().unwrap_or_default().to_string();
            if let Some(e) = edit["error"].as_str() {
                log.push(
                    Level::Error,
                    Message::new(
                        "gui-import-darktable-failed",
                        "{ $sidecar }: not kept: { $error }",
                    )
                    .with("sidecar", sidecar)
                    .with("error", e),
                );
            } else if edit["warning"].is_string() {
                let report = serde_json::from_value::<Message>(edit["warning_message"].clone())
                    .unwrap_or_else(|_| {
                        Message::new("", edit["warning"].as_str().unwrap_or_default())
                    });
                log.push(
                    Level::Warning,
                    Message::new("gui-import-darktable-report", "{ $sidecar }: { $report }")
                        .with("sidecar", sidecar)
                        .with("report", report),
                );
            }
        }
    }

    /// An import ended: a short toast, and its outcome in the Log panel.
    fn import_ended(&mut self, line: &Value) {
        if let Some(e) = line["failed"].as_str() {
            let message =
                Message::new("gui-import-failed", "The import failed: { $error }").with("error", e);
            self.core
                .log
                .borrow_mut()
                .push(Level::Error, message.clone());
            self.notify_unlogged(Level::Error, message);
            return;
        }
        let report = &line["ended"];
        // Its cards short of their backups wait for a backup job.
        self.wait_for_backups(report);
        let count = |k: &str| report[k].as_u64().unwrap_or(0);
        let photos = count("added") + count("located");
        if report["cancelled"] == true {
            let message = Message::new(
                "gui-import-cancelled",
                "The import was cancelled: { $photos } photos imported",
            )
            .with("photos", photos);
            self.core
                .log
                .borrow_mut()
                .push(Level::Info, message.clone());
            self.notify_unlogged(Level::Info, message);
            return;
        }
        if count("metadata") > 0 {
            self.core.log.borrow_mut().push(
                Level::Info,
                Message::new(
                    "gui-import-metadata",
                    "{ $photos } photos took their ratings, labels or tags from the importer",
                )
                .with("photos", count("metadata")),
            );
        }
        let (edits, reported) = (
            count("edits"),
            count("edits_reported") + count("edits_failed"),
        );
        let message = if reported > 0 {
            Message::new(
                "gui-import-ended-reported",
                "{ $photos } photos imported, { $edits } darktable edits; { $reported } not kept whole: see the Log panel",
            )
            .with("photos", photos)
            .with("edits", edits)
            .with("reported", reported)
        } else {
            Message::new(
                "gui-import-ended",
                "{ $photos } photos imported, { $edits } darktable edits",
            )
            .with("photos", photos)
            .with("edits", edits)
        };
        let level = if reported > 0 {
            Level::Warning
        } else {
            Level::Info
        };
        self.core.log.borrow_mut().push(level, message.clone());
        if level == Level::Warning {
            self.notify_unlogged(level, message);
            return;
        }
        // Information alone has no toast: an import's end has a short one.
        let text = self.text(message.clone());
        if let Some(toasts) = self.core.app.handle().service(&TOASTS) {
            toasts.notify(Toast::success(&text));
        }
        self.notes.push(text.clone());
        self.core.notices.push(Notice {
            level: Level::Info,
            text,
            key: message.key,
            action: None,
            at: self.core.now,
            report: None,
        });
    }

    /// Files and folders dropped on the window open the import window
    /// with them as its sources; while they are dragged over it, it says
    /// so.
    pub(super) fn take_dropped_files(&mut self, ctx: &Egui) {
        // The import window open takes them itself, over its contents,
        // while it is drawn in the main window (embedded). In its own OS
        // window, what is dropped on the main window becomes its sources.
        let embedded = self
            .import_window_viewport()
            .is_none_or(|v| v == egui::ViewportId::ROOT);
        if self.import_window_open() && embedded {
            return;
        }
        let (hovered, dropped): (bool, Vec<PathBuf>) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect(),
            )
        });
        if hovered {
            let text = self.text(Message::new(
                "gui-import-drop",
                "Drop to import into the library",
            ));
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("helios.import.drop"),
            ));
            let rect = ctx.content_rect();
            let visuals = ctx.global_style().visuals.clone();
            painter.rect_filled(rect, 0.0, visuals.extreme_bg_color.gamma_multiply(0.85));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(20.0),
                visuals.strong_text_color(),
            );
        }
        if dropped.is_empty() {
            return;
        }
        if let Err(e) = self.run(
            super::import_window::COMMAND,
            json!({ "paths": dropped }),
            Origin::User,
        ) {
            self.warn(e);
        }
    }
}

/// Why nothing is imported without a library open: the host runs imports
/// as `library.import`, which it has only with one (`--vault FILE`).
pub(crate) fn no_library() -> Message {
    Message::new(
        "gui-import-no-library",
        "No library is open: start Helios with one (--vault FILE) to import photos",
    )
}
