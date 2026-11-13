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
//! - applies the vault's `vault.field_changed` events (from any writer in
//!   this process sharing its bus) to the library, and its stacks again
//!   on `vault.stack_changed`;
//! - reads the library through the node, off the UI thread: its photos
//!   through a cursor (`cursor.open`, `cursor.window`, `cursor.refresh`)
//!   and a [`ReadCache`] the bus's events keep true, the search filter's
//!   photos and facet counts (`library.facets`); a coming frame shows
//!   what was read ([`Persist::ask_fill`]). Keeps a stack expanded or
//!   collapsed as the user left it;
//! - answers the Stacks panel: a new `stacks.suggest` proposal, a
//!   proposal accepted (all of it or corrected) or declined, as the user,
//!   and shows the proposals waiting again on `library.decision`;
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
//!   each save is announced on the bus as `edit.saved`;
//! - pins the image's history snapshots taken since (see
//!   `crate::edit_snapshots`): once the edit is saved, a snapshot taken
//!   at the step it holds is pinned to that version, and the image's
//!   pinned snapshots are kept in the vault; they are read back, with
//!   the values of their versions, when the image opens.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use helios_core::{Bus, Envelope, Graph, Message, NodeId, Params};
use helios_inspect::cache::ReadCache;
use helios_inspect::client::{Client, Pending};
use helios_node_core::cursor::query_json;
use helios_vault::capture::CAPTURED;
use helios_vault::cull;
use helios_vault::fields::{LABELS, PICKED, Query, RATING, REJECTED, TAGS};
use helios_vault::{
    EditSession, ImageId, LibraryRow, Location, Order, Presence, Store, Vault, Version, edit,
};
use serde_json::{Value, json};

use crate::Shell;
use crate::dialogs::Level;
use crate::library::{Change, Entry};

/// The fields the library shows (the capture time gives the day the
/// search field's dates match).
const SHOWN: [&str; 9] = [
    RATING,
    REJECTED,
    PICKED,
    LABELS,
    TAGS,
    CAPTURED,
    cull::FOCUS,
    cull::CLIP_WHITE,
    cull::CLIP_BLACK,
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

/// How often a gesture still going (a drag) is saved, in seconds.
pub const FLUSH_SECONDS: f64 = 1.0;

pub struct Persist<S: Store> {
    vault: Arc<Vault<S>>,
    bus: Arc<Bus>,
    events: Receiver<Envelope>,
    editing: Option<Editing>,
    /// A scan changed where copies are (a disk plugged in or out): the
    /// library is to be read again ([`Persist::take_locations_changed`]).
    locations_changed: bool,
    /// The library shown, refreshed by its cursor's delta.
    shown: Mutex<Option<Shown>>,
    /// The library's history as last seen ([`LibraryHistory::rev`]): when
    /// it moves, the albums are read again.
    ///
    /// [`LibraryHistory::rev`]: crate::library_history::LibraryHistory::rev
    history_rev: u64,
    /// The desktop's node, in this process: the library's writes go
    /// through it.
    node: Arc<Client>,
    /// The writes waiting their turn, in the order made.
    writes: VecDeque<Write>,
    /// The write the node runs now.
    writing: Option<(Write, Pending)>,
    /// The reads back of written images, with the images each names.
    reading: Vec<(BTreeSet<String>, Pending)>,
    /// The library's reads through the node.
    reader: Reader,
    /// The library being read again ([`Persist::ask_fill`]).
    filling: Option<Filling>,
    /// Asked again while it was read (true: by the host): read once more.
    fill_again: Option<bool>,
    /// Where the images of the library the host asked read can be read,
    /// once shown ([`Persist::take_filled`]).
    filled: Option<Files>,
    /// The search filter's query being run: the photos it takes and the
    /// facet counts.
    searching: Option<Receiver<Result<(Vec<String>, helios_vault::Facets), String>>>,
}

/// A write of the library sent to the node.
enum Write {
    /// Fields of photos (`library.fields.set`).
    Fields(Vec<Change>),
    /// A stack expanded or collapsed (`library.stacks.expand`).
    Expand(String, bool),
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
        }
    }

    /// The images whose fields it writes.
    fn images(&self) -> impl Iterator<Item = &str> {
        let changes = match self {
            Self::Fields(changes) => changes.as_slice(),
            Self::Expand(..) => &[],
        };
        changes.iter().map(|c| c.image.as_str())
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
    session: EditSession,
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
}

