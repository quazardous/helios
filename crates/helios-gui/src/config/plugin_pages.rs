//! A plugin's declared pages (`[ui] pages`, the `ui` interface): admin
//! manifests in JSON beside its manifest, read and checked here before
//! anything of the plugin runs, then shown in its section of Settings >
//! Plugins ([`super::plugins::section`]). The plugin draws nothing: the
//! Settings window renders the bricks.
//!
//! **What a page may hold** (the rest is left out, each with its reason,
//! and the rest of the page still shows):
//!
//! - one section, `main` in the `plugins` scope: its groups join the
//!   plugin's own section. No scope, wizard or other section yet;
//! - fields bound to the plugin's own settings (`<plugin id>.<key>`, a
//!   key of its contract) or to drafts of its own tasks (`<plugin id>/…`);
//! - actions naming the plugin's own commands, or one of [`NAVIGATION`];
//! - statuses and record lists reading the plugin's own `read_only`
//!   commands; row fields and row actions naming its own commands;
//! - callouts. No registered widget, and no hook: hooks are the host's.
//!
//! **Texts** are message ids of the plugin's own catalogue
//! (`<plugin slug>-…`, `i18n/<locale>/*.ftl`), translated here; any other
//! text is refused.
//!
//! **Bounds.** A file is at most [`MAX_BYTES`], a page at most
//! [`MAX_ITEMS`] items. A newer format is refused. A brick this Helios
//! does not know (a `node` outside [`NODES`]) is left out before the
//! manifest is read, so a newer brick never fails the page.

use std::path::Path;

use egui_pop::api::admin::model::{
    Action, Bind, Group, Item, Manifest, Meta, RecordSource, Records, StatusSource,
};
use helios_core::Message;
use serde_json::Value as Json;

/// The largest page file read.
pub const MAX_BYTES: u64 = 256 * 1024;

/// The most items a page may hold, every group together.
pub const MAX_ITEMS: usize = 200;

/// The host commands a page's action may name besides the plugin's own:
/// navigation only.
pub const NAVIGATION: &[&str] = &["admin.open", "help.health"];

/// The bricks this Helios reads (egui-pop's admin `node`s).
pub const NODES: &[&str] = &[
    "field", "action", "status", "callout", "widget", "list", "table",
];

/// The one section a page may declare: the plugin's own.
pub const MAIN: &str = "main";

/// The scope a page's section is in.
pub const SCOPE: &str = "plugins";

/// The groups of the plugin's section the host keeps for itself.
const HOST_GROUPS: &[&str] = &["permissions", "settings", "plugin-page-problems"];

/// What a plugin's pages give its section: their groups, checked, their
/// texts still message ids, and what was left out, with why.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub groups: Vec<Group>,
    pub problems: Vec<String>,
}

impl Page {
    /// The settings its fields place (`<plugin id>.<key>`).
    pub fn placed(&self) -> Vec<String> {
        self.groups
            .iter()
            .flat_map(|g| &g.items)
            .filter_map(|i| match i {
                Item::Field(f) => f.setting_id().map(String::from),
                _ => None,
            })
            .collect()
    }
}

/// What a page is checked against: the plugin's id, its contract's keys
/// and its commands (id, `read_only`).
pub struct Plugin<'a> {
    pub id: &'a str,
    pub keys: Vec<String>,
    pub commands: Vec<(String, bool)>,
}

impl Plugin<'_> {
    /// From its manifest.
    pub fn of(m: &helios_plugin::Manifest) -> Plugin<'_> {
        Plugin {
            id: &m.id,
            keys: m.settings.keys.keys().cloned().collect(),
            commands: m
                .commands
                .iter()
                .map(|c| (c.id.clone(), c.read_only))
                .collect(),
        }
    }

    fn own_command(&self, id: &str, read_only: bool) -> bool {
        self.commands
            .iter()
            .any(|(c, ro)| c == id && (*ro || !read_only))
    }

    /// The prefix of its message ids.
    fn prefix(&self) -> String {
        format!("{}-", helios_i18n::ids::slug(self.id))
    }
}

