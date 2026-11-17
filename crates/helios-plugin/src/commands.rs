//! The broker as commands: `plugin.list`, `plugin.capabilities`,
//! `plugin.enable`, `plugin.disable`, `plugin.grant`, `plugin.revoke`,
//! `metadata.read`, `device.list`, `device.info`, `device.capture`,
//! `device.download`, and the commands plugins contribute, each run in
//! its plugin.

use std::path::Path;
use std::sync::Arc;

use helios_shell::commands::{CommandSpec, Origin, Registry};
use serde_json::{Value as Json, json};

use crate::Broker;

fn text<'a>(args: &'a Json, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("`{key}` is missing"))
}

fn optional<'a>(args: &'a Json, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Json::as_str)
}

/// A command's handler.
pub type Handler = Box<dyn Fn(&Json, &Origin) -> Result<Json, String> + Send + Sync>;

/// Grants or takes back a plugin's capability (`plugin.grant`).
pub const GRANT: &str = "plugin.grant";
/// Every plugin's capabilities as rows (`plugin.capabilities`).
pub const CAPABILITIES: &str = "plugin.capabilities";

/// The commands that show and manage the plugins (`plugin.*`), for a
/// registry of their own or the desktop's.
pub fn admin(broker: &Arc<Broker>) -> Vec<(CommandSpec, Handler)> {
    let mut out: Vec<(CommandSpec, Handler)> = Vec::new();
    let b = broker.clone();
    out.push((
        CommandSpec::new(
            "plugin.list",
            "List plugins, their capabilities and state",
            "Plugins",
        )
        .read_only(),
        Box::new(move |_, _| Ok(b.list())),
    ));
    let b = broker.clone();
    out.push((
        CommandSpec::new(
            CAPABILITIES,
            "List every plugin's capabilities (or one's: `plugin`), each with its reason and whether it is granted",
            "Plugins",
        )
        .read_only(),
        Box::new(move |args, _| Ok(capability_rows(&b, args["plugin"].as_str()))),
    ));
    for (id, title, enable) in [
        ("plugin.enable", "Enable a plugin (`plugin`)", true),
        (
            "plugin.disable",
            "Disable a plugin, stopping it (`plugin`)",
            false,
        ),
    ] {
        let b = broker.clone();
        out.push((
            CommandSpec::new(id, title, "Plugins"),
            Box::new(move |args, _| {
                let plugin = text(args, "plugin")?;
                b.set_enabled(plugin, enable)?;
                Ok(json!({"plugin": plugin, "enabled": enable}))
            }),
        ));
    }
    for (id, title, grant) in [
        (
            GRANT,
            "Grant a plugin a capability it declares (`plugin`, `capability`; `granted`: false takes it back)",
            true,
        ),
        (
            "plugin.revoke",
            "Revoke a plugin's capability (`plugin`, `capability`)",
            false,
        ),
    ] {
        let b = broker.clone();
        out.push((
            CommandSpec::new(id, title, "Plugins"),
            Box::new(move |args, _| {
                let plugin = text(args, "plugin")?;
                let capability = text(args, "capability")?;
                let grant = grant && args.get("granted").and_then(Json::as_bool) != Some(false);
                if grant {
                    b.grant(plugin, capability)?;
                } else {
                    b.revoke(plugin, capability)?;
                }
                Ok(json!({"plugin": plugin, "capability": capability, "granted": grant}))
            }),
        ));
    }
    out
}

/// Every plugin's capabilities, one row each (`/capabilities`): `id`
/// (`<plugin> <capability>`), `plugin`, `pluginText` (its title),
/// `capability`, `reason` (its manifest's), `about` (what the capability
/// allows), `granted`.
fn capability_rows(broker: &Broker, only: Option<&str>) -> Json {
    let list = broker.list();
    let mut rows = Vec::new();
    for p in list["plugins"].as_array().into_iter().flatten() {
        if only.is_some_and(|id| p["id"] != id) {
            continue;
        }
        for c in p["capabilities"].as_array().into_iter().flatten() {
            rows.push(json!({
                "id": format!("{} {}", p["id"].as_str().unwrap_or_default(), c["name"].as_str().unwrap_or_default()),
                "plugin": p["id"],
                "pluginText": p["title"],
                "capability": c["name"],
                "reason": c["reason"],
                "about": c["about"],
                "granted": c["granted"],
            }));
        }
    }
    json!({ "capabilities": rows })
}

