//! A workspace edited on the desktop (see `docs/templates.md`, "Editing
//! a workspace"): its name, description and icon, its network, tools,
//! library and layout, kept as one undoable value of Helios's value
//! store ([`STATE`]), so an edit, a reset to its template or an import of
//! parts is one step of Edit > Undo. Each step is written to the
//! workspace's own files at once (`helios_template::workspace::Files`).
//!
//! "Save as template…" reviews what goes in, what is stripped and what
//! is asked on create, then writes a bundle among the user's templates;
//! "Add from workspace or template…" picks parts, shows the changes and
//! asks a side for each conflict. The commands take their arguments
//! whole for agents and scripts (`dry_run` says what would happen), and
//! ask for what is missing in the shell's dialogs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use egui_pop::api::dialogs;
use egui_pop::model::ids::{CommandId, PanelId};
use egui_pop::model::keymap::{Action, Layer};
use egui_pop::model::layout::Layout;
use helios_core::Message;
use helios_shell::commands::Origin;
use helios_template::workspace::{self, Extract, Files, Part, Side, State, Translation};
use helios_template::{Bundle, ImportDesc, LibraryDesc};
use serde_json::{Value, json};

use super::{Shell, lock};
use crate::dialogs::{Choice, Field, Picker};

/// The value the workspace's state is kept in.
pub const STATE: &str = "workspace/state";

/// The workspace's commands, with their titles.
pub(super) const COMMANDS: &[(&str, &str)] = &[
    (
        "workspace.edit",
        "Edit the workspace (`name`, `description`, `icon`, `changes`)",
    ),
    ("workspace.library", "Library settings of the workspace"),
    (
        "workspace.reset",
        "Reset the workspace to its template (`dry_run`)",
    ),
    (
        "workspace.saveAsTemplate",
        "Save the workspace as a template (`id`, `name`, `description`, `icon`, `dry_run`)",
    ),
    (
        "workspace.parts",
        "List the parts of the workspace, or of a workspace or template (`source`)",
    ),
    (
        "workspace.importParts",
        "Add parts from a workspace or template (`source`, `parts`, `picks`, `dry_run`)",
    ),
    (
        "workspace.importParts.review",
        "Review the parts to add: what changes, and the side to keep for each conflict",
    ),
    (
        "workspace.importParts.apply",
        "Add the parts reviewed, each conflict's side picked",
    ),
    (
        "network.delegate",
        "Delegate nodes of the network to a new sub-workspace (`name`, `nodes`, `dry_run`)",
    ),
    (
        "subworkspace.delete",
        "Delete a sub-workspace, its nodes kept in the network (`id`, `dry_run`)",
    ),
];

/// The workspace open, as the shell edits it.
pub(super) struct Edited {
    pub(super) files: Files,
    pub(super) config: PathBuf,
    /// The state's text as last applied.
    applied: String,
    /// The template it was made from, if it was.
    origin: Option<Bundle>,
    /// Its network changed since it was last taken: the image is opened
    /// again through it.
    network_changed: bool,
    /// What its files held when the desktop last wrote or read them
    /// (`State::to_json`, read as `Files::load_on_disk` reads it): a
    /// difference is a change made outside the desktop.
    pub(super) disk: Value,
    /// Its endpoint for agents and the watcher of its files.
    pub(super) live: Option<super::workspace_live::Live>,
    /// egui-pop's workspace keymap layer as the workspace's keys last
    /// loaded it: a difference is a change made in the shortcuts editor.
    keys_loaded: Vec<(String, String, String)>,
}

impl Edited {
    /// What the files held when the desktop last wrote or read them.
    pub(super) fn disk_state(&self) -> Result<State, String> {
        State::from_json(&self.disk)
    }
}

fn descriptor() -> helios_values::Descriptor {
    helios_values::Descriptor::text("workspace", "")
}

fn parse(text: &str) -> Result<State, String> {
    serde_json::from_str::<Value>(text)
        .map_err(|e| e.to_string())
        .and_then(|j| State::from_json(&j))
}

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn no_workspace() -> String {
    "no workspace is open".into()
}

impl Shell {
    /// Opens a workspace's state for editing: read from its own files
    /// (its template's until it was edited; the blank network for a
    /// workspace not made from one), its tools and library applied. The
    /// layout shown is kept: egui-pop loaded it.
    pub fn open_workspace(&mut self, config: &Path, dir: &Path) -> Result<(), String> {
        let files = Files::new(config, dir);
        let mut state = files.load()?;
        // The layout egui-pop loaded, as shown.
        state.bundle.layout = self.own_layout();
        let origin = files.origin().transpose()?;
        let disk = files.load_on_disk()?.to_json();
        let text = state.to_json().to_string();
        lock(&self.values).declare_with(
            STATE,
            descriptor(),
            helios_core::Value::Text(text.clone()),
        );
        self.apply_workspace(None, &state);
        // The workspace held before (a test opening another) is let go.
        self.release_workspace();
        let (live, problem) = super::workspace_live::Live::start(&files);
        if let Some(e) = problem {
            self.notes.push(format!(
                "the workspace is not reachable by agents through this window: {e}"
            ));
        }
        self.edit = Some(Edited {
            files,
            config: config.into(),
            applied: text,
            origin,
            network_changed: false,
            disk,
            live: Some(live),
            keys_loaded: Vec::new(),
        });
        self.offer_workspace_keys();
        self.keys_loaded();
        Ok(())
    }

