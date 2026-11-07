//! Exporting photos from the desktop (see `docs/export.md`):
//!
//! - **Where from.** File > Export… (Ctrl+Shift+E, as Lightroom), the
//!   Library's context menu and the palette run `library.export`: the
//!   selected images, else every image the library shows (its filter,
//!   the workspace's query). Without a folder the export dialog asks for
//!   the settings, starting from the workspace's (`library.export` in its
//!   template), and "Remember for this workspace" makes the answer the
//!   workspace's (one undo step).
//! - **What runs.** The shell queues an [`ExportAsk`] (the images and the
//!   settings); the host plans it over the library into a serialisable
//!   batch request and runs it on its scheduler
//!   ([`crate::export::Exporter`]): a parent job and an export per image,
//!   in the jobs centre and the Queue panel of the node exported.
//! - **What it did.** Each image that failed goes to the Log panel with
//!   the reason; the end is a toast with the counts and "Open folder"
//!   (`library.openFolder`).

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use egui_pop::api::toasts::{TOASTS, Toast};
use helios_core::Message;
use helios_export::batch::{Clash, Line, Settings, Summary};
use helios_export::{Format, Output, OutputSpace};
use helios_shell::commands::Origin;
use helios_template::{ExportDesc, LibraryDesc};
use helios_values::Descriptor;
use serde_json::{Value, json};

use super::{Shell, lock};
use crate::dialogs::{Field, Form, Level, Notice};

/// The command that shows a folder in the file manager (`path`).
pub const OPEN_FOLDER: &str = "library.openFolder";

/// An export the shell asked for, for the host to plan and run.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportAsk {
    /// The images, by library id, in the library's order.
    pub images: Vec<String>,
    pub settings: Settings,
    /// "Open folder", in the interface's language: what the finished
    /// batch offers in the jobs centre.
    pub open_label: String,
}

/// A folder as the settings write it: `~` the home folder.
pub fn folder_path(folder: &str) -> Option<PathBuf> {
    let folder = folder.trim();
    if folder.is_empty() {
        return None;
    }
    let home = || std::env::home_dir().filter(|h| h.is_absolute());
    if folder == "~" {
        return home();
    }
    if let Some(rest) = folder.strip_prefix("~/") {
        return home().map(|h| h.join(rest));
    }
    Some(PathBuf::from(folder))
}

/// The settings a batch runs with, from the words of the workspace or
/// the dialog: the folder made absolute (`~`), each word checked.
pub fn settings_of(desc: &ExportDesc) -> Result<Settings, String> {
    let format = match desc.format.as_str() {
        "jpeg" => Format::Jpeg {
            quality: desc.quality.unwrap_or(92).clamp(1, 100),
        },
        "png" => match desc.bits.unwrap_or(8) {
            b @ (8 | 16) => Format::Png { bits: b },
            b => return Err(format!("a PNG has 8 or 16 bits, not {b}")),
        },
        "tiff" => Format::Tiff,
        other => return Err(format!("`{other}` is not a format (jpeg, png, tiff)")),
    };
    let space = OutputSpace::ALL
        .into_iter()
        .find(|s| {
            serde_json::to_value(s)
                .ok()
                .as_ref()
                .and_then(Value::as_str)
                == Some(&desc.space)
        })
        .ok_or_else(|| format!("`{}` is not a colour space", desc.space))?;
    let clash = Clash::from_word(&desc.clash)
        .ok_or_else(|| format!("`{}` is not what to do with a name taken", desc.clash))?;
    let folder = desc
        .folder
        .as_deref()
        .and_then(folder_path)
        .ok_or("no folder to export to")?;
    if !folder.is_absolute() {
        return Err(format!("the folder {} is not absolute", folder.display()));
    }
    if desc.pattern.trim().is_empty() {
        return Err("no file name pattern".into());
    }
    Ok(Settings {
        output: Output {
            format,
            space,
            long_edge: desc.long_edge.filter(|e| *e > 0),
        },
        folder,
        pattern: desc.pattern.trim().into(),
        clash,
    })
}

