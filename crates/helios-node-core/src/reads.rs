//! The reads the desktop's worker threads ask the node for, so none of
//! them holds the vault: an export's plan and the versions its renders
//! apply, and what the import window's prefilter and offline listings
//! know of the library (`docs/protocol.md`, "Worker reads").
//!
//! - `library.version.get` (`version`) reads one version, its edit
//!   included; null when the vault has none of that id.
//! - `export.plan` (`images`, `settings`, `node`) plans an export over
//!   the library (`helios_library::export::plan`): the batch request a
//!   run takes.
//! - `import.known` reads the library's images as `[size, partial]` and
//!   the present copies as `[image, path, size, modified]`.
//! - `import.recorded` (`folder`) reads what the image map saw under a
//!   folder, on the known volume last mounted above it.

use std::path::{Path, PathBuf};

use helios_vault::{ImageId, Store, Vault, VersionId};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, Need, object};

/// Reads one version of an image, by id.
pub const VERSION_GET: &str = "library.version.get";

/// Plans an export over the library.
pub const EXPORT_PLAN: &str = "export.plan";

/// Reads what the import window's prefilter knows of the library.
pub const IMPORT_KNOWN: &str = "import.known";

/// Reads what the image map recorded under a folder.
pub const IMPORT_RECORDED: &str = "import.recorded";

/// The commands, in [`specs`]' order.
pub const IDS: &[&str] = &[VERSION_GET, EXPORT_PLAN, IMPORT_KNOWN, IMPORT_RECORDED];

/// Whether `id` is one of the commands run here.
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// The commands' specs.
pub fn specs() -> Vec<CommandSpec> {
    let spec = |id: &str, label: &str, category: &str| {
        CommandSpec::new(id, label, category)
            .needs(Need::Vault)
            .read_only()
    };
    let absolute = || json!({ "type": "string", "description": "an absolute path" });
    vec![
        spec(VERSION_GET, "Read one version of an image", "Library").params(object(&[(
            "version",
            json!({ "type": "string", "description": "a version's id (64 hex digits)" }),
            true,
        )])),
        spec(EXPORT_PLAN, "Plan an export over the library", "Export").params(object(&[
            (
                "images",
                json!({ "type": "array", "items": { "type": "string" },
                        "description": "image ids, in the export's order" }),
                true,
            ),
            (
                "settings",
                json!({ "type": "object", "description": "the export's settings" }),
                true,
            ),
            (
                "node",
                json!({ "type": ["string", "null"], "description": "the graph node rendered" }),
                false,
            ),
        ])),
        spec(
            IMPORT_KNOWN,
            "Read the library's images and present copies, for an import's prefilter",
            "Import",
        ),
        spec(
            IMPORT_RECORDED,
            "Read what the image map saw under a folder",
            "Import",
        )
        .params(object(&[("folder", absolute(), true)])),
    ]
}

/// Runs command `id` (params already checked against its schema) on
/// `vault`.
///
/// # Errors
/// The vault's refusal, or params that do not read.
pub fn run<S: Store>(vault: &Vault<S>, id: &str, params: &Value) -> Result<Value, String> {
    match id {
        VERSION_GET => {
            let id = VersionId::from_hex(params["version"].as_str().unwrap_or_default())?;
            Ok(json!({ "version": vault.version(&id)? }))
        }
        EXPORT_PLAN => {
            let images = params["images"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| ImageId::from_hex(i.as_str().unwrap_or_default()))
                .collect::<Result<Vec<_>, _>>()?;
            let settings = serde_json::from_value(params["settings"].clone())
                .map_err(|e| format!("`settings`: {e}"))?;
            let node = params["node"].as_str().map(str::to_string);
            let request = helios_library::export::plan(vault, &images, settings, node)?;
            Ok(json!({ "request": request }))
        }
        IMPORT_KNOWN => {
            let partials: Vec<Value> = vault
                .images()?
                .into_iter()
                .map(|i| json!([i.size, i.partial]))
                .collect();
            let present: Vec<Value> = vault
                .all_locations()?
                .into_iter()
                .filter(|(_, l)| l.present())
                .map(|(id, l)| json!([id.hex(), l.path, l.size, l.modified]))
                .collect();
            Ok(json!({ "partials": partials, "present": present }))
        }
        IMPORT_RECORDED => {
            let folder = PathBuf::from(params["folder"].as_str().unwrap_or_default());
            recorded(vault, &folder)
        }
        other => Err(format!("no command `{other}` here")),
    }
}