    /// The shortcuts editor offers to change keys for this workspace,
    /// and names where each key comes from: `template` for a key its
    /// template set, egui-pop's `workspace` and `user` for the others.
    fn offer_workspace_keys(&mut self) {
        use helios_profile::{System, local_keys};
        let h = self.app.handle().clone();
        h.offer_workspace_keys(true);
        let from_template: BTreeSet<(String, String)> = self
            .edit
            .as_ref()
            .and_then(|e| e.origin.as_ref())
            .map(|o| &o.template.tools.keymap)
            .into_iter()
            .flatten()
            .map(|b| (local_keys(&b.keys, System::current()), b.command.clone()))
            .collect();
        let label = self.text(Message::new("gui-keys-from-template", "template"));
        h.name_key_sources(move |b| {
            let Action::Run(command) = &b.action else {
                return None;
            };
            (b.layer == Layer::Workspace
                && from_template.contains(&(b.keys.to_string(), command.to_string())))
            .then(|| label.clone())
        });
    }

    /// egui-pop's workspace layer, as compared: keys, command, clause.
    fn layer_keys(&self) -> Vec<(String, String, String)> {
        self.app
            .handle()
            .export_keymap(Layer::Workspace)
            .bindings
            .into_iter()
            .filter_map(|b| Some((b.keys, b.command?, b.when)))
            .collect()
    }

    /// Remembers egui-pop's workspace layer as the workspace's keys
    /// loaded it.
    fn keys_loaded(&mut self) {
        let keys = self.layer_keys();
        if let Some(edit) = &mut self.edit {
            edit.keys_loaded = keys;
        }
    }

    /// Once per frame: keys changed for the workspace in the shortcuts
    /// editor become the workspace's own, one undo step written to its
    /// files (`tools.keymap`, a removal written `-<command>`).
    pub(super) fn follow_workspace_keys(&mut self) {
        use helios_profile::{System, local_keys};
        let Some(edit) = &self.edit else {
            return;
        };
        let now = self.layer_keys();
        if now == edit.keys_loaded {
            return;
        }
        let made = self.workspace_step("change the workspace's keys", &Origin::User, |state| {
            let mut next = state.clone();
            let before = &state.bundle.template.tools.keymap;
            next.bundle.template.tools.keymap = now
                .iter()
                .map(|(keys, command, when)| {
                    // A key unchanged keeps how the template wrote it
                    // (`Mod+…`).
                    before
                        .iter()
                        .find(|b| {
                            local_keys(&b.keys, System::current()) == *keys
                                && b.command == *command
                                && b.when == *when
                        })
                        .cloned()
                        .unwrap_or_else(|| helios_template::format::Binding {
                            keys: keys.clone(),
                            command: command.clone(),
                            when: when.clone(),
                        })
                })
                .collect();
            Ok(next)
        });
        if let Err(e) = made {
            self.warn(e);
        }
        self.keys_loaded();
    }

    /// The open workspace as it stands, the layout shown included.
    pub fn workspace_state(&self) -> Option<State> {
        self.edit.as_ref()?;
        let value = lock(&self.values).get(STATE)?.value.clone();
        let helios_core::Value::Text(text) = value else {
            return None;
        };
        let mut state = parse(&text).ok()?;
        state.bundle.layout = self.own_layout();
        Some(state)
    }

    /// The layout shown, Helios's panels only: egui-pop's own (the jobs
    /// centre) belong to the application, not to a workspace.
    fn own_layout(&self) -> Layout {
        self.own_panels(self.layout())
    }

    /// A layout with Helios's panels only.
    pub(super) fn own_panels(&self, mut layout: Layout) -> Layout {
        let known = |p: &PanelId| self.panels.iter().any(|s| s.id == *p);
        let foreign: Vec<PanelId> = layout
            .placed()
            .into_iter()
            .filter(|p| !known(p))
            .cloned()
            .collect();
        for p in &foreign {
            layout.remove(p);
        }
        layout.hidden.retain(|p| known(p));
        layout
    }

    /// What an image opened goes through: the workspace's network.
    pub fn workspace_bundle(&self) -> Option<Bundle> {
        self.workspace_state().map(|s| s.bundle)
    }

    /// Whether the workspace's network changed since the last call: the
    /// host opens the image shown again through it.
    pub fn take_network_changed(&mut self) -> bool {
        self.edit
            .as_mut()
            .is_some_and(|e| std::mem::take(&mut e.network_changed))
    }

