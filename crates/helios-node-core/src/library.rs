//! The node's commands on its vault's library that are not control
//! verbs: the stacks (`library.stacks.*`), darktable's sidecars kept as
//! versions (`library.importDarktable`, one sidecar file with
//! `library.keepSidecar`) and an image's edit read and saved
//! (`library.edit.head`, `library.edit.save`), photos' fields set and
//! read (`library.fields.set`, `library.fields.get`), photos taken out
//! of the Library (`library.images.remove`), their files moved to the
//! system trash as a journaled job (`library.images.trash`, its jobs
//! read with `library.trash.jobs`), the facet counts of
//! a query (`library.facets`), the photos changed outside Helios
//! (`library.changed.list`), the import history listed
//! (`import.history.list`) and one import run again
//! (`import.history.again`, run as `library.import` by whoever serves
//! it), the library's documents (`library.documents.get`, `.set`), its
//! folders counted (`library.folders`) and the workshop sessions' records
//! (`library.workshop.sessions.list`, `.set`), the albums and the quick
//! collection read (`library.albums.list`) and changed
//! (`library.albums.edit`, answering what changed for an undo history).
//! `helios-node` lists them
//! with its verbs; the desktop's node serves them too
//! ([`crate::endpoint`]). They run on the vault the node owns, by whoever
//! the call's origin names; the stacks' changes are announced on the
//! node's bus.
//!
//! An image is named by its id or by an absolute path the vault knows.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::commands::{CommandSpec, Need, object};
use helios_core::Bus;
use helios_library::stacks::{self as suggest, Resolvers, RuleResolver};
use helios_vault::{ImageId, Store, Vault};
use serde_json::{Value, json};

/// The ids of the commands run here.
pub const IDS: &[&str] = &[
    "library.importDarktable",
    "library.keepSidecar",
    "library.edit.head",
    "library.edit.save",
    "library.fields.set",
    REMOVE,
    "library.fields.get",
    "library.facets",
    "library.changed.list",
    "library.stacks.list",
    "library.stacks.create",
    "library.stacks.dissolve",
    "library.stacks.expand",
    "library.stacks.top",
    "library.stacks.suggest",
    "library.stacks.accept",
    "library.stacks.decline",
    "library.stacks.decisions",
    "library.documents.get",
    "library.documents.set",
    "library.folders",
    "library.workshop.sessions.list",
    "library.workshop.sessions.set",
    "library.albums.list",
    "library.albums.edit",
    HISTORY_LIST,
    AGAIN,
];

/// Takes photos out of the Library (Lightroom's "Remove from Catalog"):
/// their flag only, no file touched; answers the step (`changes`), which
/// the vault's revert undoes.
pub const REMOVE: &str = "library.images.remove";

/// Moves photos' files to the system trash (Lightroom's "Delete from
/// Disk", darktable's "delete (trash)") as a journaled job: refused on
/// cards and backup volumes, the photos taken out of the Library; `plan`
/// only says what it would do, `job` resumes a job kept
/// (`helios_library::trash`).
pub const TRASH: &str = "library.images.trash";

/// Reads the trash jobs kept, or one (`job`), with each file's outcome.
pub const TRASH_JOBS: &str = "library.trash.jobs";

/// Lists the import history (`helios_library::import_history`).
pub const HISTORY_LIST: &str = "import.history.list";

/// Runs an import of the history again: answered as `library.import` with
/// its record's params, by the node's importer ([`again_params`]).
pub const AGAIN: &str = "import.history.again";

/// The `library.import` params that run again the import of the history
/// that `params` names (`id`).
///
/// # Errors
/// No such import, or one an importer plugin listed.
pub fn again_params<S: Store>(vault: &Vault<S>, params: &Value) -> Result<Value, String> {
    let id = params["id"]
        .as_str()
        .ok_or("`id` is missing: an import of `import.history.list`")?;
    let mut again = helios_library::import_history::again(vault, id)?;
    if let Some(fields) = again.as_object_mut() {
        // The node's `library.import` takes no darktable choice.
        fields.remove("darktable");
    }
    Ok(again)
}

/// Whether `id` is one of the commands run here.
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

fn string() -> Value {
    json!({ "type": "string" })
}

fn absolute() -> Value {
    json!({ "type": "string", "description": "an absolute path" })
}

fn images() -> Value {
    json!({ "type": "array", "items": { "type": "string" },
            "description": "image ids, or absolute paths the vault knows" })
}

