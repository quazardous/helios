//! Reads at scale: the library's photos read through cursors on the node
//! (see `docs/protocol.md`, "Cursors").
//!
//! `cursor.open` runs a query once and keeps its ids on the node, an
//! ordered snapshot ([`helios_vault::catalog`]); `cursor.window` reads a
//! page of it with the fields asked for, `cursor.rank` where a photo
//! stands in it, `cursor.refresh` brings it up to date and says what moved,
//! `cursor.close` lets it go. A client never pulls the whole collection:
//! a screen is one window.
//!
//! The snapshot is what makes paging stable: a write during the paging
//! never shifts the pages, so each photo of the snapshot is read once.
//! A window says whether the catalog moved since the snapshot (`stale`);
//! the client then refreshes the cursor (the inserted and removed photos)
//! or reopens it.
//!
//! A cursor is a lease: one not read for its lease (60 seconds unless the
//! client asks for another) is closed by the node, so a client that went
//! away holds no memory. Reading it again answers [`GONE`], and the
//! client reopens it from its query.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use helios_inspect::rpc::{Call, Error};
use helios_vault::fields::{BackupFilter, BackupState, BackupVolume, Backups};
use helios_vault::storage::VolumeId;
use helios_vault::{CursorId, ImageId, Location, Order, Presence, Query, Sort, Store, Vault};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, Need, object};

/// The ids of the cursor commands.
pub const IDS: &[&str] = &[
    "cursor.open",
    "cursor.window",
    "cursor.rank",
    "cursor.refresh",
    "cursor.close",
];

/// The error of a cursor the node does not hold: never opened, closed, or
/// its lease ran out. Reopen it from its query.
pub const GONE: i64 = -32007;

/// How long a cursor lives unread, unless its client asks otherwise.
pub const LEASE: Duration = Duration::from_mins(1);

/// The longest lease a client may ask for.
pub const MAX_LEASE: Duration = Duration::from_hours(1);

/// The most rows one window reads.
pub const MAX_WINDOW: usize = 10_000;

/// Rows read between two looks at the call's cancellation: one store
/// query each.
pub const CHUNK: usize = 500;

/// Whether `id` is a cursor command.
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// The cursor commands' specs.
pub fn specs() -> Vec<CommandSpec> {
    let cursor = || ("cursor", json!({ "type": "integer", "minimum": 1 }), true);
    let spec = |id: &str, label: &str| CommandSpec::new(id, label, "Library").needs(Need::Vault);
    vec![
        spec("cursor.open", "Open a cursor on the library's photos")
            .params(object(&[
                (
                    "query",
                    json!({ "type": "object",
                            "description": "which photos, in which order (docs/protocol.md, Cursors)" }),
                    false,
                ),
                (
                    "lease",
                    json!({ "type": "integer", "minimum": 1, "maximum": MAX_LEASE.as_secs(),
                            "description": "seconds the cursor lives unread" }),
                    false,
                ),
            ]))
            .read_only(),
        spec("cursor.window", "Read a page of a cursor")
            .params(object(&[
                cursor(),
                ("offset", json!({ "type": "integer", "minimum": 0 }), true),
                (
                    "len",
                    json!({ "type": "integer", "minimum": 0, "maximum": MAX_WINDOW }),
                    true,
                ),
                (
                    "fields",
                    json!({ "type": "array", "items": { "type": "string" } }),
                    false,
                ),
                (
                    "locations",
                    json!({ "type": "boolean",
                            "description": "each row with where its copies are (`locations`)" }),
                    false,
                ),
            ]))
            .read_only(),
        spec("cursor.rank", "Where a photo stands in a cursor")
            .params(object(&[cursor(), ("image", json!({ "type": "string" }), true)]))
            .read_only(),
        spec("cursor.refresh", "Bring a cursor up to date")
            .params(object(&[cursor()]))
            .read_only(),
        spec("cursor.close", "Close a cursor")
            .params(object(&[cursor()]))
            .read_only(),
    ]
}

/// The cursors a node's clients hold, each with its lease.
#[derive(Default)]
pub struct Cursors {
    leases: Mutex<HashMap<u64, Lease>>,
}

struct Lease {
    span: Duration,
    until: Instant,
}

impl Cursors {
    /// Closes the cursors whose lease ran out.
    fn expire<S: Store>(&self, vault: &Vault<S>, now: Instant) {
        let mut leases = self.leases.lock().expect("leases lock");
        leases.retain(|&id, lease| {
            let alive = lease.until > now;
            if !alive {
                vault.close_cursor(CursorId(id));
            }
            alive
        });
    }

