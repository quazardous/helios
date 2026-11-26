//! What the desktop keeps in the vault, and how it hears about changes.
//!
//! The shell itself holds no vault: [`Persist`] is the host's link
//! between them. Once per frame ([`Persist::frame`]) it
//!
//! - writes what the user changed in the library, as field commands in one
//!   batch, through the desktop's node (`library.fields.set` on its
//!   in-process client, [`Persist::node`]): the call runs off the UI
//!   thread and its answer is picked up at a later frame, the writes
//!   sent one after the other in the order made. Whether the batch is
//!   kept or refused, the touched images are read back through the node
//!   (`library.fields.get`) and shown as the vault holds them, so a failed
//!   write never leaves the grid showing a value that is not stored (and
//!   a field another process changed meanwhile shows too);
//! - applies the vault's `helios.vault.field_changed` events (from any writer in
//!   this process sharing its bus) to the library, and reads its stacks
//!   again on `helios.vault.stack_changed`;
//! - reads the library through the node, off the UI thread: its photos
//!   through a cursor (`cursor.open`, `cursor.window`, `cursor.refresh`)
//!   and a [`ReadCache`] the bus's events keep true, the search filter's
//!   photos and facet counts (`library.facets`); a coming frame shows
//!   what was read ([`Persist::ask_fill`]). Keeps a stack expanded or
//!   collapsed as the user left it;
//! - answers the Stacks panel through the node: a new `stacks.suggest`
//!   proposal, a proposal accepted (all of it or corrected) or declined,
//!   as the user (`library.stacks.suggest`, `.accept`, `.decline`), in
//!   turn with the library's other writes; the stacks and the proposals
//!   waiting (`library.stacks.list`, `library.stacks.decisions`) are read
//!   with the library, and again off the UI thread on
//!   `helios.vault.stack_changed` and `helios.library.decision`;
//! - saves the edit of the image shown ([`Persist::open_image`]), with
//!   the labels and locks of its mask nodes (its `marks`) and the modules
//!   bypassed, as a version each time the value store commits a step: a
//!   write outside a gesture, the end of a gesture (a drag is one step,
//!   saved once when it ends), an undo, a redo. While a gesture goes on, what it changed
//!   is saved every [`FLUSH_SECONDS`] as a version labelled "in progress",
//!   so a crash in the middle of a long drag loses at most that much of
//!   it; a drag saved that way and then cancelled saves the values it put
//!   back, so the given-up values never stay the latest edit. Each
//!   version names who made its step (the user, an agent, a script), and
//!   each save is announced on the bus as `helios.edit.saved`, and keeps whether
//!   the edit changes the photo from its original (its `edited` field, the
//!   Library's and the filmstrip's "edited" badge);
//! - pins the image's history snapshots taken since (see
//!   `crate::edit_snapshots`): once the edit is saved, a snapshot taken
//!   at the step it holds is pinned to that version, and the image's
//!   pinned snapshots are kept in the vault; they are read back, with
//!   the values of their versions, when the image opens.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui_pop::api::bus::bus_contract;
use helios_core::{Bus, Envelope, Graph, Message, NodeId, Params};
use helios_inspect::cache::ReadCache;
use helios_inspect::client::{Client, Pending};
use helios_node_core::cursor::query_json;
use helios_vault::capture::CAPTURED;
use helios_vault::cull;
use helios_vault::fields::{LABELS, PICKED, Query, RATING, REJECTED, ROTATION, TAGS, TITLE};
use helios_vault::{
    EditSession, ImageId, LibraryRow, Location, Order, Presence, Store, Version, edit,
    edit_steps::StepKind,
};
use serde_json::{Value, json};

use crate::Shell;

mod fixtures;
use crate::dialogs::Level;
use crate::library::{Change, Entry};

/// A search filter's answer: the photos it takes and the facet counts.
type Searched = (Vec<String>, helios_vault::Facets);

/// The fields the library shows (the capture time gives the day the
/// search field's dates match; the title is copied with the marks; the
/// camera, lens and exposure are the Results' details columns).
const SHOWN: [&str; 21] = [
    RATING,
    REJECTED,
    PICKED,
    LABELS,
    TAGS,
    TITLE,
    helios_vault::meta::DESCRIPTION,
    helios_vault::meta::CREATOR,
    helios_vault::meta::COPYRIGHT,
    ROTATION,
    CAPTURED,
    cull::FOCUS,
    cull::CLIP_WHITE,
    cull::CLIP_BLACK,
    helios_vault::edit::EDITED,
    helios_vault::capture::CAMERA_MAKE,
    helios_vault::capture::CAMERA_MODEL,
    helios_vault::capture::LENS,
    helios_vault::capture::APERTURE,
    helios_vault::capture::SHUTTER,
    helios_vault::capture::ISO,
];

/// Who the vault's audit trail names for what the desktop writes.
pub const WHO: &str = "user";

/// Who made a change, as the vault records it: `user`, `agent:<id>`,
/// `script:<id>`, `midi`, `remote:<id>`.
pub fn who(origin: &helios_shell::commands::Origin) -> String {
    use helios_shell::commands::Origin;
    match origin {
        Origin::User => WHO.into(),
        Origin::Agent(id) => format!("agent:{id}"),
        Origin::Script(id) => format!("script:{id}"),
        Origin::Midi => "midi".into(),
        Origin::Remote(id) => format!("remote:{id}"),
    }
}

/// The owner of the listeners this layer registers for the panels' asks.
pub const BUS_OWNER: &str = "helios.persist";
/// The listener that sends the Stacks panel's asks to the node.
pub const STACKS: &str = "helios.persist.stacks";
/// The listener that sends the Library's writes to the node.
pub const LIBRARY: &str = "helios.persist.library";

/// How often a gesture still going (a drag) is saved, in seconds.
pub const FLUSH_SECONDS: f64 = 1.0;

pub struct Persist<S: Store> {
    /// The node's library, for what the desktop wires on it.
    library: helios_node_core::desktop::Library<S>,
    bus: Arc<Bus>,
    events: Receiver<Envelope>,
    editing: Option<Editing>,
    /// The edit open, lent to `image.open` / `image.close`'s `edit.save`
    /// listener while the request passes ([`Persist::switch`]).
    desk: Arc<Mutex<Option<Editing>>>,
    /// The bus `image.open` / `image.close`'s listeners are registered on
    /// (the shell's), once.
    darkroom: Option<Arc<dyn egui_pop::api::bus::bus_contract::Bus>>,
    /// Where the desktop says the library changed (an import ended, a
    /// step undone), heard with the vault's events as one change
    /// ([`crate::library_changed`]).
    changes: (
        crate::library_changed::Publisher,
        Receiver<(crate::library_changed::Kind, &'static str)>,
    ),
    /// The bus `library.changed`'s listeners are registered on (the
    /// shell's), once.
    listening: Option<Arc<dyn egui_pop::api::bus::bus_contract::Bus>>,
    /// The bus the panels' asks' listeners are registered on (the
    /// shell's), once ([`Persist::listen_asks`]).
    asking: Mutex<Option<Arc<dyn egui_pop::api::bus::bus_contract::Bus>>>,
    /// The desktop's node, in this process: the library's writes go
    /// through it.
    node: Arc<Client>,
    /// The writes waiting their turn, in the order made; shared with the
    /// listeners of the asks made while the panels drew (`stacks.answer`,
    /// `library.write`), which add theirs in the pass they are heard.
    writes: Arc<Mutex<VecDeque<Write>>>,
    /// The write the node runs now.
    writing: Option<(Write, Pending)>,
    /// The reads back of written images, with the images each names.
    reading: Vec<(BTreeSet<String>, Pending)>,
    /// The library's reads through the node.
    reader: Reader,
    /// The library's reads off the interface's thread (the Library read
    /// again, the stacks), shared with `library.changed`'s listeners.
    reads: Arc<Reads>,
    /// Where the images of the library the host asked read can be read,
    /// once shown ([`Persist::take_filled`]).
    filled: Option<Files>,
    /// The search filters' queries being run, by gallery: the photos
    /// each takes and the facet counts.
    searching: Vec<(String, Receiver<Result<Searched, String>>)>,
    /// The library's other reads through the node (the Library's filter
    /// history, the folders' counts), answered at a coming frame; shared
    /// with `library.write`'s listener (a trash asked first).
    asked: Arc<Mutex<Vec<(Asked, Pending)>>>,
}

/// A read of the library sent to the node, shown when it is answered.
enum Asked {
    /// The Library's pinned and recent filters (`library.documents.get`).
    FilterHistory,
    /// The photos counted by folder (`library.folders`).
    Folders,
    /// The albums and the quick collection (`library.albums.list`).
    Albums,
    /// What moving photos' files to the trash would do
    /// (`library.images.trash`, `plan`), to ask first.
    TrashPlan(crate::library::TrashAsk),
    /// A photo's heads, for the versions strip (`library.versions.list`).
    Versions(String),
    /// The metadata templates (`library.documents.get`).
    Templates,
    /// A photo's capture details, for the Metadata section
    /// (`library.fields.get`).
    Exif(String),
}

/// A write of the library sent to the node.
enum Write {
    /// Fields of photos (`library.fields.set`).
    Fields(Vec<Change>),
    /// A stack expanded or collapsed (`library.stacks.expand`).
    Expand(String, bool),
    /// Stacks proposed over images, every image when none
    /// (`library.stacks.suggest`).
    Suggest(Vec<String>),
    /// A proposal accepted: the groups taken, all when none
    /// (`library.stacks.accept`).
    Accept(String, Option<Vec<usize>>),
    /// A proposal declined (`library.stacks.decline`).
    Decline(String),
    /// Photos taken out of the Library (`library.images.remove`), asked
    /// at a time (seconds): one step of the library's history.
    Remove(Vec<String>, f64, crate::library::Batch),
    /// Photos turned (`library.images.rotate`), asked at a time
    /// (seconds): one step of the library's history.
    Rotate(
        Vec<String>,
        crate::library::Turn,
        f64,
        crate::library::Batch,
    ),
    /// Photos duplicated (`library.images.duplicate`): a new version of
    /// each, from its current edit or the original.
    Duplicate(Vec<String>, helios_vault::versions::DuplicateFrom),
    /// Photos' files moved to the system trash, confirmed
    /// (`library.images.trash`): a journaled job, not a history step.
    Trash(Vec<String>),
    /// One of a photo's heads made its current version
    /// (`library.versions.switch`), asked at a time (seconds): one step of
    /// the library's history.
    SwitchVersion(String, String, f64, crate::library::Batch),
    /// A copy removed (`library.versions.remove`), asked at a time
    /// (seconds): one step of the library's history.
    RemoveVersion(String, String, f64, crate::library::Batch),
    /// Photos grouped, ungrouped or a stack's top set
    /// (`library.stacks.group`, `.ungroup`, `.top`), asked at a time
    /// (seconds): one step of the library's history.
    Stack(crate::library::StackAsk, f64, crate::library::Batch),
    /// A keyword branch renamed or merged on every photo
    /// (`library.keywords.rename`, `.merge`), asked at a time (seconds):
    /// one step of the library's history.
    Keyword(crate::library::KeywordAsk, f64, crate::library::Batch),
    /// Photos' metadata set as a group (`library.meta.set`), asked at a
    /// time (seconds): one step of the library's history.
    Meta(crate::library::MetaEdit, f64, crate::library::Batch),
    /// RAW+JPEG files paired or unpaired (`library.pairs.pair`,
    /// `.unpair`), asked at a time (seconds): one step of the library's
    /// history.
    Pair(crate::library::PairAsk, f64, crate::library::Batch),
    /// A library document kept, or removed when none
    /// (`library.documents.set`).
    Document {
        ns: &'static str,
        key: &'static str,
        document: Option<Value>,
    },
    /// A workshop session's record kept, or dropped when none
    /// (`library.workshop.sessions.set`).
    Session(String, Option<Value>),
    /// An album or the quick collection changed (`library.albums.edit`),
    /// recorded in the library's history at `f64` (seconds) once done.
    Album(crate::library::AlbumEdit, f64, crate::library::Batch),
    /// A step of the edit shown (`library.edit.add`).
    Edit(Box<EditStep>),
    /// An image's snapshots kept (`library.snapshots.set`).
    Snapshots(ImageId, Vec<helios_vault::snapshots::Snapshot>),
}

/// A step of an edit session sent to the node.
struct EditStep {
    version: Version,
    in_progress: bool,
    /// The edit changes the image from its original
    /// (`crate::edit_marks::Marks::edited`), kept as its `edited` field.
    edited: bool,
    /// The session once the step is written.
    after: EditSession,
}

impl Write {
    /// The node's method and params.
    fn call(&self) -> (&'static str, Value) {
        match self {
            Self::Fields(changes) => (
                "library.fields.set",
                json!({ "changes": changes.iter().map(change_json).collect::<Vec<_>>() }),
            ),
            Self::Expand(stack, expanded) => (
                "library.stacks.expand",
                json!({ "stack": stack, "expanded": expanded }),
            ),
            Self::Suggest(images) => ("library.stacks.suggest", json!({ "images": images })),
            Self::Accept(decision, groups) => (
                "library.stacks.accept",
                json!({ "decision": decision, "groups": groups }),
            ),
            Self::Decline(decision) => ("library.stacks.decline", json!({ "decision": decision })),
            Self::Remove(images, ..) => (
                helios_node_core::library::REMOVE,
                json!({ "images": images }),
            ),
            Self::Rotate(images, turn, ..) => (
                helios_node_core::library::ROTATE,
                json!({ "images": images, "turn": turn_word(*turn) }),
            ),
            Self::Duplicate(images, from) => (
                helios_node_core::library::DUPLICATE,
                json!({ "images": images, "from": match from {
                    helios_vault::versions::DuplicateFrom::Current => "current",
                    helios_vault::versions::DuplicateFrom::Original => "original",
                } }),
            ),
            Self::Trash(images) => (
                helios_node_core::library::TRASH,
                json!({ "images": images }),
            ),
            Self::Stack(ask, ..) => {
                use crate::library::StackAsk;
                match ask {
                    StackAsk::Group { images, top } => (
                        helios_node_core::library::STACKS_GROUP,
                        json!({ "images": images, "top": top }),
                    ),
                    StackAsk::Ungroup { images } => (
                        helios_node_core::library::STACKS_UNGROUP,
                        json!({ "images": images }),
                    ),
                    StackAsk::Top { stack, image } => (
                        "library.stacks.top",
                        json!({ "stack": stack, "image": image }),
                    ),
                }
            }
            Self::Keyword(ask, ..) => {
                use crate::library::KeywordAsk;
                match ask {
                    KeywordAsk::Rename { from, to } => (
                        helios_node_core::library::KEYWORDS_RENAME,
                        json!({ "from": from, "to": to }),
                    ),
                    KeywordAsk::Merge { from, into } => (
                        helios_node_core::library::KEYWORDS_MERGE,
                        json!({ "from": from, "into": into }),
                    ),
                }
            }
            Self::Meta(edit, ..) => (helios_node_core::library::META_SET, edit.params()),
            Self::Pair(ask, ..) => {
                use crate::library::PairAsk;
                match ask {
                    PairAsk::Pair { images, exact } => (
                        helios_node_core::library::PAIRS_PAIR,
                        json!({ "images": images, "exact": exact }),
                    ),
                    PairAsk::Unpair { images } => (
                        helios_node_core::library::PAIRS_UNPAIR,
                        json!({ "images": images }),
                    ),
                }
            }
            Self::SwitchVersion(image, version, ..) => (
                helios_node_core::library::VERSIONS_SWITCH,
                json!({ "image": image, "version": version }),
            ),
            Self::RemoveVersion(image, version, ..) => (
                helios_node_core::library::VERSIONS_REMOVE,
                json!({ "image": image, "version": version }),
            ),
            Self::Document { ns, key, document } => (
                "library.documents.set",
                json!({ "ns": ns, "key": key, "document": document }),
            ),
            Self::Session(id, record) => (
                "library.workshop.sessions.set",
                json!({ "id": id, "record": record }),
            ),
            Self::Album(edit, ..) => ("library.albums.edit", album_params(edit)),
            Self::Edit(step) => (
                "library.edit.add",
                json!({
                    "version": step.version,
                    "step": step.after.steps(),
                    "inProgress": step.in_progress,
                    "edited": step.edited,
                }),
            ),
            Self::Snapshots(image, snapshots) => (
                "library.snapshots.set",
                json!({
                    "image": image.hex(),
                    "snapshots": snapshots.iter().map(helios_vault::snapshots::Snapshot::to_json).collect::<Vec<_>>(),
                }),
            ),
        }
    }