/// The commands' specs.
pub fn specs() -> Vec<CommandSpec> {
    let stack = || ("stack", string(), true);
    let decision = || ("decision", string(), true);
    let library = |id: &str, label: &str| CommandSpec::new(id, label, "Library").needs(Need::Vault);
    vec![
        library(
            "library.importDarktable",
            "Keep darktable's sidecars as versions",
        )
        .params(object(&[(
            "paths",
            json!({ "type": "array", "items": { "type": "string", "description": "an absolute path" } }),
            true,
        )])),
        library(
            "library.keepSidecar",
            "Keep one darktable sidecar of an image as a version",
        )
        .params(object(&[
            ("image", absolute(), true),
            ("sidecar", absolute(), true),
            (
                "version",
                json!({ "type": "integer", "minimum": 0,
                        "description": "darktable's duplicate number, 0 for the base sidecar" }),
                true,
            ),
            ("exact", json!({ "type": "boolean" }), false),
            ("base", json!({ "type": "boolean" }), false),
        ])),
        library("library.edit.head", "Read an image's newest edit")
            .params(object(&[("image", string(), true)]))
            .read_only(),
        library("library.edit.save", "Save a step of an image's edit").params(object(&[
            ("image", string(), true),
            ("edit", json!({ "type": "object" }), true),
            ("label", string(), true),
        ])),
        library("library.fields.set", "Set photos' fields").params(object(&[(
            "changes",
            json!({ "type": "array", "items": { "type": "object" },
                    "description": "{image, field, value, op}: a field set (value null clears it); \
                                    op `add` or `remove` puts a name in or out of a set field, \
                                    `set_if_absent` sets only an empty field" }),
            true,
        )])),
        library(
            REMOVE,
            "Take photos out of the Library, their files untouched",
        )
        .params(object(&[("images", images(), true)])),
        library(
            TRASH,
            "Move photos' files to the system trash, as a journaled job",
        )
        .params(object(&[
            ("images", images(), false),
            (
                "plan",
                json!({ "type": "boolean",
                        "description": "only say which files would go and which are refused" }),
                false,
            ),
            (
                "job",
                json!({ "type": "string",
                        "description": "a job kept, to resume: its pending and failed files" }),
                false,
            ),
        ])),
        library(TRASH_JOBS, "Read the trash jobs and each file's outcome")
            .params(object(&[("job", string(), false)]))
            .read_only(),
        library("library.fields.get", "Read photos' fields")
            .params(object(&[
                ("images", images(), true),
                (
                    "fields",
                    json!({ "type": "array", "items": { "type": "string" } }),
                    true,
                ),
                (
                    "locations",
                    json!({ "type": "boolean",
                            "description": "each row with where its copies are (`locations`)" }),
                    false,
                ),
            ]))
            .read_only(),
        library("library.facets", "Count the photos of a query by facet")
            .params(object(&[
                (
                    "query",
                    json!({ "type": "object",
                            "description": "which photos, as a cursor's query (docs/protocol.md, Cursors)" }),
                    false,
                ),
                (
                    "rule",
                    json!({ "type": "object",
                            "description": "the backup rule the `backup` facet counts against" }),
                    false,
                ),
            ]))
            .read_only(),
        library(
            "library.changed.list",
            "List the photos changed outside Helios",
        )
        .read_only(),
        library("library.stacks.list", "List the stacks").read_only(),
        library("library.stacks.create", "Stack images").params(object(&[
            ("images", images(), true),
            ("top", json!({ "type": ["string", "null"] }), false),
        ])),
        library("library.stacks.dissolve", "Dissolve a stack").params(object(&[stack()])),
        library("library.stacks.expand", "Expand or collapse a stack").params(object(&[
            stack(),
            ("expanded", json!({ "type": "boolean" }), true),
        ])),
        library("library.stacks.top", "Put an image on top of its stack")
            .params(object(&[stack(), ("image", string(), true)])),
        library("library.stacks.suggest", "Suggest stacks").params(object(&[
            ("images", images(), false),
            ("resolver", json!({ "type": ["string", "null"] }), false),
            (
                "window",
                json!({ "type": ["number", "null"], "minimum": 0 }),
                false,
            ),
            (
                "distance",
                json!({ "type": ["integer", "null"], "minimum": 0 }),
                false,
            ),
        ])),
        library("library.stacks.accept", "Accept suggested stacks").params(object(&[
            decision(),
            (
                "groups",
                json!({ "type": ["array", "null"], "items": { "type": "integer", "minimum": 0 } }),
                false,
            ),
        ])),
        library("library.stacks.decline", "Decline suggested stacks")
            .params(object(&[decision()])),
        library("library.stacks.decisions", "List the stack suggestions").read_only(),
        library("library.documents.get", "Read a library document")
            .params(object(&[("ns", string(), true), ("key", string(), true)]))
            .read_only(),
        library("library.documents.set", "Keep a library document").params(object(&[
            ("ns", string(), true),
            ("key", string(), true),
            (
                "document",
                json!({ "description": "any JSON value; null removes the document" }),
                true,
            ),
        ])),
        library("library.folders", "Count the photos by folder")
            .params(object(&[(
                "query",
                json!({ "type": "object",
                        "description": "which photos, as a cursor's query (docs/protocol.md, Cursors)" }),
                false,
            )]))
            .read_only(),
        library(
            "library.workshop.sessions.list",
            "List the workshop sessions' records",
        )
        .read_only(),
        library(
            "library.workshop.sessions.set",
            "Keep or drop a workshop session's record",
        )
        .params(object(&[
            ("id", string(), true),
            (
                "record",
                json!({ "type": ["object", "null"], "description": "null drops the record" }),
                true,
            ),
        ])),
        library(
            "library.albums.list",
            "List the albums and the quick collection",
        )
        .read_only(),
        library("library.albums.edit", "Change an album or the quick collection").params(
            object(&[
                (
                    "op",
                    json!({ "type": "string", "enum": ALBUM_OPS }),
                    true,
                ),
                ("id", string(), false),
                ("name", string(), false),
                ("images", images(), false),
                ("set", json!({ "type": ["string", "null"] }), false),
                ("members", json!({ "type": "boolean" }), false),
                ("clear", json!({ "type": "boolean" }), false),
                (
                    "rules",
                    json!({ "type": "object", "description": "a smart album's rules (docs/protocol.md, Cursors)" }),
                    false,
                ),
            ]),
        ),
        CommandSpec::new(HISTORY_LIST, "List the import history", "Library")
            .needs(Need::Vault)
            .params(object(&[(
                "limit",
                json!({ "type": ["integer", "null"], "minimum": 1 }),
                false,
            )]))
            .read_only(),
        CommandSpec::new(AGAIN, "Import again", "Library")
            .needs(Need::Vault)
            .params(object(&[(
                "id",
                json!({ "type": "string", "description": "an import's id, from import.history.list" }),
                true,
            )])),
    ]
}