/// How many images one window of the library's cursor holds: the rows
/// the node reads in one store query, fields and locations together.
pub const PAGE: usize = helios_node_core::cursor::CHUNK;

/// How long the cursor of the library shown lives unread on the node, in
/// seconds: past it, the next refresh reads the library whole again.
const SHOWN_LEASE: u64 = 3_600;

/// The desktop's node as the library's reads reach it: its in-process
/// client, and the cache of its answers ([`ReadCache`]) the bus's events
/// keep true. Cloned to read on a thread of its own: the UI thread never
/// waits for a read.
#[derive(Clone)]
pub struct Reader {
    node: Arc<Client>,
    cache: Arc<ReadCache>,
}

impl Reader {
    /// Reads `method` from the cache, else from the node (waiting for it).
    fn read(&self, method: &str, params: Value) -> Result<Value, String> {
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
    if read.is_err() {
        reader.close(cursor);
    }
    let (entries, files) = read?;
    Ok(LibraryRows {
        entries,
        files,
        cursor: Some(cursor),
        order,
    })
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

/// An image as the library shows it, and where it can be read (its first
/// present copy).
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
    /// The cursor shown, brought up to date: what moved, if anything did.
    Delta(Shown, Option<Moved>),
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
            match refresh(reader, &mut shown, known) {
                Ok(moved) => return Ok(Fetched::Delta(shown, moved)),
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
    let rule = rule.map_or(Value::Null, helios_node_core::cursor::backups_json);
    let counted = reader.read(
        "library.facets",
        json!({ "query": query.clone(), "rule": rule }),
    )?;
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
    shell
        .library()
        .entries
        .iter()
        .map(|e| e.id.clone())
        .collect()
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
    /// A vault on `store`, announcing its changes on `bus`.
    pub fn new(store: S, bus: Arc<Bus>) -> Self {
        Self::on(Arc::new(Vault::new(store).with_bus(bus.clone())), bus)
    }

    /// The link to a vault opened by its owner (the local node), whose
    /// changes are announced on `bus`.
    pub fn on(vault: Arc<Vault<S>>, bus: Arc<Bus>) -> Self {
        let events = bus.subscribe();
        let service = helios_node_core::endpoint::VaultService::new(
            vault.clone(),
            bus.clone(),
            helios_core::jobs::Jobs::new(bus.clone()),
            &format!("desktop-{}", std::process::id()),
        );
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
        };
        Self {
            vault,
            bus,
            events,
            editing: None,
            locations_changed: false,
            shown: Mutex::new(None),
            history_rev: 0,
            node,
            writes: VecDeque::new(),
            writing: None,
            reading: Vec::new(),
            reader,
            filling: None,
            fill_again: None,
            filled: None,
            searching: None,
        }
    }

    /// The library's reads through the node, for a thread of its own
    /// ([`read_library`]).
    pub fn reader(&self) -> Reader {
        self.reader.clone()
    }

    /// The client of the desktop's node the library's writes go through.
    pub fn node(&self) -> &Client {
        &self.node
    }

    /// Whether a call to the node is still to be answered: the host
    /// comes back at the next frame to pick its answer up.
    pub fn busy(&self) -> bool {
        self.writing.is_some()
            || !self.writes.is_empty()
            || !self.reading.is_empty()
            || self.filling.is_some()
            || self.searching.is_some()
    }

    /// Waits until the node has answered every call made, and shows what
    /// they did: before Helios closes, so nothing the user changed is
    /// left unwritten, and in tests that look at the vault right after.
    pub fn settle(&mut self, shell: &mut Shell) {
        // A node that never answers is not waited for forever.
        let deadline = std::time::Instant::now() + std::time::Duration::from_mins(1);
        while self.busy() && std::time::Instant::now() < deadline {
            self.pump(shell);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.hear(shell);
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
            && let Some(write) = self.writes.pop_front()
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
        self.pump_reads(shell);
    }

    /// Shows the library's reads that came: the library read again, the
    /// search filter's photos and facet counts.
    fn pump_reads(&mut self, shell: &mut Shell) {
        use std::sync::mpsc::TryRecvError;
        if let Some(filling) = &self.filling {
            let read = match filling.answer.try_recv() {
                Ok(read) => Some(read),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("the library's reading stopped (a panic)".to_string()))
                }
            };
            if let Some(read) = read {
                let host = filling.host;
                self.filling = None;
                match read.and_then(|fetched| self.apply(shell, fetched)) {
                    Ok(files) => {
                        if host {
                            self.filled = Some(files);
                        }
                    }
                    Err(e) => unreadable(shell, e),
                }
                if let Some(host) = self.fill_again.take() {
                    self.ask_fill(shell, host);
                }
            }
        }
        if let Some(searching) = &self.searching {
            let read = match searching.try_recv() {
                Ok(read) => Some(read),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("the search stopped (a panic)".to_string()))
                }
            };
            if let Some(read) = read {
                self.searching = None;
                match read {
                    Ok((ids, facets)) => {
                        let mut library = shell.library_mut();
                        library.set_facets(facets);
                        library.set_matches(ids);
                    }
                    Err(e) => unreadable(shell, e),
                }
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
        match (&write, answer) {
            (_, Ok(_)) => {}
            (Write::Fields(_), Err(e)) => shell.notify(
                Level::Warning,
                Message::new(
                    "gui-library-not-saved",
                    "Not saved, the library shows what was saved last: { $error }",
                )
                .with("error", e.message),
            ),
            (Write::Expand(..), Err(e)) => shell.notify(Level::Warning, e.message),
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
        let coming: BTreeSet<&str> = self
            .writing
            .iter()
            .map(|(w, _)| w)
            .chain(&self.writes)
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

    pub fn vault(&self) -> &Arc<Vault<S>> {
        &self.vault
    }

    pub fn bus(&self) -> &Arc<Bus> {
        &self.bus
    }

    /// Whether a scan changed where copies are since the last call (a
    /// disk plugged in or out), or a photo was marked changed outside
    /// Helios or acknowledged: the host reads the library again, so its
    /// badges follow.
    pub fn take_locations_changed(&mut self) -> bool {
        std::mem::take(&mut self.locations_changed)
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
        let shown = self.shown.lock().expect("library lock").take();
        let fetched = fetch(&self.reader, shown, shell.library().sort(), &known(shell))?;
        self.apply(shell, fetched)
    }

    /// Reads the library again as [`Persist::fill_library`] does, on a
    /// thread of its own: a coming frame shows it. `host`: the host takes
    /// where its images can be read once shown ([`Persist::take_filled`]),
    /// for thumbnails.
    pub fn ask_fill(&mut self, shell: &Shell, host: bool) {
        if self.filling.is_some() {
            self.fill_again = Some(self.fill_again.unwrap_or(false) || host);
            return;
        }
        let shown = self.shown.lock().expect("library lock").take();
        let (reader, order, known) = (self.reader.clone(), shell.library().sort(), known(shell));
        let answer = spawn("helios-library-fill", move || {
            fetch(&reader, shown, order, &known)
        });
        self.filling = Some(Filling { answer, host });
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
            Fetched::Delta(shown, moved) => self.show_moved(shell, shown, moved),
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
        // Read before the library's sort changed (on the thread reading it
        // as the window opens): read again in the sort, next frame.
        if rows.order != shell.library().sort() {
            shell.library_mut().ask_resort();
        }
        shell.set_library(rows.entries);
        let old = match rows.cursor {
            Some(cursor) => self.shown.lock().expect("library lock").replace(Shown {
                cursor,
                files: rows.files.iter().cloned().collect(),
                order: rows.order,
            }),
            None => None,
        };
        if let Some(old) = old {
            self.reader.close(old.cursor);
        }
        self.show_stacks(shell)?;
        self.show_decisions(shell)?;
        Ok(rows.files)
    }

    /// Shows the library brought up to date by its cursor's delta.
    fn show_moved(
        &self,
        shell: &mut Shell,
        shown: Shown,
        moved: Option<Moved>,
    ) -> Result<Vec<(String, PathBuf)>, String> {
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
        let old = self.shown.lock().expect("library lock").replace(shown);
        if let Some(old) = old {
            self.reader.close(old.cursor);
        }
        self.show_stacks(shell)?;
        self.show_decisions(shell)?;
        Ok(files)
    }

    /// Shows the vault's stacks in the library.
    pub fn show_stacks(&self, shell: &mut Shell) -> Result<(), String> {
        let stacks = self.vault.stacks()?;
        shell.library_mut().set_stacks(&stacks);
        Ok(())
    }

    /// Shows the `stacks.suggest` proposals waiting in the Stacks panel.
    pub fn show_decisions(&self, shell: &mut Shell) -> Result<(), String> {
        let decisions = helios_library::stacks::decisions(self.vault.as_ref())?;
        shell.stack_review_mut().set_decisions(decisions);
        Ok(())
    }

    /// Runs what the Stacks panel asked: a proposal, an answer.
    fn answer_stacks(&self, shell: &mut Shell, now: u64) {
        use crate::stack_review::Request;
        use helios_library::stacks;
        let requests = shell.stack_review_mut().take_requests();
        if requests.is_empty() {
            return;
        }
        let bus = Some(self.bus.as_ref());
        for r in requests {
            let done = match r {
                Request::Suggest { images } => images
                    .iter()
                    .map(|h| ImageId::from_hex(h))
                    .collect::<Result<Vec<_>, _>>()
                    .and_then(|ids| {
                        let resolvers = stacks::Resolvers::default();
                        stacks::suggest(
                            self.vault.as_ref(),
                            bus,
                            &resolvers,
                            stacks::RULES,
                            &ids,
                            now,
                        )
                    })
                    .map(|_| ()),
                Request::Accept { decision, groups } => stacks::accept(
                    self.vault.as_ref(),
                    bus,
                    &decision,
                    groups.as_deref(),
                    WHO,
                    now,
                )
                .map(|_| ()),
                Request::Decline { decision } => {
                    stacks::decline(self.vault.as_ref(), bus, &decision, WHO, now).map(|_| ())
                }
            };
            if let Err(e) = done {
                shell.notify(Level::Warning, e);
            }
        }
        if let Err(e) = self.show_decisions(shell) {
            shell.notify(Level::Error, e);
        }
    }

    /// Shows an image's network in the viewport with the image's saved
    /// edit: its newest head, returned (none for an image never edited).
    /// From then on each step is saved as a version continuing it.
    pub fn open_image(
        &mut self,
        shell: &mut Shell,
        image: ImageId,
        mut graph: Graph,
        node: NodeId,
        port: &str,
    ) -> Result<Option<Version>, String> {
        // Opened for the first time, a raw gets its camera's default
        // calibration profile (as its first edit, read just below).
        if let Some(said) = crate::calibrate::on_open(&self.vault, image, &graph, shell.values()) {
            shell.notify(Level::Info, said);
        }
        let (session, head) = EditSession::open(&self.vault, image)?;
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
        {
            let mut book = crate::edit_snapshots::lock(shell.snapshots());
            book.load(image, snapshots);
            book.set_saved(saved_params(&modules));
        }
        let committed = {
            let mut store = shell.values().lock().expect("value store lock");
            // The labels and locks it was saved with; another image's go.
            let marks = head.as_ref().map_or(&Value::Null, |h| &h.edit["marks"]);
            crate::mask_tree::restore_marks(&mut store, marks);
            store.committed()
        };
        self.editing = Some(Editing {
            session,
            modules,
            committed,
            flushed_at: None,
            gesture: None,
            unsaved: false,
        });
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
        let kept = match self.vault.snapshots(&image) {
            Ok(k) => k,
            Err(e) => {
                shell.notify(Level::Warning, e);
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for s in kept {
            let edit = match &s.version {
                None => Ok(unedited.clone()),
                Some(v) => self
                    .vault
                    .version(v)
                    .and_then(|found| found.ok_or_else(|| format!("no version {}", v.hex())))
                    .map(|found| found.edit),
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
        use crate::edit_snapshots::{Pin, head_step};
        let Some(e) = &self.editing else {
            return;
        };
        let image = e.session.image();
        let (step, committed, settled) = {
            let store = shell.values().lock().expect("value store lock");
            let settled =
                !e.unsaved && store.open_gesture().is_none() && store.committed() == e.committed;
            (head_step(&store), store.committed(), settled)
        };
        let (pinned, write) = {
            let mut book = crate::edit_snapshots::lock(shell.snapshots());
            if book.image() != Some(image) {
                return;
            }
            let pin = e.session.head().map_or(Pin::Unedited, Pin::Version);
            book.settle(settled.then_some((step, committed, pin)));
            (book.pinned(), book.take_dirty())
        };
        // Said once: the next change writes the whole list again.
        if write && let Err(err) = self.vault.set_snapshots(&image, &pinned) {
            shell.notify(
                Level::Error,
                Message::new(
                    "gui-snapshots-not-saved",
                    "The snapshots were not saved: { $error }",
                )
                .with("error", err),
            );
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
    pub fn close_image(&mut self, shell: &mut Shell, now: f64) {
        self.save_edit(shell, now);
        self.editing = None;
    }

    /// The edit being saved, if an image is open.
    pub fn session(&self) -> Option<&EditSession> {
        self.editing.as_ref().map(|e| &e.session)
    }

    /// Reads the Library's pinned and recent filters once, and writes
    /// them back each time they change (`crate::recent_filters`): a
    /// library document, outside the library's history.
    fn keep_filter_history(&mut self, shell: &mut Shell) {
        use crate::recent_filters::{FilterHistory, KEY, NS};
        if shell.library_mut().take_filter_history_wanted() {
            let read = self.vault.library_document(NS, KEY);
            match read {
                Ok(doc) => shell
                    .library_mut()
                    .set_filter_history(FilterHistory::of(doc.as_ref())),
                Err(e) => shell.notify(Level::Warning, e),
            }
        }
        let write = shell.library_mut().take_filter_history_write();
        if let Some(history) = write
            && let Err(e) = self
                .vault
                .set_library_document(NS, KEY, Some(&history.to_json()))
        {
            shell.notify(Level::Warning, e);
        }
    }

    /// Makes the album changes the library asked, each one step of the
    /// library's history; reads the albums again when that history moved
    /// (a change, an undo, a redo, an import).
    fn edit_albums(&mut self, shell: &mut Shell, now: f64) {
        use crate::library::AlbumEdit as E;
        let edits = shell.library_mut().take_album_edits();
        let history = shell.library_history();
        for edit in edits {
            let ids = |images: &[String]| -> Result<Vec<ImageId>, String> {
                images.iter().map(|i| ImageId::from_hex(i)).collect()
            };
            let before = self.vault.album(edit.id()).ok().flatten();
            let name = before.as_ref().map(|a| a.name.clone()).unwrap_or_default();
            // Whether B put the photos in the quick collection.
            let mut added = true;
            let done = match &edit {
                E::Create { id, name, images } => ids(images)
                    .and_then(|i| self.vault.create_album(Some(id), name, &i))
                    .map(|(a, c)| (a.name, a.images.len(), c)),
                E::Rename { id, name: to } => self
                    .vault
                    .rename_album(id, to)
                    .map(|c| (name.clone(), 0, c)),
                E::Delete { id, members } => self
                    .vault
                    .delete_collection(id, *members)
                    .map(|c| (name.clone(), 0, c)),
                E::CreateSet { id, name, set } => self
                    .vault
                    .create_set(Some(id), name, set.as_deref())
                    .map(|(a, c)| (a.name, 0, c)),
                E::Move { id, set } => self
                    .vault
                    .move_collection(id, set.as_deref())
                    .map(|c| (name.clone(), 0, c)),
                E::Add { id, images } => ids(images)
                    .and_then(|i| self.vault.add_to_album(id, &i))
                    .map(|c| (name.clone(), images.len(), c)),
                E::Remove { id, images } => ids(images)
                    .and_then(|i| self.vault.remove_from_album(id, &i))
                    .map(|c| (name.clone(), images.len(), c)),
                E::CreateSmart { id, name, rules } => self
                    .vault
                    .create_smart_album(Some(id), name, rules.clone())
                    .map(|(a, c)| (a.name, 0, c)),
                E::Rules { id, rules } => self
                    .vault
                    .set_album_rules(id, rules.clone())
                    .map(|c| (name.clone(), 0, c)),
                E::QuickToggle { images } => {
                    // The photos it moved: those already in are not counted.
                    let held = self.vault.quick_collection().map_or(0, |q| q.len());
                    ids(images)
                        .and_then(|i| self.vault.toggle_quick(&i))
                        .map(|(a, c)| {
                            added = a;
                            let now = self.vault.quick_collection().map_or(held, |q| q.len());
                            (String::new(), now.abs_diff(held), c)
                        })
                }
                E::QuickClear => {
                    let count = self.vault.quick_collection().map_or(0, |q| q.len());
                    self.vault.clear_quick().map(|c| (String::new(), count, c))
                }
                E::QuickSave { id, name, clear } => self
                    .vault
                    .save_quick(Some(id), name, *clear)
                    .map(|(a, c)| (a.name, a.images.len(), c)),
            };
            match done {
                Ok((name, count, changes)) => {
                    let label = crate::library_history::album_label(&edit, &name, count, added);
                    history.record(label, changes, now);
                }
                Err(e) => shell.notify(
                    Level::Error,
                    Message::new(
                        "gui-album-not-changed",
                        "The album was not changed: { $error }",
                    )
                    .with("error", e),
                ),
            }
        }
        self.keep_filter_history(shell);
        let rev = history.rev();
        if rev != self.history_rev {
            self.history_rev = rev;
            shell.library_mut().ask_albums();
        }
        if shell.library_mut().take_albums_wanted() {
            match self
                .vault
                .albums()
                .and_then(|a| Ok((a, self.vault.quick_collection()?)))
            {
                Ok((albums, quick)) => {
                    shell.library_mut().set_quick(quick);
                    shell.library_mut().set_albums(albums);
                }
                Err(e) => shell.notify(
                    Level::Error,
                    Message::new(
                        "gui-vault-unreadable",
                        "Cannot read the library: { $error }",
                    )
                    .with("error", e),
                ),
            }
        }
    }

    /// Writes what the user changed and shows what other writers did;
    /// `now` is the time in seconds (since the epoch for a real clock).
    pub fn frame(&mut self, shell: &mut Shell, now: f64) {
        let at = now.max(0.0) as u64;
        let changes = shell.take_library_changes();
        if !changes.is_empty() {
            self.writes.push_back(Write::Fields(changes));
        }
        shell.set_edited_image(self.editing.as_ref().map(|e| e.session.image().hex()));
        if let Some(e) = &mut self.editing {
            e.session.set_meta(shell.session_meta());
        }
        self.save_edit(shell, now);
        self.write_sessions(shell);
        self.pin_snapshots(shell);
        self.answer_stacks(shell, at);
        let expansions = shell.library_mut().take_expansions();
        self.writes.extend(
            expansions
                .into_iter()
                .map(|(stack, expanded)| Write::Expand(stack, expanded)),
        );
        self.pump(shell);
        // The library sorted another way: read again in that order.
        if shell.library_mut().take_resort() {
            self.ask_fill(shell, false);
        }
        // The Folders section open: its counts, one store query.
        if shell.library_mut().take_folders_wanted() {
            match self.vault.folders(&helios_vault::Query::default()) {
                Ok(folders) => shell.library_mut().set_folders(folders),
                Err(e) => shell.notify(
                    Level::Error,
                    Message::new(
                        "gui-vault-unreadable",
                        "Cannot read the library: { $error }",
                    )
                    .with("error", e),
                ),
            }
        }
        self.edit_albums(shell, now);
        let query = shell.library_mut().take_query();
        if let Some(query) = query {
            // How many photos carry each value of each facet, then the
            // photos it takes, read through the node on a thread of its
            // own: a coming frame shows them. A newer query replaces one
            // still running, whose answer is not shown.
            let rule = shell.library().facet_rule().cloned();
            let reader = self.reader.clone();
            self.searching = Some(spawn("helios-library-search", move || {
                search(&reader, &query, rule.as_ref())
            }));
        }
        self.hear(shell);
    }

    /// Shows what the vault's events say changed.
    fn hear(&mut self, shell: &mut Shell) {
        let mut stacks_changed = false;
        let mut decided = false;
        while let Ok(event) = self.events.try_recv() {
            if event.kind == "vault.stack_changed" {
                stacks_changed = true;
            } else if event.kind == "library.decision" {
                decided = true;
            } else if event.kind == helios_vault::LOCATIONS_CHANGED
                || event.kind == helios_vault::changed::CHANGED_OUTSIDE
                || event.kind == helios_vault::capture::CAPTURES_SET
            {
                self.locations_changed = true;
            } else if event.kind == "vault.field_changed" {
                let p = &event.payload;
                if let (Some(image), Some(field)) = (p["image"].as_str(), p["field"].as_str()) {
                    let new = Some(&p["new"]).filter(|v| !v.is_null());
                    shell.library_mut().show_field(image, field, new);
                }
            }
        }
        if stacks_changed && let Err(e) = self.show_stacks(shell) {
            shell.notify(Level::Error, e);
        }
        if decided && let Err(e) = self.show_decisions(shell) {
            shell.notify(Level::Error, e);
        }
    }

    /// Keeps the workshop sessions' records the desktop wrote, and drops
    /// those it ended unkept.
    fn write_sessions(&self, shell: &mut Shell) {
        for (id, record) in shell.take_session_writes() {
            if let Err(e) = self.vault.set_workshop_session(&id, record.as_ref()) {
                shell.notify(Level::Warning, e);
            }
        }
    }

    /// Drops the workshop sessions a former run left unkept (it ended
    /// before their session did): a session is kept only when the user
    /// says so (or, later, when it is generalised).
    pub fn drop_unkept_sessions(&self) -> Result<usize, String> {
        let mut n = 0;
        for (id, record) in self.vault.workshop_sessions()? {
            if record["kept"].as_bool() != Some(true) {
                self.vault.set_workshop_session(&id, None)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Saves the edit when the value store committed a step, or while a
    /// gesture goes on, every [`FLUSH_SECONDS`].
    fn save_edit(&mut self, shell: &mut Shell, now: f64) {
        let Some(e) = &mut self.editing else {
            return;
        };
        let at = now.max(0.0) as u64;
        let (committed, label, origin, gesture, current) = {
            let store = shell.values().lock().expect("value store lock");
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
            let marks = crate::mask_tree::marks(&store);
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
            )
        };
        let saved = if committed == e.committed {
            if let Some((gesture, by)) = gesture {
                e.gesture = Some((gesture.clone(), by.clone()));
                let since = *e.flushed_at.get_or_insert(now);
                if now - since < FLUSH_SECONDS {
                    return;
                }
                e.flushed_at = Some(now);
                e.session
                    .save_in_progress(&self.vault, current, &gesture, &by, at)
            } else {
                e.flushed_at = None;
                // A gesture ended with no step: cancelled. If it was
                // saved in progress, the values it put back are saved.
                let Some((gesture, by)) = e.gesture.take() else {
                    return;
                };
                if !e.session.in_progress() {
                    return;
                }
                e.session.save(
                    &self.vault,
                    current,
                    &format!("{gesture} (cancelled)"),
                    &by,
                    at,
                )
            }
        } else {
            e.committed = committed;
            e.flushed_at = None;
            e.gesture = None;
            e.session.save(&self.vault, current, &label, &origin, at)
        };
        e.unsaved = saved.is_err();
        if saved.is_ok() && !e.session.in_progress() {
            // Renders of the image's head version are made again once a
            // gesture ends, not at each save while it goes (a drag).
            shell.library_mut().edit_changed(&e.session.image().hex());
        }
        if let Err(err) = saved {
            shell.notify(
                Level::Error,
                Message::new("gui-edit-not-saved", "The edit was not saved: { $error }")
                    .with("error", err),
            );
        }
    }
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