/// Reads and checks every page file `manifest` lists, in `dir`.
pub fn read(dir: &Path, manifest: &helios_plugin::Manifest) -> Page {
    let plugin = Plugin::of(manifest);
    let mut page = Page::default();
    for file in &manifest.ui.pages {
        let path = dir.join(file);
        let text = match std::fs::metadata(&path) {
            Ok(m) if m.len() > MAX_BYTES => Err(format!("{} bytes, at most {MAX_BYTES}", m.len())),
            Ok(_) => std::fs::read_to_string(&path).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        match text.and_then(|t| serde_json::from_str::<Json>(&t).map_err(|e| e.to_string())) {
            Ok(json) => {
                let (groups, problems) = check(&plugin, json);
                page.groups.extend(groups);
                page.problems
                    .extend(problems.into_iter().map(|p| format!("{file}: {p}")));
            }
            Err(e) => page.problems.push(format!("{file}: not read ({e})")),
        }
    }
    for p in &page.problems {
        log::warn!("plugin `{}`'s page: {p}", manifest.id);
    }
    page
}

/// Checks one page file's JSON: the groups it gives the plugin's
/// section, and what was left out.
pub fn check(plugin: &Plugin<'_>, mut json: Json) -> (Vec<Group>, Vec<String>) {
    let mut problems = Vec::new();
    unknown_nodes(&mut json, &mut problems);
    let mut m = match Manifest::from_json(&json) {
        Ok(m) => m,
        Err(e) => return (Vec::new(), vec![format!("refused: {e:?}")]),
    };
    if !m.scopes.is_empty() || !m.wizards.is_empty() || !m.quick.is_empty() {
        problems.push("scopes, wizards and quick settings are not for plugins: left out".into());
    }
    let mut groups = Vec::new();
    let mut items = 0;
    for s in std::mem::take(&mut m.sections) {
        if s.scope != SCOPE || s.meta.id != MAIN {
            problems.push(format!(
                "section `{}`: a page declares only `{SCOPE}/{MAIN}`, the plugin's own: left out",
                s.path()
            ));
            continue;
        }
        for mut g in s.groups {
            if HOST_GROUPS.contains(&g.meta.id.as_str())
                || groups.iter().any(|x: &Group| x.meta.id == g.meta.id)
            {
                problems.push(format!("group `{}`: the id is taken: left out", g.meta.id));
                continue;
            }
            if let Err(e) = texts(plugin, &g.meta) {
                problems.push(format!("group `{}`: {e}: left out", g.meta.id));
                continue;
            }
            g.items.retain(|item| {
                let at = format!("{}/{}", g.meta.id, item.meta().id);
                if items >= MAX_ITEMS {
                    problems.push(format!("{at}: more than {MAX_ITEMS} items: left out"));
                    return false;
                }
                match allowed(plugin, item) {
                    Ok(()) => {
                        items += 1;
                        true
                    }
                    Err(e) => {
                        problems.push(format!("{at}: {e}: left out"));
                        false
                    }
                }
            });
            groups.push(g);
        }
    }
    (groups, problems)
}

/// Leaves out the items whose `node` this Helios does not know, so a
/// newer brick never fails the page.
fn unknown_nodes(json: &mut Json, problems: &mut Vec<String>) {
    let Some(sections) = json.get_mut("sections").and_then(Json::as_array_mut) else {
        return;
    };
    for s in sections {
        let Some(groups) = s.get_mut("groups").and_then(Json::as_array_mut) else {
            continue;
        };
        for g in groups {
            let group = g["id"].as_str().unwrap_or_default().to_string();
            let Some(items) = g.get_mut("items").and_then(Json::as_array_mut) else {
                continue;
            };
            items.retain(|i| {
                let node = i["node"].as_str().unwrap_or_default();
                if NODES.contains(&node) {
                    return true;
                }
                problems.push(format!(
                    "{group}/{}: unknown brick `{node}` (a newer format): left out",
                    i["id"].as_str().unwrap_or_default()
                ));
                false
            });
        }
    }
}

/// Every text of a node is a message id of the plugin's (or empty).
fn texts(plugin: &Plugin<'_>, meta: &Meta) -> Result<(), String> {
    let all = [&meta.title, &meta.description]
        .into_iter()
        .chain(meta.reason.as_ref());
    message_ids(plugin, all)
}

fn message_ids<'a>(
    plugin: &Plugin<'_>,
    texts: impl IntoIterator<Item = &'a String>,
) -> Result<(), String> {
    let prefix = plugin.prefix();
    for t in texts {
        if !t.is_empty() && !t.starts_with(&prefix) {
            return Err(format!(
                "text `{t}` is not one of the plugin's message ids (`{prefix}…`)"
            ));
        }
    }
    Ok(())
}

fn action(plugin: &Plugin<'_>, a: &Action) -> Result<(), String> {
    if !plugin.own_command(&a.command, false) && !NAVIGATION.contains(&a.command.as_str()) {
        return Err(format!(
            "command `{}` is neither the plugin's nor a navigation one",
            a.command
        ));
    }
    if a.confirm.is_some() {
        return Err("a confirmation hook is the host's".into());
    }
    texts(plugin, &a.meta)?;
    if let Some(c) = &a.confirmation {
        message_ids(plugin, [&c.title, &c.text, &c.verb])?;
    }
    Ok(())
}

