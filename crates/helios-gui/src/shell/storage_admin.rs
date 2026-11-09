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
//! - `library.volumeSet`: renames a volume or sets its flags
//!   (`{"volume", "name", "offSite", "raid"}`, each optional).
//! - `library.markVolume`: writes the identity file on a weakly
//!   identified volume plugged in (`{"volume"}`), after the page's
//!   confirmation.
//! - `library.setBackupStrategy`: turns the strategy on or off and sets
//!   one of its parts (`{"key": "on" | "copies" | "destination" |
//!   "offSite" | "onImport" | "onConnect", "value"}`), saved at once.
//! - `library.setUpBackups`: sets the whole strategy at once (`{"copies",
//!   "destination", "offSite", "onImport", "onConnect"}`), the backup
//!   wizard's result.
//!
//! The texts in the answers are in the interface's language.

use std::collections::HashMap;
use std::sync::Arc;

use helios_core::Message;
use helios_shell::commands::CommandSpec;
use helios_storage::strategy::{self, Destination, Strategy, Trigger};
use helios_storage::{Identity, Role, Volume, VolumeId};
use serde_json::{Value, json};

use super::Shell;
use super::storage::{StorageLink, backup_line, day_of, destination_text, kind_of, role_text};

/// Lists the volumes known (read-only).
pub const VOLUMES: &str = "library.volumes";

/// The library's backup strategy (read-only).
pub const STRATEGY: &str = "library.backupStrategy";

/// Gives a volume a role or takes it back.
pub const VOLUME_ROLE: &str = "library.volumeRole";

/// Renames a volume or sets its off-site and RAID flags.
pub const VOLUME_SET: &str = "library.volumeSet";

/// Writes the identity file on a weakly identified volume.
pub const MARK_VOLUME: &str = "library.markVolume";

/// Sets a part of the library's backup strategy.
pub const SET_STRATEGY: &str = "library.setBackupStrategy";