    pub(super) fn workspace_catalogue(&self) -> helios_template::Catalogue {
        crate::templates::catalogue(&self.template_panels(), self.specs())
    }

    /// Applies the state the value store holds when it is not the one
    /// applied (an edit, an undo, a redo): written to the workspace's
    /// files at once, its tools, library, layout and name put in place.
    pub(super) fn sync_workspace(&mut self) {
        let Some(edit) = &self.edit else {
            return;
        };
        let value = lock(&self.values).get(STATE).map(|e| e.value.clone());
        let Some(helios_core::Value::Text(text)) = value else {
            return;
        };
        if text == edit.applied {
            return;
        }
        let (before, next) = match (parse(&edit.applied), parse(&text)) {
            (Ok(b), Ok(n)) => (b, n),
            (Err(e), _) | (_, Err(e)) => {
                return self.warn(e);
            }
        };
        // Files changed outside since the desktop last wrote them are not
        // written over: taken in first, as a step of their own, which
        // writes the result.
        let outside = self.changed_outside();
        if outside.is_none() {
            self.write_workspace(&next);
        }
        self.apply_workspace(Some(&before), &next);
        if before.bundle.template.tools != next.bundle.template.tools {
            self.keys_loaded();
        }
        let network = before.bundle.template.network != next.bundle.template.network
            || before.bundle.composites != next.bundle.composites;
        if let Some(edit) = &mut self.edit {
            edit.applied = text;
            edit.network_changed |= network;
        }
        if let Some(theirs) = outside {
            self.reload_workspace(theirs);
        }
    }

    /// Writes a state to the workspace's files (its layout being
    /// egui-pop's to save), remembered as what the files hold.
    pub(super) fn write_workspace(&mut self, state: &State) {
        let Some(edit) = &mut self.edit else {
            return;
        };
        match edit.files.save(state, false) {
            Ok(()) => edit.disk = state.to_json(),
            Err(e) => self.warn(
                Message::new(
                    "gui-workspace-not-saved",
                    "The workspace was not saved: { $error }",
                )
                .with("error", e),
            ),
        }
    }

    /// Puts a state in place: what changed since `before` (everything
    /// but the layout without one).
    fn apply_workspace(&mut self, before: Option<&State>, next: &State) {
        let t = &next.bundle.template;
        let changed = |f: &dyn Fn(&State) -> bool| before.is_none_or(f);
        if changed(&|b| b.bundle.template.tools != t.tools) {
            self.apply_template_tools(&next.meta.name, &t.tools);
        }
        if changed(&|b| b.bundle.template.library != t.library) {
            self.import_source = t.library.as_ref().and_then(|l| l.import.clone());
            let mut lib = self.library.borrow_mut();
            lib.auto_advance = t.library.as_ref().is_some_and(|l| l.auto_advance);
            lib.import_offered = self.import_source.is_some();
        }
        if let Some(b) = before
            && b.bundle.layout != next.bundle.layout
        {
            self.set_layout(next.bundle.layout.clone());
        }
        if changed(&|b| b.meta.name != next.meta.name) {
            self.set_title(&next.meta.name);
        }
    }

    /// One undoable step on the workspace: `make` turns the state as it
    /// stands (the layout shown included) into the next one, which is
    /// written to the value store as one step labelled `label`, then
    /// applied. A change made to its files outside the desktop is taken
    /// in first, as a step of its own.
    fn workspace_step(
        &mut self,
        label: &str,
        origin: &Origin,
        make: impl FnOnce(&State) -> Result<State, String>,
    ) -> Result<State, String> {
        self.check_workspace_files();
        self.workspace_step_as_is(label, origin, make)
    }

    /// [`Shell::workspace_step`], the files not looked at first.
    pub(super) fn workspace_step_as_is(
        &mut self,
        label: &str,
        origin: &Origin,
        make: impl FnOnce(&State) -> Result<State, String>,
    ) -> Result<State, String> {
        let now = self.workspace_state().ok_or_else(no_workspace)?;
        let next = make(&now)?;
        let now_text = now.to_json().to_string();
        {
            let mut store = lock(&self.values);
            // The layout shown, so that an undo gives it back as it was.
            store.declare_with(
                STATE,
                descriptor(),
                helios_core::Value::Text(now_text.clone()),
            );
            let group = store.begin_group(origin.clone(), label);
            let set = store.set(
                helios_values::Set::new(
                    STATE,
                    helios_values::Change::To(helios_core::Value::Text(next.to_json().to_string())),
                    origin.clone(),
                )
                .gesture(group),
            );
            store.end(group);
            set.map_err(|e| e.to_string())?;
        }
        if let Some(edit) = &mut self.edit {
            edit.applied = now_text;
        }
        self.sync_workspace();
        Ok(next)
    }

