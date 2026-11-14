//! The init steps on the node (`helios_init`): `init.steps` lists every
//! step modules declare, in order, and `init.status` says which are done
//! over the vault, and what each module still misses. An agent driving
//! Helios headless reads them, then fills a step with `init.set` (the
//! settings it fills, `{"values": {"library.folder": "/p/Helios"}}`,
//! each checked as its keeper does: the library folder is made if
//! missing, written in, with room left), or on a desktop by running its
//! command (`init.libraryFolder` with a `path`).

use std::path::PathBuf;

use helios_init::{LIBRARY_FOLDER_SETTING, Registry, Values};
use helios_vault::{Store, Vault};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, Need, object};

/// Lists the init steps.
pub const STEPS: &str = "init.steps";

/// Says which init steps are done.
pub const STATUS: &str = "init.status";

/// Fills init steps: sets the settings they fill.
pub const SET: &str = "init.set";

/// Whether `id` is one of the commands run here.
pub fn handles(id: &str) -> bool {
    id == STEPS || id == STATUS || id == SET
}

/// The commands' specs.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(
            STEPS,
            "List the init steps modules declare, in order",
            "Init",
        )
        .read_only(),
        CommandSpec::new(
            STATUS,
            "Say which init steps are done, and what each module misses (`module`: one only)",
            "Init",
        )
        .params(object(&[("module", json!({ "type": "string" }), false)]))
        .needs(Need::Vault)
        .read_only(),
        CommandSpec::new(
            SET,
            "Fill init steps: set the settings they fill (`values`: setting to value, such as `library.folder`)",
            "Init",
        )
        .params(object(&[(
            "values",
            json!({ "type": "object" }),
            true,
        )]))
        .needs(Need::Vault),
    ]
}

/// The settings the init steps read, from the vault: the library folder
/// is the library's (`helios_library::import::library_root`).
pub struct VaultValues<'a, S: Store>(pub &'a Vault<S>);

impl<S: Store> Values for VaultValues<'_, S> {
    fn value(&self, key: &str) -> Option<Value> {
        match key {
            LIBRARY_FOLDER_SETTING => helios_library::import::library_root(self.0)
                .inspect_err(|e| log::warn!("init: library folder: {e}"))
                .ok()
                .flatten()
                .map(|p: PathBuf| json!(p.display().to_string())),
            _ => None,
        }
    }
}

impl<S: Store> VaultValues<'_, S> {
    /// Sets setting `key`, checked as its keeper does: the library folder
    /// must be a full path Helios can write in, with room left.
    ///
    /// # Errors
    /// A setting no init step fills here, a value of the wrong type, or
    /// the keeper's refusal.
    pub fn set(&self, key: &str, value: &Value) -> Result<(), String> {
        match key {
            LIBRARY_FOLDER_SETTING => {
                let path = value
                    .as_str()
                    .filter(|p| !p.trim().is_empty())
                    .ok_or("`library.folder`: a folder's path")?;
                let dir = PathBuf::from(path);
                helios_library::import::check_library_folder(&dir)
                    .map_err(|p| format!("`library.folder`: {}", p.message().english))?;
                helios_library::import::set_library_root(self.0, &dir)
            }
            _ => Err(format!("`{key}` is not a setting an init step fills")),
        }
    }
}

