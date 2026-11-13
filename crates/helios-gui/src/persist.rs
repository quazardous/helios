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
//! - runs the vault query of the library's search filter, and keeps a
//!   stack expanded or collapsed as the user left it;
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
use helios_inspect::client::{Client, Pending};
use helios_vault::capture::CAPTURED;
use helios_vault::cull;
use helios_vault::fields::{LABELS, PICKED, Query, RATING, REJECTED, TAGS};
use helios_vault::{
    CursorId, EditSession, ImageId, LibraryRow, Location, Order, Presence, Store, Vault, Version,
    edit,
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
    node: Client,
    /// The writes waiting their turn, in the order made.
    writes: VecDeque<Write>,
    /// The write the node runs now.
    writing: Option<(Write, Pending)>,
    /// The reads back of written images, with the images each names.
    reading: Vec<(BTreeSet<String>, Pending)>,
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

/// How many images one window of the library's cursor holds: each window
/// is one store query, fields and locations together.
pub const PAGE: usize = 2_000;

/// The library as the vault holds it: its entries (each with its
/// presence: present, offline or missing), and where each image can be
/// read (its first present copy), for thumbnails.
pub struct LibraryRows {
    entries: Vec<Entry>,
    files: Vec<(String, PathBuf)>,
    /// The cursor it was read through, kept open so the library shown is
    /// refreshed by its delta ([`Persist::fill_library`]).
    cursor: Option<CursorId>,
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

/// Reads the library from the vault through a cursor, a window of
/// [`PAGE`] images at a time: one query opens it, one query per window
/// reads the images' fields and locations, one more the photos changed
/// outside Helios. Nothing is asked of the files: presence (present,
/// offline, missing) is what the scans stored, and the file's size and
/// date are those of its location. Nothing of the window either: a thread
/// of its own can read it while the window opens
/// ([`Persist::show_library`] shows it).
///
/// The photos come in `order` (the library's sort), sorted by the vault in
/// the query that opens the cursor.
pub fn read_library<S: Store>(vault: &Vault<S>, order: Order) -> Result<LibraryRows, String> {
    let cursor = vault.open_cursor(Query {
        order,
        ..Query::default()
    })?;
    let read = read_cursor(vault, cursor.id, cursor.count);
    if read.is_err() {
        vault.close_cursor(cursor.id);
    }
    let (entries, files) = read?;
    Ok(LibraryRows {
        entries,
        files,
        cursor: Some(cursor.id),
        order,
    })
}

/// Where each image can be read, by id.
type Files = Vec<(String, PathBuf)>;

/// Every image of a cursor as the library shows it, window by window.
fn read_cursor<S: Store>(
    vault: &Vault<S>,
    cursor: CursorId,
    count: usize,
) -> Result<(Vec<Entry>, Files), String> {
    let changed = changed_outside(vault)?;
    let mut entries = Vec::with_capacity(count);
    let mut files = Vec::with_capacity(count);
    for offset in (0..count).step_by(PAGE) {
        for row in vault.fetch_library(cursor, offset, PAGE, &SHOWN)? {
            let (entry, path) = entry_of(row, &changed);
            if let Some(path) = path {
                files.push((entry.id.clone(), path));
            }
            entries.push(entry);
        }
    }
    Ok((entries, files))
}

/// The photos another application changed, until acknowledged.
fn changed_outside<S: Store>(vault: &Vault<S>) -> Result<HashMap<ImageId, u64>, String> {
    Ok(vault.changed_outside_images()?.into_iter().collect())
}

/// An image as the library shows it, and where it can be read (its first
/// present copy).
fn entry_of(row: LibraryRow, changed: &HashMap<ImageId, u64>) -> (Entry, Option<PathBuf>) {
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
        changed_outside: changed.get(&row.id).copied(),
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

/// The library shown: the cursor it was read through and where each of
/// its images can be read.
struct Shown {
    cursor: CursorId,
    files: BTreeMap<String, PathBuf>,
    /// The order its cursor sorts by.
    order: Order,
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
        let node = Client::in_process(Arc::new(service)).with_origin(WHO);
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
        }
    }

    /// The client of the desktop's node the library's writes go through.
    pub fn node(&self) -> &Client {
        &self.node
    }

    /// Whether a call to the node is still to be answered: the host
    /// comes back at the next frame to pick its answer up.
    pub fn busy(&self) -> bool {
        self.writing.is_some() || !self.writes.is_empty() || !self.reading.is_empty()
    }

    /// Waits until the node has answered every call made, and shows what
    /// they did: before Helios closes, so nothing the user changed is
    /// left unwritten, and in tests that look at the vault right after.
    pub fn settle(&mut self, shell: &mut Shell) {
        // A node that never answers is not waited for forever.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
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

    /// Fills the library with the vault's images; returns where each one
    /// can be read (its first present copy), for thumbnails. Once shown,
    /// the library is brought up to date by its cursor's delta: nothing is
    /// read when the catalog did not change, else the images added and
    /// those whose copies moved (an import, a scan, an undo), in windows
    /// of [`PAGE`]; the others keep their entries as shown.
    ///
    /// A library sorted another way since is read whole again, in its
    /// sort, through a new cursor.
    pub fn fill_library(&self, shell: &mut Shell) -> Result<Vec<(String, PathBuf)>, String> {
        let order = shell.library().sort();
        let shown = self.shown.lock().expect("library lock").take();
        if let Some(shown) = shown {
            let cursor = shown.cursor;
            if shown.order == order {
                match self.refresh_library(shell, shown) {
                    Ok(files) => return Ok(files),
                    // Read whole again, through a new cursor.
                    Err(_) => self.vault.close_cursor(cursor),
                }
            } else {
                self.vault.close_cursor(cursor);
            }
        }
        let rows = read_library(&self.vault, order)?;
        self.show_library(shell, rows)
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
            self.vault.close_cursor(old.cursor);
        }
        self.show_stacks(shell)?;
        self.show_decisions(shell)?;
        Ok(rows.files)
    }

    /// Brings the library shown up to date by its cursor's delta.
    fn refresh_library(
        &self,
        shell: &mut Shell,
        mut shown: Shown,
    ) -> Result<Vec<(String, PathBuf)>, String> {
        let delta = self.vault.refresh(shown.cursor)?;
        if delta.changed {
            // The entries shown, their fields kept up to date by the
            // vault's events.
            let mut entries: HashMap<String, Entry> = shell
                .library()
                .entries
                .iter()
                .map(|e| (e.id.clone(), e.clone()))
                .collect();
            for id in &delta.removed {
                entries.remove(&id.hex());
                shown.files.remove(&id.hex());
            }
            let order = self.vault.cursor_ids(shown.cursor, 0, delta.count)?;
            // The images added, those whose copies moved, and any the
            // library does not show (it was filled from elsewhere).
            let mut read: Vec<ImageId> = delta.inserted.clone();
            read.extend(&delta.replaced);
            read.extend(
                order
                    .iter()
                    .filter(|id| !entries.contains_key(&id.hex()) && !delta.inserted.contains(id)),
            );
            let changed = changed_outside(&self.vault)?;
            for ids in read.chunks(PAGE) {
                for row in self.vault.library_rows(ids, &SHOWN)? {
                    let (entry, path) = entry_of(row, &changed);
                    match path {
                        Some(path) => shown.files.insert(entry.id.clone(), path),
                        None => shown.files.remove(&entry.id),
                    };
                    entries.insert(entry.id.clone(), entry);
                }
            }
            let entries = order
                .iter()
                .filter_map(|id| {
                    let mut entry = entries.remove(&id.hex())?;
                    entry.changed_outside = changed.get(id).copied();
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
        *self.shown.lock().expect("library lock") = Some(shown);
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

    /// The images the vault holds for a query, in its order.
    fn select(&self, query: Query) -> Result<Vec<String>, String> {
        let cursor = self.vault.open_cursor(query)?;
        let rows = self.vault.fetch(cursor.id, 0, cursor.count, &[]);
        self.vault.close_cursor(cursor.id);
        Ok(rows?.into_iter().map(|r| r.id.hex()).collect())
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
        if shell.library_mut().take_resort()
            && let Err(e) = self.fill_library(shell)
        {
            shell.notify(
                Level::Error,
                Message::new(
                    "gui-vault-unreadable",
                    "Cannot read the library: { $error }",
                )
                .with("error", e),
            );
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
            // The photos it takes, then how many carry each value of each
            // facet: two store queries, whatever the library's size.
            let rule = shell.library().facet_rule().cloned();
            let answer = self
                .vault
                .facets(&query, rule.as_ref())
                .and_then(|facets| self.select(query).map(|ids| (ids, facets)));
            match answer {
                Ok((ids, facets)) => {
                    let mut library = shell.library_mut();
                    library.set_facets(facets);
                    library.set_matches(ids);
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