fn records(plugin: &Plugin<'_>, r: &Records) -> Result<(), String> {
    match &r.source {
        RecordSource::Command { command, .. } if plugin.own_command(command, true) => {}
        RecordSource::Command { command, .. } => {
            return Err(format!(
                "records from `{command}`, not a `read_only` command of the plugin"
            ));
        }
        RecordSource::Hook { .. } => return Err("a records hook is the host's".into()),
    }
    for f in &r.row_fields {
        if !plugin.own_command(&f.command, false) {
            return Err(format!(
                "row field command `{}` is not the plugin's",
                f.command
            ));
        }
        texts(plugin, &f.meta)?;
        message_ids(plugin, f.choices.iter().map(|c| &c.label))?;
    }
    for a in &r.row_actions {
        action(plugin, a)?;
    }
    if let Some(e) = &r.empty {
        message_ids(plugin, [&e.text])?;
        if let Some(a) = &e.action {
            action(plugin, a)?;
        }
    }
    message_ids(plugin, r.columns.iter().map(|c| &c.title))?;
    texts(plugin, &r.meta)
}

/// Whether a page may hold `item`, and why not.
pub fn allowed(plugin: &Plugin<'_>, item: &Item) -> Result<(), String> {
    match item {
        Item::Field(f) => {
            match &f.bind {
                Bind::Setting { setting } => {
                    let own = setting
                        .strip_prefix(plugin.id)
                        .and_then(|k| k.strip_prefix('.'))
                        .is_some_and(|k| plugin.keys.iter().any(|x| x == k));
                    if !own {
                        return Err(format!("setting `{setting}` is not one of the plugin's"));
                    }
                }
                Bind::Draft { task, .. } => {
                    let own = task == plugin.id
                        || task
                            .strip_prefix(plugin.id)
                            .is_some_and(|r| r.starts_with('/'));
                    if !own {
                        return Err(format!("draft of task `{task}`, not one of the plugin's"));
                    }
                }
            }
            if !f.hooks.is_empty() {
                return Err("field hooks are the host's".into());
            }
            message_ids(plugin, [&f.help])?;
            message_ids(plugin, f.input.choices.iter().map(|c| &c.label))?;
            texts(plugin, &f.meta)
        }
        Item::Action(a) => action(plugin, a),
        Item::Status(s) => match &s.source {
            StatusSource::Command { command, .. } if plugin.own_command(command, true) => {
                texts(plugin, &s.meta)
            }
            StatusSource::Command { command, .. } => Err(format!(
                "status from `{command}`, not a `read_only` command of the plugin"
            )),
            StatusSource::Hook { .. } => Err("a status hook is the host's".into()),
        },
        Item::Callout(c) => {
            message_ids(plugin, [&c.text])?;
            if let Some(a) = &c.action {
                action(plugin, a)?;
            }
            texts(plugin, &c.meta)
        }
        Item::List(r) | Item::Table(r) => records(plugin, r),
        Item::Widget(_) => Err("registered widgets are the host's".into()),
    }
}