    /// Edits the workspace (`name`, `description`, `icon`, `network`,
    /// `tools`, `library`, `layout`), checked first, as one undo step.
    pub fn edit_workspace(&mut self, changes: &Value, origin: &Origin) -> Result<Value, String> {
        let catalogue = self.workspace_catalogue();
        let next = self.workspace_step("edit the workspace", origin, |now| {
            let next = now.edited(changes)?;
            let problems = next.problems(&catalogue);
            if problems.is_empty() {
                Ok(next)
            } else {
                Err(problems.join("; "))
            }
        })?;
        Ok(json!({ "name": next.meta.name }))
    }

    /// Resets the workspace to its template, as one undo step; a dry run
    /// only says whether it changes anything.
    pub fn reset_workspace(&mut self, dry_run: bool, origin: &Origin) -> Result<Value, String> {
        let origin_bundle = self
            .edit
            .as_ref()
            .ok_or_else(no_workspace)?
            .origin
            .clone()
            .ok_or("the workspace was not made from a template")?;
        let catalogue = self.workspace_catalogue();
        let now = self.workspace_state().ok_or_else(no_workspace)?;
        let next = now.reset(&origin_bundle, &catalogue)?;
        let answer = json!({
            "template": origin_bundle.template.id,
            "version": origin_bundle.template.version,
            "changed": now != next,
            "dry_run": dry_run,
        });
        if !dry_run {
            self.workspace_step("reset the workspace to its template", origin, |_| Ok(next))?;
        }
        Ok(answer)
    }

    /// The parts of the workspace, or of `source` (a workspace's or a
    /// template's directory, or a template's id).
    pub fn workspace_parts(&self, source: Option<&str>) -> Result<Value, String> {
        let bundle = match source.filter(|s| !s.is_empty()) {
            Some(s) => {
                let config = &self.edit.as_ref().ok_or_else(no_workspace)?.config;
                workspace::source(config, s)?
            }
            None => self.workspace_bundle().ok_or_else(no_workspace)?,
        };
        Ok(json!({ "id": bundle.template.id, "parts": workspace::parts(&bundle) }))
    }

    /// Copies parts of a source into the workspace (`source`, `parts`,
    /// `picks` by conflict id, `dry_run`), as one undo step; refused while
    /// a conflict has no side picked.
    pub fn import_workspace_parts(
        &mut self,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        let name = text(args, "source")
            .filter(|s| !s.is_empty())
            .ok_or("`source` is missing")?;
        let config = self.edit.as_ref().ok_or_else(no_workspace)?.config.clone();
        let from = workspace::source(&config, name)?;
        let names: Vec<String> = serde_json::from_value(args["parts"].clone()).unwrap_or_default();
        let parts = Part::list(&names)?;
        let picks: BTreeMap<String, Side> = if args["picks"].is_null() {
            BTreeMap::new()
        } else {
            serde_json::from_value(args["picks"].clone()).map_err(|e| format!("picks: {e}"))?
        };
        let catalogue = self.workspace_catalogue();
        if args["dry_run"].as_bool().unwrap_or(false) {
            let now = self.workspace_state().ok_or_else(no_workspace)?;
            let merged = workspace::import(&now, &from, &parts, &picks, &catalogue)?;
            let mut answer = merged.to_json();
            answer["problems"] = json!(merged.state.problems(&catalogue));
            answer["dry_run"] = json!(true);
            return Ok(answer);
        }
        let mut answer = Value::Null;
        self.workspace_step(&format!("add parts from {name}"), origin, |now| {
            let merged = workspace::import(now, &from, &parts, &picks, &catalogue)?;
            let unresolved: Vec<&str> = merged.unresolved().iter().map(|c| c.id.as_str()).collect();
            if !unresolved.is_empty() {
                return Err(format!(
                    "pick a side for each conflict first: {}",
                    unresolved.join(", ")
                ));
            }
            let problems = merged.state.problems(&catalogue);
            if !problems.is_empty() {
                return Err(problems.join("; "));
            }
            answer = merged.to_json();
            Ok(merged.state)
        })?;
        answer["dry_run"] = json!(false);
        Ok(answer)
    }

    /// Delegates nodes of the network to a new sub-workspace (`name`,
    /// `nodes`, `dry_run`), as one undo step; checked first. The answer
    /// gives its id, its parent and the sub-workspaces moved inside it.
    pub fn delegate_nodes(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        let name = text(args, "name")
            .filter(|s| !s.trim().is_empty())
            .ok_or("`name` is missing")?
            .to_string();
        let nodes: Vec<String> =
            serde_json::from_value(args["nodes"].clone()).map_err(|e| format!("nodes: {e}"))?;
        self.subworkspace_step("delegate nodes", args, origin, |now| {
            now.delegated(&name, &nodes)
        })
    }

    /// Deletes a sub-workspace (`id`, `dry_run`), as one undo step: its
    /// nodes stay in the network, the ones nested in it move up, its id
    /// is retired.
    pub fn delete_subworkspace(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        let id = text(args, "id")
            .filter(|s| !s.is_empty())
            .ok_or("`id` is missing")?
            .to_string();
        self.subworkspace_step("delete a sub-workspace", args, origin, |now| {
            now.subworkspace_deleted(&id)
        })
    }