    /// The images whose fields it writes.
    fn images(&self) -> impl Iterator<Item = &str> {
        let changes = match self {
            Self::Fields(changes) => changes.as_slice(),
            Self::Expand(..)
            | Self::Suggest(_)
            | Self::Accept(..)
            | Self::Decline(_)
            | Self::Remove(..)
            | Self::Rotate(..)
            | Self::Duplicate(..)
            | Self::Trash(_)
            | Self::SwitchVersion(..)
            | Self::RemoveVersion(..)
            | Self::Stack(..)
            | Self::Keyword(..)
            | Self::Meta(..)
            | Self::Pair(..)
            | Self::Document { .. }
            | Self::Session(..)
            | Self::Album(..)
            | Self::Edit(_)
            | Self::Snapshots(..) => &[],
        };
        changes.iter().map(|c| c.image.as_str())
    }
}

/// A turn as `library.images.rotate` names it.
fn turn_word(turn: crate::library::Turn) -> &'static str {
    use crate::library::Turn;
    match turn {
        Turn::Left => "left",
        Turn::Right => "right",
        Turn::Reset => "reset",
    }
}

/// The params of `library.albums.edit` making `edit`.
fn album_params(edit: &crate::library::AlbumEdit) -> Value {
    use crate::library::AlbumEdit as E;
    match edit {
        E::Create { id, name, images } => {
            json!({ "op": "create", "id": id, "name": name, "images": images })
        }
        E::Rename { id, name } => json!({ "op": "rename", "id": id, "name": name }),
        E::Delete { id, members } => json!({ "op": "delete", "id": id, "members": members }),
        E::CreateSet { id, name, set } => {
            json!({ "op": "createSet", "id": id, "name": name, "set": set })
        }
        E::Move { id, set } => json!({ "op": "move", "id": id, "set": set }),
        E::Add { id, images } => json!({ "op": "add", "id": id, "images": images }),
        E::Remove { id, images } => json!({ "op": "remove", "id": id, "images": images }),
        E::CreateSmart { id, name, rules } => {
            json!({ "op": "createSmart", "id": id, "name": name, "rules": rules.to_json() })
        }
        E::Rules { id, rules } => json!({ "op": "rules", "id": id, "rules": rules.to_json() }),
        E::QuickToggle { id, images } => {
            json!({ "op": "quickToggle", "id": id, "images": images })
        }
        E::QuickClear { id } => json!({ "op": "quickClear", "id": id }),
        E::QuickSave {
            from,
            id,
            name,
            clear,
        } => {
            json!({ "op": "quickSave", "from": from, "id": id, "name": name, "clear": clear })
        }
        E::QuickCreate { id, name, images } => {
            json!({ "op": "quickCreate", "id": id, "name": name, "images": images })
        }
        E::QuickRename { id, name } => json!({ "op": "quickRename", "id": id, "name": name }),
        E::QuickDelete { id } => json!({ "op": "quickDelete", "id": id }),
        E::QuickTarget { id } => json!({ "op": "quickTarget", "id": id }),
        E::QuickAdd { id, images } => json!({ "op": "quickAdd", "id": id, "images": images }),
        E::QuickRemove { id, images } => {
            json!({ "op": "quickRemove", "id": id, "images": images })
        }
    }
}

/// One change of `library.fields.set`.
fn change_json(change: &Change) -> Value {
    use helios_vault::fields::Op;
    let (op, value) = match &change.op {
        Op::Set(value) => ("set", value.clone().unwrap_or(Value::Null)),
        Op::SetIfAbsent(value) => ("set_if_absent", value.clone()),
        Op::Add(name) => ("add", json!(name)),
        Op::Remove(name) => ("remove", json!(name)),
    };
    json!({ "image": change.image, "field": change.field, "op": op, "value": value })
}

/// The edit of the image shown.
struct Editing {
    /// The session as the steps made: some may still be on their way to
    /// the node.
    session: EditSession,
    /// The session as the node wrote it: what a refused step goes back to.
    confirmed: EditSession,
    /// The module each node of the shown network runs, and the
    /// parameters it has, by node name: what an edit saves.
    modules: BTreeMap<String, (String, BTreeSet<String>)>,
    /// The value store's committed steps when last saved.
    committed: u64,
    /// When the gesture going on was last saved (or started).
    flushed_at: Option<f64>,
    /// The gesture going on, by label, and who makes it.
    gesture: Option<(String, String)>,
    /// The last save failed: the head is not the edit shown.
    unsaved: bool,
    /// Why the last save failed, while it is unsaved.
    failed: Option<String>,
}

/// How many images one window of the library's cursor holds: the rows
/// the node reads in one store query, fields and locations together.
pub const PAGE: usize = helios_node_core::cursor::CHUNK;

/// How long the cursor of the library shown lives unread on the node, in
/// seconds: past it, the next refresh reads the library whole again.
const SHOWN_LEASE: u64 = 3_600;

/// The image's undo history rebuilt from its saved versions
/// (`library.edit.history`) and installed into `values`, when it fits
/// them and the image has no step yet ([`helios_values::Store::install_image_line`]):
/// whether it was.
pub fn resume_from_versions(
    values: &Mutex<helios_values::Store>,
    reader: &Reader,
    image: &str,
    unedited: &Value,
) -> Result<bool, String> {
    let answer = reader.read(
        helios_node_core::library::EDIT_HISTORY,
        json!({ "image": image }),
    )?;
    let mut store = held(values);
    let (undo, redo) = crate::rebuilt_history::steps_of(&store, &answer, unedited)?;
    Ok((!undo.is_empty() || !redo.is_empty()) && store.install_image_line(undo, redo))
}

/// The desktop's node as the library's reads reach it: its in-process
/// client, and the cache of its answers ([`ReadCache`]) the bus's events
/// keep true. Cloned to read on a thread of its own: the UI thread never
/// waits for a read.
#[derive(Clone)]
pub struct Reader {
    node: Arc<Client>,
    cache: Arc<ReadCache>,
    /// How many times the library shown was brought up to date
    /// (`cursor.refresh` asked of the node), for tests.
    refreshes: Arc<std::sync::atomic::AtomicU64>,
}