/// A page's groups with their texts translated through the plugin's
/// catalogue (`t`); a message missing there shows its id.
pub fn translated(groups: &[Group], t: &dyn Fn(Message) -> String) -> Vec<Group> {
    let tr = |s: &mut String| {
        if !s.is_empty() {
            *s = t(Message::new(s, s));
        }
    };
    let meta = |m: &mut Meta| {
        tr(&mut m.title);
        tr(&mut m.description);
        if let Some(r) = m.reason.as_mut() {
            tr(r);
        }
    };
    let act = |a: &mut Action| {
        meta(&mut a.meta);
        if let Some(c) = a.confirmation.as_mut() {
            tr(&mut c.title);
            tr(&mut c.text);
            tr(&mut c.verb);
        }
    };
    let mut groups = groups.to_vec();
    for g in &mut groups {
        meta(&mut g.meta);
        for item in &mut g.items {
            match item {
                Item::Field(f) => {
                    meta(&mut f.meta);
                    tr(&mut f.help);
                    f.input.choices.iter_mut().for_each(|c| tr(&mut c.label));
                }
                Item::Action(a) => act(a),
                Item::Status(s) => meta(&mut s.meta),
                Item::Callout(c) => {
                    meta(&mut c.meta);
                    tr(&mut c.text);
                    if let Some(a) = c.action.as_mut() {
                        act(a);
                    }
                }
                Item::List(r) | Item::Table(r) => {
                    meta(&mut r.meta);
                    r.columns.iter_mut().for_each(|c| tr(&mut c.title));
                    for f in &mut r.row_fields {
                        meta(&mut f.meta);
                        f.choices.iter_mut().for_each(|c| tr(&mut c.label));
                    }
                    r.row_actions.iter_mut().for_each(act);
                    if let Some(e) = r.empty.as_mut() {
                        tr(&mut e.text);
                        if let Some(a) = e.action.as_mut() {
                            act(a);
                        }
                    }
                }
                Item::Widget(_) => {}
            }
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "org.example.pages";

    fn plugin() -> Plugin<'static> {
        Plugin {
            id: ID,
            keys: vec!["api".into(), "mode".into()],
            commands: vec![("pages.check".into(), true), ("pages.reset".into(), false)],
        }
    }

    fn page(items: Json) -> Json {
        json!({
            "version": 1,
            "sections": [{"id": "main", "scope": "plugins", "groups": [{
                "id": "connection",
                "title": "org-example-pages-connection",
                "items": items,
            }]}],
        })
    }

    fn ids(groups: &[Group]) -> Vec<String> {
        groups
            .iter()
            .flat_map(|g| &g.items)
            .map(|i| i.meta().id.clone())
            .collect()
    }

    #[test]
    fn a_sound_page_keeps_every_item() {
        let (groups, problems) = check(
            &plugin(),
            page(json!([
                {"node": "field", "id": "api", "bind": {"to": "setting", "setting": "org.example.pages.api"}},
                {"node": "status", "id": "reach", "title": "org-example-pages-reach",
                 "source": {"from": "command", "command": "pages.check", "pointer": "/status"}},
                {"node": "action", "id": "test", "title": "org-example-pages-test",
                 "command": "pages.check"},
                {"node": "action", "id": "health", "title": "org-example-pages-health",
                 "command": "help.health"},
            ])),
        );
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(ids(&groups), ["api", "reach", "test", "health"]);
    }

    #[test]
    fn what_reaches_beyond_the_plugin_is_left_out_and_the_rest_stays() {
        let (groups, problems) = check(
            &plugin(),
            page(json!([
                {"node": "field", "id": "foreign", "bind": {"to": "setting", "setting": "helios.library.folder"}},
                {"node": "field", "id": "api", "bind": {"to": "setting", "setting": "org.example.pages.api"}},
                {"node": "action", "id": "wipe", "command": "library.forget"},
                {"node": "status", "id": "writes", "source": {"from": "command", "command": "pages.reset", "pointer": ""}},
                {"node": "status", "id": "hook", "source": {"from": "hook", "hook": "storage.free"}},
                {"node": "callout", "id": "plain", "tone": "info", "text": "Not a message id"},
                {"node": "widget", "id": "w", "kind": "helios.curve"},
                {"node": "gauge", "id": "future"},
                {"node": "action", "id": "test", "title": "org-example-pages-test",
                 "command": "pages.check"},
            ])),
        );
        assert_eq!(ids(&groups), ["api", "test"], "{problems:#?}");
        for expect in [
            "unknown brick `gauge`",
            "setting `helios.library.folder` is not one of the plugin's",
            "command `library.forget` is neither the plugin's nor a navigation one",
            "status from `pages.reset`, not a `read_only` command",
            "a status hook is the host's",
            "text `Not a message id` is not one of the plugin's message ids",
            "registered widgets are the host's",
        ] {
            assert!(
                problems.iter().any(|p| p.contains(expect)),
                "{expect}: {problems:#?}"
            );
        }
    }

    #[test]
    fn other_sections_scopes_and_newer_formats_are_refused() {
        let (groups, problems) = check(
            &plugin(),
            json!({
                "version": 1,
                "scopes": [{"id": "mine", "title": "org-example-pages-mine"}],
                "sections": [
                    {"id": "general", "scope": "app", "groups": []},
                    {"id": "main", "scope": "plugins", "groups": [
                        {"id": "permissions", "title": "", "items": []},
                    ]},
                ],
            }),
        );
        assert!(groups.is_empty());
        assert_eq!(problems.len(), 3, "{problems:#?}");
        let (groups, problems) = check(&plugin(), json!({"version": 99}));
        assert!(groups.is_empty());
        assert!(problems[0].contains("Newer"), "{problems:?}");
    }

    #[test]
    fn a_page_is_capped() {
        let many: Vec<Json> = (0..MAX_ITEMS + 5)
            .map(|i| json!({"node": "action", "id": format!("a{i}"), "command": "pages.check"}))
            .collect();
        let (groups, problems) = check(&plugin(), page(Json::Array(many)));
        assert_eq!(groups[0].items.len(), MAX_ITEMS);
        assert_eq!(problems.len(), 5);
    }
}