/// Runs command `id` (params already checked against its schema) on
/// `vault`.
///
/// # Errors
/// An unknown command or module, or steps declared in a loop.
pub fn run<S: Store>(vault: &Vault<S>, id: &str, params: &Value) -> Result<Value, String> {
    let registry = Registry::builtin();
    match id {
        STEPS => {
            let steps: Vec<Value> = registry
                .ordered()?
                .into_iter()
                .map(|s| {
                    let mut v = serde_json::to_value(s).expect("steps serialize");
                    v["owner"] = json!(registry.owner(&s.id));
                    v
                })
                .collect();
            Ok(json!({ "steps": steps, "requires": registry.requirements() }))
        }
        STATUS => {
            let values = VaultValues(vault);
            let modules: Vec<String> = match params["module"].as_str() {
                Some(m) if registry.requirements().contains_key(m) => vec![m.to_owned()],
                Some(m) => return Err(format!("module `{m}` requires no init step")),
                None => registry.requirements().keys().cloned().collect(),
            };
            let mut missing = serde_json::Map::new();
            for m in modules {
                missing.insert(m.clone(), json!(registry.missing(&m, &values)?));
            }
            Ok(json!({ "steps": registry.status(&values)?, "missing": missing }))
        }
        SET => {
            let values = VaultValues(vault);
            let given = params["values"].as_object().ok_or("`values`: an object")?;
            for (key, value) in given {
                values.set(key, value)?;
            }
            Ok(json!({ "steps": registry.status(&values)? }))
        }
        _ => Err(format!("no init command `{id}`")),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use helios_core::Bus;
    use helios_init::{CHOOSE_LIBRARY_FOLDER, IMPORT, LIBRARY_FOLDER};
    use helios_vault_sqlite::SqliteStore;

    use super::*;

    /// The library folder step's command is the health warning's fix.
    #[test]
    fn the_folder_step_is_filled_by_the_health_fix() {
        assert_eq!(
            CHOOSE_LIBRARY_FOLDER,
            helios_health::checks::CHOOSE_LIBRARY_FOLDER
        );
        assert_eq!(LIBRARY_FOLDER, helios_health::checks::FOLDER_CHOSEN);
    }

    /// `init.steps` and `init.status` answer through the node's commands;
    /// the folder step flips as the library folder is set and cleared.
    #[test]
    fn the_init_commands_answer_and_follow_the_library_folder() {
        let dir = std::env::temp_dir().join(format!("helios-node-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let vault = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap());
        let bus = Arc::new(Bus::new());
        let run = |id: &str, params: Value| {
            crate::endpoint::command(&vault, &bus, id, &params, "agent:test")
                .unwrap()
                .0
        };

        let steps = run(STEPS, json!({}));
        assert_eq!(steps["steps"][0]["id"], LIBRARY_FOLDER);
        assert_eq!(steps["steps"][0]["owner"], "library");
        assert_eq!(steps["steps"][0]["command"], CHOOSE_LIBRARY_FOLDER);
        assert_eq!(steps["steps"][0]["required"], true);
        assert_eq!(steps["requires"][IMPORT][0], LIBRARY_FOLDER);

        let status = run(STATUS, json!({}));
        assert_eq!(status["steps"][0]["done"], false);
        assert_eq!(status["missing"][IMPORT], json!([LIBRARY_FOLDER]));

        helios_library::import::set_library_root(&vault, &dir.join("Photos")).unwrap();
        let status = run(STATUS, json!({ "module": IMPORT }));
        assert_eq!(status["steps"][0]["done"], true);
        assert_eq!(status["missing"][IMPORT], json!([]));

        helios_library::import::set_library_root(&vault, std::path::Path::new("")).unwrap();
        let status = run(STATUS, json!({}));
        assert_eq!(status["steps"][0]["done"], false, "cleared: missing again");

        assert!(
            crate::endpoint::command(&vault, &bus, STATUS, &json!({ "module": "nope" }), "cli")
                .is_err()
        );

        // An agent fills the step with `init.set`: the folder is made,
        // checked writable, and the step is done.
        let folder = dir.join("Pictures").join("Helios");
        let set = run(
            SET,
            json!({ "values": { LIBRARY_FOLDER_SETTING: folder.display().to_string() } }),
        );
        assert_eq!(set["steps"][0]["done"], true, "{set}");
        assert!(folder.is_dir(), "made");
        assert_eq!(
            helios_library::import::library_root(&vault).unwrap(),
            Some(folder)
        );
        let refused = |values: Value| {
            crate::endpoint::command(&vault, &bus, SET, &json!({ "values": values }), "cli")
                .unwrap_err()
        };
        assert!(
            refused(json!({ LIBRARY_FOLDER_SETTING: "relative/Helios" })).contains("full path")
        );
        assert!(refused(json!({ LIBRARY_FOLDER_SETTING: "" })).contains("folder"));
        assert!(refused(json!({ "nope.key": "x" })).contains("not a setting"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