/// The sightings of image files under `folder`, on the known volume last
/// mounted above it (its mount answered as `mount`), else on no volume.
fn recorded<S: Store>(vault: &Vault<S>, folder: &Path) -> Result<Value, String> {
    let volumes = helios_storage::registry::known(vault)?;
    let on = volumes
        .iter()
        .filter_map(|v| {
            let mount = v.last_seen.as_ref()?.mount.as_ref()?;
            let rel = folder.strip_prefix(mount).ok()?;
            Some((v, mount.clone(), rel.to_path_buf()))
        })
        .max_by_key(|(_, mount, _)| mount.components().count());
    let (volume, mount, rel) = match on {
        Some((v, mount, rel)) => (Some(v.id.clone()), Some(mount), rel),
        None => (None, None, folder.to_path_buf()),
    };
    let sightings: Vec<_> = vault
        .sightings_under(volume.as_ref(), &rel)?
        .into_iter()
        .filter(|s| {
            s.path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| {
                    helios_vault::IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())
                })
        })
        .collect();
    Ok(json!({ "mount": mount, "sightings": sightings }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::validate;

    #[test]
    fn every_worker_read_is_declared_once_with_a_schema_its_example_fits() {
        let ids: Vec<String> = specs().iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, IDS);
        let examples = [
            json!({ "version": "0a" }),
            json!({ "images": ["0a"], "settings": {}, "node": "export" }),
            json!({}),
            json!({ "folder": "/media/card/DCIM" }),
        ];
        assert_eq!(examples.len(), IDS.len(), "one example per command");
        for (spec, example) in specs().iter().zip(examples) {
            assert!(spec.read_only, "{}", spec.id);
            validate(&spec.params, &example)
                .unwrap_or_else(|e| panic!("{}: its example does not fit: {e}", spec.id));
        }
    }

    #[test]
    fn the_worker_reads_answer_from_the_vault() {
        let dir = std::env::temp_dir().join(format!("helios-reads-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.jpg");
        std::fs::write(&file, b"not much of a picture").unwrap();
        let vault = Vault::new(helios_vault::MemoryStore::default());
        let image = vault.import_file(&file, 1).unwrap();

        let known = run(&vault, IMPORT_KNOWN, &json!({})).unwrap();
        assert_eq!(known["partials"].as_array().unwrap().len(), 1, "{known}");
        assert_eq!(known["present"][0][0], image.hex(), "{known}");
        assert_eq!(known["present"][0][1], json!(file), "{known}");

        let none = run(&vault, VERSION_GET, &json!({ "version": "0".repeat(64) })).unwrap();
        assert_eq!(none["version"], Value::Null);

        let recorded = run(
            &vault,
            IMPORT_RECORDED,
            &json!({ "folder": dir.join("none") }),
        )
        .unwrap();
        assert_eq!(recorded["sightings"], json!([]), "{recorded}");
        assert_eq!(recorded["mount"], Value::Null);

        let output = helios_export::Output::from(helios_export::Format::Tiff);
        let settings = json!({ "output": output, "folder": dir.join("out") });
        let plan = run(
            &vault,
            EXPORT_PLAN,
            &json!({ "images": [image.hex()], "settings": settings }),
        )
        .unwrap();
        let item = &plan["request"]["items"][0];
        assert_eq!(item["image"], image.hex(), "{plan}");
        assert_eq!(item["source"], json!(file), "{plan}");
        let refused = run(
            &vault,
            EXPORT_PLAN,
            &json!({ "images": [], "settings": {} }),
        );
        assert!(refused.unwrap_err().contains("`settings`"));
        drop(std::fs::remove_dir_all(&dir));
    }
}