/// The commands of the plugins' services, run in the plugins serving
/// them: `metadata.read`, `device.list`, `device.info`, `device.capture`,
/// `device.download`.
pub fn services(broker: &Arc<Broker>) -> Vec<(CommandSpec, Handler)> {
    let mut out: Vec<(CommandSpec, Handler)> = Vec::new();
    let b = broker.clone();
    out.push((
        CommandSpec::new(
            "metadata.read",
            "Read an image's metadata through plugins (`image`)",
            "Metadata",
        ),
        Box::new(move |args, _| b.metadata_read(Path::new(text(args, "image")?))),
    ));
    let b = broker.clone();
    out.push((
        CommandSpec::new("device.list", "List connected cameras", "Devices"),
        Box::new(move |_, _| b.device_list()),
    ));
    let b = broker.clone();
    out.push((
        CommandSpec::new(
            "device.info",
            "Describe a camera (`plugin`, `device`)",
            "Devices",
        ),
        Box::new(move |args, _| b.device_info(optional(args, "plugin"), optional(args, "device"))),
    ));
    let b = broker.clone();
    out.push((
        CommandSpec::new(
            "device.capture",
            "Take a picture and download it (`folder`, `plugin`, `device`)",
            "Devices",
        ),
        Box::new(move |args, _| {
            b.device_capture(
                optional(args, "plugin"),
                optional(args, "device"),
                Path::new(text(args, "folder")?),
            )
        }),
    ));
    let b = broker.clone();
    out.push((
        CommandSpec::new(
            "device.download",
            "Download files from a camera (`files`, `folder`, `plugin`, `device`)",
            "Devices",
        ),
        Box::new(move |args, _| {
            let files: Vec<String> = args
                .get("files")
                .and_then(Json::as_array)
                .ok_or("`files` is missing")?
                .iter()
                .filter_map(|f| f.as_str().map(String::from))
                .collect();
            b.device_download(
                optional(args, "plugin"),
                optional(args, "device"),
                &files,
                Path::new(text(args, "folder")?),
            )
        }),
    ));
    out
}

/// The commands the plugins contribute (`[[commands]]`), each run in
/// its plugin, started on `onCommand:<id>` and held to its grants as on
/// any call.
pub fn contributed(broker: &Arc<Broker>) -> Vec<(CommandSpec, Handler)> {
    by_plugin(broker)
        .into_iter()
        .map(|(_, spec, handler)| (spec, handler))
        .collect()
}

/// [`contributed`], each with its plugin's id.
fn by_plugin(broker: &Arc<Broker>) -> Vec<(String, CommandSpec, Handler)> {
    let mut out: Vec<(String, CommandSpec, Handler)> = Vec::new();
    for plugin in broker.ids().map(String::from).collect::<Vec<_>>() {
        let manifest = broker.manifest(&plugin).expect("listed").clone();
        for c in manifest.commands {
            let category = if c.category.is_empty() {
                "Plugins"
            } else {
                &c.category
            };
            let b = broker.clone();
            let (p, id) = (plugin.clone(), c.id.clone());
            out.push((
                plugin.clone(),
                CommandSpec::new(&c.id, &c.title, category),
                Box::new(move |args, _| b.execute(&p, &id, args.clone())),
            ));
        }
    }
    out
}

/// Every plugin command: [`admin`], [`services`], [`contributed`], the
/// ones the desktop declares on its registry.
pub fn all(broker: &Arc<Broker>) -> Vec<(CommandSpec, Handler)> {
    let mut out = admin(broker);
    out.extend(services(broker));
    out.extend(contributed(broker));
    out
}

/// Declares the plugin commands on a registry, over a shared broker. The
/// answer lists the contributed commands left out, because their id was
/// already taken.
pub fn register(registry: &mut Registry, broker: Arc<Broker>) -> Result<Vec<String>, String> {
    for (spec, handler) in admin(&broker).into_iter().chain(services(&broker)) {
        registry.register(spec, handler)?;
    }
    let mut skipped = Vec::new();
    for (plugin, spec, handler) in by_plugin(&broker) {
        if let Err(e) = registry.register(spec, handler) {
            skipped.push(format!("plugin `{plugin}`: {e}"));
        }
    }
    Ok(skipped)
}