/// The id of the import command, run by the node's importer.
pub const IMPORT: &str = "library.import";

/// `library.import`'s spec: files and folders imported into the library,
/// copied or not.
pub fn import_spec() -> CommandSpec {
    let or_null = |kind: &str| json!({ "type": [kind, "null"] });
    CommandSpec::new(IMPORT, "Import into the library", "Library")
        .params(object(&[
            ("paths", json!({ "type": "array", "items": absolute() }), true),
            ("copy", json!({ "type": "boolean" }), false),
            ("into", json!({ "type": ["string", "null"], "description": "an absolute path" }), false),
            ("rename", json!({ "type": ["string", "null"], "description": "a rename pattern" }), false),
            (
                "layout",
                json!({ "type": ["string", "null"], "enum": ["day", "month", "year", "flat", null] }),
                false,
            ),
            (
                "duplicates",
                json!({ "type": ["string", "null"], "enum": ["skip", "keep", "ask", null] }),
                false,
            ),
            ("signals", or_null("boolean"), false),
            (
                "camera_look",
                json!({ "type": ["boolean", "null"],
                        "description": "the cameras' default looks put on the raws brought in (on when absent)" }),
                false,
            ),
        ]))
        .needs(Need::Vault)
}

/// An import's options from `library.import`'s params; `cancel` stops it
/// before its next file.
pub fn import_options(
    params: &Value,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<helios_library::import::Options, String> {
    use helios_library::import::{Duplicates, Layout, Options, Placement};
    let mut options = Options::default();
    let into = params["into"].as_str().map(PathBuf::from);
    if params["copy"] == true || into.is_some() {
        options.placement = Placement::Copy { into };
    }
    if let Some(pattern) = params["rename"].as_str() {
        options.rename = Some(helios_library::rename::Pattern::parse(pattern)?);
    }
    if let Some(layout) = params["layout"].as_str() {
        options.layout = Layout::parse(layout)?;
    }
    if let Some(duplicates) = params["duplicates"].as_str() {
        options.duplicates = Duplicates::parse(duplicates)?;
    }
    if let Some(signals) = params["signals"].as_bool() {
        options.signals = signals;
    }
    options.cancel = cancel;
    Ok(options)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `library.images.trash`: a plan, a new job worked, or a job resumed.
fn trash<S: Store>(
    vault: &Vault<S>,
    bus: &Arc<Bus>,
    params: &Value,
    origin: &str,
) -> Result<Value, String> {
    use helios_library::trash::{self, Outcome};
    let volumes = trash::volumes(vault)?;
    let id = match params["job"].as_str() {
        Some(job) => job.to_string(),
        None => {
            let ids = named(vault, params, "images")?;
            if ids.is_empty() {
                return Err("`images` names no photo".into());
            }
            if params["plan"].as_bool() == Some(true) {
                let files = trash::plan(vault, &ids, &volumes)?;
                let job = trash::Job {
                    id: String::new(),
                    at: now(),
                    by: origin.into(),
                    images: ids.iter().map(ImageId::hex).collect(),
                    files,
                    done: false,
                };
                let refused: Vec<_> = job
                    .files
                    .iter()
                    .filter(|f| f.outcome == Outcome::Refused)
                    .collect();
                return Ok(json!({
                    "images": ids.len(),
                    "files": job.count(Outcome::Pending),
                    "folders": job.folders(),
                    "refused": to_json(&refused),
                }));
            }
            trash::start(vault, &ids, &volumes, origin, now())?.id
        }
    };
    let jobs = helios_core::jobs::Jobs::new(Arc::clone(bus));
    let worked = trash::work(vault, &id, &*trash::bin(), Some(&jobs), now())?;
    let job = &worked.job;
    let of = |outcome: Outcome| {
        job.files
            .iter()
            .filter(|f| f.outcome == outcome)
            .collect::<Vec<_>>()
    };
    Ok(json!({
        "job": job.id,
        "trashed": job.count(Outcome::Trashed),
        "missing": job.count(Outcome::Missing),
        "refused": to_json(&of(Outcome::Refused)),
        "failed": to_json(&of(Outcome::Failed)),
        "removed": worked.changes.images.len(),
    }))
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("library values serialize")
}

/// The changes `library.albums.edit` makes.
const ALBUM_OPS: &[&str] = &[
    "create",
    "rename",
    "delete",
    "createSet",
    "move",
    "add",
    "remove",
    "createSmart",
    "rules",
    "quickToggle",
    "quickClear",
    "quickSave",
];

/// Changes an album or the quick collection as `params` asks
/// (`library.albums.edit`). The answer: what changed (`changes`, for an
/// undo history), the album's name (the new one for a creation, the
/// name it had otherwise), how many photos the step put in or took out
/// (`count`) and, for `quickToggle`, whether it put them in (`added`).
fn edit_album<S: Store>(vault: &Vault<S>, params: &Value) -> Result<Value, String> {
    let id = text(params, "id");
    let name = text(params, "name");
    let set = params["set"].as_str();
    let rules = || helios_vault::rules::Rule::parse(&params["rules"]);
    let before = || -> String {
        vault
            .album(id)
            .ok()
            .flatten()
            .map(|a| a.name)
            .unwrap_or_default()
    };
    let photos = || params["images"].as_array().map_or(0, Vec::len);
    let mut added = true;
    let (name, count, changes) = match text(params, "op") {
        "create" => vault
            .create_album(Some(id), name, &named(vault, params, "images")?)
            .map(|(a, c)| (a.name, a.images.len(), c))?,
        "rename" => {
            let was = before();
            (was, 0, vault.rename_album(id, name)?)
        }
        "delete" => {
            let was = before();
            let members = params["members"].as_bool().unwrap_or(false);
            (was, 0, vault.delete_collection(id, members)?)
        }
        "createSet" => vault
            .create_set(Some(id), name, set)
            .map(|(a, c)| (a.name, 0, c))?,
        "move" => (before(), 0, vault.move_collection(id, set)?),
        "add" => (
            before(),
            photos(),
            vault.add_to_album(id, &named(vault, params, "images")?)?,
        ),
        "remove" => (
            before(),
            photos(),
            vault.remove_from_album(id, &named(vault, params, "images")?)?,
        ),
        "createSmart" => vault
            .create_smart_album(Some(id), name, rules()?)
            .map(|(a, c)| (a.name, 0, c))?,
        "rules" => (before(), 0, vault.set_album_rules(id, rules()?)?),
        "quickToggle" => {
            // The photos it moved: those already in are not counted.
            let held = vault.quick_collection().map_or(0, |q| q.len());
            let (put, changes) = vault.toggle_quick(&named(vault, params, "images")?)?;
            added = put;
            let now = vault.quick_collection().map_or(held, |q| q.len());
            (String::new(), now.abs_diff(held), changes)
        }
        "quickClear" => {
            let count = vault.quick_collection().map_or(0, |q| q.len());
            (String::new(), count, vault.clear_quick()?)
        }
        "quickSave" => vault
            .save_quick(Some(id), name, params["clear"].as_bool().unwrap_or(false))
            .map(|(a, c)| (a.name, a.images.len(), c))?,
        other => return Err(format!("no album change `{other}`: one of {ALBUM_OPS:?}")),
    };
    Ok(json!({ "name": name, "count": count, "added": added, "changes": to_json(&changes) }))
}

/// An image named by its id, or by an absolute path the vault knows.
fn image<S: Store>(vault: &Vault<S>, text: &str) -> Result<ImageId, String> {
    if let Ok(id) = ImageId::from_hex(text) {
        return Ok(id);
    }
    vault
        .at_path(Path::new(text))?
        .map(|(id, _)| id)
        .ok_or_else(|| format!("`{text}` is neither an image id nor a file the vault knows"))
}

fn named<S: Store>(vault: &Vault<S>, params: &Value, field: &str) -> Result<Vec<ImageId>, String> {
    params[field]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|n| image(vault, n))
        .collect()
}

/// What one change of `library.fields.set` does: its `op` (`set` when
/// not given) with its `value`.
fn op_of(change: &Value) -> Result<helios_vault::fields::Op, String> {
    use helios_vault::fields::Op;
    let value = &change["value"];
    let name = || {
        value
            .as_str()
            .map(String::from)
            .ok_or_else(|| "`add` and `remove` take a name as `value`".to_string())
    };
    Ok(match change["op"].as_str().unwrap_or("set") {
        "set" => Op::Set(Some(value.clone()).filter(|v| !v.is_null())),
        "set_if_absent" => Op::SetIfAbsent(value.clone()),
        "add" => Op::Add(name()?),
        "remove" => Op::Remove(name()?),
        other => {
            return Err(format!(
                "`{other}` is not an op: set, set_if_absent, add or remove"
            ));
        }
    })
}

fn text<'a>(params: &'a Value, field: &str) -> &'a str {
    params[field].as_str().unwrap_or_default()
}

