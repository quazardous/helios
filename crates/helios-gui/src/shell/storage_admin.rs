//! The Storage page's reads and edits as commands, for the Settings app's
//! Storage scope (`crate::config::manifest`): its volume table, its
//! statuses and its row actions read and run these, never the page's own
//! widgets.
//!
//! - `library.volumes` (read-only): every volume known, online ones
//!   first, as rows (`/volumes`) with their state's tone, and counts.
//! - `library.backupStrategy` (read-only): the library's backup strategy,
//!   and a summary line with its tone (`/summary`).
//! - `library.volumeRole`: gives a volume a role, or takes it back
//!   (`{"volume", "role", "on"}`; without `on` the role is toggled).
//!
//! The texts in the answers are in the interface's language.

use std::collections::HashMap;
use std::sync::Arc;

use helios_core::Message;
use helios_shell::commands::CommandSpec;
use helios_storage::strategy::{self, Strategy, Trigger};
use helios_storage::{Role, Volume, VolumeId};
use serde_json::{Value, json};

use super::Shell;
use super::storage::{StorageLink, backup_line, destination_text, kind_of, role_text};

/// Lists the volumes known (read-only).
pub const VOLUMES: &str = "library.volumes";

/// The library's backup strategy (read-only).
pub const STRATEGY: &str = "library.backupStrategy";

/// Gives a volume a role or takes it back.
pub const VOLUME_ROLE: &str = "library.volumeRole";

/// Below this share of free space an online volume is said short of it.
const LOW_FREE: f64 = 0.1;

/// A role by its stored name (`backup`).
fn role_named(name: &str) -> Option<Role> {
    serde_json::from_value(json!(name)).ok()
}