    /// A sub-workspace change as one undo step, checked first; a dry run
    /// answers what it would do (with its `problems`) and changes
    /// nothing.
    fn subworkspace_step(
        &mut self,
        label: &str,
        args: &Value,
        origin: &Origin,
        make: impl Fn(&State) -> Result<(State, Value), String>,
    ) -> Result<Value, String> {
        let catalogue = self.workspace_catalogue();
        if args["dry_run"].as_bool().unwrap_or(false) {
            let now = self.workspace_state().ok_or_else(no_workspace)?;
            let (next, mut answer) = make(&now)?;
            answer["problems"] = json!(next.problems(&catalogue));
            answer["dry_run"] = json!(true);
            return Ok(answer);
        }
        let mut answer = Value::Null;
        self.workspace_step(label, origin, |now| {
            let (next, made) = make(now)?;
            let problems = next.problems(&catalogue);
            if !problems.is_empty() {
                return Err(problems.join("; "));
            }
            answer = made;
            Ok(next)
        })?;
        answer["problems"] = json!([]);
        answer["dry_run"] = json!(false);
        Ok(answer)
    }

    /// Saves the workspace as a template (what an agent passes whole, or
    /// the review form's values): the review, and the bundle written
    /// among the user's templates unless it is a dry run.
    pub fn save_workspace_as_template(&mut self, args: &Value) -> Result<Value, String> {
        let config = self.edit.as_ref().ok_or_else(no_workspace)?.config.clone();
        let state = self.workspace_state().ok_or_else(no_workspace)?;
        let x = extract_of(args)?;
        let review = workspace::review(&state, &x, &self.workspace_catalogue());
        let mut answer = review.to_json();
        let dry_run = args["dry_run"].as_bool().unwrap_or(false);
        answer["dry_run"] = json!(dry_run);
        if dry_run {
            return Ok(answer);
        }
        let overwrite = args["overwrite"].as_bool().unwrap_or(false);
        let dir = workspace::save_template(&review, &config, overwrite)?;
        if let Err(e) =
            crate::templates::add_texts(&self.i18n, std::slice::from_ref(&review.bundle))
        {
            self.notes.push(e);
        }
        self.note(
            Message::new(
                "gui-workspace-template-saved",
                "template { $name } saved in { $path }",
            )
            .with("name", &x.name)
            .with("path", dir.display().to_string()),
        );
        answer["written"] = json!(dir);
        Ok(answer)
    }

    /// Runs one of [`COMMANDS`], asking in a dialog for what is missing.
    pub(super) fn workspace_command(
        &mut self,
        id: &str,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        let has = |k: &str| args.get(k).is_some_and(|v| !v.is_null());
        match id {
            "workspace.edit" => {
                if has("changes") {
                    return self.edit_workspace(&args["changes"], origin);
                }
                let mut changes = serde_json::Map::new();
                for k in ["name", "description", "icon"] {
                    if let Some(v) = args.get(k).filter(|v| !v.is_null()) {
                        changes.insert(k.into(), v.clone());
                    }
                }
                if changes.is_empty() {
                    self.edit_form()?;
                    return Ok(Value::Null);
                }
                self.edit_workspace(&Value::Object(changes), origin)
            }
            "workspace.library" => {
                if !has("source") {
                    self.library_form()?;
                    return Ok(Value::Null);
                }
                let library = library_of(args);
                self.edit_workspace(&json!({ "library": library }), origin)
            }
            "workspace.reset" => {
                self.reset_workspace(args["dry_run"].as_bool().unwrap_or(false), origin)
            }
            "workspace.saveAsTemplate" => {
                if text(args, "id").is_none_or(str::is_empty) {
                    self.template_form()?;
                    return Ok(Value::Null);
                }
                self.save_workspace_as_template(args)
            }
            "workspace.parts" => self.workspace_parts(text(args, "source")),
            "workspace.importParts" => {
                if has("parts") || has("picks") || has("dry_run") {
                    return self.import_workspace_parts(args, origin);
                }
                match text(args, "source").filter(|s| !s.is_empty()) {
                    None => self.sources_picker(),
                    Some(source) => self.parts_form(source),
                }
                Ok(Value::Null)
            }
            "workspace.importParts.review" => {
                self.import_review(args)?;
                Ok(Value::Null)
            }
            "workspace.importParts.apply" => {
                let mut picks = serde_json::Map::new();
                if let Some(o) = args.as_object() {
                    for (k, v) in o {
                        if k != "source" && k != "parts" {
                            picks.insert(k.clone(), v.clone());
                        }
                    }
                }
                let direct = json!({
                    "source": args["source"],
                    "parts": args["parts"],
                    "picks": picks,
                });
                self.import_workspace_parts(&direct, origin)
            }
            "network.delegate" => self.delegate_nodes(args, origin),
            "subworkspace.delete" => self.delete_subworkspace(args, origin),
            other => Err(format!("no workspace command `{other}`")),
        }
    }

