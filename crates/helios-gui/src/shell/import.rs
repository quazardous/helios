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
use egui_pop::api::bus::FilesDrop;
use egui_pop::api::bus::bus_contract::{HookError, ListenOpts};
use egui_pop::api::toasts::Toast;
use egui_pop::model::ids::CommandId;
use helios_core::Message;
use helios_shell::commands::Origin;
use helios_template::{ImportDesc, LibraryDesc};
use helios_values::Descriptor;
use serde_json::{Value, json};

use super::Shell;
use crate::dialogs::{Field, Form, Level, Notice};
use crate::import_end::{self, CameraLooks, ImportEnd, Publisher, Reactions};

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

    /// Follows the promises of the desktop's node (`node`): the status
    /// bar shows those running, and the person's imports go to it as
    /// imports by rule, answered at once with their promise
    /// ([`crate::activity`]).
    pub fn follow_activity(&mut self, node: std::sync::Arc<helios_inspect::client::Client>) {
        self.core.activity.follow(node);
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

    /// An automatic import of `args`'s files (`paths`) from the saved
    /// source named `source`: as `library.importFiles` without a question,
    /// recorded in the import history as `auto:<source>`.
    pub(super) fn import_files_automatically(
        &mut self,
        args: &Value,
        source: &str,
    ) -> Result<Value, String> {
        self.can_import()?;
        let paths = paths_of(&json!({ "paths": args["paths"] }));
        if paths.is_empty() {
            return Err("`paths` is missing".into());
        }
        let who = helios_library::auto_import::origin(source);
        self.import_now_as(json!({ "paths": paths }), args, &Origin::User, who)
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
    /// `darktable`, `camera_look`), remembered first when asked. Without a library open,
    /// nothing is queued: the error says so.
    fn import_now(
        &mut self,
        target: Value,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        let who = crate::persist::who(origin);
        self.import_now_as(target, args, origin, who)
    }

    /// [`Self::import_now`], recorded in the import history as asked by
    /// `who`.
    fn import_now_as(
        &mut self,
        mut target: Value,
        args: &Value,
        origin: &Origin,
        who: String,
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
        // The cameras' default looks are applied unless turned off.
        if args["camera_look"] == false {
            target["camera_look"] = json!(false);
        }
        // Run later as the user: who asked is said, for the history.
        target["origin"] = json!(who);
        self.queue_import(target, copy, args)
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

    /// Starts an import (`library.import`'s arguments), placed, renamed
    /// and with duplicates handled as `choices` say (`into`, `layout`,
    /// `rename`, `duplicates`), else as the workspace's import source
    /// says; `copy` overrides its placement (a reference is never renamed
    /// nor filed). The person's own imports go to the node, which answers
    /// at once with a promise (`{"promise": …}`, [`Self::import_by_rule`]);
    /// the others (an agent's or a script's, an automatic import), and
    /// every import of a shell with no node, are queued for the host's
    /// `library.import` (`{"queued": args}`).
    ///
    /// # Errors
    /// The node refused the import.
    pub(super) fn queue_import(
        &mut self,
        mut args: Value,
        copy: Option<bool>,
        choices: &Value,
    ) -> Result<Value, String> {
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
        // A saved source's filter and what its photos are given.
        for key in ["filter", "apply"] {
            if let Some(value) = choices.get(key).filter(|v| !v.is_null()) {
                args[key] = value.clone();
            }
        }
        if self.core.activity.has_node() && args["origin"] == crate::persist::WHO {
            return self.import_by_rule(args);
        }
        self.core.imports.push(args.clone());
        Ok(json!({ "queued": args }))
    }

    /// Hands an import (`library.import`'s arguments: `paths`, the
    /// choices) to the node as an import by rule, its `items` the paths:
    /// the node answers at once with its promise (`{"promise": …}`), which
    /// the status bar and the import window follow while it runs.
    ///
    /// # Errors
    /// The node refused it: its choices, a path gone, no library folder.
    pub(super) fn import_by_rule(&mut self, args: Value) -> Result<Value, String> {
        let paths = paths_of(&args);
        if paths.is_empty() {
            return Err("`paths` is missing".into());
        }
        let mut params = args;
        if let Some(fields) = params.as_object_mut() {
            // Who asked is the node's caller; the paths are the rule's.
            for key in ["paths", "path", "origin"] {
                fields.remove(key);
            }
            // The desktop keeps darktable's edits unless told not to;
            // the node leaves them out unless told to.
            let darktable = fields.get("darktable") != Some(&json!(false));
            fields.insert("darktable".into(), json!(darktable));
        }
        params["rule"] = json!({ "items": paths });
        let promise = self.core.activity.start(params)?;
        // Its end is shown in the Library, as the user's.
        self.user_import_started(promise.request.clone());
        Ok(json!({ "promise": promise }))
    }

    /// Takes in the lines imports sent (duplicates to their panel, the
    /// darktable edits not kept whole to the Log panel) and their ends,
    /// each passed through its point ([`Shell::import_end_heard`]).
    /// How many duplicates now wait for an answer among them.
    pub(super) fn receive_imports(&mut self) -> usize {
        // The ends first: each was said after its import's lines, which
        // are thus all there to take below, before it is heard.
        let ends: Vec<ImportEnd> = self.core.import_ends.1.try_iter().collect();
        let mut asked = self.core.duplicates.borrow_mut().receive();
        while let Ok((import, line)) = self.core.import_lines.1.try_recv() {
            // An end said as a line (a host running an import by hand):
            // heard as every end is.
            if let Some(end) = ImportEnd::of_line(&import, &line) {
                asked += self.import_end_heard(end);
                continue;
            }
            if let Some(id) = line["id"].as_str()
                && matches!(
                    line["status"].as_str(),
                    Some("added" | "located" | "unchanged")
                )
            {
                let ids = &mut self.focus.imports.borrow_mut().imported_ids;
                if !ids.iter().any(|i| i == id) {
                    ids.push(id.to_string());
                }
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
        for end in ends {
            asked += self.import_end_heard(end);
        }
        asked
    }

    /// Where every import's end is said, from any thread
    /// ([`crate::import_end::Publisher`]): the cameras' looks and the
    /// window woken as [`Shell::set_import_ends`] set them.
    pub fn import_ends(&self) -> Publisher {
        self.core.import_ends.0.clone()
    }

    /// The cameras' default looks put on the raws of every import that
    /// ends (`looks`), and `ctx` woken once one ends.
    pub fn set_import_ends(&mut self, looks: Option<CameraLooks>, ctx: Egui) {
        let mut ends = self.core.import_ends.0.clone().waking(ctx);
        if let Some(looks) = looks {
            ends = ends.camera_looks(looks);
        }
        self.core.import_ends.0 = ends;
    }

    /// Registers the host's listener `id` of [`ImportEnded`](crate::import_end::ImportEnded) (one of
    /// [`crate::import_end::ORDER`]'s, such as `backups.on-import` or
    /// `library.refill`), run on the interface's thread in its declared
    /// place.
    ///
    /// # Errors
    /// `id` is not a declared listener, or the bus refused it.
    pub fn listen_import_ended_by(
        &self,
        id: &'static str,
        run: impl Fn(&ImportEnd) + Send + Sync + 'static,
    ) -> Result<(), String> {
        import_end::listen(&*self.core.app.handle().bus(), id, move |end, _| run(end))
    }

    /// The order the import points' listeners run in (`import.ended`,
    /// then `import.failed`).
    pub fn import_end_order(&self) -> String {
        import_end::order(&*self.core.app.handle().bus())
    }

    /// The shell's listeners of the import's points, each doing its own
    /// part through the handles it holds: the undo step recorded (a shared
    /// listener), the others local to the interface's thread, where the
    /// shell passes the end ([`Shell::import_end_heard`]), so they run
    /// inside the pass, in the declared order.
    pub(super) fn listen_import_ended(&mut self) {
        let bus = self.core.app.handle().bus();
        let history = self.library_history();
        let teller = self.teller();
        let mut listened = vec![
            import_end::listen(&*bus, import_end::HISTORY, move |end, r| {
                if let (Some(changes), Ok(report)) = (r.changes.take(), &end.outcome) {
                    let label = crate::library_history::import_label(report.added + report.located);
                    let at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0.0, |d| d.as_secs_f64());
                    history.record(label, changes, at);
                }
            }),
            self.listen_cards_waiting(),
        ];
        let duplicates = self.core.duplicates.clone();
        listened.push(import_end::listen_local(
            &*bus,
            import_end::DUPLICATES,
            move |_, r| r.asked += duplicates.borrow_mut().receive(),
        ));
        let log = self.core.log.clone();
        listened.push(import_end::listen_local(
            &*bus,
            import_end::LOG,
            move |end, _| {
                let (before, level, message, _) = import_end_said(&end.line());
                let mut log = log.borrow_mut();
                for (level, message) in before {
                    log.push(level, message);
                }
                log.push(level, message);
            },
        ));
        let toast = teller.clone();
        listened.push(import_end::listen_local(
            &*bus,
            import_end::TOAST,
            move |end, _| toast_import_end(&toast, &end.line()),
        ));
        listened.push(self.listen_import_session());
        let window = self.import.state.clone();
        listened.push(import_end::listen_local(
            &*bus,
            import_end::WINDOW_HISTORY,
            // An import ended, the import window's History lists it at
            // once, whoever ran it: read again at the window's next
            // frame, asked now (an open window otherwise draws again
            // only when something moves in it).
            move |_, _| {
                let mut w = window.borrow_mut();
                w.history.read_again();
                w.waker()();
            },
        ));
        let imports = self.focus.imports.clone();
        listened.push(import_end::listen_local(
            &*bus,
            import_end::SELECT,
            // The user's import: its photos selected once the Library
            // shows them, at `library.filled` (unless the user turned it
            // off).
            move |_, r| {
                let mut imports = imports.borrow_mut();
                let ids = std::mem::take(&mut imports.imported_ids);
                if r.select && !ids.is_empty() {
                    imports.select_imported = ids;
                }
            },
        ));
        for e in listened.into_iter().filter_map(Result::err) {
            self.notes.push(format!("{}: {e}", import_end::ENDED));
        }
    }

    /// What the shell's local listeners of the bus need of it: its words,
    /// toasts and the record of what they said ([`super::said`]).
    pub(super) fn teller(&self) -> super::said::Teller {
        super::said::Teller {
            i18n: self.core.i18n.clone(),
            app: self.core.app.handle().clone(),
            said: self.core.said.clone(),
        }
    }

    /// What the shell's local listeners said, taken into its notes and
    /// notices.
    pub(super) fn absorb_said(&mut self) {
        let said = std::mem::take(&mut *self.core.said.borrow_mut());
        self.notes.extend(said.notes);
        for mut notice in said.notices {
            notice.at = self.core.now;
            self.core.notices.push(notice);
        }
    }

    /// An import's end, passed through its point (`import.ended`, or
    /// `import.failed`), each listener doing its part inside the pass,
    /// then the point's name said on the journal with who asked, the
    /// report, the cameras' looks and the listeners that ran (`ran`).
    /// How many duplicates now wait for an answer.
    pub(super) fn import_end_heard(&mut self, mut end: ImportEnd) -> usize {
        let changes = end
            .outcome
            .as_mut()
            .ok()
            .map(|r| std::mem::take(&mut r.changes));
        let (said, mut data) = match &end.outcome {
            Ok(report) => (
                import_end::ENDED,
                json!({
                    "origin": end.origin,
                    "report": report,
                    "camera_looks": end.camera_looks,
                }),
            ),
            Err(e) => (
                import_end::FAILED,
                json!({ "origin": end.origin, "error": e }),
            ),
        };
        let handle = self.core.app.handle().clone();
        let point = end.point();
        let altered = import_end::pass(
            &*handle.bus(),
            end,
            Reactions {
                changes,
                ..Reactions::default()
            },
        );
        for failure in &altered.failures {
            log::warn!(target: "helios", "{point}: {failure}");
        }
        // The photos the import brought are spent with its end, selected
        // or not.
        self.focus.imports.borrow_mut().imported_ids.clear();
        self.absorb_said();
        // Which listeners ran, in their order: for agents and tests.
        data["ran"] = json!(altered.value.ran);
        handle.publish("helios", None, said, data);
        altered.value.asked
    }

    /// The shell's listener of `library.filled`
    /// ([`crate::library_changed::SELECT`]): the photos the last import
    /// brought selected once the Library holds them (else at a later
    /// fill).
    pub(super) fn listen_library_filled(&mut self) {
        let bus = self.core.app.handle().bus();
        let (imports, library) = (self.focus.imports.clone(), self.focus.library.clone());
        let listened = crate::library_changed::listen_filled_local(
            &*bus,
            crate::library_changed::SELECT,
            move |_| {
                let mut imports = imports.borrow_mut();
                if imports.select_imported.is_empty() {
                    return;
                }
                let mut library = library.borrow_mut();
                if imports.select_imported.iter().any(|id| library.holds(id)) {
                    library.select_ids(&imports.select_imported);
                    imports.select_imported.clear();
                }
            },
        );
        if let Err(e) = listened {
            self.notes
                .push(format!("{}: {e}", crate::library_changed::FILLED));
        }
    }

    /// The Library read again is shown (the host's fill landed): passed
    /// through `library.filled`, each listener doing its part inside the
    /// pass (the photos the last import brought selected), then
    /// `library.filled` said on the journal with the listeners that ran.
    pub fn library_filled(&mut self) {
        use crate::library_changed as lc;
        let handle = self.core.app.handle().clone();
        let photos = self.focus.library.borrow().entries.len();
        let ran = lc::pass_filled(&*handle.bus(), lc::Filled { photos });
        handle.publish(
            "helios",
            None,
            lc::FILLED,
            json!({ "photos": photos, "ran": ran }),
        );
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

    /// Files and folders dropped on any of Helios's windows (the main
    /// one, the import window, a focus window, a detached panel's window)
    /// open the import window with them as its sources: the listener
    /// [`DROP_LISTENER`] of egui-pop's `files.drop`, after the file
    /// browser's. Dropped over a browser's target (the import window's
    /// photos), the browser took them: nothing more. Nothing in Helios
    /// reads egui's dropped or hovering files itself; while files hover
    /// a window, egui-pop shows whether anything takes them.
    pub(super) fn listen_files_dropped(&mut self) {
        let bus = self.core.app.handle().bus();
        let listened = bus.listen::<FilesDrop>(
            DROP_LISTENER,
            ListenOpts::new()
                .owner(import_end::BUS_OWNER)
                .after(egui_pop::api::bus::FILES_BROWSER),
            |drop, did| {
                if !did.is_empty() {
                    return Ok(());
                }
                let Some(app) = egui_pop::api::bus::asking() else {
                    return Err(HookError("no application asking".into()));
                };
                let command = CommandId::new(super::import_window::COMMAND)
                    .map_err(|e| HookError(e.to_string()))?;
                app.run(command, json!({ "paths": drop.paths }));
                did.push(format!("{DROP_LISTENER}: {} files", drop.paths.len()));
                Ok(())
            },
        );
        if let Err(e) = listened {
            self.notes.push(format!("files.drop: {e}"));
        }
    }
}

/// What the imports leave the Library, shared by the shell and its
/// listeners of the imports' ends.
#[derive(Default)]
pub(super) struct Imports {
    /// The photos the imports running brought so far.
    pub(super) imported_ids: Vec<String>,
    /// Those the last one brought, to select once the Library shows them.
    pub(super) select_imported: Vec<String>,
    /// The Library's import sessions, as last read.
    pub(super) sessions: super::library_imports::Sessions,
    /// The imports the user started here and handed to the node, by
    /// their requests, until they end.
    pub(super) started: Vec<Value>,
}

/// `import.toast`: an import's outcome (its end line) in a short toast
/// (a notice of its level when it failed, was cancelled or reports edits
/// not kept whole).
fn toast_import_end(teller: &super::said::Teller, line: &Value) {
    let (_, level, message, plain) = import_end_said(line);
    if !plain {
        teller.notify_unlogged(level, message);
        return;
    }
    // Information alone has no toast: an import's end has a short one.
    let text = teller.text(message.clone());
    teller.toast(Toast::success(&text));
    teller.notice(Notice {
        level: Level::Info,
        text,
        key: message.key,
        action: None,
        at: 0.0,
        report: None,
    });
}

/// Helios's listener of egui-pop's `files.drop`: files dropped on any
/// window open the import window on them.
pub const DROP_LISTENER: &str = "helios.import";

/// Why nothing is imported without a library open: the host runs imports
/// as `library.import`, which it has only with one (`--vault FILE`).
pub(crate) fn no_library() -> Message {
    Message::new(
        "gui-import-no-library",
        "No library is open: start Helios with one (--vault FILE) to import photos",
    )
}
/// What an import's end says (its end line): the Log panel's entries
/// before its own, its own message with its level, and whether it
/// takes the short success toast (else a notice of its level).
fn import_end_said(line: &Value) -> (Vec<(Level, Message)>, Level, Message, bool) {
    if let Some(e) = line["failed"].as_str() {
        let message =
            Message::new("gui-import-failed", "The import failed: { $error }").with("error", e);
        return (Vec::new(), Level::Error, message, false);
    }
    let report = &line["ended"];
    let count = |k: &str| report[k].as_u64().unwrap_or(0);
    // A RAW+JPEG pair is one photo: its JPEG, added, is its raw's file.
    let photos = (count("added") + count("located")).saturating_sub(count("paired"));
    if report["cancelled"] == true {
        let message = Message::new(
            "gui-import-cancelled",
            "The import was cancelled: { $photos } photos imported",
        )
        .with("photos", photos);
        return (Vec::new(), Level::Info, message, false);
    }
    let mut before = Vec::new();
    if count("metadata") > 0 {
        before.push((
            Level::Info,
            Message::new(
                "gui-import-metadata",
                "{ $photos } photos took their ratings, labels or tags from the importer",
            )
            .with("photos", count("metadata")),
        ));
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
    (before, level, message, level == Level::Info)
}