/// Sets the whole backup strategy at once (the backup wizard's finish).
pub const SET_UP_STRATEGY: &str = "library.setUpBackups";

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
/// stored names), `roleText` and `role_<name>` (a boolean per role),
/// `state` (`online`, `offline`), `online` and `stateText`, `tone`
/// (`success` online, `warning` online and short of space, none
/// offline), `mount`, `free` and `capacity` (bytes), `used` (the share
/// used, 0 to 1) and `space`, `photos`, `backup` (whether the strategy
/// copies to it) and `backupText`, `offSite`, `raid`, `seen` (when and
/// where an offline one was last seen, or the node a fixed one is in),
/// `weak` (known only by its label and size, plugged in, not a card:
/// it may be marked).
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
            let used = match (v.free, v.capacity.filter(|&c| c > 0)) {
                (Some(free), Some(capacity)) => {
                    json!((capacity - free.min(capacity)) as f64 / capacity as f64)
                }
                _ => Value::Null,
            };
            let seen = if !online && let Some(seen) = &v.last_seen {
                t(
                    Message::new("gui-storage-last-seen", "Last seen { $date } on { $node }")
                        .with("date", day_of(seen.at))
                        .with("node", seen.node.clone()),
                )
            } else if v.kind.is_fixed()
                && let Some(node) = &v.node
            {
                t(Message::new("gui-storage-node", "In { $node }").with("node", node.clone()))
            } else {
                String::new()
            };
            let mut row = json!({
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
                "online": online,
                "used": used,
                "seen": seen,
                "weak": v.identified_by == Identity::Weak && online && !v.is_card(),
            });
            for r in Role::ALL {
                row[format!("role_{}", role_name(r))] = json!(v.roles.contains(&r));
            }
            row
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
        let text = t(Message::new(
            "gui-storage-strategy-none",
            "No backup set up",
        ));
        return json!({
            "set": false,
            "on": false,
            "summaryText": text,
            "tone": "warning",
            "summary": { "text": text, "tone": "warning" },
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
    let tone = if online.is_empty() {
        "warning"
    } else {
        "success"
    };
    let mut choices = vec![
        Destination::Role(Role::Backup),
        Destination::Role(Role::Archive),
    ];
    choices.extend(
        volumes
            .iter()
            .filter(|v| strategy::may_back_up_to(v).is_ok())
            .map(|v| Destination::Volume(v.id.clone())),
    );
    let first = s
        .destinations
        .first()
        .cloned()
        .unwrap_or(Destination::Role(Role::Backup));
    json!({
        "set": true,
        "on": true,
        "summaryText": line,
        "tone": tone,
        "destination": destination_value(&first),
        "destinationChoices": choices
            .iter()
            .map(|d| json!({ "value": destination_value(d), "label": destination_text(t, d, volumes) }))
            .collect::<Vec<_>>(),
        "offSite": s.off_site_required,
        "onImport": s.triggers.contains(&Trigger::OnImport),
        "onConnect": s.triggers.contains(&Trigger::OnConnect),
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
        "summary": { "text": line, "tone": tone },
    })
}

/// A destination as a choice's value: `backup`, `archive` (any disk of
/// the role) or `volume:<id>`.
fn destination_value(d: &Destination) -> String {
    match d {
        Destination::Role(r) => role_name(*r),
        Destination::Volume(id) => format!("volume:{}", id.as_str()),
    }
}

fn destination_named(value: &str) -> Option<Destination> {
    match value.strip_prefix("volume:") {
        Some(id) => Some(Destination::Volume(VolumeId(id.into()))),
        None => role_named(value).map(Destination::Role),
    }
}

/// `library.setBackupStrategy`'s edit: the strategy with one part set
/// (`on` turns it on, the default one, or off).
fn with_strategy(current: Option<&Strategy>, args: &Value) -> Result<Option<Strategy>, String> {
    let key = args["key"].as_str().ok_or("`key` is missing")?;
    let value = &args["value"];
    let flag = || {
        value
            .as_bool()
            .ok_or_else(|| format!("`{key}` takes true or false"))
    };
    if key == "on" {
        return Ok(flag()?.then(|| current.cloned().unwrap_or_default()));
    }
    let mut s = current
        .cloned()
        .ok_or("no backup strategy: turn it on first")?;
    match key {
        "copies" => {
            let n = value
                .as_u64()
                .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
                .filter(|n| (1..=5).contains(n))
                .ok_or("`copies` is 1 to 5")?;
            s.copies = n as u8;
        }
        "destination" => {
            let d = value
                .as_str()
                .and_then(destination_named)
                .ok_or("`destination` is backup, archive or volume:<id>")?;
            s.destinations = vec![d];
        }
        "offSite" => s.off_site_required = flag()?,
        "onImport" | "onConnect" => {
            let trigger = if key == "onImport" {
                Trigger::OnImport
            } else {
                Trigger::OnConnect
            };
            if flag()? {
                s.triggers.insert(trigger);
            } else {
                s.triggers.remove(&trigger);
            }
        }
        other => return Err(format!("no strategy part `{other}`")),
    }
    Ok(Some(s))
}

/// `library.setUpBackups`'s strategy: every part given at once (`copies`
/// 1 to 5, `destination`; `offSite`, `onImport` and `onConnect` false
/// when left out or null), made by hand too.
fn strategy_of(args: &Value) -> Result<Strategy, String> {
    let copies = args["copies"]
        .as_u64()
        .or_else(|| args["copies"].as_str().and_then(|v| v.parse().ok()))
        .filter(|n| (1..=5).contains(n))
        .ok_or("`copies` is 1 to 5")?;
    let destination = args["destination"]
        .as_str()
        .and_then(destination_named)
        .ok_or("`destination` is backup, archive or volume:<id>")?;
    let flag = |key: &str| match &args[key] {
        Value::Null => Ok(false),
        v => v
            .as_bool()
            .ok_or_else(|| format!("`{key}` takes true or false")),
    };
    let mut triggers = std::collections::BTreeSet::from([Trigger::ByHand]);
    if flag("onImport")? {
        triggers.insert(Trigger::OnImport);
    }
    if flag("onConnect")? {
        triggers.insert(Trigger::OnConnect);
    }
    Ok(Strategy {
        copies: copies as u8,
        destinations: vec![destination],
        off_site_required: flag("offSite")?,
        triggers,
        ..Strategy::default()
    })
}

/// `library.volumeSet`'s edit: the volume renamed or its flags set.
fn with_set(volumes: &[Volume], args: &Value) -> Result<Volume, String> {
    let mut v = volume_of(volumes, args)?;
    if let Some(name) = args.get("name").and_then(Value::as_str) {
        let name = name.trim();
        if name.is_empty() {
            return Err("a volume's name is not empty".into());
        }
        v.name = name.into();
    }
    if let Some(on) = args.get("offSite").and_then(Value::as_bool) {
        v.off_site = on;
    }
    if let Some(on) = args.get("raid").and_then(Value::as_bool) {
        v.raid = on;
    }
    Ok(v)
}

fn volume_of(volumes: &[Volume], args: &Value) -> Result<Volume, String> {
    let id = args["volume"].as_str().ok_or("`volume` is missing")?;
    volumes
        .iter()
        .find(|v| v.id.as_str() == id)
        .cloned()
        .ok_or_else(|| format!("no volume `{id}`"))
}

/// "3 days ago", "today": how old a snapshot taken at `taken` is.
pub fn age_text(t: &dyn Fn(Message) -> String, taken: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    match now.saturating_sub(taken) / 86_400 {
        0 => t(Message::new("gui-catalogue-today", "today")),
        1 => t(Message::new("gui-catalogue-one-day", "1 day ago")),
        days => t(Message::new("gui-catalogue-days", "{ $days } days ago").with("days", days)),
    }
}

/// The catalogue's snapshots as rows (`library.catalogue.snapshots`):
/// each with `whenText` (how old) and `placeText` (next to the
/// catalogue, or on a disk).
pub fn snapshot_rows(t: &dyn Fn(Message) -> String, list: Value) -> Value {
    let Value::Array(list) = list else {
        return list;
    };
    Value::Array(
        list.into_iter()
            .map(|mut s| {
                s["whenText"] = json!(age_text(t, s["taken"].as_u64().unwrap_or(0)));
                s["placeText"] = json!(match s["disk"].as_str() {
                    Some(disk) => t(Message::new("gui-catalogue-on-disk", "on { $disk }")
                        .with("disk", disk.to_string())),
                    None => t(Message::new("gui-catalogue-here", "next to the catalogue")),
                });
                s
            })
            .collect(),
    )
}

/// `library.volumeRole`'s edit: the volume with the role given or taken
/// back (toggled without `on`).
fn with_role(volumes: &[Volume], args: &Value) -> Result<Volume, String> {
    let role = args["role"]
        .as_str()
        .ok_or("`role` is missing")
        .and_then(|r| {
            role_named(r).ok_or("`role` is library, archive, backup, ingest or scratch")
        })?;
    let mut v = volume_of(volumes, args)?;
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
        let l = link.clone();
        let set = self.register(
            CommandSpec::new(
                VOLUME_SET,
                "Rename a volume or set its flags (`volume`, and `name`, `offSite`, `raid`)",
                "Library",
            ),
            move |args, _| {
                let saved = l.save(&with_set(&l.volumes()?, args)?)?;
                Ok(json!({ "volume": saved.id.as_str(), "name": saved.name, "offSite": saved.off_site, "raid": saved.raid }))
            },
        );
        let l = link.clone();
        let mark = self.register(
            CommandSpec::new(
                MARK_VOLUME,
                "Write the identity file (.helios/volume.json) on a volume known only by its label and size, plugged in (`volume`)",
                "Library",
            ),
            move |args, _| {
                let v = volume_of(&l.volumes()?, args)?;
                if v.identified_by != Identity::Weak || !v.online() || v.is_card() {
                    return Err(format!("{} is not a disk to mark", v.name));
                }
                let marked = l.mark(&v)?;
                Ok(json!({ "volume": marked.id.as_str() }))
            },
        );
        let l = link.clone();
        let set_strategy = self.register(
            CommandSpec::new(
                SET_STRATEGY,
                "Set the library's backup strategy (`key`: on, copies, destination, offSite, onImport or onConnect; `value`)",
                "Library",
            ),
            move |args, _| {
                let s = with_strategy(l.strategy()?.as_ref(), args)?;
                l.set_strategy(s.as_ref())?;
                Ok(json!({ "set": s.is_some() }))
            },
        );
        let l = link.clone();
        let set_up = self.register(
            CommandSpec::new(
                SET_UP_STRATEGY,
                "Set up the library's backups at once (`copies`, `destination`, `offSite`, `onImport`, `onConnect`): the backup wizard's result",
                "Library",
            ),
            move |args, _| {
                let s = strategy_of(args)?;
                l.set_strategy(Some(&s))?;
                Ok(json!({ "set": true, "copies": s.copies }))
            },
        );
        for r in [volumes, strategy, role, set, mark, set_strategy, set_up] {
            if let Err(e) = r {
                log::warn!("the Storage page's commands: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use helios_storage::{VolumeKind, detect};

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

    #[test]
    fn a_volume_is_renamed_and_its_flags_set() {
        let volumes = [disk("d", 1, true, &[])];
        let v = with_set(
            &volumes,
            &json!({ "volume": "d", "name": " Studio ", "raid": true }),
        )
        .unwrap();
        assert_eq!(v.name, "Studio");
        assert!(v.raid && !v.off_site);
        assert!(with_set(&volumes, &json!({ "volume": "d", "name": " " })).is_err());
        let rows = volume_rows(&english, &volumes, &HashMap::new(), None);
        let row = &rows["volumes"][0];
        assert_eq!(row["role_backup"], false);
        assert_eq!(row["online"], true);
        assert!(row["used"].as_f64().unwrap() > 0.99);
    }

    #[test]
    fn the_strategy_is_turned_on_and_its_parts_set() {
        let on = with_strategy(None, &json!({ "key": "on", "value": true }))
            .unwrap()
            .unwrap();
        assert_eq!(on, Strategy::default());
        assert!(with_strategy(None, &json!({ "key": "copies", "value": 2 })).is_err());
        let s = with_strategy(Some(&on), &json!({ "key": "copies", "value": "3" }))
            .unwrap()
            .unwrap();
        assert_eq!(s.copies, 3);
        assert!(with_strategy(Some(&on), &json!({ "key": "copies", "value": 9 })).is_err());
        let s = with_strategy(
            Some(&s),
            &json!({ "key": "destination", "value": "volume:d" }),
        )
        .unwrap()
        .unwrap();
        assert_eq!(s.destinations, [Destination::Volume(VolumeId("d".into()))]);
        let s = with_strategy(Some(&s), &json!({ "key": "onImport", "value": false }))
            .unwrap()
            .unwrap();
        assert!(!s.triggers.contains(&Trigger::OnImport));
        assert!(
            with_strategy(Some(&s), &json!({ "key": "on", "value": false }))
                .unwrap()
                .is_none()
        );
        let a = strategy_answer(&english, Some(&s), &[disk("d", 1, true, &[Role::Backup])]);
        assert_eq!(a["destination"], "volume:d");
        assert_eq!(a["destinationChoices"].as_array().unwrap().len(), 3);
        assert_eq!(a["onImport"], false);
    }

    #[test]
    fn the_whole_strategy_is_set_at_once() {
        let s = strategy_of(&json!({
            "copies": 2, "destination": "archive", "offSite": true,
            "onImport": true, "onConnect": null
        }))
        .unwrap();
        assert_eq!(s.copies, 2);
        assert_eq!(s.destinations, [Destination::Role(Role::Archive)]);
        assert!(s.off_site_required);
        assert!(s.triggers.contains(&Trigger::OnImport));
        assert!(!s.triggers.contains(&Trigger::OnConnect));
        assert!(s.triggers.contains(&Trigger::ByHand));
        let s = strategy_of(&json!({ "copies": "1", "destination": "backup" })).unwrap();
        assert!(!s.off_site_required);
        assert!(strategy_of(&json!({ "copies": 0, "destination": "backup" })).is_err());
        assert!(strategy_of(&json!({ "copies": 2 })).is_err());
        assert!(
            strategy_of(&json!({ "copies": 2, "destination": "backup", "offSite": 3 })).is_err()
        );
    }

    #[test]
    fn snapshots_say_their_age_and_place() {
        let rows = snapshot_rows(
            &english,
            json!([{ "file": "a", "taken": 0, "reason": "daily", "disk": "Backup" }]),
        );
        assert_eq!(rows[0]["placeText"], "on Backup");
        assert!(rows[0]["whenText"].as_str().unwrap().ends_with("days ago"));
    }
}