    /// A form whose answer runs `command` with `args` and its values.
    fn form_with(&mut self, f: crate::dialogs::Form, args: Value) -> Result<(), String> {
        let command = CommandId::new(f.command.as_str()).map_err(|e| e.to_string())?;
        let mut form = dialogs::Form::new(&f.title, &f.verb, command);
        form.args = args;
        for field in &f.fields {
            form = form.field_at(
                &field.key,
                crate::dialogs::descriptor(&field.descriptor),
                crate::dialogs::value(&field.value),
            );
        }
        self.open_dialog(form);
        Ok(())
    }

    /// The name, description and icon, asked.
    fn edit_form(&mut self) -> Result<(), String> {
        let s = self.workspace_state().ok_or_else(no_workspace)?;
        let fields = vec![
            text_field(
                "name",
                &self.text(Message::new("gui-workspace-name", "name")),
                &s.meta.name,
            ),
            text_field(
                "description",
                &self.text(Message::new("gui-workspace-description", "description")),
                &s.meta.description,
            ),
            text_field(
                "icon",
                &self.text(Message::new("gui-workspace-icon", "icon")),
                s.meta.icon.as_deref().unwrap_or_default(),
            ),
        ];
        let f = crate::dialogs::Form {
            title: self.text(Message::new(
                "gui-workspace-edit-title",
                "Edit the workspace",
            )),
            fields,
            verb: self.text(Message::new("gui-workspace-edit-verb", "Save")),
            command: "workspace.edit".into(),
        };
        self.form_with(f, json!({}))
    }

    /// The library settings, asked: the import source (or none), how
    /// files are placed and renamed, what duplicates do, stacks and
    /// auto-advance.
    fn library_form(&mut self) -> Result<(), String> {
        use helios_values::Descriptor;
        let s = self.workspace_state().ok_or_else(no_workspace)?;
        let library = s.bundle.template.library.unwrap_or_default();
        let import = library.import.clone();
        let source = import.as_ref().map_or("none", |i| i.source.as_str());
        let placement = import
            .as_ref()
            .map_or("reference", |i| i.placement.as_str());
        let duplicates = import.as_ref().map_or("skip", |i| i.duplicates.as_str());
        let rename = import
            .as_ref()
            .and_then(|i| i.rename.clone())
            .unwrap_or_default();
        let stacks = import.as_ref().is_some_and(|i| i.suggest_stacks);
        let choice = |key: &str, label: String, choices: &[&str], value: &str| Field {
            key: key.into(),
            descriptor: Descriptor::choice(&label, choices, value),
            value: helios_core::Value::Choice(value.into()),
        };
        let boolean = |key: &str, label: String, value: bool| Field {
            key: key.into(),
            descriptor: Descriptor::bool(&label, value),
            value: helios_core::Value::Bool(value),
        };
        let fields = vec![
            choice(
                "source",
                self.text(Message::new(
                    "gui-workspace-library-source",
                    "import source",
                )),
                &["none", "card", "folder"],
                source,
            ),
            choice(
                "placement",
                self.text(Message::new("gui-workspace-library-placement", "placement")),
                &["reference", "copy"],
                placement,
            ),
            text_field(
                "rename",
                &self.text(Message::new(
                    "gui-workspace-library-rename",
                    "rename copies as",
                )),
                &rename,
            ),
            choice(
                "duplicates",
                self.text(Message::new(
                    "gui-workspace-library-duplicates",
                    "exact duplicates",
                )),
                &["skip", "keep", "ask"],
                duplicates,
            ),
            boolean(
                "suggest_stacks",
                self.text(Message::new(
                    "gui-workspace-library-stacks",
                    "suggest stacks after an import",
                )),
                stacks,
            ),
            boolean(
                "auto_advance",
                self.text(Message::new(
                    "gui-workspace-library-advance",
                    "culling advances after each mark",
                )),
                library.auto_advance,
            ),
        ];
        let f = crate::dialogs::Form {
            title: self.text(Message::new(
                "gui-workspace-library-title",
                "The workspace's library",
            )),
            fields,
            verb: self.text(Message::new("gui-workspace-edit-verb", "Save")),
            command: "workspace.library".into(),
        };
        self.form_with(f, json!({}))
    }

