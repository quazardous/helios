//! The broker as commands: `plugin.list`, `plugin.enable`,
//! `plugin.disable`, `plugin.grant`, `plugin.revoke`, `metadata.read`,
//! `device.list`, `device.info`, `device.capture`, `device.download`, and
//! the commands plugins contribute, each run in its plugin.

use std::path::Path;
use std::sync::Arc;

use helios_shell::commands::{CommandSpec, Registry};
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

/// Declares the plugin commands on a registry, over a shared broker. The
/// answer lists the contributed commands left out, because their id was
/// already taken.
pub fn register(registry: &mut Registry, broker: Arc<Broker>) -> Result<Vec<String>, String> {
    let b = broker.clone();
    registry.register(
        CommandSpec::new(
            "plugin.list",
            "List plugins, their capabilities and state",
            "Plugins",
        ),
        move |_, _| Ok(b.list()),
    )?;
    for (id, title, enable) in [
        ("plugin.enable", "Enable a plugin (`plugin`)", true),
        (
            "plugin.disable",
            "Disable a plugin, stopping it (`plugin`)",
            false,
        ),
    ] {
        let b = broker.clone();
        registry.register(CommandSpec::new(id, title, "Plugins"), move |args, _| {
            let plugin = text(args, "plugin")?;
            b.set_enabled(plugin, enable)?;
            Ok(json!({"plugin": plugin, "enabled": enable}))
        })?;
    }
    for (id, title, grant) in [
        (
            "plugin.grant",
            "Grant a plugin a capability it declares (`plugin`, `capability`)",
            true,
        ),
        (
            "plugin.revoke",
            "Revoke a plugin's capability (`plugin`, `capability`)",
            false,
        ),
    ] {
        let b = broker.clone();
        registry.register(CommandSpec::new(id, title, "Plugins"), move |args, _| {
            let plugin = text(args, "plugin")?;
            let capability = text(args, "capability")?;
            if grant {
                b.grant(plugin, capability)?;
            } else {
                b.revoke(plugin, capability)?;
            }
            Ok(json!({"plugin": plugin, "capability": capability, "granted": grant}))
        })?;
    }
    let b = broker.clone();
    registry.register(
        CommandSpec::new(
            "metadata.read",
            "Read an image's metadata through plugins (`image`)",
            "Metadata",
        ),
        move |args, _| b.metadata_read(Path::new(text(args, "image")?)),
    )?;
    let b = broker.clone();
    registry.register(
        CommandSpec::new("device.list", "List connected cameras", "Devices"),
        move |_, _| b.device_list(),
    )?;
    let b = broker.clone();
    registry.register(
        CommandSpec::new(
            "device.info",
            "Describe a camera (`plugin`, `device`)",
            "Devices",
        ),
        move |args, _| b.device_info(optional(args, "plugin"), optional(args, "device")),
    )?;
    let b = broker.clone();
    registry.register(
        CommandSpec::new(
            "device.capture",
            "Take a picture and download it (`folder`, `plugin`, `device`)",
            "Devices",
        ),
        move |args, _| {
            b.device_capture(
                optional(args, "plugin"),
                optional(args, "device"),
                Path::new(text(args, "folder")?),
            )
        },
    )?;
    let b = broker.clone();
    registry.register(
        CommandSpec::new(
            "device.download",
            "Download files from a camera (`files`, `folder`, `plugin`, `device`)",
            "Devices",
        ),
        move |args, _| {
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
        },
    )?;
    let mut skipped = Vec::new();
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
            let declared = registry.register(
                CommandSpec::new(&c.id, &c.title, category),
                move |args, _| b.execute(&p, &id, args.clone()),
            );
            if let Err(e) = declared {
                skipped.push(format!("plugin `{plugin}`: {e}"));
            }
        }
    }
    Ok(skipped)
}