fn role_name(role: Role) -> String {
    serde_json::to_value(role)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// The volumes as rows: `id`, `name`, `label`, `kind`, `roles` (their
/// stored names) and `roleText`, `state` (`online`, `offline`) and
/// `stateText`, `tone` (`success` online, `warning` online and short of
/// space, none offline), `mount`, `free` and `capacity` (bytes) and
/// `space`, `photos`, `backup` (whether the strategy copies to it) and
/// `backupText`, `offSite`, `raid`.
pub fn volume_rows(
    t: &dyn Fn(Message) -> String,
    volumes: &[Volume],
    photos: &HashMap<VolumeId, usize>,
    strategy: Option<&Strategy>,
) -> Value {
    let rows: Vec<Value> = volumes
        .iter()
        .map(|v| {
            let online = v.online();
            let short = match (v.free, v.capacity.filter(|&c| c > 0)) {
                (Some(free), Some(capacity)) => (free as f64) < capacity as f64 * LOW_FREE,
                _ => false,
            };
            let tone = match (online, short) {
                (false, _) => Value::Null,
                (true, true) => json!("warning"),
                (true, false) => json!("success"),
            };
            let space = match (v.free, v.capacity.filter(|&c| c > 0)) {
                (Some(free), Some(capacity)) => t(Message::new(
                    "gui-storage-free",
                    "{ $free } free of { $capacity }",
                )
                .with("free", helios_storage::capacity_text(free.min(capacity)))
                .with("capacity", helios_storage::capacity_text(capacity))),
                _ => String::new(),
            };
            let backup =
                strategy.is_some_and(|s| !v.is_card() && s.destinations.iter().any(|d| d.takes(v)));
            let roles: Vec<String> = Role::ALL
                .into_iter()
                .filter(|r| v.roles.contains(r))
                .map(role_name)
                .collect();
            let role_text: Vec<String> = Role::ALL
                .into_iter()
                .filter(|r| v.roles.contains(r))
                .map(|r| t(role_text(r)))
                .collect();
            json!({
                "id": v.id.as_str(),
                "name": v.name,
                "label": v.label,
                "kind": t(kind_of(v.kind).1),
                "roles": roles,
                "roleText": role_text.join(", "),
                "state": if online { "online" } else { "offline" },
                "stateText": if online {
                    t(Message::new("gui-storage-online", "online"))
                } else {
                    t(Message::new("gui-storage-offline", "offline"))
                },
                "tone": tone,
                "mount": v.mount.as_ref().map(|m| m.display().to_string()),
                "free": v.free,
                "capacity": v.capacity,
                "space": space,
                "photos": photos.get(&v.id).copied().unwrap_or(0),
                "backup": backup,
                "backupText": if backup {
                    t(Message::new("gui-storage-backup-destination", "takes copies"))
                } else {
                    String::new()
                },
                "offSite": v.off_site,
                "raid": v.raid,
            })
        })
        .collect();
    let online = volumes.iter().filter(|v| v.online()).count();
    let offline = volumes.len() - online;
    json!({
        "volumes": rows,
        "online": online,
        "offline": offline,
        "summary": {
            "text": t(Message::new(
                "gui-storage-summary",
                "{ $online } online, { $offline } offline",
            )
            .with("online", online as u64)
            .with("offline", offline as u64)),
            "tone": if online > 0 { "success" } else { "info" },
        },
    })
}

/// The strategy as an answer: `set`, and when set its `name`, `copies`,
/// `destinations` (their words), `offSiteRequired`, `triggers` (their
/// stored names), and `online` (its disks plugged in); `summary` is its
/// line with a tone (`success` with a disk plugged in, `warning`
/// without, or no strategy).
pub fn strategy_answer(
    t: &dyn Fn(Message) -> String,
    strategy: Option<&Strategy>,
    volumes: &[Volume],
) -> Value {
    let Some(s) = strategy else {
        return json!({
            "set": false,
            "summary": {
                "text": t(Message::new("gui-storage-strategy-none", "No backup set up")),
                "tone": "warning",
            },
        });
    };
    let online = strategy::destinations_online(s, volumes);
    let (line, _) = backup_line(t, Some(s), volumes);
    let triggers: Vec<Value> = s
        .triggers
        .iter()
        .filter(|tr| **tr != Trigger::ByHand)
        .map(|tr| serde_json::to_value(tr).unwrap_or(Value::Null))
        .collect();
    json!({
        "set": true,
        "name": s.name,
        "copies": s.copies,
        "destinations": s
            .destinations
            .iter()
            .map(|d| destination_text(t, d, volumes))
            .collect::<Vec<_>>(),
        "offSiteRequired": s.off_site_required,
        "triggers": triggers,
        "online": online.len(),
        "summary": {
            "text": line,
            "tone": if online.is_empty() { "warning" } else { "success" },
        },
    })
}

/// `library.volumeRole`'s edit: the volume with the role given or taken
/// back (toggled without `on`).
fn with_role(volumes: &[Volume], args: &Value) -> Result<Volume, String> {
    let id = args["volume"].as_str().ok_or("`volume` is missing")?;
    let role = args["role"]
        .as_str()
        .ok_or("`role` is missing")
        .and_then(|r| {
            role_named(r).ok_or("`role` is library, archive, backup, ingest or scratch")
        })?;
    let mut v = volumes
        .iter()
        .find(|v| v.id.as_str() == id)
        .cloned()
        .ok_or_else(|| format!("no volume `{id}`"))?;
    let on = args["on"].as_bool().unwrap_or(!v.roles.contains(&role));
    if on {
        v.roles.insert(role);
    } else {
        v.roles.remove(&role);
    }
    Ok(v)
}

impl Shell {
    /// Registers the Storage page's commands over `link`, once.
    pub(super) fn register_storage_commands(&mut self, link: &Arc<dyn StorageLink>) {
        if self.has_command(VOLUMES) {
            return;
        }
        let (l, i18n) = (link.clone(), self.i18n.clone());
        let volumes = self.register(
            CommandSpec::new(
                VOLUMES,
                "List the volumes known, online ones first: state, roles, space, photos, backup",
                "Library",
            )
            .read_only(),
            move |_, _| {
                let t = |m: Message| helios_i18n::message(&i18n, &m);
                let volumes = l.volumes()?;
                let photos = l.photos()?;
                let strategy = l.strategy()?;
                Ok(volume_rows(&t, &volumes, &photos, strategy.as_ref()))
            },
        );
        let (l, i18n) = (link.clone(), self.i18n.clone());
        let strategy = self.register(
            CommandSpec::new(
                STRATEGY,
                "Show the library's backup strategy: copies, destinations, triggers, its disks plugged in",
                "Library",
            )
            .read_only(),
            move |_, _| {
                let t = |m: Message| helios_i18n::message(&i18n, &m);
                let volumes = l.volumes()?;
                Ok(strategy_answer(&t, l.strategy()?.as_ref(), &volumes))
            },
        );
        let l = link.clone();
        let role = self.register(
            CommandSpec::new(
                VOLUME_ROLE,
                "Give a volume a role or take it back (`volume`, `role`, `on`; without `on`, toggled)",
                "Library",
            ),
            move |args, _| {
                let v = with_role(&l.volumes()?, args)?;
                let saved = l.save(&v)?;
                let roles: Vec<String> = saved.roles.iter().map(|r| role_name(*r)).collect();
                Ok(json!({ "volume": saved.id.as_str(), "roles": roles }))
            },
        );
        for r in [volumes, strategy, role] {
            if let Err(e) = r {
                log::warn!("the Storage page's commands: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use helios_storage::{Identity, VolumeKind, detect};

    use super::*;

    fn english(m: Message) -> String {
        helios_i18n::message(&helios_i18n::I18n::new("en"), &m)
    }

    fn disk(id: &str, free: u64, mounted: bool, roles: &[Role]) -> Volume {
        Volume {
            id: VolumeId(id.into()),
            identified_by: Identity::Uuid,
            name: id.into(),
            kind: VolumeKind::External,
            capacity: Some(1_000_000_000),
            free: Some(free),
            mount: mounted.then(|| PathBuf::from("/media").join(id)),
            roles: roles.iter().copied().collect(),
            ..detect::weak(id, None)
        }
    }

    #[test]
    fn volumes_are_rows_with_their_state_tone_and_backup() {
        let volumes = [
            disk("studio", 400_000_000, true, &[Role::Library]),
            disk("full", 10_000_000, true, &[Role::Backup]),
            disk("away", 400_000_000, false, &[Role::Backup]),
        ];
        let photos = HashMap::from([(VolumeId("studio".into()), 3)]);
        let s = Strategy::default();
        let a = volume_rows(&english, &volumes, &photos, Some(&s));
        let rows = a["volumes"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["tone"], "success");
        assert_eq!(rows[0]["photos"], 3);
        assert_eq!(rows[0]["roles"], json!(["library"]));
        assert_eq!(rows[0]["backup"], false);
        assert_eq!(rows[1]["tone"], "warning", "short of space");
        assert_eq!(rows[1]["backup"], true);
        assert_eq!(rows[2]["state"], "offline");
        assert!(rows[2]["tone"].is_null());
        assert_eq!(
            (a["online"].as_u64(), a["offline"].as_u64()),
            (Some(2), Some(1))
        );
        assert_eq!(a["summary"]["tone"], "success");
    }

    #[test]
    fn the_strategy_says_its_disks_plugged_in() {
        let none = strategy_answer(&english, None, &[]);
        assert_eq!(none["set"], false);
        assert_eq!(none["summary"]["tone"], "warning");
        let s = Strategy::default();
        let away = [disk("away", 400_000_000, false, &[Role::Backup])];
        let a = strategy_answer(&english, Some(&s), &away);
        assert_eq!(a["copies"], 1);
        assert_eq!(a["online"], 0);
        assert_eq!(a["summary"]["tone"], "warning");
        let plugged = [disk("here", 400_000_000, true, &[Role::Backup])];
        let a = strategy_answer(&english, Some(&s), &plugged);
        assert_eq!(a["summary"]["tone"], "success");
        assert_eq!(a["triggers"], json!(["on_import", "on_connect"]));
    }

    #[test]
    fn a_role_is_given_taken_or_toggled() {
        let volumes = [disk("d", 1, true, &[Role::Archive])];
        let v = with_role(&volumes, &json!({ "volume": "d", "role": "backup" })).unwrap();
        assert!(v.roles.contains(&Role::Backup));
        let v = with_role(&volumes, &json!({ "volume": "d", "role": "archive" })).unwrap();
        assert!(v.roles.is_empty());
        let v = with_role(
            &volumes,
            &json!({ "volume": "d", "role": "archive", "on": true }),
        )
        .unwrap();
        assert!(v.roles.contains(&Role::Archive));
        assert!(with_role(&volumes, &json!({ "volume": "x", "role": "backup" })).is_err());
        assert!(with_role(&volumes, &json!({ "volume": "d", "role": "nope" })).is_err());
    }
}