    /// The review of "Save as template": its id, text and icon, the parts
    /// that go in, what is asked on create; what is stripped said under
    /// the id.
    fn template_form(&mut self) -> Result<(), String> {
        use helios_values::Descriptor;
        let s = self.workspace_state().ok_or_else(no_workspace)?;
        let id = format!(
            "user.{}",
            helios_template::format::slug(&s.meta.name.to_ascii_lowercase())
                .split('-')
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
                .join("-")
        );
        let x = Extract {
            id: id.clone(),
            name: s.meta.name.clone(),
            description: s.meta.description.clone(),
            ..Extract::default()
        };
        let review = workspace::review(&s, &x, &self.workspace_catalogue());
        let stripped: Vec<String> = review.stripped.iter().map(|st| st.detail.clone()).collect();
        let mut id_field = text_field(
            "id",
            &self.text(Message::new("gui-workspace-template-id", "template id")),
            &id,
        );
        id_field.descriptor.description = self.text(
            Message::new("gui-workspace-template-stripped", "Never kept: { $what }")
                .with("what", stripped.join("; ")),
        );
        let boolean = |key: &str, label: String, value: bool| Field {
            key: key.into(),
            descriptor: Descriptor::bool(&label, value),
            value: helios_core::Value::Bool(value),
        };
        let mut fields = vec![
            id_field,
            Field {
                key: "version".into(),
                descriptor: Descriptor::int(
                    &self.text(Message::new("gui-workspace-template-version", "version")),
                    1,
                    9999,
                    1,
                ),
                value: helios_core::Value::Int(1),
            },
            text_field(
                "name",
                &self.text(Message::new("gui-workspace-name", "name")),
                &s.meta.name,
            ),
            text_field(
                "description",
                &self.text(Message::new("gui-workspace-description", "description")),
                &s.meta.description,
            ),
            text_field(
                "icon",
                &self.text(Message::new("gui-workspace-icon", "icon")),
                s.meta.icon.as_deref().unwrap_or_default(),
            ),
            text_field(
                "name_fr",
                &self.text(Message::new(
                    "gui-workspace-template-name-fr",
                    "name in French",
                )),
                "",
            ),
            text_field(
                "description_fr",
                &self.text(Message::new(
                    "gui-workspace-template-description-fr",
                    "description in French",
                )),
                "",
            ),
            boolean(
                "keys",
                self.text(Message::new("gui-workspace-template-keys", "with its keys")),
                true,
            ),
            boolean(
                "toolbox",
                self.text(Message::new(
                    "gui-workspace-template-toolbox",
                    "with its tools",
                )),
                true,
            ),
            boolean(
                "library",
                self.text(Message::new(
                    "gui-workspace-template-library",
                    "with its library settings",
                )),
                true,
            ),
        ];
        for c in &review.candidates {
            fields.push(boolean(
                &format!("ask:{}", c.id),
                self.text(
                    Message::new("gui-workspace-template-ask", "ask on create: { $name }")
                        .with("name", &c.name),
                ),
                true,
            ));
        }
        fields.push(boolean(
            "overwrite",
            self.text(Message::new(
                "gui-workspace-template-overwrite",
                "replace a template of that id",
            )),
            false,
        ));
        let f = crate::dialogs::Form {
            title: self.text(Message::new(
                "gui-workspace-template-title",
                "Save as template",
            )),
            fields,
            verb: self.text(Message::new("gui-workspace-edit-verb", "Save")),
            command: "workspace.saveAsTemplate".into(),
        };
        self.form_with(f, json!({}))
    }