/// A number given as a number or as text (a form's choice).
fn number(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// The settings asked for: the arguments over the workspace's.
fn desc_of(args: &Value, base: &ExportDesc) -> ExportDesc {
    let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    let mut d = base.clone();
    if let Some(f) = text("format") {
        d.format = f;
    }
    if let Some(q) = args.get("quality").and_then(number) {
        d.quality = Some(q.min(100) as u8);
    }
    if let Some(b) = args.get("bits").and_then(number) {
        d.bits = Some(b.min(255) as u8);
    }
    if let Some(e) = args.get("long_edge").and_then(number) {
        d.long_edge = (e > 0).then_some(e as u32);
    }
    if let Some(s) = text("space") {
        d.space = s;
    }
    if let Some(f) = text("folder") {
        d.folder = Some(f).filter(|f| !f.trim().is_empty());
    }
    if let Some(p) = text("pattern") {
        d.pattern = p;
    }
    if let Some(c) = text("clash") {
        d.clash = c;
    }
    // What a format does not take is not kept.
    if d.format != "jpeg" {
        d.quality = None;
    }
    if d.format != "png" {
        d.bits = None;
    }
    d
}

fn choice(key: &str, label: String, choices: &[&str], value: &str) -> Field {
    Field {
        key: key.into(),
        descriptor: Descriptor::choice(&label, choices, value),
        value: helios_core::Value::Choice(value.into()),
    }
}

impl Shell {
    /// Where the host sends what the exports it runs say, from any thread.
    pub fn export_sink(&self) -> Sender<Line> {
        self.export_lines.0.clone()
    }

    /// The exports asked for since the last call, for the host to run.
    pub fn take_exports(&mut self) -> Vec<ExportAsk> {
        std::mem::take(&mut self.exports)
    }

    /// The workspace's export settings, or the defaults.
    pub fn export_settings(&self) -> ExportDesc {
        self.workspace_state()
            .and_then(|s| s.bundle.template.library)
            .and_then(|l| l.export)
            .unwrap_or_default()
    }

    /// The images an export takes: `images` given, else as `scope` says
    /// (`selected`, `shown`), else the selection, else what the library
    /// shows.
    fn export_images(&self, args: &Value) -> Vec<String> {
        if let Some(ids) = args.get("images").and_then(Value::as_array) {
            return ids
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect();
        }
        let library = self.library.borrow();
        let selected = library.targets().unwrap_or_default();
        match args.get("scope").and_then(Value::as_str) {
            Some("shown") => library.shown(),
            Some("selected") => selected,
            _ if selected.is_empty() => library.shown(),
            _ => selected,
        }
    }

    /// `library.export`: the images (`images`, or `scope`) exported with
    /// the settings given over the workspace's; without a folder (or with
    /// `ask`), the export dialog asks for them.
    pub(super) fn export(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        let base = self.export_settings();
        let given = args
            .get("folder")
            .and_then(Value::as_str)
            .is_some_and(|f| !f.trim().is_empty());
        if !given || args["ask"] == true {
            let images = self.export_images(args);
            if images.is_empty() {
                return Err("no image to export: the library shows none".into());
            }
            let desc = desc_of(args, &base);
            self.export_form(&desc, args)?;
            return Ok(Value::Null);
        }
        let images = self.export_images(args);
        if images.is_empty() {
            return Err("no image to export: the library shows none".into());
        }
        let desc = desc_of(args, &base);
        let settings = settings_of(&desc)?;
        if args["remember"] == true {
            self.remember_export(&desc, origin)?;
        }
        let count = images.len();
        let folder = settings.folder.clone();
        // The Queue panel follows the node the exports render.
        let node = self
            .workspace_bundle()
            .unwrap_or_else(helios_template::workspace::blank)
            .template
            .network
            .view
            .node;
        self.select_exported_node(&node);
        self.exports.push(ExportAsk {
            images,
            settings,
            open_label: self.text(Message::new("gui-export-open-folder", "Open folder")),
        });
        self.note(
            Message::new(
                "gui-export-queued",
                "Exporting { $count } photos to { $folder }",
            )
            .with("count", count)
            .with("folder", folder.display().to_string()),
        );
        Ok(json!({ "queued": count, "folder": folder }))
    }

    /// The export dialog, from `desc`: the images (the selection or all
    /// shown), the format and its quality, the size, the colour space,
    /// the folder, the names and what a name taken does.
    fn export_form(&mut self, desc: &ExportDesc, args: &Value) -> Result<(), String> {
        let selected = self.library.borrow().targets().unwrap_or_default().len();
        let shown = self.library.borrow().shown().len();
        let mut fields = Vec::new();
        if args.get("images").is_none() {
            let scope = match args.get("scope").and_then(Value::as_str) {
                Some(s @ ("selected" | "shown")) => s,
                _ if selected == 0 => "shown",
                _ => "selected",
            };
            let mut field = choice(
                "scope",
                self.text(Message::new("gui-export-scope", "photos")),
                &["selected", "shown"],
                scope,
            );
            field.descriptor.description = self.text(
                Message::new(
                    "gui-export-scope-help",
                    "selected: the { $selected } selected; shown: the { $shown } the library shows",
                )
                .with("selected", selected)
                .with("shown", shown),
            );
            fields.push(field);
        }
        fields.push(choice(
            "format",
            self.text(Message::new("gui-export-format", "format")),
            &helios_template::format::EXPORT_FORMATS,
            &desc.format,
        ));
        let quality = i64::from(desc.quality.unwrap_or(92));
        let mut q = Field {
            key: "quality".into(),
            descriptor: Descriptor::int(
                &self.text(Message::new("gui-export-quality", "JPEG quality")),
                1,
                100,
                quality,
            ),
            value: helios_core::Value::Int(quality),
        };
        q.descriptor.description = self.text(Message::new(
            "gui-export-quality-help",
            "1 to 100, for JPEG only",
        ));
        fields.push(q);
        let bits = desc.bits.unwrap_or(8).to_string();
        fields.push(choice(
            "bits",
            self.text(Message::new("gui-export-bits", "PNG bits per channel")),
            &["8", "16"],
            &bits,
        ));
        let edge = i64::from(desc.long_edge.unwrap_or(0));
        let mut e = Field {
            key: "long_edge".into(),
            descriptor: Descriptor::int(
                &self.text(Message::new("gui-export-long-edge", "long edge (pixels)")),
                0,
                65535,
                edge,
            ),
            value: helios_core::Value::Int(edge),
        };
        e.descriptor.description = self.text(Message::new(
            "gui-export-long-edge-help",
            "The longer side at most; 0 keeps the full size",
        ));
        fields.push(e);
        fields.push(choice(
            "space",
            self.text(Message::new("gui-export-space", "colour space")),
            &helios_template::format::EXPORT_SPACES,
            &desc.space,
        ));
        let mut folder = Field {
            key: "folder".into(),
            descriptor: Descriptor::path(
                &self.text(Message::new("gui-export-folder", "folder")),
                helios_values::PathKind::Dir,
                &[],
            ),
            value: helios_core::Value::Text(desc.folder.clone().unwrap_or_default()),
        };
        folder.descriptor.default = folder.value.clone();
        fields.push(folder);
        let mut pattern = Field {
            key: "pattern".into(),
            descriptor: Descriptor::text(
                &self.text(Message::new("gui-export-pattern", "file names")),
                &desc.pattern,
            ),
            value: helios_core::Value::Text(desc.pattern.clone()),
        };
        pattern.descriptor.description = self.text(
            Message::new(
                "gui-export-pattern-help",
                "As an import renames: { $fields }; the extension follows the format",
            )
            .with("fields", "{stem}, {date}, {time}, {camera}, {seq}…"),
        );
        fields.push(pattern);
        fields.push(choice(
            "clash",
            self.text(Message::new(
                "gui-export-clash",
                "when a file name is taken",
            )),
            &helios_template::format::CLASHES,
            &desc.clash,
        ));
        if self.edit.is_some() {
            let mut remember = Field {
                key: "remember".into(),
                descriptor: Descriptor::bool(
                    &self.text(Message::new(
                        "gui-export-remember",
                        "Remember for this workspace",
                    )),
                    false,
                ),
                value: helios_core::Value::Bool(false),
            };
            remember.descriptor.description = self.text(Message::new(
                "gui-export-remember-help",
                "The workspace's export settings: the next export starts from them",
            ));
            fields.push(remember);
        }
        let form = Form {
            title: self.text(Message::new("gui-export-form-title", "Export photos")),
            fields,
            verb: self.text(Message::new("gui-export-verb", "Export")),
            command: "library.export".into(),
        };
        let mut kept = json!({});
        if let Some(images) = args.get("images") {
            kept["images"] = images.clone();
        }
        self.form_with(form, kept)
    }

    /// Makes `desc` the workspace's export settings (one undo step).
    fn remember_export(&mut self, desc: &ExportDesc, origin: &Origin) -> Result<(), String> {
        let state = self
            .workspace_state()
            .ok_or_else(|| String::from("no workspace open: export settings are a workspace's"))?;
        let mut library: LibraryDesc = state.bundle.template.library.unwrap_or_default();
        library.export = Some(desc.clone());
        self.edit_workspace(&json!({ "library": library }), origin)?;
        Ok(())
    }

    /// `library.openFolder`: a folder (`path`) shown in the file manager.
    pub(super) fn open_folder(args: &Value) -> Result<Value, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .ok_or("`path` is missing")?;
        egui_pop::platform::dirs::reveal(std::path::Path::new(path))?;
        Ok(json!({ "opened": path }))
    }

    /// Takes in what the exports said: each failure to the Log panel,
    /// each end to a toast offering the folder.
    pub(super) fn receive_exports(&mut self) {
        while let Ok(line) = self.export_lines.1.try_recv() {
            match line {
                Line::Failed { item, error, .. } => self.log.borrow_mut().push(
                    Level::Error,
                    Message::new(
                        "gui-export-image-failed",
                        "{ $image }: not exported: { $error }",
                    )
                    .with("image", item)
                    .with("error", error),
                ),
                Line::Ended(summary) => self.export_ended(&summary),
            }
        }
    }

    /// An export ended: its counts in the Log panel and a toast offering
    /// to open the folder.
    fn export_ended(&mut self, s: &Summary) {
        let written = s.written.len();
        let folder = s.folder.display().to_string();
        let (message, level) = if s.interrupted {
            (
                Message::new(
                    "gui-export-cancelled",
                    "The export was cancelled: { $written } photos written to { $folder }",
                )
                .with("written", written)
                .with("folder", folder.as_str()),
                Level::Info,
            )
        } else if s.failed > 0 {
            (
                Message::new(
                    "gui-export-ended-failed",
                    "{ $written } photos exported to { $folder }; { $failed } failed: see the Log panel",
                )
                .with("written", written)
                .with("folder", folder.as_str())
                .with("failed", s.failed),
                Level::Warning,
            )
        } else {
            (
                Message::new(
                    "gui-export-ended",
                    "{ $written } photos exported to { $folder }",
                )
                .with("written", written)
                .with("folder", folder.as_str()),
                Level::Info,
            )
        };
        let message = if s.skipped > 0 {
            Message::new(
                "gui-export-ended-skipped",
                "{ $outcome } ({ $skipped } left out: their file exists)",
            )
            .with("outcome", message)
            .with("skipped", s.skipped)
        } else {
            message
        };
        self.log.borrow_mut().push(level, message.clone());
        let text = self.text(message.clone());
        let open = self.text(Message::new("gui-export-open-folder", "Open folder"));
        if let Some(toasts) = self.app.handle().service(&TOASTS) {
            let toast = match level {
                Level::Warning => Toast::warning(&text),
                Level::Error => Toast::error(&text),
                Level::Info => Toast::success(&text),
            };
            toasts.notify(toast.action(
                &open,
                egui_pop::model::ids::CommandId::from_static(OPEN_FOLDER),
                json!({ "path": s.folder }),
            ));
        }
        self.notes.push(text.clone());
        self.notices.push(Notice {
            level,
            text,
            key: message.key,
            action: Some((open, OPEN_FOLDER.to_string())),
            at: self.now,
        });
    }

    /// The Queue panel follows the node an export renders: selected when
    /// nothing is.
    fn select_exported_node(&mut self, node: &str) {
        use helios_values::{Change, Set};
        let mut values = lock(&self.values);
        let empty = values
            .get(crate::params::SELECTION)
            .is_none_or(|v| matches!(&v.value, helios_core::Value::Text(t) if t.is_empty()));
        if empty {
            let _ = values.set(Set::new(
                crate::params::SELECTION,
                Change::To(helios_core::Value::Text(node.into())),
                Origin::User,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_become_settings_and_bad_ones_are_refused() {
        let desc = ExportDesc {
            format: "png".into(),
            bits: Some(16),
            long_edge: Some(2048),
            space: "adobe-rgb".into(),
            folder: Some("/tmp/out".into()),
            pattern: "{date}-{seq}".into(),
            clash: "skip".into(),
            ..ExportDesc::default()
        };
        let s = settings_of(&desc).unwrap();
        assert_eq!(s.output.format, Format::Png { bits: 16 });
        assert_eq!(s.output.space, OutputSpace::AdobeRgb);
        assert_eq!(s.output.long_edge, Some(2048));
        assert_eq!(s.folder, PathBuf::from("/tmp/out"));
        assert_eq!(s.clash, Clash::Skip);
        for bad in [
            ExportDesc {
                format: "webp".into(),
                ..desc.clone()
            },
            ExportDesc {
                space: "cmyk".into(),
                ..desc.clone()
            },
            ExportDesc {
                folder: None,
                ..desc.clone()
            },
            ExportDesc {
                folder: Some("relative".into()),
                ..desc.clone()
            },
        ] {
            assert!(settings_of(&bad).is_err(), "{bad:?}");
        }
        // A form's answer over the workspace's: what a format does not
        // take goes.
        let asked = desc_of(
            &json!({ "format": "jpeg", "quality": 80, "bits": "8", "long_edge": 0 }),
            &desc,
        );
        assert_eq!(
            (asked.quality, asked.bits, asked.long_edge),
            (Some(80), None, None)
        );
    }
}