impl Reader {
    /// Reads `method` from the cache, else from the node (waiting for it):
    /// on a thread of its own, never the UI's.
    pub fn read(&self, method: &str, params: Value) -> Result<Value, String> {
        if method == "cursor.refresh" {
            self.refreshes
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.cache
            .read(&self.node, method, params)
            .map_err(|e| e.message)
    }

    /// Lets cursor `cursor` go, without waiting for the node.
    fn close(&self, cursor: u64) {
        self.cache.forget_cursor(cursor);
        drop(self.node.call("cursor.close", json!({ "cursor": cursor })));
    }

    /// The cache of the node's reads.
    pub fn cache(&self) -> &ReadCache {
        &self.cache
    }

    /// How many times the library shown was brought up to date by its
    /// cursor's delta (`cursor.refresh` asked of the node): one per fill
    /// of a library already shown.
    pub fn refreshes(&self) -> u64 {
        self.refreshes.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// The library as the vault holds it: its entries (each with its
/// presence: present, offline or missing), and where each image can be
/// read (its first present copy), for thumbnails.
pub struct LibraryRows {
    entries: Vec<Entry>,
    files: Vec<(String, PathBuf)>,
    /// The node's cursor it was read through, kept open so the library
    /// shown is refreshed by its delta ([`Persist::fill_library`]).
    cursor: Option<u64>,
    /// The order it was read in.
    order: Order,
    /// Its stacks and the proposals waiting.
    stacked: Stacked,
    /// How many heads the photos with more than one have.
    heads: HashMap<String, usize>,
}

impl LibraryRows {
    /// How many images the library holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries read, in the library's order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

/// Reads the library through the desktop's node: a cursor opened on it
/// (`cursor.open`), read a window of [`PAGE`] images at a time
/// (`cursor.window`, fields and locations together), and the photos
/// changed outside Helios (`library.changed.list`). Nothing is asked of
/// the files: presence (present, offline, missing) is what the scans
/// stored, and the file's size and date are those of its location. It
/// waits for the node: a thread of its own reads it
/// ([`Persist::show_library`] shows it).
///
/// The photos come in `order` (the library's sort), sorted by the vault in
/// the query that opens the cursor.
pub fn read_library(reader: &Reader, order: Order) -> Result<LibraryRows, String> {
    let query = Query {
        order,
        ..Query::default()
    };
    let opened = reader.read(
        "cursor.open",
        json!({ "query": query_json(&query), "lease": SHOWN_LEASE }),
    )?;
    let cursor = opened["cursor"]
        .as_u64()
        .ok_or("the node opened no cursor")?;
    let count = to_usize(&opened["count"]);
    let read = read_cursor(reader, cursor, count);
    let read = read.and_then(|read| Ok((read, read_stacked(reader)?)));
    let read = read.and_then(|read| Ok((read, read_heads(reader)?)));
    if read.is_err() {
        reader.close(cursor);
    }
    let (((entries, files), stacked), heads) = read?;
    Ok(LibraryRows {
        entries,
        files,
        cursor: Some(cursor),
        order,
        stacked,
        heads,
    })
}

/// How many heads the photos with more than one have, as the node
/// counts them (`library.versions.heads`). Waits for the node.
fn read_heads(reader: &Reader) -> Result<HashMap<String, usize>, String> {
    let read = reader.read(helios_node_core::library::HEADS, json!({}))?;
    Ok(read["heads"]
        .as_object()
        .map(|heads| {
            heads
                .iter()
                .map(|(id, n)| (id.clone(), to_usize(n)))
                .collect()
        })
        .unwrap_or_default())
}

/// The library's stacks and the `stacks.suggest` proposals, as the node
/// lists them.
struct Stacked {
    stacks: Vec<helios_vault::Stack>,
    decisions: Vec<helios_library::stacks::Decision>,
    /// How many heads the photos with more than one have: a copy made,
    /// removed or brought back by an undo changes them without moving
    /// the library.
    heads: HashMap<String, usize>,
}

/// Reads the stacks and the proposals through the node
/// (`library.stacks.list`, `library.stacks.decisions`): the read cache
/// keeps both until a stack or a decision changes. Waits for the node.
fn read_stacked(reader: &Reader) -> Result<Stacked, String> {
    let listed = reader.read("library.stacks.list", json!({}))?;
    let stacks = serde_json::from_value(listed["stacks"].clone())
        .map_err(|e| format!("the node's stacks: {e}"))?;
    let waiting = reader.read("library.stacks.decisions", json!({}))?;
    let decisions = serde_json::from_value(waiting["decisions"].clone())
        .map_err(|e| format!("the node's stack proposals: {e}"))?;
    Ok(Stacked {
        stacks,
        decisions,
        heads: read_heads(reader)?,
    })
}

/// [`read_stacked`] once the read cache heard the bus up to `seq`, so a
/// change announced before is never shown older (a cache that cannot
/// follow is read as it stands: the node is asked whatever it dropped).
fn read_stacked_after(reader: &Reader, seq: u64) -> Result<Stacked, String> {
    reader
        .cache
        .wait_seen(seq, std::time::Duration::from_secs(10));
    read_stacked(reader)
}

fn to_usize(value: &Value) -> usize {
    value
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
}

/// Where each image can be read, by id.
type Files = Vec<(String, PathBuf)>;

/// Every image of a cursor as the library shows it, window by window.
fn read_cursor(reader: &Reader, cursor: u64, count: usize) -> Result<(Vec<Entry>, Files), String> {
    let changed = changed_outside(reader)?;
    let mut entries = Vec::with_capacity(count);
    let mut files = Vec::with_capacity(count);
    for offset in (0..count).step_by(PAGE) {
        let window = reader.read(
            "cursor.window",
            json!({ "cursor": cursor, "offset": offset, "len": PAGE,
                    "fields": SHOWN, "locations": true }),
        )?;
        for row in window["rows"].as_array().into_iter().flatten() {
            let (entry, path) = entry_of(row_of(row)?, &changed);
            if let Some(path) = path {
                files.push((entry.id.clone(), path));
            }
            entries.push(entry);
        }
    }
    Ok((entries, files))
}

/// The ids a cursor holds from `offset`, `len` of them, in its order.
fn cursor_ids(reader: &Reader, cursor: u64, len: usize) -> Result<Vec<String>, String> {
    let window = helios_node_core::cursor::MAX_WINDOW;
    let mut ids = Vec::with_capacity(len);
    for offset in (0..len).step_by(window) {
        let read = reader.read(
            "cursor.window",
            json!({ "cursor": cursor, "offset": offset, "len": window, "fields": [] }),
        )?;
        ids.extend(
            read["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| r["id"].as_str().map(String::from)),
        );
    }
    Ok(ids)
}

/// The photos another application changed, until acknowledged.
fn changed_outside(reader: &Reader) -> Result<HashMap<String, u64>, String> {
    let listed = reader.read("library.changed.list", json!({}))?;
    Ok(listed["images"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| Some((i["image"].as_str()?.to_string(), i["at"].as_u64()?)))
        .collect())
}

/// A row of a window (or of `library.fields.get`) read with its
/// locations.
fn row_of(row: &Value) -> Result<LibraryRow, String> {
    let location = |l: &Value| Location {
        path: PathBuf::from(l["path"].as_str().unwrap_or_default()),
        size: l["size"].as_u64().unwrap_or(0),
        modified: l["modified"].as_u64().unwrap_or(0),
        last_verified: l["last_verified"].as_u64().unwrap_or(0),
        volume: l["volume"]
            .as_str()
            .map(|v| helios_vault::storage::VolumeId(v.to_string())),
        rel_path: l["rel_path"].as_str().map(PathBuf::from),
        presence: Presence::parse(l["presence"].as_str().unwrap_or_default()),
    };
    Ok(LibraryRow {
        id: ImageId::from_hex(row["id"].as_str().unwrap_or_default())?,
        fields: row["fields"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        locations: row["locations"]
            .as_array()
            .into_iter()
            .flatten()
            .map(location)
            .collect(),
    })
}

/// An image as the library shows it, and where it can be read: its first
/// present copy, the node listing them in read order (the library's copy
/// before a card's file, `helios_vault::ReadOrder`).
fn entry_of(row: LibraryRow, changed: &HashMap<String, u64>) -> (Entry, Option<PathBuf>) {
    let presence = Presence::of(&row.locations);
    // The volume to plug in, for an image with no copy at hand.
    let offline_on = row
        .locations
        .iter()
        .find(|l| l.presence == Presence::Offline)
        .and_then(|l| l.volume.as_ref())
        .map(|v| v.as_str().to_string());
    let present = row.locations.into_iter().find(Location::present);
    let id = row.id.hex();
    let mut entry = Entry {
        id: id.clone(),
        presence,
        offline_on,
        changed_outside: changed.get(&id).copied(),
        name: present
            .as_ref()
            .and_then(|l| l.path.file_name())
            .map_or_else(|| id[..12].to_string(), |n| n.to_string_lossy().into()),
        ..Entry::default()
    };
    // The file's size and date as its location holds them (what the
    // import or the last scan saw), for the library's details.
    if let Some(l) = &present {
        entry.bytes = Some(l.size);
        entry.modified = i64::try_from(l.modified).ok().filter(|&m| m > 0);
    }
    for field in SHOWN {
        entry.show(field, row.fields.get(field));
    }
    (entry, present.map(|l| l.path))
}

/// The library shown: the node's cursor it was read through and where
/// each of its images can be read.
struct Shown {
    cursor: u64,
    files: BTreeMap<String, PathBuf>,
    /// The order its cursor sorts by.
    order: Order,
}

/// What a read of the library brings back to the UI thread.
enum Fetched {
    /// Read whole, through a new cursor.
    Whole(LibraryRows),
    /// The cursor shown, brought up to date: what moved, if anything did,
    /// and the stacks and proposals read again.
    Delta(Shown, Option<Moved>, Stacked),
}

/// What moved in the library shown since it was read.
struct Moved {
    /// Every image now, in the library's order.
    order: Vec<String>,
    removed: Vec<String>,
    /// The images read again: added, their copies moved, or not shown.
    read: Vec<(Entry, Option<PathBuf>)>,
    changed: HashMap<String, u64>,
}

/// Reads the library in `order`: the cursor `shown` brought up to date
/// when it is still in that order (the images `known` are shown already),
/// else the library read whole through a new cursor. Waits for the node.
fn fetch(
    reader: &Reader,
    shown: Option<Shown>,
    order: Order,
    known: &BTreeSet<String>,
) -> Result<Fetched, String> {
    if let Some(mut shown) = shown {
        if shown.order == order {
            match refresh(reader, &mut shown, known)
                .and_then(|moved| Ok((moved, read_stacked(reader)?)))
            {
                Ok((moved, stacked)) => return Ok(Fetched::Delta(shown, moved, stacked)),
                // Read whole again, through a new cursor.
                Err(_) => reader.close(shown.cursor),
            }
        } else {
            reader.close(shown.cursor);
        }
    }
    read_library(reader, order).map(Fetched::Whole)
}

/// Brings the cursor shown up to date (`cursor.refresh`): nothing is read
/// when the catalog did not change, else the images added and those whose
/// copies moved (an import, a scan, an undo), in windows of [`PAGE`].
fn refresh(
    reader: &Reader,
    shown: &mut Shown,
    known: &BTreeSet<String>,
) -> Result<Option<Moved>, String> {
    let delta = reader.read("cursor.refresh", json!({ "cursor": shown.cursor }))?;
    if delta["changed"] != true {
        return Ok(None);
    }
    // The windows kept of the old snapshot no longer page it.
    reader.cache.forget_cursor(shown.cursor);
    let ids = |name: &str| -> Vec<String> {
        delta[name]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    };
    let (inserted, replaced, removed) = (ids("inserted"), ids("replaced"), ids("removed"));
    for id in &removed {
        shown.files.remove(id);
    }
    let order = cursor_ids(reader, shown.cursor, to_usize(&delta["count"]))?;
    // The images added, those whose copies moved, and any the library
    // does not show (it was filled from elsewhere).
    let mut wanted: Vec<String> = inserted.clone();
    wanted.extend(replaced);
    wanted.extend(
        order
            .iter()
            .filter(|id| !known.contains(*id) && !inserted.contains(id))
            .cloned(),
    );
    let changed = changed_outside(reader)?;
    let mut read = Vec::with_capacity(wanted.len());
    for ids in wanted.chunks(PAGE) {
        let rows = reader.read(
            "library.fields.get",
            json!({ "images": ids, "fields": SHOWN, "locations": true }),
        )?;
        for row in rows["rows"].as_array().into_iter().flatten() {
            let (entry, path) = entry_of(row_of(row)?, &changed);
            match &path {
                Some(path) => shown.files.insert(entry.id.clone(), path.clone()),
                None => shown.files.remove(&entry.id),
            };
            read.push((entry, path));
        }
    }
    Ok(Some(Moved {
        order,
        removed,
        read,
        changed,
    }))
}

/// The photos a query takes, in its order, and how many of them carry
/// each value of each facet: `library.facets`, then a cursor opened on
/// the query and read for its ids. Waits for the node.
fn search(
    reader: &Reader,
    query: &Query,
    rule: Option<&helios_vault::fields::Backups>,
) -> Result<(Vec<String>, helios_vault::Facets), String> {
    let query = query_json(query);
    // No rule, no `rule` term: the node's schema takes an object, never
    // null, and refuses the whole call otherwise.
    let mut params = json!({ "query": query.clone() });
    if let Some(rule) = rule {
        params["rule"] = helios_node_core::cursor::backups_json(rule);
    }
    let counted = reader.read("library.facets", params)?;
    let facets = serde_json::from_value(counted["facets"].clone())
        .map(helios_vault::Facets)
        .map_err(|e| format!("the node's facet counts: {e}"))?;
    let opened = reader.read("cursor.open", json!({ "query": query }))?;
    let cursor = opened["cursor"]
        .as_u64()
        .ok_or("the node opened no cursor")?;
    let ids = cursor_ids(reader, cursor, to_usize(&opened["count"]));
    reader.close(cursor);
    Ok((ids?, facets))
}

/// A read of the library going on, on a thread of its own.
struct Filling {
    answer: Receiver<Result<Fetched, String>>,
    /// The host asked it, for the thumbnails ([`Persist::take_filled`]).
    host: bool,
}

/// The library's reads off the interface's thread that `library.changed`'s
/// listeners ask (`library.fill`, `library.stacks`) inside the pass, and
/// [`Persist`]'s frame shows once answered: shared between them (`Arc`:
/// [`Persist`] is built on the boot's thread).
struct Reads {
    reader: Reader,
    bus: Arc<Bus>,
    /// The library shown, refreshed by its cursor's delta.
    shown: Mutex<Option<Shown>>,
    /// The library being read again ([`Reads::ask_fill`]).
    filling: Mutex<Option<Filling>>,
    /// Asked again while it was read (true: by the host): read once more.
    fill_again: Mutex<Option<bool>>,
    /// The stacks and the proposals waiting being read again
    /// ([`Reads::ask_stacks`]).
    stacking: Mutex<Option<Receiver<Result<Stacked, String>>>>,
    /// Asked again while they were read: read once more.
    stack_again: AtomicBool,
}

/// `m` locked, even when a thread panicked holding it.
fn held<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Reads {
    /// Reads `library` again as [`Persist::fill_library`] does, on a
    /// thread of its own: a coming frame shows it. `host`: the host takes
    /// where its images can be read once shown ([`Persist::take_filled`]).
    /// Asked while a read goes on, it is read once more after it.
    fn ask_fill(&self, library: &crate::library::Library, host: bool) {
        let mut filling = held(&self.filling);
        if filling.is_some() {
            let mut again = held(&self.fill_again);
            *again = Some(again.unwrap_or(false) || host);
            return;
        }
        let shown = self.shown.lock().expect("library lock").take();
        let (reader, order, known) = (self.reader.clone(), library.sort(), known_in(library));
        let answer = spawn("helios-library-fill", move || {
            fetch(&reader, shown, order, &known)
        });
        *filling = Some(Filling { answer, host });
    }

    /// Reads the stacks and the proposals waiting again through the node,
    /// on a thread of its own: a coming frame shows them. Asked while a
    /// read goes on, they are read once more after it.
    fn ask_stacks(&self) {
        let mut stacking = held(&self.stacking);
        if stacking.is_some() {
            self.stack_again.store(true, Ordering::Relaxed);
            return;
        }
        let (reader, seq) = (self.reader.clone(), self.bus.last_seq());
        *stacking = Some(spawn("helios-stacks-read", move || {
            read_stacked_after(&reader, seq)
        }));
    }
}

/// Runs `read` on a thread of its own; its answer comes on the receiver.
fn spawn<T: Send + 'static>(
    name: &str,
    read: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Receiver<Result<T, String>> {
    let (send, answer) = std::sync::mpsc::channel();
    let sender = send.clone();
    let spawned = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _ = sender.send(read());
        });
    if let Err(e) = spawned {
        let _ = send.send(Err(format!("no thread to read the library: {e}")));
    }
    answer
}

/// The images the library shows.
fn known(shell: &Shell) -> BTreeSet<String> {
    known_in(&shell.library())
}

/// The photos `library` shows.
fn known_in(library: &crate::library::Library) -> BTreeSet<String> {
    library.entries.iter().map(|e| e.id.clone()).collect()
}

/// Shows the stacks in the library and the proposals waiting in the
/// Stacks panel.
fn show_stacked(shell: &mut Shell, stacked: Stacked) {
    shell.library_mut().set_stacks(&stacked.stacks);
    shell.stack_review_mut().set_decisions(stacked.decisions);
}

/// Drops the workshop sessions a former run left unkept, through the
/// node `node` (`library.workshop.sessions.list`, then `.set` for each):
/// how many were dropped. Waits for the node, so it runs on a thread of
/// its own, never the UI's.
///
/// # Errors
/// The node's refusal, or a node that does not answer.
pub fn drop_unkept_sessions(node: &Client) -> Result<usize, String> {
    let listed = node
        .request("library.workshop.sessions.list", json!({}))
        .map_err(|e| e.message)?;
    let mut n = 0;
    for session in listed["sessions"].as_array().into_iter().flatten() {
        if session["record"]["kept"].as_bool() == Some(true) {
            continue;
        }
        let Some(id) = session["id"].as_str() else {
            continue;
        };
        node.request(
            "library.workshop.sessions.set",
            json!({ "id": id, "record": null }),
        )
        .map_err(|e| e.message)?;
        n += 1;
    }
    Ok(n)
}

/// Shows a read of [`Asked`] the node answered.
fn show_asked(shell: &mut Shell, asked: &Asked, answer: Result<Value, helios_inspect::rpc::Error>) {
    let answer = match answer {
        Ok(answer) => answer,
        Err(e) => {
            match asked {
                // The filters' history is a convenience: a warning.
                Asked::FilterHistory => shell.notify(Level::Warning, e.message),
                Asked::Folders
                | Asked::Albums
                | Asked::Versions(_)
                | Asked::Templates
                | Asked::Exif(_) => {
                    unreadable(shell, e.message);
                }
                Asked::TrashPlan(_) => not_trashed(shell, &e.message),
            }
            return;
        }
    };
    match asked {
        Asked::FilterHistory => {
            use crate::recent_filters::FilterHistory;
            let doc = Some(&answer["document"]).filter(|d| !d.is_null());
            shell
                .library_mut()
                .set_filter_history(FilterHistory::of(doc));
        }
        Asked::Folders => match folders_of(&answer) {
            Ok(folders) => shell.library_mut().set_folders(folders),
            Err(e) => unreadable(shell, e),
        },
        Asked::Albums => {
            let albums = answer["albums"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(helios_vault::collections::Album::from_json)
                .collect();
            let quick = answer["quick"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect();
            // A node from before named quick collections answers none:
            // the first one, holding `quick`, is the target.
            let quicks: Vec<helios_vault::collections::QuickCollection> =
                serde_json::from_value(answer["quicks"].clone()).unwrap_or_default();
            let target = answer["target"]
                .as_str()
                .unwrap_or(helios_vault::collections::QUICK)
                .to_string();
            shell.library_mut().set_quick(quick);
            shell.library_mut().set_quick_collections(quicks, target);
            shell.library_mut().set_albums(albums);
        }
        Asked::TrashPlan(ask) => ask_trash(shell, ask, &answer),
        Asked::Versions(image) => {
            let heads = answer["heads"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(crate::library::VersionHead::from_json)
                .collect();
            shell.library_mut().set_versions(image.clone(), heads);
        }
        Asked::Templates => {
            let doc = Some(&answer["document"]).filter(|d| !d.is_null());
            shell
                .library_mut()
                .set_templates(crate::library::Template::list_of(doc));
        }
        Asked::Exif(image) => {
            let fields = answer["rows"][0]["fields"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            shell.library_mut().set_exif(image.clone(), fields);
        }
    }
}

/// Records a step of a photo's versions (`library.versions.switch` or
/// `remove`) in the library's history, so Ctrl+Z takes it back.
fn record_versions_step(
    shell: &mut Shell,
    answer: &Value,
    at: f64,
    removed: bool,
    batch: &crate::library::Batch,
) {
    match serde_json::from_value::<helios_vault::Changes>(answer["changes"].clone()) {
        Ok(changes) if changes.is_empty() => {}
        Ok(changes) => {
            let label = crate::library_history::version_label(removed);
            shell
                .library_history()
                .record_in(batch.as_ref(), label, changes, at);
        }
        Err(e) => shell.notify(
            Level::Warning,
            format!("the photo's versions cannot be set back with undo: {e}"),
        ),
    }
}

/// A write the Library asked, heard on `library.write`: added to the
/// writes waiting their turn, asked at the time it is heard; a trash not
/// confirmed asks the node what it would do first, a version's removal
/// not confirmed asks the user.
fn write_asked(
    writes: &Mutex<VecDeque<Write>>,
    asked: &Mutex<Vec<(Asked, Pending)>>,
    node: &Client,
    library: &crate::shell::LibraryParts,
    write: crate::library_write::WriteAsk,
) {
    use crate::library::VersionAsk;
    use crate::library_write::WriteAsk as W;
    let now = seconds_now();
    let write = match write {
        W::Remove(images, batch) => Write::Remove(images, now, batch),
        W::Rotate(images, turn, batch) => Write::Rotate(images, turn, now, batch),
        W::Duplicate(images, from) => Write::Duplicate(images, from),
        W::Trash(ask) if ask.confirmed => Write::Trash(ask.images),
        W::Trash(ask) => {
            // Asked first: the node says which files would go and which
            // stay, then the question names them.
            let pending = node.call(
                helios_node_core::library::TRASH,
                json!({ "images": &ask.images, "plan": true }),
            );
            held(asked).push((Asked::TrashPlan(ask), pending));
            return;
        }
        W::Stack(ask, batch) => Write::Stack(ask, now, batch),
        W::Keyword(ask, batch) => Write::Keyword(ask, now, batch),
        W::Meta(edit, batch) => Write::Meta(edit, now, batch),
        W::Pair(ask, batch) => Write::Pair(ask, now, batch),
        W::Version(VersionAsk::Switch { image, version }, batch) => {
            Write::SwitchVersion(image, version, now, batch)
        }
        W::Version(
            VersionAsk::Remove {
                image,
                version,
                confirmed: true,
            },
            batch,
        ) => Write::RemoveVersion(image, version, now, batch),
        W::Version(VersionAsk::Remove { image, version, .. }, _) => {
            ask_remove_version(&library.library(), &image, &version);
            return;
        }
        W::Album(edit, batch) => Write::Album(edit, now, batch),
        W::Expand(stack, expanded) => Write::Expand(stack, expanded),
        W::Templates(list) => Write::Document {
            ns: helios_vault::meta::TEMPLATES_NS,
            key: helios_vault::meta::TEMPLATES_KEY,
            document: Some(crate::library::Template::document(&list)),
        },
    };
    held(writes).push_back(write);
}

/// Asks before removing a copy of a photo: its number and name, in the
/// dialog the shell answers after the drawing.
fn ask_remove_version(library: &crate::library::Library, image: &str, version: &str) {
    let Some(app) = egui_pop::api::bus::asking() else {
        log::error!(target: "helios", "a version's removal asked outside a pass: not asked");
        return;
    };
    let ctx = app.egui();
    let text = |m: Message| crate::i18n::text(&ctx, m);
    let head = library
        .versions_of(image)
        .and_then(|heads| heads.iter().find(|h| h.version == version))
        .map_or(0, |h| h.number);
    let confirm = crate::dialogs::Confirm {
        title: text(
            Message::new("gui-version-remove-title", "Remove version { $number }?")
                .with("number", head),
        ),
        detail: text(Message::new(
            "gui-version-remove-detail",
            "This copy of the photo and the edits only it holds leave the Library; the file and the other versions stay. Undo brings it back.",
        )),
        verb: text(Message::new("gui-version-remove-verb", "Remove Version")),
        danger: true,
        command: crate::library::REMOVE_VERSION_COMMAND.to_owned(),
        args: json!({ "image": image, "version": version, "confirmed": true }),
    };
    crate::dialogs::request(&ctx, crate::dialogs::Request::Confirm(confirm));
}

/// Says the files were not moved to the trash, and why.
fn not_trashed(shell: &mut Shell, error: &str) {
    shell.notify(
        Level::Warning,
        Message::new(
            "gui-trash-failed",
            "The files were not moved to the Trash: { $error }",
        )
        .with("error", error),
    );
}

/// The refusals of a trash plan or job, in one line: how many, and the
/// first reason.
fn refusals(answer: &Value) -> Option<Message> {
    let refused = answer["refused"].as_array().filter(|r| !r.is_empty())?;
    let reason = refused[0]["reason"].as_str().unwrap_or_default();
    Some(
        Message::new(
            "gui-trash-refused",
            "{ $count } files stay where they are: { $reason }",
        )
        .with("count", refused.len())
        .with("reason", reason),
    )
}

/// Asks before moving files to the trash, naming how many and where;
/// says so when nothing can go.
fn ask_trash(shell: &mut Shell, ask: &crate::library::TrashAsk, plan: &Value) {
    use crate::library::{DELETE_REJECTED_COMMAND, TRASH_COMMAND};
    let files = plan["files"].as_u64().unwrap_or(0);
    if files == 0 {
        let why = refusals(plan).map_or_else(String::new, |m| shell.text(m));
        shell.notify(
            Level::Warning,
            Message::new(
                "gui-trash-nothing",
                "No file can be moved to the Trash. { $why }",
            )
            .with("why", why),
        );
        return;
    }
    let folders: Vec<&str> = plan["folders"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let place = match folders.as_slice() {
        [one] => (*one).to_string(),
        many => shell.text(
            Message::new("gui-trash-folders", "{ $count } folders").with("count", many.len()),
        ),
    };
    let count = ask.images.len();
    let title = if ask.rejected {
        Message::new(
            "gui-trash-rejected-title",
            "Delete { $count } rejected photos?",
        )
    } else {
        Message::new("gui-trash-title", "Move { $count } photos to the Trash?")
    }
    .with("count", count);
    let mut detail = shell.text(
        Message::new(
            "gui-trash-detail",
            "Their { $files } files in { $place } go to the system Trash, which can restore them; Undo cannot, and they leave the Library.",
        )
        .with("files", files)
        .with("place", place),
    );
    if let Some(m) = refusals(plan) {
        detail = format!("{detail} {}", shell.text(m));
    }
    let confirm = crate::dialogs::Confirm {
        title: shell.text(title),
        detail,
        verb: shell.text(Message::new("gui-trash-verb", "Move to Trash")),
        danger: true,
        command: if ask.rejected {
            DELETE_REJECTED_COMMAND
        } else {
            TRASH_COMMAND
        }
        .to_owned(),
        args: json!({ "images": ask.images, "confirmed": true }),
    };
    shell.confirm(confirm);
}

/// The photos counted by folder, from `library.folders`'s answer.
fn folders_of(answer: &Value) -> Result<helios_vault::VolumeFolders, String> {
    let mut folders = helios_vault::VolumeFolders::new();
    for volume in answer["folders"].as_array().into_iter().flatten() {
        let id = volume["volume"]
            .as_str()
            .map(|v| helios_vault::storage::VolumeId(v.to_string()));
        let counts = serde_json::from_value(volume["folders"].clone())
            .map_err(|e| format!("the folders' counts: {e}"))?;
        folders.insert(id, counts);
    }
    Ok(folders)
}

/// Says the library could not be read.
fn unreadable(shell: &mut Shell, error: String) {
    shell.notify(
        Level::Error,
        Message::new(
            "gui-vault-unreadable",
            "Cannot read the library: { $error }",
        )
        .with("error", error),
    );
}

impl<S: Store + 'static> Persist<S> {
    /// A library on `store` of its own, announcing its changes on `bus`
    /// (tests and tools).
    pub fn new(store: S, bus: Arc<Bus>) -> Self {
        Self::on_library(helios_node_core::desktop::Library::on_store(store, bus))
    }

    /// The link to the node's `library`: the node's service in this
    /// process, reached through an in-process client.
    pub fn on_library(library: helios_node_core::desktop::Library<S>) -> Self {
        let bus = library.bus().clone();
        let events = bus.subscribe();
        let service = library.service(&format!("desktop-{}", std::process::id()));
        let node = Arc::new(Client::in_process(Arc::new(service)).with_origin(WHO));
        let cache = Arc::new(ReadCache::new());
        // The cache hears the vault's events on a thread of its own, so a
        // read on any thread never shows a value older than a change
        // announced (docs/protocol.md, "A client cache").
        let heard = bus.subscribe();
        let fed = cache.clone();
        let following = std::thread::Builder::new()
            .name("helios-read-cache".into())
            .spawn(move || {
                for event in heard {
                    fed.event(&json!({
                        "seq": event.seq, "kind": event.kind, "payload": event.payload,
                    }));
                }
            });
        if following.is_err() {
            // Unfollowed, it keeps nothing.
            cache.reset("no thread to follow the vault's events");
        }
        let reader = Reader {
            node: node.clone(),
            cache,
            refreshes: Arc::default(),
        };
        let reads = Arc::new(Reads {
            reader: reader.clone(),
            bus: bus.clone(),
            shown: Mutex::new(None),
            filling: Mutex::new(None),
            fill_again: Mutex::new(None),
            stacking: Mutex::new(None),
            stack_again: AtomicBool::new(false),
        });
        Self {
            library,
            bus,
            events,
            editing: None,
            desk: Arc::default(),
            darkroom: None,
            changes: crate::library_changed::Publisher::channel(),
            listening: None,
            asking: Mutex::new(None),
            node,
            writes: Arc::default(),
            writing: None,
            reading: Vec::new(),
            reader,
            reads,
            filled: None,
            searching: Vec::new(),
            asked: Arc::default(),
        }
    }

    /// The library's reads through the node, for a thread of its own
    /// ([`read_library`]).
    pub fn reader(&self) -> Reader {
        self.reader.clone()
    }

    /// The client of the desktop's node the library's writes go through.
    pub fn node(&self) -> &Arc<Client> {
        &self.node
    }

    /// Whether a call to the node is still to be answered: the host
    /// comes back at the next frame to pick its answer up.
    pub fn busy(&self) -> bool {
        self.writing.is_some()
            || !held(&self.writes).is_empty()
            || !self.reading.is_empty()
            || held(&self.reads.filling).is_some()
            || !self.searching.is_empty()
            || held(&self.reads.stacking).is_some()
            || !held(&self.asked).is_empty()
    }

    /// Waits until the node has answered every call made, and shows what
    /// they did: before Helios closes, so nothing the user changed is
    /// left unwritten, and in tests that look at the vault right after.
    pub fn settle(&mut self, shell: &mut Shell) {
        // A node that never answers is not waited for forever.
        let deadline = std::time::Instant::now() + std::time::Duration::from_mins(1);
        loop {
            while self.busy() && std::time::Instant::now() < deadline {
                self.pump(shell);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            // What the events heard ask read again (the stacks) is waited
            // for too.
            self.hear(shell);
            if !self.busy() || std::time::Instant::now() >= deadline {
                break;
            }
        }
    }

    /// [`Persist::frame`], then [`Persist::settle`].
    pub fn frame_settled(&mut self, shell: &mut Shell, now: f64) {
        self.frame(shell, now);
        self.settle(shell);
    }

    /// Picks up the node's answers that came and sends the next write.
    fn pump(&mut self, shell: &mut Shell) {
        if let Some(answer) = self.writing.as_ref().and_then(|(_, p)| p.poll()) {
            let (write, _) = self.writing.take().expect("a write runs");
            self.written(shell, write, answer);
        }
        if self.writing.is_none()
            && let Some(write) = held(&self.writes).pop_front()
        {
            let (method, params) = write.call();
            let pending = self.node.call(method, params);
            self.writing = Some((write, pending));
        }
        let mut still = Vec::new();
        for (images, pending) in std::mem::take(&mut self.reading) {
            match pending.poll() {
                None => still.push((images, pending)),
                Some(answer) => self.show_read(shell, &images, answer),
            }
        }
        self.reading = still;
        let mut still = Vec::new();
        let asked = std::mem::take(&mut *held(&self.asked));
        for (asked, pending) in asked {
            match pending.poll() {
                None => still.push((asked, pending)),
                Some(answer) => show_asked(shell, &asked, answer),
            }
        }
        // Asked meanwhile (shown answers ask again): kept after.
        let mut asked = held(&self.asked);
        still.append(&mut asked);
        *asked = still;
        drop(asked);
        self.pump_reads(shell);
    }

    /// Shows the library's reads that came: the library read again, the
    /// search filter's photos and facet counts.
    fn pump_reads(&mut self, shell: &mut Shell) {
        use std::sync::mpsc::TryRecvError;
        let filled = held(&self.reads.filling).as_ref().and_then(|filling| {
            let read = match filling.answer.try_recv() {
                Ok(read) => Some(read),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("the library's reading stopped (a panic)".to_string()))
                }
            };
            read.map(|read| (read, filling.host))
        });
        if let Some((read, host)) = filled {
            *held(&self.reads.filling) = None;
            match read.and_then(|fetched| self.apply(shell, fetched)) {
                Ok(files) => {
                    if host {
                        self.filled = Some(files);
                    }
                    // What follows the Library shown: in this pass.
                    shell.library_filled();
                }
                Err(e) => unreadable(shell, e),
            }
            let again = held(&self.reads.fill_again).take();
            if let Some(host) = again {
                self.ask_fill(shell, host);
            }
        }
        let mut i = 0;
        while i < self.searching.len() {
            let read = match self.searching[i].1.try_recv() {
                Ok(read) => Some(read),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("the search stopped (a panic)".to_string()))
                }
            };
            let Some(read) = read else {
                i += 1;
                continue;
            };
            let (gallery, _) = self.searching.swap_remove(i);
            match read {
                Ok((ids, facets)) => shell.library_mut().set_answer(&gallery, facets, ids),
                Err(e) => unreadable(shell, e),
            }
        }
        let stacked =
            held(&self.reads.stacking)
                .as_ref()
                .and_then(|stacking| match stacking.try_recv() {
                    Ok(read) => Some(read),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => {
                        Some(Err("the stacks' reading stopped (a panic)".to_string()))
                    }
                });
        if let Some(read) = stacked {
            *held(&self.reads.stacking) = None;
            match read {
                Ok(stacked) => show_stacked(shell, stacked),
                Err(e) => shell.notify(Level::Error, e),
            }
            if self.reads.stack_again.swap(false, Ordering::Relaxed) {
                self.ask_stacks();
            }
        }
    }

    /// A write answered: a refusal said, the written images read back.
    fn written(
        &mut self,
        shell: &mut Shell,
        write: Write,
        answer: Result<Value, helios_inspect::rpc::Error>,
    ) {
        let write = match write {
            Write::Edit(step) => return self.edit_written(shell, *step, answer),
            other => other,
        };
        match (&write, answer) {
            (Write::Snapshots(..), Err(e)) => shell.notify(
                Level::Error,
                Message::new(
                    "gui-snapshots-not-saved",
                    "The snapshots were not saved: { $error }",
                )
                .with("error", e.message),
            ),
            (Write::Remove(_, at, batch), Ok(answer)) => {
                // The step the node made, recorded so Ctrl+Z puts the
                // photos back; the Library reads itself again on the
                // vault's event.
                match serde_json::from_value::<helios_vault::Changes>(answer["changes"].clone()) {
                    Ok(changes) => {
                        let count = changes.images.len();
                        let label = crate::library_history::remove_label(count);
                        shell
                            .library_history()
                            .record_in(batch.as_ref(), label, changes, *at);
                    }
                    Err(e) => shell.notify(
                        Level::Warning,
                        format!("the photos taken out cannot be put back with undo: {e}"),
                    ),
                }
            }
            (Write::SwitchVersion(image, _, at, batch), Ok(answer)) => {
                // Recorded so Ctrl+Z picks the version before; the strip
                // and the photo's picture follow.
                record_versions_step(shell, &answer, *at, false, batch);
                shell.library_mut().versions_changed();
                shell.library_mut().edit_changed(image);
            }
            (Write::RemoveVersion(image, _, at, batch), Ok(answer)) => {
                record_versions_step(shell, &answer, *at, true, batch);
                let heads = to_usize(&answer["heads"]);
                let mut lib = shell.library_mut();
                lib.set_heads_of(image, heads);
                lib.versions_changed();
                lib.edit_changed(image);
            }
            (Write::Stack(ask, at, batch), Ok(answer)) => {
                // The step the node made, recorded so Ctrl+Z takes it
                // back; the stacks are read again below.
                match serde_json::from_value::<helios_vault::Changes>(answer["changes"].clone()) {
                    Ok(changes) if changes.is_empty() => {}
                    Ok(changes) => {
                        let label = crate::library_history::stack_label(ask);
                        shell
                            .library_history()
                            .record_in(batch.as_ref(), label, changes, *at);
                    }
                    Err(e) => shell.notify(
                        Level::Warning,
                        format!("the stack cannot be set back with undo: {e}"),
                    ),
                }
            }
            (Write::Pair(ask, at, batch), Ok(answer)) => {
                log::info!(
                    target: "helios",
                    "library pairing ({ask:?}): paired {}, unpaired {}, conflicts {}",
                    answer["paired"],
                    answer["unpaired"],
                    answer["conflicts"]
                );
                // The step the node made, recorded so Ctrl+Z takes it
                // back; the Library reads itself again on the vault's
                // event. Marks the merge chose between are said.
                match serde_json::from_value::<helios_vault::Changes>(answer["changes"].clone()) {
                    Ok(changes) if changes.is_empty() => {}
                    Ok(changes) => {
                        let count = to_usize(&answer["paired"]).max(to_usize(&answer["unpaired"]));
                        let label = crate::library_history::pair_label(ask, count);
                        shell
                            .library_history()
                            .record_in(batch.as_ref(), label, changes, *at);
                    }
                    Err(e) => shell.notify(
                        Level::Warning,
                        format!("the pairing cannot be set back with undo: {e}"),
                    ),
                }
                let conflicts = to_usize(&answer["conflicts"]);
                if conflicts > 0 {
                    shell.notify(
                        Level::Info,
                        Message::new(
                            "gui-library-pair-conflicts",
                            "{ $pairs } pairs made one photo; { $count } marks set differently kept the RAW's",
                        )
                        .with("pairs", to_usize(&answer["paired"]))
                        .with("count", conflicts),
                    );
                }
            }
            (Write::Pair(ask, ..), Err(e)) => {
                log::warn!(target: "helios", "library pairing ({ask:?}) refused: {}", e.message);
                shell.notify(
                    Level::Warning,
                    Message::new(
                        "gui-library-pair-not-changed",
                        "The files were not paired or unpaired: { $error }",
                    )
                    .with("error", e.message),
                );
            }
            (Write::Stack(..), Err(e)) => shell.notify(
                Level::Warning,
                Message::new(
                    "gui-library-stack-not-changed",
                    "The stack was not changed: { $error }",
                )
                .with("error", e.message),
            ),
            (Write::Keyword(ask, at, batch), Ok(answer)) => {
                // The step the node made, recorded so Ctrl+Z moves the
                // branch back; the vault's events show the photos' tags.
                match serde_json::from_value::<helios_vault::Changes>(answer["changes"].clone()) {
                    Ok(changes) if changes.is_empty() => {}
                    Ok(changes) => {
                        let label = crate::library_history::keyword_label(ask);
                        shell
                            .library_history()
                            .record_in(batch.as_ref(), label, changes, *at);
                    }
                    Err(e) => shell.notify(
                        Level::Warning,
                        format!("the keywords cannot be set back with undo: {e}"),
                    ),
                }
            }
            (Write::Meta(edit, at, batch), Ok(answer)) => {
                // The step the node made, recorded so Ctrl+Z sets the
                // photos' metadata back; the vault's events show it.
                match serde_json::from_value::<helios_vault::Changes>(answer["changes"].clone()) {
                    Ok(changes) if changes.is_empty() => {}
                    Ok(changes) => {
                        let label = crate::library_history::meta_label(edit.images.len());
                        shell
                            .library_history()
                            .record_in(batch.as_ref(), label, changes, *at);
                    }
                    Err(e) => shell.notify(
                        Level::Warning,
                        format!("the metadata cannot be set back with undo: {e}"),
                    ),
                }
            }
            (Write::Meta(..), Err(e)) => shell.notify(
                Level::Warning,
                Message::new(
                    "gui-library-meta-not-set",
                    "The metadata was not set: { $error }",
                )
                .with("error", e.message),
            ),
            (Write::Keyword(..), Err(e)) => shell.notify(
                Level::Warning,
                Message::new(
                    "gui-library-keyword-not-changed",
                    "The keyword was not changed: { $error }",
                )
                .with("error", e.message),
            ),
            (Write::Rotate(_, turn, at, batch), Ok(answer)) => {
                // The step the node made, recorded so Ctrl+Z turns the
                // photos back; the vault's events show the turn.
                match serde_json::from_value::<helios_vault::Changes>(answer["changes"].clone()) {
                    Ok(changes) if changes.is_empty() => {}
                    Ok(changes) => {
                        let label =
                            crate::library_history::rotate_label(*turn, changes.images.len());
                        shell
                            .library_history()
                            .record_in(batch.as_ref(), label, changes, *at);
                    }
                    Err(e) => shell.notify(
                        Level::Warning,
                        format!("the photos turned cannot be turned back with undo: {e}"),
                    ),
                }
            }
            (Write::Duplicate(..), Ok(done)) => {
                // The grid's badge counts the photos' versions now.
                if let Some(heads) = done["heads"].as_object() {
                    for (id, n) in heads {
                        shell.library_mut().set_heads_of(id, to_usize(n));
                    }
                }
            }
            (Write::Trash(_), Ok(done)) => {
                let trashed = done["trashed"].as_u64().unwrap_or(0);
                let failed = done["failed"].as_array().map_or(0, Vec::len);
                let mut line = shell.text(
                    Message::new(
                        "gui-trash-done",
                        "{ $count } files moved to the Trash (job { $job })",
                    )
                    .with("count", trashed)
                    .with("job", done["job"].as_str().unwrap_or_default()),
                );
                if let Some(m) = refusals(&done) {
                    line = format!("{line}. {}", shell.text(m));
                }
                if failed > 0 {
                    let first = done["failed"][0]["reason"].as_str().unwrap_or_default();
                    line = format!(
                        "{line}. {}",
                        shell.text(
                            Message::new("gui-trash-some-failed", "{ $count } failed: { $error }",)
                                .with("count", failed)
                                .with("error", first)
                        )
                    );
                }
                let level = if failed > 0 {
                    Level::Warning
                } else {
                    Level::Info
                };
                shell.notify(level, line);
            }
            (Write::Album(edit, at, batch), Ok(done)) => {
                match serde_json::from_value::<helios_vault::Changes>(done["changes"].clone()) {
                    Ok(changes) => {
                        let name = done["name"].as_str().unwrap_or_default();
                        let count = to_usize(&done["count"]);
                        let added = done["added"].as_bool().unwrap_or(true);
                        let label = crate::library_history::album_label(edit, name, count, added);
                        shell
                            .library_history()
                            .record_in(batch.as_ref(), label, changes, *at);
                    }
                    Err(e) => shell.notify(Level::Warning, e.to_string()),
                }
                shell.library_mut().ask_albums();
                self.read_albums_if_wanted(shell);
            }
            (Write::Album(..), Err(e)) => shell.notify(
                Level::Error,
                Message::new(
                    "gui-album-not-changed",
                    "The album was not changed: { $error }",
                )
                .with("error", e.message),
            ),
            (_, Ok(_)) => {}
            (Write::Trash(_), Err(e)) => not_trashed(shell, &e.message),
            (Write::Duplicate(..), Err(e)) => shell.notify(
                Level::Warning,
                Message::new(
                    "gui-library-not-duplicated",
                    "The photos were not duplicated: { $error }",
                )
                .with("error", e.message),
            ),
            (Write::Rotate(..), Err(e)) => shell.notify(
                Level::Warning,
                Message::new(
                    "gui-library-not-rotated",
                    "The photos were not rotated: { $error }",
                )
                .with("error", e.message),
            ),
            (Write::SwitchVersion(..) | Write::RemoveVersion(..), Err(e)) => {
                shell.library_mut().versions_changed();
                shell.notify(
                    Level::Warning,
                    Message::new(
                        "gui-library-version-not-changed",
                        "The photo's versions were not changed: { $error }",
                    )
                    .with("error", e.message),
                );
            }
            (Write::Remove(..), Err(e)) => shell.notify(
                Level::Warning,
                Message::new(
                    "gui-library-not-removed",
                    "The photos were not removed from the Library: { $error }",
                )
                .with("error", e.message),
            ),
            (Write::Fields(_), Err(e)) => shell.notify(
                Level::Warning,
                Message::new(
                    "gui-library-not-saved",
                    "Not saved, the library shows what was saved last: { $error }",
                )
                .with("error", e.message),
            ),
            (
                Write::Expand(..)
                | Write::Suggest(_)
                | Write::Accept(..)
                | Write::Decline(_)
                | Write::Document { .. }
                | Write::Session(..)
                | Write::Edit(_),
                Err(e),
            ) => shell.notify(Level::Warning, e.message),
        }
        if matches!(
            write,
            Write::Suggest(_)
                | Write::Accept(..)
                | Write::Decline(_)
                | Write::Stack(..)
                | Write::Pair(..)
        ) {
            self.ask_stacks();
        }
        let touched: BTreeSet<String> = write.images().map(String::from).collect();
        if touched.is_empty() {
            return;
        }
        // Kept or not, show what the vault holds now.
        let pending = self.node.call(
            "library.fields.get",
            json!({ "images": touched, "fields": SHOWN }),
        );
        self.reading.push((touched, pending));
    }

    /// Shows the stored fields read back, but for images with a write
    /// still to come: that write's own read back shows them.
    fn show_read(
        &self,
        shell: &mut Shell,
        images: &BTreeSet<String>,
        answer: Result<Value, helios_inspect::rpc::Error>,
    ) {
        let rows = match answer {
            Ok(answer) => answer["rows"].as_array().cloned().unwrap_or_default(),
            Err(e) => {
                shell.notify(
                    Level::Error,
                    Message::new(
                        "gui-vault-unreadable",
                        "Cannot read the library: { $error }",
                    )
                    .with("error", e.message),
                );
                return;
            }
        };
        let writes = held(&self.writes);
        let coming: BTreeSet<&str> = self
            .writing
            .iter()
            .map(|(w, _)| w)
            .chain(writes.iter())
            .flat_map(Write::images)
            .collect();
        let mut library = shell.library_mut();
        for row in rows {
            let Some(id) = row["id"].as_str() else {
                continue;
            };
            if !images.contains(id) || coming.contains(id) {
                continue;
            }
            for field in SHOWN {
                library.show_field(
                    id,
                    field,
                    Some(&row["fields"][field]).filter(|v| !v.is_null()),
                );
            }
        }
    }

    /// The node's library, for what the desktop wires on it.
    pub fn library(&self) -> &helios_node_core::desktop::Library<S> {
        &self.library
    }

    pub fn bus(&self) -> &Arc<Bus> {
        &self.bus
    }

    /// Where the desktop says the library changed, from any thread (an
    /// import ended, a library step undone or redone): heard at the next
    /// frame with the vault's events, and passed with them through
    /// `library.changed` once ([`crate::library_changed`]).
    pub fn changes(&self) -> crate::library_changed::Publisher {
        self.changes.0.clone()
    }

    /// `ctx` woken when the desktop says the library changed.
    pub fn wake_on_changes(&mut self, ctx: egui::Context) {
        self.changes.0 = self.changes.0.clone().waking(ctx);
    }

    /// Fills the Library again once an import ended: `import.ended`'s
    /// `library.refill` says the library changed (its contents).
    ///
    /// # Errors
    /// The bus refused the listener.
    pub fn listen_import_refill(&self, shell: &Shell) -> Result<(), String> {
        let changes = self.changes();
        shell.listen_import_ended_by(crate::import_end::REFILL, move |_| {
            changes.changed(
                crate::library_changed::Kind::Contents,
                crate::library_changed::BY_IMPORT,
            );
        })
    }

    /// How the library's history walks its steps (undo, redo): through
    /// the node (`library.changes.apply`: the step's rows all moved or
    /// none), then the library said changed.
    pub fn history_apply(&self) -> crate::library_history::Apply {
        let (node, changes) = (self.node.clone(), self.changes());
        Arc::new(move |walk, forward| {
            let r = node
                .request(
                    "library.changes.apply",
                    json!({ "changes": walk, "forward": forward }),
                )
                .map_err(|e| e.message)
                .and_then(|walked| serde_json::from_value(walked).map_err(|e| e.to_string()));
            changes.changed(
                crate::library_changed::Kind::Contents,
                crate::library_changed::BY_HISTORY,
            );
            r
        })
    }

    /// The order `library.changed`'s and `library.filled`'s listeners
    /// run in on the shell's bus.
    pub fn library_changed_order(&mut self, shell: &Shell) -> String {
        let bus = self.listen_library_changed(shell);
        crate::library_changed::order(&*bus)
    }

    /// Registers `library.changed`'s listeners on the shell's bus, once:
    /// local listeners, each doing its own part inside the pass, on the
    /// shell's parts ([`Shell::library_parts`]) and the reads this layer
    /// runs off the interface's thread. The bus.
    fn listen_library_changed(
        &mut self,
        shell: &Shell,
    ) -> Arc<dyn egui_pop::api::bus::bus_contract::Bus> {
        use crate::library_changed as lc;
        let bus = shell.app().handle().bus();
        if self
            .listening
            .as_ref()
            .is_some_and(|b| Arc::ptr_eq(b, &bus))
        {
            return bus;
        }
        let parts = shell.library_parts();
        let listened = [
            {
                let parts = parts.clone();
                lc::listen_changed(&*bus, lc::VOLUMES, move |_| parts.refresh_volumes())
            },
            {
                // Read off the interface's thread, shown at a coming frame.
                let (parts, reads) = (parts.clone(), self.reads.clone());
                lc::listen_changed(&*bus, lc::FILL, move |_| {
                    reads.ask_fill(&parts.library(), true);
                })
            },
            {
                let parts = parts.clone();
                lc::listen_changed(&*bus, lc::ALBUMS, move |_| parts.ask_albums())
            },
            {
                let reads = self.reads.clone();
                lc::listen_changed(&*bus, lc::STACKS, move |change| {
                    if change.kinds.contains(&lc::Kind::Stacks) {
                        reads.ask_stacks();
                    }
                    // The workspace proposes stacks after each import, to
                    // review.
                    if change.by.contains(lc::BY_IMPORT) {
                        parts.suggest_stacks_after_import();
                    }
                })
            },
        ];
        for e in listened.into_iter().filter_map(Result::err) {
            log::error!(target: "helios", "{}: {e}", lc::CHANGED);
        }
        self.listening = Some(bus.clone());
        bus
    }

    /// What one frame heard changed in the library, passed once through
    /// `library.changed`: each listener does its part inside the pass, in
    /// the order they run (the volumes, the Library read again, the
    /// albums, the stacks); then `library.changed` is said on the journal
    /// with what changed, who said it and the listeners that ran (`ran`).
    fn library_changed(&mut self, shell: &mut Shell, change: crate::library_changed::Change) {
        use crate::library_changed as lc;
        let bus = self.listen_library_changed(shell);
        let mut data = change.to_json();
        let ran = lc::pass_changed(&*bus, change);
        data["ran"] = json!(ran);
        shell
            .app()
            .handle()
            .publish("helios", None, lc::CHANGED, data);
    }

    /// Fills the library with the vault's images, read through the node;
    /// returns where each one can be read (its first present copy), for
    /// thumbnails. Once shown, the library is brought up to date by its
    /// cursor's delta: nothing is read when the catalog did not change,
    /// else the images added and those whose copies moved (an import, a
    /// scan, an undo), in windows of [`PAGE`]; the others keep their
    /// entries as shown.
    ///
    /// A library sorted another way since is read whole again, in its
    /// sort, through a new cursor.
    ///
    /// It waits for the node: the host asks [`Persist::ask_fill`] instead,
    /// read off the UI thread.
    pub fn fill_library(&self, shell: &mut Shell) -> Result<Vec<(String, PathBuf)>, String> {
        self.listen_asks(shell);
        let shown = self.reads.shown.lock().expect("library lock").take();
        let fetched = fetch(&self.reader, shown, shell.library().sort(), &known(shell))?;
        self.apply(shell, fetched)
    }

    /// Reads the library again as [`Persist::fill_library`] does, on a
    /// thread of its own: a coming frame shows it. `host`: the host takes
    /// where its images can be read once shown ([`Persist::take_filled`]),
    /// for thumbnails.
    pub fn ask_fill(&mut self, shell: &Shell, host: bool) {
        self.reads.ask_fill(&shell.library(), host);
    }

    /// Where each image of the library the host asked read
    /// ([`Persist::ask_fill`]) can be read, once shown.
    pub fn take_filled(&mut self) -> Option<Vec<(String, PathBuf)>> {
        self.filled.take()
    }

    /// Shows a read of the library; returns where each image can be read.
    fn apply(&self, shell: &mut Shell, fetched: Fetched) -> Result<Files, String> {
        match fetched {
            Fetched::Whole(rows) => self.show_library(shell, rows),
            Fetched::Delta(shown, moved, stacked) => {
                Ok(self.show_moved(shell, shown, moved, stacked))
            }
        }
    }

    /// Shows the library [`read_library`] read (on another thread, say),
    /// with the vault's stacks and the proposals waiting; returns where
    /// each image can be read, for thumbnails.
    pub fn show_library(
        &self,
        shell: &mut Shell,
        rows: LibraryRows,
    ) -> Result<Vec<(String, PathBuf)>, String> {
        // The Library shown asks its writes from now on.
        self.listen_asks(shell);
        // Read before the library's sort changed (on the thread reading it
        // as the window opens): read again in the sort, next frame.
        if rows.order != shell.library().sort() {
            shell.library_mut().ask_resort();
        }
        shell.set_library(rows.entries);
        let old = match rows.cursor {
            Some(cursor) => self
                .reads
                .shown
                .lock()
                .expect("library lock")
                .replace(Shown {
                    cursor,
                    files: rows.files.iter().cloned().collect(),
                    order: rows.order,
                }),
            None => None,
        };
        if let Some(old) = old {
            self.reader.close(old.cursor);
        }
        show_stacked(shell, rows.stacked);
        shell.library_mut().set_heads(rows.heads);
        Ok(rows.files)
    }

    /// Shows the library brought up to date by its cursor's delta.
    fn show_moved(
        &self,
        shell: &mut Shell,
        shown: Shown,
        moved: Option<Moved>,
        mut stacked: Stacked,
    ) -> Vec<(String, PathBuf)> {
        shell
            .library_mut()
            .set_heads(std::mem::take(&mut stacked.heads));
        if let Some(moved) = moved {
            // The entries shown, their fields kept up to date by the
            // vault's events.
            let mut entries: HashMap<String, Entry> = shell
                .library()
                .entries
                .iter()
                .map(|e| (e.id.clone(), e.clone()))
                .collect();
            for id in &moved.removed {
                entries.remove(id);
            }
            for (entry, _) in moved.read {
                entries.insert(entry.id.clone(), entry);
            }
            let entries = moved
                .order
                .iter()
                .filter_map(|id| {
                    let mut entry = entries.remove(id)?;
                    entry.changed_outside = moved.changed.get(id).copied();
                    Some(entry)
                })
                .collect();
            shell.set_library(entries);
        } else {
            // The backups are read again all the same (a strategy saved,
            // a copy verified: no catalog change), as a whole read does.
            shell.refresh_backups();
        }
        let files = shown
            .files
            .iter()
            .map(|(id, path)| (id.clone(), path.clone()))
            .collect();
        let old = self
            .reads
            .shown
            .lock()
            .expect("library lock")
            .replace(shown);
        if let Some(old) = old {
            self.reader.close(old.cursor);
        }
        show_stacked(shell, stacked);
        files
    }

    /// Reads the stacks and the proposals waiting again through the node,
    /// on a thread of its own: a coming frame shows them. Asked while a
    /// read goes on, they are read once more after it.
    fn ask_stacks(&mut self) {
        self.reads.ask_stacks();
    }

    /// The versions strip's reads and what was asked of photos'
    /// versions: a switch or a confirmed removal written, a removal not
    /// confirmed asked first.
    fn ask_versions(&mut self, shell: &mut Shell) {
        if let Some(image) = shell.library_mut().take_versions_wanted() {
            let pending = self.node.call(
                helios_node_core::library::VERSIONS_LIST,
                json!({ "image": image }),
            );
            held(&self.asked).push((Asked::Versions(image), pending));
        }
    }

    /// The order the listeners of the asks made while the panels drew
    /// (`stacks.answer`, `library.write`) run in on the shell's bus.
    pub fn asks_order(&self, shell: &Shell) -> String {
        let bus = self.listen_asks(shell);
        bus_contract::order_dump(
            &*bus,
            &[
                crate::decisions::ANSWER,
                crate::stack_review::ANSWER,
                crate::library_write::WRITE,
            ],
        )
    }

    /// Registers the listeners of the asks the panels make while they
    /// draw, on the shell's bus, once: each adds what was asked to the
    /// writes waiting their turn, in ask order, in the pass it is heard
    /// (after the drawing). `stacks.answer` (`persist.stacks`): a
    /// proposal asked, accepted or declined in the Stacks panel;
    /// `library.write` (`persist.library`): what the Library asks written
    /// (a removal, a turn, a stack, an album…), a trash or a version's
    /// removal not confirmed asked first. Registered by the host before
    /// the first pass ([`Persist::frame`] does it too). The bus.
    pub fn listen_asks(&self, shell: &Shell) -> Arc<dyn bus_contract::Bus> {
        use crate::stack_review::{Request, StacksAnswer};
        let bus = shell.app().handle().bus();
        if held(&self.asking)
            .as_ref()
            .is_some_and(|b| Arc::ptr_eq(b, &bus))
        {
            return bus;
        }
        crate::stack_review::declare(&*bus);
        crate::decisions::declare(&*bus);
        let writes = self.writes.clone();
        bus.unlisten(crate::stack_review::ANSWER, STACKS);
        let listened = bus.listen_local::<StacksAnswer>(
            STACKS,
            bus_contract::ListenOpts::new().owner(BUS_OWNER),
            move |r: &Request, (): &mut ()| {
                held(&writes).push_back(match r.clone() {
                    Request::Suggest { images } => Write::Suggest(images),
                    Request::Accept { decision, groups } => Write::Accept(decision, groups),
                    Request::Decline { decision } => Write::Decline(decision),
                });
                Ok(())
            },
        );
        if let Err(e) = listened {
            log::error!(target: "helios", "{}: {e}", crate::stack_review::ANSWER);
        }
        crate::library_write::declare(&*bus);
        let (writes, asked, node) = (self.writes.clone(), self.asked.clone(), self.node.clone());
        let library = shell.library_parts();
        bus.unlisten(crate::library_write::WRITE, LIBRARY);
        let listened = bus.listen_local::<crate::library_write::LibraryWrite>(
            LIBRARY,
            bus_contract::ListenOpts::new().owner(BUS_OWNER),
            move |w: &crate::library_write::WriteAsk, (): &mut ()| {
                write_asked(&writes, &asked, &node, &library, w.clone());
                Ok(())
            },
        );
        if let Err(e) = listened {
            log::error!(target: "helios", "{}: {e}", crate::library_write::WRITE);
        }
        *held(&self.asking) = Some(bus.clone());
        bus
    }

    /// Shows an image's network in the viewport with the image's saved
    /// edit: its newest head, returned (none for an image never edited).
    /// From then on each step is saved as a version continuing it.
    pub fn open_image(
        &mut self,
        shell: &mut Shell,
        image: ImageId,
        graph: Graph,
        node: NodeId,
        port: &str,
    ) -> Result<Option<Version>, String> {
        // `image.open`: the edit open saved first; refused, it stays open.
        self.switch(shell, Some(image), seconds_now())?;
        let loaded = self.load_image(shell, image, graph, node, port);
        if loaded.is_err() {
            // Not loaded: no image open, as its followers know it.
            let bus = self.listen_darkroom(shell);
            crate::image_open::follow(&*bus, crate::image_open::Shown::default());
        }
        loaded
    }

    /// Loads `image` once `image.open` went through: its saved edit on
    /// the network, shown in the viewport, then `image.opened` passed.
    fn load_image(
        &mut self,
        shell: &mut Shell,
        image: ImageId,
        mut graph: Graph,
        node: NodeId,
        port: &str,
    ) -> Result<Option<Version>, String> {
        // Opened for the first time, a raw gets its camera's default
        // calibration profile and default camera look (as its first
        // edit, read just below).
        for said in crate::calibrate::on_open(&self.node, image, &graph, shell.values()) {
            shell.notify(Level::Info, said);
        }
        // The steps of an image still on their way are written first: its
        // head is read after them.
        self.settle_edits(shell);
        let head = self
            .reader
            .read("library.edit.head", json!({ "image": image.hex() }))?;
        let head: Option<Version> = match &head["head"] {
            Value::Null => None,
            row => Some(serde_json::from_value(row.clone()).map_err(|e| e.to_string())?),
        };
        let session = EditSession::resume(image, head.as_ref());
        // The image with no edit: what a snapshot pinned to no version
        // restores.
        let unedited = edit::of_graph(&graph);
        if let Some(head) = &head {
            for skipped in edit::apply_to_graph(&mut graph, &head.edit)? {
                shell.notify(
                    Level::Warning,
                    Message::new(
                        "gui-edit-does-not-fit",
                        "Part of the saved edit does not fit this network: { $part }",
                    )
                    .with("part", skipped),
                );
            }
        }
        let modules = modules_of(&graph);
        shell.set_view_source(graph, node, port)?;
        let snapshots = self.read_snapshots(shell, image, &unedited);
        // `image.opened`: what follows the image open, once, in order.
        let bus = self.listen_darkroom(shell);
        let shown = crate::image_open::Shown {
            image: Some(image.hex()),
            snapshots,
            saved: saved_params(&modules),
            marks: head
                .as_ref()
                .map_or(Value::Null, |h| h.edit["marks"].clone()),
            unedited: unedited.clone(),
        };
        let followed = crate::image_open::follow(&*bus, shown);
        let committed = held(shell.values()).committed();
        self.editing = Some(Editing {
            confirmed: session.clone(),
            session,
            modules,
            committed,
            flushed_at: None,
            gesture: None,
            unsaved: false,
            failed: None,
        });
        shell.app().handle().publish(
            "helios",
            None,
            crate::image_open::OPENED,
            json!({ "image": image.hex(), "followed": followed.ran }),
        );
        Ok(head)
    }

    /// An image's pinned snapshots, each with the values of its version
    /// (`unedited`: the edit of one pinned to no version). One whose
    /// version cannot be read is left out, and said.
    fn read_snapshots(
        &self,
        shell: &mut Shell,
        image: ImageId,
        unedited: &Value,
    ) -> Vec<crate::edit_snapshots::Taken> {
        use crate::edit_snapshots::{Pin, Taken, origin_of, values_of_edit};
        let kept = match self
            .reader
            .read("library.snapshots.get", json!({ "image": image.hex() }))
        {
            Ok(k) => k,
            Err(e) => {
                shell.notify(Level::Warning, e);
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for row in kept["snapshots"].as_array().into_iter().flatten() {
            let s = match helios_vault::snapshots::Snapshot::from_json(row) {
                Ok(s) => s,
                Err(e) => {
                    shell.notify(Level::Warning, e);
                    continue;
                }
            };
            let edit = match (&s.version, row["error"].as_str()) {
                (_, Some(error)) => Err(error.to_string()),
                (None, None) => Ok(unedited.clone()),
                (Some(_), None) => Ok(row["edit"].clone()),
            };
            match edit.and_then(|e| values_of_edit(&e)) {
                Ok(values) => out.push(Taken {
                    id: s.id,
                    name: s.name,
                    // Taken in an earlier session: no step of this one
                    // (0; only a pending snapshot is pinned by its step).
                    step: 0,
                    origin: origin_of(&s.origin),
                    at: s.at as f64,
                    pin: Some(s.version.map_or(Pin::Unedited, Pin::Version)),
                    values,
                }),
                Err(e) => shell.notify(
                    Level::Warning,
                    Message::new(
                        "gui-snapshot-unreadable",
                        "Snapshot “{ $name }” cannot be read: { $error }",
                    )
                    .with("name", s.name)
                    .with("error", e),
                ),
            }
        }
        out
    }

    /// Pins the snapshots taken at the step the saved edit holds, once
    /// everything committed is saved and no gesture goes on, then keeps
    /// the image's pinned snapshots in the vault when they changed.
    fn pin_snapshots(&mut self, shell: &mut Shell) {
        let Some(e) = &self.editing else {
            return;
        };
        // Sent after the edit's steps they pin: a refusal is said once,
        // the next change writes the whole list again.
        if let Some((image, pinned)) = pin_book(e, shell.values(), shell.snapshots()) {
            held(&self.writes).push_back(Write::Snapshots(image, pinned));
        }
    }

    /// Waits until the node has written the steps of edits and the
    /// snapshots sent: an image opened is read after them.
    fn settle_edits(&mut self, shell: &mut Shell) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let edits = |p: &Self| {
            let edit = |w: &Write| matches!(w, Write::Edit(_) | Write::Snapshots(..));
            p.writing.iter().any(|(w, _)| edit(w)) || held(&p.writes).iter().any(edit)
        };
        while edits(self) && std::time::Instant::now() < deadline {
            self.pump(shell);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Follows the network shown after it changed in place (see
    /// `Shell::keep_image_open`): its nodes are what an edit saves from
    /// now on, and the edit is saved at the next frame, the image's
    /// session going on.
    pub fn follow_graph(&mut self, shell: &Shell) {
        let Some(e) = &mut self.editing else {
            return;
        };
        if let Some(graph) = shell.viewport().graph() {
            e.modules = modules_of(graph);
            crate::edit_snapshots::lock(shell.snapshots()).set_saved(saved_params(&e.modules));
            // Saved at the next frame, under the step that changed it (an
            // edit the same as the last one saved is not saved again).
            e.committed = u64::MAX;
        }
    }

    /// Closes the image being edited, its last committed step saved:
    /// what the viewport shows next (a raw's preview drawn while the raw
    /// is made ready) is not saved into its edit.
    ///
    /// It passes `image.close` ([`crate::image_open`]): the edit saved
    /// first, its write answered.
    ///
    /// # Errors
    /// The save failed: the image stays open, its edit unsaved; the reason,
    /// in words.
    pub fn close_image(&mut self, shell: &mut Shell, now: f64) -> Result<(), String> {
        self.switch(shell, None, now)
    }

    /// Passes `image.open` (`to`) or `image.close` (none) on the shell's
    /// bus: the edit open saved first (`edit.save`, its write answered),
    /// then its history left (`history.scope`). Refused, the image open
    /// stays open and the refusal is said on the journal; gone through,
    /// no image is edited until the next one loads, and `image.closed` is
    /// said.
    ///
    /// # Errors
    /// A listener refused: the reason, in words.
    fn switch(&mut self, shell: &mut Shell, to: Option<ImageId>, now: f64) -> Result<(), String> {
        use crate::image_open::{self as io, Switch};
        // The steps on their way are written first: the save follows them.
        self.settle_edits(shell);
        let bus = self.listen_darkroom(shell);
        let switch = Switch {
            from: self.editing.as_ref().map(|e| e.session.image().hex()),
            to: to.as_ref().map(ImageId::hex),
            at: now,
        };
        *held(&self.desk) = self.editing.take();
        let switched = io::pass(&*bus, switch.clone());
        self.editing = held(&self.desk).take();
        let handle = shell.app().handle().clone();
        if let Some((image, edited)) = &switched.saved {
            let mut library = shell.library_mut();
            library.show_field(
                image,
                helios_vault::edit::EDITED,
                Some(&Value::Bool(*edited)),
            );
            library.edit_changed(image);
        }
        if let Some(refusal) = &switched.refused {
            handle.publish("helios", None, io::REFUSED, io::said(&switch, &switched));
            let reason = if refusal.listener == io::SAVE {
                shell.text(
                    Message::new("gui-edit-not-saved", "The edit was not saved: { $error }")
                        .with("error", refusal.reason.clone()),
                )
            } else {
                refusal.reason.clone()
            };
            return Err(reason);
        }
        self.editing = None;
        if switch.from.is_some() {
            let mut said = io::said(&switch, &switched);
            // Closed with none after it: what follows the image open
            // follows that, once. An image asked passes `image.opened`
            // once loaded.
            if to.is_none() {
                said["followed"] = json!(io::follow(&*bus, io::Shown::default()).ran);
            }
            handle.publish("helios", None, io::CLOSED, said);
        }
        Ok(())
    }

    /// The order `image.open`'s and `image.close`'s listeners run in on
    /// the shell's bus.
    pub fn darkroom_order(&mut self, shell: &Shell) -> String {
        let bus = self.listen_darkroom(shell);
        crate::image_open::order(&*bus)
    }

    /// Registers `image.open` / `image.close`'s listeners on the shell's
    /// bus, once: `edit.save` on the edit lent while the request passes
    /// (its step written through the node and answered inside the pass),
    /// `history.scope` on the shell's value store and snapshot book. The
    /// bus.
    fn listen_darkroom(&mut self, shell: &Shell) -> Arc<dyn egui_pop::api::bus::bus_contract::Bus> {
        use crate::image_open as io;
        let bus = shell.app().handle().bus();
        if self.darkroom.as_ref().is_some_and(|b| Arc::ptr_eq(b, &bus)) {
            return bus;
        }
        let (desk, values, node) = (self.desk.clone(), shell.values().clone(), self.node.clone());
        let book = shell.snapshots().clone();
        let saved = io::listen_local(&*bus, io::SAVE, move |switch, switched| {
            let mut desk = held(&desk);
            let Some(e) = desk.as_mut() else {
                return Ok(());
            };
            let started = std::time::Instant::now();
            switched.saved = save_now(e, &values, &node, switch.at)?;
            // The snapshots pending at the step just saved are pinned and
            // kept now: `history.scope` clears the book next.
            keep_snapshots_now(
                e,
                &values,
                &book,
                &node,
                SAVE_WAIT.saturating_sub(started.elapsed()),
            )
        });
        let (values, snapshots) = (shell.values().clone(), shell.snapshots().clone());
        let scoped = io::listen_local(&*bus, io::SCOPE, move |switch, _| {
            // The next image's values take this one's paths: its steps
            // must not be undone into them, nor its snapshots restored.
            // They are parked under the image, back when it opens again.
            // The workspace's steps (graph edits) are not the image's:
            // they stay.
            let mut store = held(&values);
            match &switch.from {
                Some(image) => store.park_image(image),
                None => store.leave_image(),
            }
            drop(store);
            crate::edit_snapshots::lock(&snapshots).clear();
            Ok(())
        });
        // The followers of `image.opened` / `image.closed`, each on its
        // own state.
        let snapshots = shell.snapshots().clone();
        let book = io::listen_follower_local(&*bus, io::SNAPSHOTS, move |shown| {
            let Some(image) = shown
                .image
                .as_deref()
                .and_then(|i| ImageId::from_hex(i).ok())
            else {
                return;
            };
            let mut book = crate::edit_snapshots::lock(&snapshots);
            book.load(image, shown.snapshots.clone());
            book.set_saved(shown.saved.clone());
        });
        let values = shell.values().clone();
        let marks = io::listen_follower_local(&*bus, io::MARKS, move |shown| {
            if shown.image.is_some() {
                crate::mask_tree::restore_marks(&mut held(&values), &shown.marks);
            }
        });
        let workshop = shell.listen_workshop_photo(&*bus);
        // Last, once the image's values, marks included, are loaded: its
        // parked steps come back onto them.
        let (values, reader) = (shell.values().clone(), self.reader.clone());
        let resumed = io::listen_follower_local(&*bus, io::HISTORY, move |shown| {
            let Some(image) = &shown.image else {
                return;
            };
            if held(&values).resume_image(image) {
                return;
            }
            // No line parked (a restart, a crash): the one its versions
            // say, rebuilt from their step markers.
            if let Err(e) = resume_from_versions(&values, &reader, image, &shown.unedited) {
                log::warn!(target: "helios", "{}: {image}: {e}", io::HISTORY);
            }
        });
        for e in [saved, scoped, book, marks, workshop, resumed]
            .into_iter()
            .filter_map(Result::err)
        {
            log::error!(target: "helios", "{}: {e}", io::OPEN);
        }
        self.darkroom = Some(bus.clone());
        bus
    }

    /// The edit being saved, if an image is open.
    pub fn session(&self) -> Option<&EditSession> {
        self.editing.as_ref().map(|e| &e.session)
    }

    /// Whether the edit open is marked unsaved: its last save failed, and
    /// it is saved again at the next frame or switch.
    pub fn unsaved(&self) -> bool {
        self.editing.as_ref().is_some_and(|e| e.unsaved)
    }

    /// Why the edit open is not saved (its last save's failure), while it
    /// is not: what the quit's `helios.quit.unsaved` listener refuses
    /// with ([`Shell::set_unsaved_edit`], said at each frame).
    pub fn unsaved_reason(&self) -> Option<String> {
        self.editing
            .as_ref()
            .filter(|e| e.unsaved)
            .map(|e| e.failed.clone().unwrap_or_else(|| "its save failed".into()))
    }

    /// "Save" chosen before quitting ([`Shell::take_save_retry`]):
    /// the edit open saved now, its answer waited for, then the quit asked
    /// again: saved, it goes; failed again, the question is asked again.
    fn retry_save_on_quit(&mut self, shell: &mut Shell, now: f64) {
        if !shell.take_save_retry() {
            return;
        }
        self.settle_edits(shell);
        let (values, node) = (shell.values().clone(), self.node.clone());
        if let Some(e) = &mut self.editing {
            match save_now(e, &values, &node, now) {
                Ok(Some((image, edited))) => {
                    let mut library = shell.library_mut();
                    library.show_field(
                        &image,
                        helios_vault::edit::EDITED,
                        Some(&Value::Bool(edited)),
                    );
                    library.edit_changed(&image);
                }
                Ok(None) => {}
                Err(why) => {
                    log::warn!(target: "helios::quit", "the edit's save failed again: {why}");
                }
            }
        }
        shell.set_unsaved_edit(self.unsaved_reason());
        shell.quit_again();
    }

    /// Reads the metadata templates once, writes them back each time they
    /// change (a library document through the node, outside the
    /// library's history), and reads the capture details the Metadata
    /// section shows.
    fn keep_templates(&mut self, shell: &mut Shell) {
        use helios_vault::meta::{TEMPLATES_KEY, TEMPLATES_NS};
        if let Some(image) = shell.library_mut().take_exif_wanted() {
            let pending = self.node.call(
                "library.fields.get",
                json!({ "images": [image], "fields": helios_vault::capture::NAMES }),
            );
            held(&self.asked).push((Asked::Exif(image), pending));
        }
        if shell.library_mut().take_templates_wanted() {
            let pending = self.node.call(
                "library.documents.get",
                json!({ "ns": TEMPLATES_NS, "key": TEMPLATES_KEY }),
            );
            held(&self.asked).push((Asked::Templates, pending));
        }
    }

    /// Reads the Library's pinned and recent filters once, and writes
    /// them back each time they change (`crate::recent_filters`): a
    /// library document, outside the library's history. Both go through
    /// the node: the read is shown at a coming frame, the write is sent
    /// in its turn with the library's other writes.
    fn keep_filter_history(&mut self, shell: &mut Shell) {
        use crate::recent_filters::{KEY, NS};
        if shell.library_mut().take_filter_history_wanted() {
            let pending = self
                .node
                .call("library.documents.get", json!({ "ns": NS, "key": KEY }));
            held(&self.asked).push((Asked::FilterHistory, pending));
        }
        // Not written over the stored one before it is read.
        let reading = held(&self.asked)
            .iter()
            .any(|(asked, _)| matches!(asked, Asked::FilterHistory));
        if reading {
            return;
        }
        let write = shell.library_mut().take_filter_history_write();
        if let Some(history) = write {
            held(&self.writes).push_back(Write::Document {
                ns: NS,
                key: KEY,
                document: Some(history.to_json()),
            });
        }
    }

    /// Sends the album changes the library asked to the node, each one
    /// step of the library's history once answered ([`Persist::written`]);
    /// reads the albums when the Library wants them (an album changed,
    /// or `library.changed`'s `library.albums`: an import, an undo, a
    /// redo).
    fn edit_albums(&mut self, shell: &mut Shell) {
        self.keep_filter_history(shell);
        self.read_albums_if_wanted(shell);
    }

    /// Asks the node the albums and the quick collection when the
    /// Library wants them.
    fn read_albums_if_wanted(&mut self, shell: &mut Shell) {
        if shell.library_mut().take_albums_wanted() {
            let pending = self.node.call("library.albums.list", json!({}));
            held(&self.asked).push((Asked::Albums, pending));
        }
    }

    /// Writes what the user changed and shows what other writers did;
    /// `now` is the time in seconds (since the epoch for a real clock).
    pub fn frame(&mut self, shell: &mut Shell, now: f64) {
        // The asks made while the panels drew, heard: drained after the
        // drawing by the application already; here for a host that asked
        // outside a pass (a test, a command run between frames).
        self.listen_asks(shell);
        shell.app().handle().drain_asks();
        self.retry_save_on_quit(shell, now);
        let changes = shell.take_library_changes();
        if !changes.is_empty() {
            held(&self.writes).push_back(Write::Fields(changes));
        }
        if let Some(e) = &mut self.editing {
            e.session.set_meta(shell.session_meta());
        }
        self.save_edit(shell, now);
        self.write_sessions(shell);
        self.pin_snapshots(shell);
        self.ask_versions(shell);
        self.keep_templates(shell);
        self.pump(shell);
        // A view's results shown another way (thumbnails, list, a
        // size): kept with that view in the workspace.
        let changes = shell.library_mut().take_results_changes();
        for (gallery, results) in changes {
            // Another Library view keeps them in its own state, with
            // the layout (`library_views`).
            if crate::library_views::is_view(&gallery) {
                continue;
            }
            if let Err(e) = shell.keep_library_results(&gallery, results) {
                log::warn!(target: "helios", "library results not kept: {e}");
            }
        }
        // The library sorted another way: read again in that order.
        if shell.library_mut().take_resort() {
            self.ask_fill(shell, false);
        }
        // The Folders section open: its counts, one store query, read
        // through the node and shown at a coming frame.
        if shell.library_mut().take_folders_wanted() {
            let pending = self.node.call("library.folders", json!({}));
            held(&self.asked).push((Asked::Folders, pending));
        }
        self.edit_albums(shell);
        let queries = shell.library_mut().take_queries();
        for (gallery, query) in queries {
            // How many photos carry each value of each facet, then the
            // photos it takes, read through the node on a thread of its
            // own: a coming frame shows them in the gallery that asked. A
            // newer query of a gallery replaces one still running, whose
            // answer is not shown.
            let rule = shell.library().facet_rule().cloned();
            let reader = self.reader.clone();
            let running = spawn("helios-library-search", move || {
                search(&reader, &query, rule.as_ref())
            });
            self.searching.retain(|(g, _)| *g != gallery);
            self.searching.push((gallery, running));
        }
        self.hear(shell);
        // What a quit asks about: an edit whose save failed.
        shell.set_unsaved_edit(self.unsaved_reason());
    }

    /// Shows what the vault's events say changed: a field in place; what
    /// they and the desktop say changed in the library, passed once
    /// through `library.changed` ([`Persist::library_changed`]).
    fn hear(&mut self, shell: &mut Shell) {
        use crate::library_changed::{BY_VAULT, Change, Kind};
        let mut change = Change::default();
        while let Ok((kind, by)) = self.changes.1.try_recv() {
            change.add(kind, by);
        }
        while let Ok(event) = self.events.try_recv() {
            if let Some(kind) = Kind::of_event(event.kind) {
                change.add(kind, BY_VAULT);
            } else if event.kind == "helios.vault.field_changed" {
                let p = &event.payload;
                if let (Some(image), Some(field)) = (p["image"].as_str(), p["field"].as_str()) {
                    let new = Some(&p["new"]).filter(|v| !v.is_null());
                    shell.library_mut().show_field(image, field, new);
                }
            }
        }
        // Said by the desktop meanwhile (an import's end heard on
        // another thread): the same change.
        while let Ok((kind, by)) = self.changes.1.try_recv() {
            change.add(kind, by);
        }
        if !change.is_empty() {
            self.library_changed(shell, change);
        }
    }

    /// Keeps the workshop sessions' records the desktop wrote, and drops
    /// those it ended unkept.
    /// Each is sent to the node in its turn with the library's other
    /// writes (`library.workshop.sessions.set`).
    fn write_sessions(&mut self, shell: &mut Shell) {
        for (id, record) in shell.take_session_writes() {
            held(&self.writes).push_back(Write::Session(id, record));
        }
    }

    /// Drops the workshop sessions a former run left unkept (it ended
    /// before their session did): a session is kept only when the user
    /// says so (or, later, when it is generalised). Waits for the node:
    /// for a thread of its own ([`drop_unkept_sessions`]).
    pub fn drop_unkept_sessions(&self) -> Result<usize, String> {
        drop_unkept_sessions(&self.node)
    }

    /// Saves the edit when the value store committed a step, or while a
    /// gesture goes on, every [`FLUSH_SECONDS`].
    fn save_edit(&mut self, shell: &mut Shell, now: f64) {
        let Some(e) = &mut self.editing else {
            return;
        };
        let step = {
            let store = shell.values().lock().expect("value store lock");
            next_step(e, &store, now, false)
        };
        // Written by the node in its turn, off the UI thread; its answer
        // is picked up at a coming frame ([`Persist::written`]).
        if let Some(step) = step {
            held(&self.writes).push_back(Write::Edit(Box::new(step)));
        }
    }

    /// A step of an edit answered: the image's renders made again once a
    /// gesture ended; refused, the session goes back to what the node
    /// wrote, the steps made after it dropped.
    fn edit_written(
        &mut self,
        shell: &mut Shell,
        step: EditStep,
        answer: Result<Value, helios_inspect::rpc::Error>,
    ) {
        let image = step.version.image;
        let editing = self.editing.as_mut().filter(|e| e.session.image() == image);
        match answer {
            Ok(_) => {
                // Its thumbnails (the Library, the filmstrip) say whether
                // it is edited, as the vault now keeps it.
                shell.library_mut().show_field(
                    &image.hex(),
                    helios_vault::edit::EDITED,
                    Some(&Value::Bool(step.edited)),
                );
                if let Some(e) = editing {
                    e.confirmed = step.after;
                }
                // Renders of the image's head version are made again once
                // a gesture ends, not at each save while it goes (a drag).
                if !step.in_progress {
                    shell.library_mut().edit_changed(&image.hex());
                }
            }
            Err(err) => {
                if let Some(e) = editing {
                    e.session = e.confirmed.clone();
                    e.unsaved = true;
                    e.failed = Some(err.message.clone());
                }
                held(&self.writes)
                    .retain(|w| !matches!(w, Write::Edit(s) if s.version.image == image));
                shell.notify(
                    Level::Error,
                    Message::new("gui-edit-not-saved", "The edit was not saved: { $error }")
                        .with("error", err.message),
                );
            }
        }
    }
}

/// The step to save of the edit `e`, from the value store: a step when
/// the store committed one; while a gesture goes on, one every
/// [`FLUSH_SECONDS`] (`force`: at once); none when nothing changed.
fn next_step(
    e: &mut Editing,
    store: &helios_values::Store,
    now: f64,
    force: bool,
) -> Option<EditStep> {
    let at = now.max(0.0) as u64;
    let (committed, label, origin, gesture, current, edited, replayed) = {
        let mut modules: BTreeMap<String, (String, Params)> = BTreeMap::new();
        for (path, entry) in store.list("node/") {
            let Some((node, param)) = path
                .strip_prefix("node/")
                .and_then(|rest| rest.rsplit_once('/'))
            else {
                continue;
            };
            // A node or a parameter the network no longer has is kept
            // in the store for an undo, never saved.
            if let Some((module, params)) = e.modules.get(node)
                && params.contains(param)
            {
                modules
                    .entry(node.to_string())
                    .or_insert_with(|| (module.clone(), Params::new()))
                    .1
                    .set(param, entry.value.clone());
            }
        }
        let mut current = edit::from_modules(&modules);
        // The modules bypassed, marked on their entries.
        let bypassed: BTreeSet<String> = store
            .list(helios_values::BYPASS)
            .into_iter()
            .filter(|(_, e)| e.value.as_bool() == Some(true))
            .filter_map(|(p, _)| p.strip_prefix(helios_values::BYPASS).map(String::from))
            .filter(|n| e.modules.contains_key(n))
            .collect();
        edit::mark_bypassed(&mut current, &bypassed);
        // The Masks panel's labels and locks, kept with the edit.
        let marks = crate::mask_tree::marks(store);
        if !marks.is_empty() {
            current["marks"] = Value::Object(marks);
        }
        let gesture = store
            .open_gesture()
            .zip(store.open_gesture_origin())
            .map(|(label, origin)| (label.to_string(), who(origin)));
        (
            store.committed(),
            store.last_committed().to_string(),
            store.last_origin().map_or_else(|| WHO.to_string(), who),
            gesture,
            current,
            crate::edit_marks::Marks::read(store).edited(),
            // An undo or a redo (a jump ends with one): marked so in the
            // version, for the history rebuilt after a restart.
            store.last_replayed().then(|| {
                if store.last_committed().starts_with("undo ") {
                    StepKind::Undo
                } else {
                    StepKind::Redo
                }
            }),
        )
    };
    let step = if committed == e.committed {
        if let Some((gesture, by)) = gesture {
            e.gesture = Some((gesture.clone(), by.clone()));
            let since = *e.flushed_at.get_or_insert(now);
            if now - since < FLUSH_SECONDS && !force {
                return None;
            }
            e.flushed_at = Some(now);
            e.session.step_in_progress(current, &gesture, &by, at)
        } else {
            e.flushed_at = None;
            // A gesture ended with no step: cancelled. If it was
            // saved in progress, the values it put back are saved.
            let (gesture, by) = e.gesture.take()?;
            if !e.session.in_progress() {
                return None;
            }
            e.session.mark(StepKind::Cancel);
            e.session
                .step(current, &format!("{gesture} (cancelled)"), &by, at)
        }
    } else {
        e.committed = committed;
        e.flushed_at = None;
        e.gesture = None;
        if let Some(kind) = replayed {
            e.session.mark(kind);
        }
        e.session.step(current, &label, &origin, at)
    };
    e.unsaved = false;
    e.failed = None;
    step.map(|version| EditStep {
        version,
        in_progress: e.session.in_progress(),
        edited,
        after: e.session.clone(),
    })
}

/// Pins the snapshots of `e`'s image taken at the step its saved edit
/// holds (once everything committed is saved and no gesture goes on):
/// the image and its pinned snapshots, when the vault must keep them
/// again.
fn pin_book(
    e: &Editing,
    values: &Arc<Mutex<helios_values::Store>>,
    book: &crate::edit_snapshots::Shared,
) -> Option<(ImageId, Vec<helios_vault::snapshots::Snapshot>)> {
    use crate::edit_snapshots::{Pin, head_step};
    let image = e.session.image();
    let (step, committed, settled) = {
        let store = held(values);
        let settled =
            !e.unsaved && store.open_gesture().is_none() && store.committed() == e.committed;
        (head_step(&store), store.committed(), settled)
    };
    let mut book = crate::edit_snapshots::lock(book);
    if book.image() != Some(image) {
        return None;
    }
    let pin = e.session.head().map_or(Pin::Unedited, Pin::Version);
    book.settle(settled.then_some((step, committed, pin)));
    book.take_dirty().then(|| (image, book.pinned()))
}

/// `edit.save`, once the edit is saved: the snapshots pending at the step
/// it holds pinned, and the image's pinned snapshots kept through `node`,
/// its answer waited for up to `wait` (the image is about to be left, its
/// book cleared).
///
/// # Errors
/// The node refused them (or did not answer): the book stays to be kept
/// again; the reason, in words.
fn keep_snapshots_now(
    e: &Editing,
    values: &Arc<Mutex<helios_values::Store>>,
    book: &crate::edit_snapshots::Shared,
    node: &Client,
    wait: std::time::Duration,
) -> Result<(), String> {
    let Some((image, pinned)) = pin_book(e, values, book) else {
        return Ok(());
    };
    let (method, params) = Write::Snapshots(image, pinned).call();
    let answer = node.call(method, params).wait_timeout(wait);
    match answer {
        Some(Ok(_)) => Ok(()),
        failed => {
            crate::edit_snapshots::lock(book).keep_again();
            Err(match failed {
                Some(Err(err)) => format!("its snapshots: {}", err.message),
                _ => "its snapshots: the library did not answer in time".to_string(),
            })
        }
    }
}

/// How long `edit.save` waits for the node to answer a step.
const SAVE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
// A save that ends within its own wait is never counted late by the bus
// (a late `edit.save` refuses the switch).
const _: () = assert!(SAVE_WAIT.as_secs() < crate::image_open::SAVE_DEADLINE.as_secs());

/// `edit.save`: the step of the edit `e` not saved yet written through
/// `node`, its answer waited for (the image is about to be left). Written,
/// the image and whether its edit changes it; nothing to save, none.
///
/// # Errors
/// The node refused the step (or did not answer): the edit goes back to
/// what the node holds, marked unsaved, and the node's reason.
fn save_now(
    e: &mut Editing,
    values: &Arc<Mutex<helios_values::Store>>,
    node: &Client,
    now: f64,
) -> Result<Option<(String, bool)>, String> {
    // A save that failed before: the edit shown is saved again, as a step.
    if e.unsaved {
        e.committed = u64::MAX;
    }
    let step = next_step(e, &held(values), now, true);
    let Some(step) = step else {
        return Ok(None);
    };
    let (image, edited) = (step.version.image.hex(), step.edited);
    let write = Write::Edit(Box::new(step));
    let (method, params) = write.call();
    let answer = node
        .call(method, params)
        .wait_timeout(SAVE_WAIT)
        .unwrap_or_else(|| {
            Err(helios_inspect::rpc::Error::new(
                -32603,
                "the library did not answer within 30 s",
            ))
        });
    match (answer, write) {
        (Ok(_), Write::Edit(step)) => {
            e.confirmed = step.after;
            Ok(Some((image, edited)))
        }
        (Err(err), _) => {
            e.session = e.confirmed.clone();
            e.unsaved = true;
            e.failed = Some(err.message.clone());
            Err(err.message)
        }
        (Ok(_), _) => Ok(None),
    }
}

/// The time now, in seconds since the epoch.
fn seconds_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The parameters an edit saves, by node name.
fn saved_params(
    modules: &BTreeMap<String, (String, BTreeSet<String>)>,
) -> BTreeMap<String, BTreeSet<String>> {
    modules
        .iter()
        .map(|(node, (_, params))| (node.clone(), params.clone()))
        .collect()
}

/// The module each module node of `graph` runs, and its parameters, by
/// node name.
fn modules_of(graph: &Graph) -> BTreeMap<String, (String, BTreeSet<String>)> {
    graph
        .ids()
        .filter_map(|id| {
            let manifest = graph.manifest(id).ok()??;
            let params = manifest.params.iter().map(|p| p.name.clone()).collect();
            Some((graph.name(id).ok()?.to_string(), (manifest.name, params)))
        })
        .collect()
}