    /// Where parts may come from: the built-in and user templates, the
    /// recent workspaces.
    fn sources_picker(&mut self) {
        let config = self
            .edit
            .as_ref()
            .map_or_else(super::config_dir, |e| e.config.clone());
        let mut choices: Vec<Choice> = crate::templates::all(&config)
            .iter()
            .map(|b| Choice {
                value: b.template.id.clone(),
                label: self.text(Message::new(&b.name_key(), &b.template.meta.name)),
                detail: Some(self.text(Message::new(&b.scenario_key(), &b.template.meta.scenario))),
            })
            .collect();
        let own = self.edit.as_ref().map(|e| e.files.own.clone());
        for dir in super::recent_workspaces() {
            if Some(Files::new(&config, &dir).own) == own {
                continue;
            }
            choices.push(Choice {
                value: dir.display().to_string(),
                label: dir
                    .file_name()
                    .map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into()),
                detail: Some(dir.display().to_string()),
            });
        }
        let title = self.text(Message::new(
            "gui-workspace-import-title",
            "Add from a workspace or a template",
        ));
        self.pick(Picker {
            title,
            choices,
            command: "workspace.importParts".into(),
            arg: "source".into(),
        });
    }

    /// The parts of a source to add, asked (each on by default).
    fn parts_form(&mut self, source: &str) {
        use helios_values::Descriptor;
        let fields = Part::ALL
            .iter()
            .map(|p| Field {
                key: p.name().into(),
                descriptor: Descriptor::bool(p.name(), true),
                value: helios_core::Value::Bool(true),
            })
            .collect();
        let f = crate::dialogs::Form {
            title: self.text(
                Message::new(
                    "gui-workspace-import-parts",
                    "Parts to add from { $source }",
                )
                .with("source", source),
            ),
            fields,
            verb: self.text(Message::new("gui-workspace-import-next", "Review")),
            command: "workspace.importParts.review".into(),
        };
        if let Err(e) = self.form_with(f, json!({ "source": source })) {
            self.warn(e);
        }
    }

    /// The import worked out without writing anything: its changes said
    /// in the title, a side asked for each conflict (the workspace's by
    /// default).
    fn import_review(&mut self, args: &Value) -> Result<(), String> {
        use helios_values::Descriptor;
        let source = text(args, "source")
            .ok_or("`source` is missing")?
            .to_string();
        let parts: Vec<String> = Part::ALL
            .iter()
            .filter(|p| args[p.name()].as_bool().unwrap_or(false))
            .map(|p| p.name().to_string())
            .collect();
        if parts.is_empty() {
            return Err(self.text(Message::new("gui-workspace-import-none", "no part chosen")));
        }
        let dry = self.import_workspace_parts(
            &json!({ "source": source, "parts": parts, "dry_run": true }),
            &Origin::User,
        )?;
        let fields = dry["conflicts"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| {
                let id = c["id"].as_str().unwrap_or_default();
                let mut d = Descriptor::choice(
                    c["what"].as_str().unwrap_or(id),
                    &["ours", "theirs"],
                    "ours",
                );
                d.description = format!("ours: {}; theirs: {}", c["ours"], c["theirs"]);
                Field {
                    key: id.into(),
                    descriptor: d,
                    value: helios_core::Value::Choice("ours".into()),
                }
            })
            .collect();
        let changes = dry["changes"].as_array().map_or(0, Vec::len);
        let f = crate::dialogs::Form {
            title: self.text(
                Message::new(
                    "gui-workspace-import-review",
                    "Add from { $source }: { $changes } changes, { $conflicts } to decide",
                )
                .with("source", source.as_str())
                .with("changes", changes)
                .with("conflicts", dry["conflicts"].as_array().map_or(0, Vec::len)),
            ),
            fields,
            verb: self.text(Message::new("gui-workspace-import-verb", "Add")),
            command: "workspace.importParts.apply".into(),
        };
        self.form_with(f, json!({ "source": source, "parts": parts }))
    }
}

fn text_field(key: &str, label: &str, value: &str) -> Field {
    Field {
        key: key.into(),
        descriptor: helios_values::Descriptor::text(label, value),
        value: helios_core::Value::Text(value.into()),
    }
}

/// The library settings a form or an agent gives.
fn library_of(args: &Value) -> Value {
    let t = |k: &str, default: &str| text(args, k).unwrap_or(default).to_string();
    let source = t("source", "none");
    let import = (source != "none").then(|| ImportDesc {
        source,
        placement: t("placement", "reference"),
        rename: text(args, "rename")
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(String::from),
        duplicates: t("duplicates", "skip"),
        suggest_stacks: args["suggest_stacks"].as_bool().unwrap_or(false),
    });
    let auto_advance = args["auto_advance"].as_bool().unwrap_or(false);
    if import.is_none() && !auto_advance {
        return Value::Null;
    }
    json!(LibraryDesc {
        import,
        auto_advance
    })
}

/// What "Save as template" is asked, from an agent's arguments or the
/// review form's values (`keys`, `toolbox`, `library` off leave them out;
/// `ask:<id>` on asks it; `name_fr`, `description_fr` the French text).
fn extract_of(args: &Value) -> Result<Extract, String> {
    let s = |k: &str| text(args, k).unwrap_or_default().trim().to_string();
    let mut translations: BTreeMap<String, Translation> = if args["translations"].is_null() {
        BTreeMap::new()
    } else {
        serde_json::from_value(args["translations"].clone())
            .map_err(|e| format!("translations: {e}"))?
    };
    if !s("name_fr").is_empty() {
        translations.insert(
            "fr".into(),
            Translation {
                name: s("name_fr"),
                description: s("description_fr"),
            },
        );
    }
    let mut without: Vec<Part> = if args["without"].is_null() {
        Vec::new()
    } else {
        serde_json::from_value(args["without"].clone()).map_err(|e| format!("without: {e}"))?
    };
    for p in [Part::Keys, Part::Toolbox, Part::Library] {
        if args[p.name()].as_bool() == Some(false) && !without.contains(&p) {
            without.push(p);
        }
    }
    let mut ask: Option<Vec<String>> = if args["ask"].is_null() {
        None
    } else {
        Some(serde_json::from_value(args["ask"].clone()).map_err(|e| format!("ask: {e}"))?)
    };
    if let Some(o) = args.as_object() {
        for (k, v) in o {
            if let Some(id) = k.strip_prefix("ask:") {
                let list = ask.get_or_insert_with(Vec::new);
                if v.as_bool() == Some(true) {
                    list.push(id.to_string());
                }
            }
        }
    }
    Ok(Extract {
        id: s("id"),
        version: args["version"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(1),
        name: s("name"),
        description: s("description"),
        icon: Some(s("icon")).filter(|i| !i.is_empty()),
        translations,
        without,
        ask,
    })
}