    /// Renews cursor `id`'s lease; [`GONE`] if the node does not hold it.
    fn touch(&self, id: u64, now: Instant) -> Result<(), Error> {
        let mut leases = self.leases.lock().expect("leases lock");
        let lease = leases.get_mut(&id).ok_or_else(|| gone(id))?;
        lease.until = now + lease.span;
        Ok(())
    }

    /// How many cursors are open.
    pub fn len(&self) -> usize {
        self.leases.lock().expect("leases lock").len()
    }

    /// Whether no cursor is open.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Runs cursor command `id` (params checked) on `vault`; the leases
    /// are measured against `now`.
    ///
    /// # Errors
    /// [`GONE`] for a cursor the node does not hold, a refused query, or
    /// the call cancelled.
    pub fn run<S: Store>(
        &self,
        vault: &Vault<S>,
        id: &str,
        params: &Value,
        call: &Call,
        now: Instant,
    ) -> Result<Value, Error> {
        self.expire(vault, now);
        let refused = |e: String| Error::new(helios_inspect::rpc::code::REFUSED, e);
        if id == "cursor.open" {
            let query = query_of(&params["query"]).map_err(Error::invalid_params)?;
            let span = params["lease"]
                .as_u64()
                .map_or(LEASE, Duration::from_secs)
                .min(MAX_LEASE);
            let info = vault.open_cursor(query).map_err(refused)?;
            self.leases.lock().expect("leases lock").insert(
                info.id.0,
                Lease {
                    span,
                    until: now + span,
                },
            );
            return Ok(json!({
                "cursor": info.id.0,
                "count": info.count,
                "version": info.version,
                "lease": span.as_secs(),
            }));
        }
        let cursor = params["cursor"].as_u64().unwrap_or_default();
        if id == "cursor.close" {
            let held = self
                .leases
                .lock()
                .expect("leases lock")
                .remove(&cursor)
                .is_some();
            vault.close_cursor(CursorId(cursor));
            return Ok(json!({ "cursor": cursor, "closed": held }));
        }
        self.touch(cursor, now)?;
        let handle = CursorId(cursor);
        match id {
            "cursor.window" => {
                let offset = to_usize(&params["offset"]);
                let len = to_usize(&params["len"]).min(MAX_WINDOW);
                let names: Vec<&str> = params["fields"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                let mut rows = Vec::with_capacity(len);
                let mut at = offset;
                while at < offset + len {
                    // A window of thousands of rows stops between chunks
                    // when its call is cancelled; the cursor stays open.
                    if call.is_cancelled() {
                        return Err(Error::cancelled());
                    }
                    let n = CHUNK.min(offset + len - at);
                    let read = if params["locations"] == true {
                        let chunk = vault
                            .fetch_library(handle, at, n, &names)
                            .map_err(|e| missing_or(cursor, e))?;
                        let read = chunk.len();
                        rows.extend(chunk.into_iter().map(|row| {
                            json!({ "id": row.id.hex(), "fields": row.fields,
                                    "locations": locations_json(&row.locations) })
                        }));
                        read
                    } else {
                        let chunk = vault
                            .fetch(handle, at, n, &names)
                            .map_err(|e| missing_or(cursor, e))?;
                        let read = chunk.len();
                        rows.extend(
                            chunk
                                .into_iter()
                                .map(|row| json!({ "id": row.id.hex(), "fields": row.fields })),
                        );
                        read
                    };
                    if read < n {
                        break;
                    }
                    at += n;
                }
                let current = vault.catalog_version();
                let info = vault
                    .cursor_info(handle)
                    .map_err(|e| missing_or(cursor, e))?;
                Ok(json!({
                    "cursor": cursor,
                    "offset": offset,
                    "count": info.count,
                    "version": info.version,
                    "current": current,
                    "stale": info.version != current,
                    "rows": rows,
                }))
            }
            "cursor.rank" => {
                let image = ImageId::from_hex(params["image"].as_str().unwrap_or_default())
                    .map_err(Error::invalid_params)?;
                let rank = vault
                    .rank_of(handle, &image)
                    .map_err(|e| missing_or(cursor, e))?;
                Ok(json!({ "cursor": cursor, "image": image.hex(), "rank": rank }))
            }
            "cursor.refresh" => {
                let delta = vault.refresh(handle).map_err(|e| missing_or(cursor, e))?;
                let hex = |ids: &[ImageId]| ids.iter().map(ImageId::hex).collect::<Vec<_>>();
                Ok(json!({
                    "cursor": cursor,
                    "changed": delta.changed,
                    "count": delta.count,
                    "version": delta.version,
                    "inserted": hex(&delta.inserted),
                    "removed": hex(&delta.removed),
                    "replaced": hex(&delta.replaced),
                }))
            }
            other => Err(Error::method_not_found(other)),
        }
    }
}

fn to_usize(value: &Value) -> usize {
    value
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
}

fn gone(cursor: u64) -> Error {
    Error::new(
        GONE,
        format!("no cursor {cursor}: closed, or its lease ran out; open it again"),
    )
    .with_data(json!({ "cursor": cursor }))
}

/// A vault's "no cursor" as [`GONE`], any other failure as a refusal.
fn missing_or(cursor: u64, error: String) -> Error {
    if error.starts_with("no cursor") {
        gone(cursor)
    } else {
        Error::new(helios_inspect::rpc::code::REFUSED, error)
    }
}

/// Where an image's copies are, as a window or `library.fields.get`
/// carries them: `path`, `size`, `modified`, `last_verified`, `volume`,
/// `rel_path`, `presence` (a path that is not UTF-8 is written lossily).
pub fn locations_json(locations: &[Location]) -> Value {
    locations
        .iter()
        .map(|l| {
            json!({
                "path": l.path.to_string_lossy(),
                "size": l.size,
                "modified": l.modified,
                "last_verified": l.last_verified,
                "volume": l.volume.as_ref().map(VolumeId::as_str),
                "rel_path": l.rel_path.as_ref().map(|p| p.to_string_lossy()),
                "presence": l.presence.as_str(),
            })
        })
        .collect()
}

/// A backup rule from its JSON form (`library.facets`' `rule`, a backup
/// filter's): `{"volumes": [{"id", "drive", "off_site"}], "copies",
/// "off_site_required"}`.
///
/// # Errors
/// A term of the wrong type.
pub fn backups_of(value: &Value) -> Result<Backups, String> {
    let volumes = value["volumes"]
        .as_array()
        .ok_or("a backup rule's `volumes` is a list")?
        .iter()
        .map(|v| {
            Ok(BackupVolume {
                id: VolumeId(
                    v["id"]
                        .as_str()
                        .ok_or("a backup volume has an `id`")?
                        .to_string(),
                ),
                drive: v["drive"].as_str().unwrap_or_default().to_string(),
                off_site: v["off_site"].as_bool().unwrap_or(false),
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(Backups {
        volumes,
        copies: value["copies"]
            .as_u64()
            .and_then(|c| u8::try_from(c).ok())
            .unwrap_or(0),
        off_site_required: value["off_site_required"].as_bool().unwrap_or(false),
    })
}

/// A backup rule's JSON form, as [`backups_of`] reads it.
pub fn backups_json(rule: &Backups) -> Value {
    json!({
        "volumes": rule.volumes.iter().map(|v| json!({
            "id": v.id.as_str(), "drive": v.drive, "off_site": v.off_site,
        })).collect::<Vec<_>>(),
        "copies": rule.copies,
        "off_site_required": rule.off_site_required,
    })
}

/// A query's JSON form, as [`query_of`] reads it: only the terms it sets.
pub fn query_json(query: &Query) -> Value {
    let mut terms = serde_json::Map::new();
    let mut put = |name: &str, value: Value| {
        terms.insert(name.into(), value);
    };
    if let Some(r) = query.min_rating {
        put("min_rating", json!(r));
    }
    if let Some(l) = &query.label {
        put("label", json!(l));
    }
    for (name, list) in [
        ("labels", &query.labels),
        ("flags", &query.flags),
        ("tags", &query.tags),
        ("words", &query.words),
        ("cameras", &query.cameras),
        ("lenses", &query.lenses),
        ("file_types", &query.file_types),
        ("cull", &query.cull),
    ] {
        if !list.is_empty() {
            put(name, json!(list));
        }
    }
    if let Some(t) = query.captured_from {
        put("captured_from", json!(t));
    }
    if let Some(t) = query.captured_before {
        put("captured_before", json!(t));
    }
    if let Some(f) = &query.folder {
        put("folder", json!(f));
    }
    if let Some(a) = &query.album {
        put("album", json!(a));
    }
    if let Some(rules) = &query.rules {
        put("rules", rules.to_json());
    }
    if let Some(now) = query.now {
        put("now", json!(now));
    }
    if !query.presence.is_empty() {
        put(
            "presence",
            json!(
                query
                    .presence
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
            ),
        );
    }
    if let Some(b) = &query.backup {
        put(
            "backup",
            json!({
                "rule": backups_json(&b.rule),
                "states": b.states.iter().map(|s| s.name()).collect::<Vec<_>>(),
            }),
        );
    }
    if query.order != Order::default() {
        put(
            "order",
            json!({ "by": query.order.by.name(), "descending": query.order.descending }),
        );
    }
    Value::Object(terms)
}

/// A query from its JSON form: every term optional, an empty or missing
/// object selects the whole Library by id.
///
/// ```json
/// {"min_rating": 3, "labels": ["red"], "flags": ["pick"], "tags": ["sea"],
///  "captured_from": 0, "captured_before": 1, "words": ["beach"],
///  "cameras": [], "lenses": [], "file_types": ["cr3"], "folder": "/photos",
///  "album": "id", "presence": ["present"], "cull": ["sharp"],
///  "order": {"by": "captured", "descending": true}}
/// ```
///
/// # Errors
/// A term of the wrong type, an unknown term, sort, flag or culling state.
pub fn query_of(value: &Value) -> Result<Query, String> {
    let mut query = Query::default();
    let Some(terms) = value.as_object() else {
        return if value.is_null() {
            Ok(query)
        } else {
            Err("a query is an object".into())
        };
    };
    let strings = |name: &str, v: &Value| -> Result<Vec<String>, String> {
        v.as_array()
            .ok_or_else(|| format!("`{name}` is a list of strings"))?
            .iter()
            .map(|s| {
                s.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("`{name}` is a list of strings"))
            })
            .collect()
    };
    let int = |name: &str, v: &Value| v.as_i64().ok_or_else(|| format!("`{name}` is an integer"));
    let text = |name: &str, v: &Value| {
        v.as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("`{name}` is a string"))
    };
    for (name, v) in terms {
        match name.as_str() {
            "min_rating" => query.min_rating = Some(int(name, v)?),
            "label" => query.label = Some(text(name, v)?),
            "labels" => query.labels = strings(name, v)?,
            "flags" => query.flags = strings(name, v)?,
            "tags" => query.tags = strings(name, v)?,
            "captured_from" => query.captured_from = Some(int(name, v)?),
            "captured_before" => query.captured_before = Some(int(name, v)?),
            "words" => query.words = strings(name, v)?,
            "cameras" => query.cameras = strings(name, v)?,
            "lenses" => query.lenses = strings(name, v)?,
            "file_types" => query.file_types = strings(name, v)?,
            "folder" => query.folder = Some(text(name, v)?),
            "album" => query.album = Some(text(name, v)?),
            "cull" => query.cull = strings(name, v)?,
            "rules" => query.rules = Some(helios_vault::rules::Rule::parse(v)?),
            "now" => query.now = Some(int(name, v)?),
            "backup" => {
                let states = strings("backup.states", &v["states"])?
                    .iter()
                    .map(|s| {
                        BackupState::ALL
                            .into_iter()
                            .find(|b| b.name() == s)
                            .ok_or_else(|| format!("`{s}` is not a backup state"))
                    })
                    .collect::<Result<_, _>>()?;
                query.backup = Some(BackupFilter {
                    rule: backups_of(&v["rule"])?,
                    states,
                });
            }
            "presence" => {
                query.presence = strings(name, v)?
                    .iter()
                    .map(|p| match p.as_str() {
                        "present" | "offline" | "missing" => Ok(Presence::parse(p)),
                        other => Err(format!(
                            "`{other}` is not a presence (present, offline, missing)"
                        )),
                    })
                    .collect::<Result<_, _>>()?;
            }
            "order" => {
                let by = v["by"].as_str().unwrap_or("id");
                let by = Sort::parse(by).ok_or_else(|| {
                    format!(
                        "`{by}` is not a sort ({})",
                        Sort::ALL.map(Sort::name).join(", ")
                    )
                })?;
                query.order = Order {
                    by,
                    descending: v["descending"].as_bool().unwrap_or(false),
                };
            }
            other => return Err(format!("`{other}` is not a query term")),
        }
    }
    query.check()?;
    Ok(query)
}