/// Runs command `id` (params already checked against its schema) on
/// `vault`, made by `origin`.
pub fn run<S: Store>(
    vault: &Vault<S>,
    bus: &Arc<Bus>,
    id: &str,
    params: &Value,
    origin: &str,
) -> Result<Value, String> {
    if id == TRASH {
        return trash(vault, bus, params, origin);
    }
    let bus = Some(&**bus);
    match id {
        HISTORY_LIST => {
            let limit = params["limit"].as_u64().map(|n| n as usize);
            helios_library::import_history::list(vault, limit)
                .map(|records| json!({ "imports": to_json(&records) }))
        }
        AGAIN => Err(format!(
            "`{AGAIN}` runs as `{IMPORT}`, by the node's importer"
        )),
        "library.importDarktable" => {
            let paths: Vec<PathBuf> = params["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect();
            let (mut files, mut warnings) = (Vec::new(), Vec::new());
            let kept = helios_library::darktable::import(
                vault,
                &paths,
                now(),
                &mut |line| files.push(line.clone()),
                &mut |path, warning| {
                    warnings.push(json!({ "path": path, "warning": warning }));
                },
            )?;
            Ok(json!({ "kept": kept, "files": files, "warnings": warnings }))
        }
        "library.keepSidecar" => {
            let file = helios_xmp::files::SidecarFile {
                path: PathBuf::from(text(params, "sidecar")),
                version: params["version"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or(0),
                exact: params["exact"].as_bool().unwrap_or(true),
            };
            let base = params["base"].as_bool().unwrap_or(false);
            // Its import's warning is in the answer (`warning`).
            Ok(helios_library::darktable::keep_file(
                vault,
                Path::new(text(params, "image")),
                &file,
                base,
                now(),
                &mut |_, _| {},
            ))
        }
        "library.edit.head" => {
            let id = image(vault, text(params, "image"))?;
            let (_, head) = helios_vault::EditSession::open(vault, id)?;
            Ok(json!({
                "image": id.hex(),
                "head": head.map(|v| json!({ "version": v.id.hex(), "edit": v.edit })),
            }))
        }
        "library.edit.save" => {
            let id = image(vault, text(params, "image"))?;
            let (mut session, _) = helios_vault::EditSession::open(vault, id)?;
            let saved = session.save(
                vault,
                params["edit"].clone(),
                text(params, "label"),
                origin,
                now(),
            )?;
            Ok(json!({ "image": id.hex(), "version": saved.map(|v| v.id.hex()) }))
        }
        "library.fields.set" => {
            let mut commands = Vec::new();
            for change in params["changes"].as_array().into_iter().flatten() {
                let id = image(vault, change["image"].as_str().unwrap_or_default())?;
                let field = change["field"]
                    .as_str()
                    .ok_or("a change names its `field`")?;
                commands.push(helios_vault::Command::with(id, field, op_of(change)?));
            }
            vault
                .apply(&commands, origin, None, now())
                .map(|report| json!({ "report": to_json(&report) }))
        }
        "library.fields.get" => {
            let ids = named(vault, params, "images")?;
            let names: Vec<&str> = params["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if params["locations"] == true {
                let rows = vault.library_rows(&ids, &names)?;
                return Ok(json!({
                    "rows": rows
                        .into_iter()
                        .map(|row| json!({ "id": row.id.hex(), "fields": row.fields,
                                           "locations": crate::cursor::locations_json(&row.locations) }))
                        .collect::<Vec<_>>(),
                }));
            }
            let rows = vault.fields_of(&ids, &names)?;
            Ok(json!({
                "rows": ids
                    .iter()
                    .zip(rows)
                    .map(|(id, fields)| json!({ "id": id.hex(), "fields": fields }))
                    .collect::<Vec<_>>(),
            }))
        }
        REMOVE => {
            let ids = named(vault, params, "images")?;
            if ids.is_empty() {
                return Err("`images` names no photo".into());
            }
            vault.set_in_library(&ids, false, origin, now()).map(
                |changes| json!({ "removed": changes.images.len(), "changes": to_json(&changes) }),
            )
        }
        TRASH_JOBS => match params["job"].as_str() {
            Some(job) => helios_library::trash::get(vault, job)?
                .map(|job| json!({ "job": to_json(&job) }))
                .ok_or_else(|| format!("no trash job {job}")),
            None => {
                helios_library::trash::list(vault).map(|jobs| json!({ "jobs": to_json(&jobs) }))
            }
        },
        "library.facets" => {
            let query = crate::cursor::query_of(&params["query"])?;
            let rule = match &params["rule"] {
                Value::Null => None,
                rule => Some(crate::cursor::backups_of(rule)?),
            };
            vault
                .facets(&query, rule.as_ref())
                .map(|facets| json!({ "facets": to_json(&facets) }))
        }
        "library.changed.list" => vault.changed_outside_images().map(|images| {
            json!({
                "images": images
                    .into_iter()
                    .map(|(id, at)| json!({ "image": id.hex(), "at": at }))
                    .collect::<Vec<_>>(),
            })
        }),
        "library.stacks.list" => vault.stacks().map(|s| json!({ "stacks": to_json(&s) })),
        "library.stacks.create" => {
            let members = named(vault, params, "images")?;
            let top = params["top"]
                .as_str()
                .map(|t| image(vault, t))
                .transpose()?;
            vault
                .create_stack(&members, top, origin, now())
                .map(|s| json!({ "stack": to_json(&s) }))
        }
        "library.stacks.dissolve" => vault
            .dissolve_stack(text(params, "stack"))
            .map(|s| json!({ "stack": to_json(&s) })),
        "library.stacks.expand" => vault
            .set_stack_expanded(
                text(params, "stack"),
                params["expanded"].as_bool().unwrap_or(true),
            )
            .map(|s| json!({ "stack": to_json(&s) })),
        "library.stacks.top" => {
            let top = image(vault, text(params, "image"))?;
            vault
                .set_stack_top(text(params, "stack"), &top)
                .map(|s| json!({ "stack": to_json(&s) }))
        }
        "library.stacks.suggest" => {
            let selection = named(vault, params, "images")?;
            let window = params["window"].as_f64();
            let distance = params["distance"].as_u64();
            let resolvers = if window.is_some() || distance.is_some() {
                let mut rules = suggest::Rules::default();
                if let Some(window) = window {
                    rules.window = window;
                }
                if let Some(distance) = distance {
                    rules.distance = u32::try_from(distance).unwrap_or(u32::MAX);
                }
                Resolvers::new().with(suggest::RULES, Arc::new(RuleResolver { rules }))
            } else {
                Resolvers::default()
            };
            let resolver = params["resolver"].as_str().unwrap_or(suggest::RULES);
            suggest::suggest(vault, bus, &resolvers, resolver, &selection, now())
                .map(|d| json!({ "decision": suggest::to_json(&d) }))
        }
        "library.stacks.accept" => {
            let groups: Option<Vec<usize>> = params["groups"].as_array().map(|g| {
                g.iter()
                    .filter_map(Value::as_u64)
                    .filter_map(|n| usize::try_from(n).ok())
                    .collect()
            });
            suggest::accept(
                vault,
                bus,
                text(params, "decision"),
                groups.as_deref(),
                origin,
                now(),
            )
            .map(|d| json!({ "decision": suggest::to_json(&d) }))
        }
        "library.stacks.decline" => {
            suggest::decline(vault, bus, text(params, "decision"), origin, now())
                .map(|d| json!({ "decision": suggest::to_json(&d) }))
        }
        "library.stacks.decisions" => {
            suggest::decisions(vault).map(|d| json!({ "decisions": to_json(&d) }))
        }
        "library.documents.get" => vault
            .library_document(text(params, "ns"), text(params, "key"))
            .map(|document| json!({ "document": document })),
        "library.documents.set" => {
            let document = Some(&params["document"]).filter(|d| !d.is_null());
            vault
                .set_library_document(text(params, "ns"), text(params, "key"), document)
                .map(|()| json!({ "kept": document.is_some() }))
        }
        "library.folders" => {
            let query = crate::cursor::query_of(&params["query"])?;
            vault.folders(&query).map(|folders| {
                json!({
                    "folders": folders
                        .into_iter()
                        .map(|(volume, folders)| json!({ "volume": volume.map(|v| v.0), "folders": folders }))
                        .collect::<Vec<_>>(),
                })
            })
        }
        "library.workshop.sessions.list" => vault.workshop_sessions().map(|sessions| {
            json!({
                "sessions": sessions
                    .into_iter()
                    .map(|(id, record)| json!({ "id": id, "record": record }))
                    .collect::<Vec<_>>(),
            })
        }),
        "library.workshop.sessions.set" => {
            let record = Some(&params["record"]).filter(|r| !r.is_null());
            vault
                .set_workshop_session(text(params, "id"), record)
                .map(|()| json!({ "kept": record.is_some() }))
        }
        "library.albums.list" => vault
            .albums()
            .and_then(|albums| Ok((albums, vault.quick_collection()?)))
            .map(|(albums, quick)| json!({ "albums": to_json(&albums), "quick": quick })),
        "library.albums.edit" => edit_album(vault, params),
        other => Err(format!("no library command `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use crate::commands::validate;

    use super::*;

    #[test]
    fn every_command_is_declared_once_with_a_schema_its_example_fits() {
        let ids: Vec<String> = specs().iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, IDS);
        let examples = [
            json!({ "paths": ["/photos"] }),
            json!({ "image": "/p/a.raf", "sidecar": "/p/a.raf.xmp", "version": 0, "base": true }),
            json!({ "image": "0a" }),
            json!({ "image": "0a", "edit": {}, "label": "macro.make" }),
            json!({ "changes": [{ "image": "0a", "field": "rating", "value": 3 }] }),
            json!({ "images": ["0a"] }),
            json!({ "images": ["/p/a.raf", "0a"], "fields": ["rating", "label"] }),
            json!({ "query": {} }),
            json!({}),
            json!({}),
            json!({ "images": ["/p/a.raf", "0a"], "top": null }),
            json!({ "stack": "s" }),
            json!({ "stack": "s", "expanded": false }),
            json!({ "stack": "s", "image": "/p/a.raf" }),
            json!({ "images": [], "resolver": null, "window": 3.0, "distance": 8 }),
            json!({ "decision": "d", "groups": [0, 2] }),
            json!({ "decision": "d" }),
            json!({}),
            json!({ "ns": "filters", "key": "history" }),
            json!({ "ns": "filters", "key": "history", "document": { "pinned": [] } }),
            json!({ "query": {} }),
            json!({}),
            json!({ "id": "w1", "record": null }),
            json!({}),
            json!({ "op": "quickToggle", "images": [] }),
            json!({ "limit": 10 }),
            json!({ "id": "1700000000000-42-0" }),
        ];
        assert_eq!(examples.len(), IDS.len(), "one example per command");
        for (spec, example) in specs().iter().zip(examples) {
            validate(&spec.params, &example).unwrap_or_else(|e| panic!("{}: {e}", spec.id));
        }
    }

    #[test]
    fn documents_folders_and_workshop_sessions_are_kept_and_read() {
        let vault = Vault::new(helios_vault::MemoryStore::default());
        let bus = Arc::new(Bus::new());
        let call = |id: &str, params: Value| run(&vault, &bus, id, &params, "user").unwrap();
        let key = json!({ "ns": "filters", "key": "history" });
        assert_eq!(
            call("library.documents.get", key.clone()),
            json!({ "document": null })
        );
        let doc = json!({ "pinned": ["rating >= 3"] });
        let mut set = key.clone();
        set["document"] = doc.clone();
        assert_eq!(call("library.documents.set", set), json!({ "kept": true }));
        assert_eq!(
            call("library.documents.get", key.clone()),
            json!({ "document": doc })
        );
        let mut removed = key.clone();
        removed["document"] = Value::Null;
        call("library.documents.set", removed);
        assert_eq!(
            call("library.documents.get", key),
            json!({ "document": null })
        );

        assert_eq!(call("library.folders", json!({})), json!({ "folders": [] }));

        let record = json!({ "kept": true, "steps": [] });
        call(
            "library.workshop.sessions.set",
            json!({ "id": "w1", "record": record }),
        );
        call(
            "library.workshop.sessions.set",
            json!({ "id": "w2", "record": { "kept": false } }),
        );
        call(
            "library.workshop.sessions.set",
            json!({ "id": "w2", "record": null }),
        );
        assert_eq!(
            call("library.workshop.sessions.list", json!({})),
            json!({ "sessions": [{ "id": "w1", "record": record }] })
        );
    }

    #[test]
    fn albums_are_changed_and_listed_and_their_changes_read_back() {
        let vault = Vault::new(helios_vault::MemoryStore::default());
        let bus = Arc::new(Bus::new());
        let call = |id: &str, params: Value| run(&vault, &bus, id, &params, "user").unwrap();
        let made = call(
            "library.albums.edit",
            json!({ "op": "create", "id": "trip", "name": "Trip", "images": [] }),
        );
        assert_eq!(made["name"], "Trip");
        let changes: helios_vault::Changes =
            serde_json::from_value(made["changes"].clone()).unwrap();
        assert!(!changes.is_empty(), "{made}");
        // Read back as it was: an undo walks these.
        assert_eq!(to_json(&changes), made["changes"]);
        let renamed = call(
            "library.albums.edit",
            json!({ "op": "rename", "id": "trip", "name": "Journey" }),
        );
        assert_eq!(renamed["name"], "Trip", "the name it had");
        let listed = call("library.albums.list", json!({}));
        assert_eq!(listed["quick"], json!([]));
        let albums: Vec<_> = listed["albums"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| helios_vault::collections::Album::from_json(a).unwrap())
            .collect();
        assert_eq!(albums.len(), 1);
        assert_eq!(
            (albums[0].id.as_str(), albums[0].name.as_str()),
            ("trip", "Journey")
        );
        let wrong = run(
            &vault,
            &bus,
            "library.albums.edit",
            &json!({ "op": "nope" }),
            "user",
        );
        assert!(wrong.is_err());
    }

    #[test]
    fn stacks_are_made_and_listed_on_the_vault() {
        let vault = Vault::new(helios_vault::MemoryStore::default());
        let bus = Arc::new(Bus::new());
        let listed = run(&vault, &bus, "library.stacks.list", &json!({}), "cli").unwrap();
        assert_eq!(listed, json!({ "stacks": [] }));
        let refused = run(
            &vault,
            &bus,
            "library.stacks.create",
            &json!({ "images": ["/nowhere/a.raf"] }),
            "cli",
        );
        assert!(refused.unwrap_err().contains("neither an image id"));
    }
}
