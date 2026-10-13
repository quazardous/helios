//! The library: the vault's images in egui-pop's photo library widgets (the
//! Vault panel's thumbnail grid, the Filmstrip, the Compare panel), with
//! their rating, flag (picked or rejected), colour labels and tags.
//!
//! The shell does not hold the vault: the host gives it the images
//! ([`Shell::set_library`](crate::Shell::set_library)) and their stacks
//! ([`Library::set_stacks`]), sends their thumbnails as they are read (from
//! any thread, through [`Library::thumbnails`]: the embedded previews of
//! raws), and applies what the user changes, collected with
//! [`Shell::take_library_changes`](crate::Shell::take_library_changes) as
//! vault field commands. The shown values change at once; the host puts
//! back what the vault holds ([`Library::show_field`]) when a write fails
//! or another writer changes a field.
//!
//! Every mark goes through egui-pop's library steps (`library.rate`,
//! `library.flag`, `library.label`, `library.tag`), whether it comes from
//! the grid's culling keys, the marks bar, a Helios command (`vault.*`, the
//! keys a profile or a template binds) or an agent's `library.*` command:
//! a plugin may veto or redirect it, and what ran is what is written.
//!
//! A change says what the user did, never "here is my copy": a colour
//! label or a tag is added or removed, so one another writer put on
//! meanwhile stays, and a reject is a field of its own, so taking it back
//! keeps the stars.
//!
//! The search field's filter becomes a vault query (rating, labels, flags,
//! tags; [`query_of`]), run by the host ([`Library::take_query`],
//! [`Library::set_matches`]); its words and dates are matched here, on the
//! names, tags and days the library holds.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, ColorImage, TextureHandle, Ui};
use egui_pop::api::AppHandle;
use egui_pop::views::library::compare::{CompareState, CompareView, Pane};
use egui_pop::views::library::filter::{Candidate, Filter, SearchField, SearchState};
use egui_pop::views::library::marks::{ColorLabels, FlagToggle, RatingStars};
use egui_pop::views::library::tags::{TagInput, TagInputState};
use egui_pop::views::library::{
    Badge, Change as Mark, ColorLabel, CullKeymap, Flag, LibraryItems, LibraryState, Marks, Signal,
    Stack, Thumb, ThumbGrid, ThumbSource, change,
};
use helios_core::{Image, Message, Region};
pub use helios_vault::fields::Op;
use helios_vault::fields::{LABELS as LABELS_FIELD, PICKED, Query, RATING, REJECTED, TAGS};
use serde_json::Value;

/// The library's name in egui-pop's steps and commands (`library` in
/// their arguments).
pub const LIBRARY: &str = "helios.library";

/// The labels, in the order darktable gives them F1 to F5.
pub const LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

/// The stack an image is in, as the library shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StackOf {
    /// The vault's stack id.
    pub id: String,
    /// The image shown when it is collapsed.
    pub top: String,
    /// How many of its images the library holds.
    pub count: usize,
}

/// An image of the library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    /// The vault's id (hex).
    pub id: String,
    /// What to call it: its file name.
    pub name: String,
    /// 0 to 5 stars.
    pub rating: i64,
    /// Rejected; the stars stay beside it.
    pub rejected: bool,
    /// Picked while culling.
    pub picked: bool,
    pub labels: Vec<String>,
    pub tags: Vec<String>,
    /// The file's size in bytes, when it is on disk.
    pub bytes: Option<u64>,
    /// When the file was last modified, in seconds since 1970 (UTC); shown
    /// in the system's time zone.
    pub modified: Option<i64>,
    /// Its day (`YYYY-MM-DD`), for the search field's dates.
    pub day: Option<String>,
    pub stack: Option<StackOf>,
}

/// A file size as the interface's language writes it: `12.3 MiB`,
/// `12,3 Mio` in French.
pub fn size_text(ctx: &egui::Context, bytes: u64) -> String {
    let locale = crate::i18n::locale(ctx);
    let b = bytes as f64;
    let (unit, value) = if bytes < 1024 {
        (Message::new("gui-size-bytes", "{ $size } B"), b)
    } else if b < 1024.0 * 1024.0 {
        (Message::new("gui-size-kib", "{ $size } KiB"), b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        (
            Message::new("gui-size-mib", "{ $size } MiB"),
            b / (1024.0 * 1024.0),
        )
    } else {
        (
            Message::new("gui-size-gib", "{ $size } GiB"),
            b / (1024.0 * 1024.0 * 1024.0),
        )
    };
    let size = crate::i18n::number(&locale, value, 1);
    crate::i18n::text(ctx, unit.with("size", size))
}

/// A moment (seconds since 1970) as the calendar day and time it was in
/// time zone `tz`, its offset at that moment (summer time included).
pub fn day_in<Tz: chrono::TimeZone>(tz: &Tz, seconds: i64) -> egui_pop::i18n::format::Date {
    use chrono::Offset;
    let offset = chrono::DateTime::from_timestamp(seconds, 0).map_or(0, |utc| {
        i64::from(
            tz.offset_from_utc_datetime(&utc.naive_utc())
                .fix()
                .local_minus_utc(),
        )
    });
    egui_pop::i18n::format::Date::from_unix(seconds + offset)
}

/// The day a moment (seconds since 1970) was in the system's time zone,
/// as `YYYY-MM-DD`.
pub fn day_text(seconds: i64) -> Option<String> {
    let utc = chrono::DateTime::from_timestamp(seconds, 0)?;
    Some(
        utc.with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string(),
    )
}

/// What the library says of an image's file beside its name: its size and
/// the day it was last modified, in the interface's language and the
/// system's time zone.
pub fn file_details(ctx: &egui::Context, entry: &Entry) -> Option<String> {
    file_details_in(ctx, entry, &chrono::Local)
}

/// [`file_details`] with the day as it was in time zone `tz`.
pub fn file_details_in<Tz: chrono::TimeZone>(
    ctx: &egui::Context,
    entry: &Entry,
    tz: &Tz,
) -> Option<String> {
    use egui_pop::i18n::format::{DateStyle, date};
    let locale = crate::i18n::locale(ctx);
    let size = entry.bytes.map(|b| size_text(ctx, b));
    let day = entry
        .modified
        .map(|s| date(&locale, &day_in(tz, s), DateStyle::Medium));
    match (size, day) {
        (Some(size), Some(day)) => Some(crate::i18n::text(
            ctx,
            Message::new("gui-library-file", "{ $size } · modified { $day }")
                .with("size", size)
                .with("day", day),
        )),
        (Some(one), None) | (None, Some(one)) => Some(one),
        (None, None) => None,
    }
}

/// A set field as a list of names.
fn names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

impl Entry {
    /// Shows a field's stored value (`None`: the field is empty).
    pub fn show(&mut self, field: &str, value: Option<&Value>) {
        match field {
            RATING => self.rating = value.and_then(Value::as_i64).unwrap_or(0),
            REJECTED => self.rejected = value.and_then(Value::as_bool).unwrap_or(false),
            PICKED => self.picked = value.and_then(Value::as_bool).unwrap_or(false),
            LABELS_FIELD => {
                self.labels = names(value);
                self.labels
                    .sort_by_key(|l| LABELS.iter().position(|k| k == l));
            }
            TAGS => self.tags = names(value),
            _ => {}
        }
    }

    /// Its flag: a reject wins over a pick.
    pub fn flag(&self) -> Flag {
        if self.rejected {
            Flag::Reject
        } else if self.picked {
            Flag::Pick
        } else {
            Flag::None
        }
    }

    /// Its marks, as the views draw them.
    pub fn marks(&self) -> Marks {
        Marks {
            rating: self.rating.clamp(0, 5) as u8,
            labels: self
                .labels
                .iter()
                .filter_map(|l| ColorLabel::parse(l))
                .collect(),
            flag: self.flag(),
        }
    }
}

/// A command on one field of one image, for the vault.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub image: String,
    pub field: String,
    pub op: Op,
}

impl Change {
    /// What a later change replaces: a set replaces a set, adding or
    /// removing a name replaces a change of the same name.
    fn replaces(&self, other: &Change) -> bool {
        self.image == other.image
            && self.field == other.field
            && match (&self.op, &other.op) {
                (Op::Add(a) | Op::Remove(a), Op::Add(b) | Op::Remove(b)) => a == b,
                (Op::Set(_), Op::Set(_)) => true,
                _ => false,
            }
    }
}

/// The vault query a search filter makes: its rating, labels, flags and
/// tags (its words and dates are matched on what the library holds).
pub fn query_of(filter: &Filter) -> Query {
    Query {
        min_rating: filter.min_rating.map(i64::from),
        labels: filter.labels.iter().map(|l| l.name().to_string()).collect(),
        flags: filter.flags.iter().map(|f| f.name().to_string()).collect(),
        tags: filter.tags.clone(),
        ..Query::default()
    }
}

/// The longest side of a thumbnail, in pixels.
pub const THUMB: u32 = 256;

pub struct Library {
    pub entries: Vec<Entry>,
    /// The image under the cursor, by its place in `entries`.
    pub selected: Option<usize>,
    /// Culling: after a mark, the cursor moves to the next image.
    pub auto_advance: bool,
    /// The comparison's zoom and pan follow each other.
    pub synced: bool,
    /// Marks a plugin refused, to say.
    pub notes: Vec<String>,
    /// Each entry's place, by id.
    index: HashMap<String, usize>,
    /// What the views show, in order: the filter's matches, a stack's
    /// members right after its top image.
    order: Vec<usize>,
    revision: u64,
    state: LibraryState<String>,
    search: SearchState,
    filter: Filter,
    /// The images the vault matched for the filter, once the host ran its
    /// query.
    matches: Option<HashSet<String>>,
    /// A query for the host to run.
    query: Option<Query>,
    tags: TagInputState,
    /// Every tag the library's images carry, for completion.
    known_tags: Vec<String>,
    compare: CompareState,
    /// The image marked best in the comparison.
    best: Option<String>,
    app: Option<AppHandle>,
    changes: Vec<Change>,
    /// Stacks expanded or collapsed, for the vault: (stack, expanded).
    expansions: Vec<(String, bool)>,
    /// An image opened (Enter, a double click), for the host.
    activated: Option<String>,
    sender: Sender<(String, Image)>,
    receiver: Receiver<(String, Image)>,
    textures: HashMap<String, TextureHandle>,
}

impl Default for Library {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Library {
            entries: Vec::new(),
            selected: None,
            auto_advance: false,
            synced: true,
            notes: Vec::new(),
            index: HashMap::new(),
            order: Vec::new(),
            revision: 0,
            state: LibraryState::new(LIBRARY),
            search: SearchState::new(),
            filter: Filter::default(),
            matches: None,
            query: None,
            tags: TagInputState::default(),
            known_tags: Vec::new(),
            compare: CompareState::new(),
            best: None,
            app: None,
            changes: Vec::new(),
            expansions: Vec::new(),
            activated: None,
            sender,
            receiver,
            textures: HashMap::new(),
        }
    }
}

/// An image scaled to no larger than [`THUMB`]: what a thread reading
/// thumbnails sends, so the interface thread never scales a full-size
/// preview (scaling a small one again costs a copy).
pub fn small(image: &Image) -> Image {
    let r = image.region();
    let scale = (f64::from(THUMB) / f64::from(r.width.max(r.height).max(1))).min(1.0);
    let w = ((f64::from(r.width) * scale).round() as u32).max(1);
    let h = ((f64::from(r.height) * scale).round() as u32).max(1);
    let at = |v: i64| (v as f64 * scale).floor() as i64;
    image.sample_at(
        Region {
            x: at(r.x),
            y: at(r.y),
            width: w,
            height: h,
        },
        scale,
    )
}

/// An image in the working space (linear Rec.2020) as an sRGB picture no
/// larger than [`THUMB`].
pub fn thumbnail(image: &Image) -> ColorImage {
    use helios_photo::color;
    let small = small(image);
    let (w, h) = (small.region().width, small.region().height);
    let to_srgb = color::mul(
        &color::inverse(&color::SRGB_TO_XYZ).expect("invertible primaries"),
        &color::REC2020_TO_XYZ,
    );
    let n = small.channels() as usize;
    let pixels = small
        .data()
        .chunks_exact(n)
        .map(|p| {
            let rgb = [0, 1, 2].map(|c| f64::from(p[c.min(n - 1)]));
            let v = color::apply(&to_srgb, rgb)
                .map(|v| (color::srgb_encode(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
            Color32::from_rgb(v[0], v[1], v[2])
        })
        .collect();
    ColorImage::new([w as usize, h as usize], pixels)
}

/// The images the views show.
struct Items<'a> {
    entries: &'a [Entry],
    order: &'a [usize],
    /// Stacks shown (a filter flattens them).
    stacks: bool,
    revision: u64,
}

impl Items<'_> {
    fn entry(&self, i: usize) -> &Entry {
        &self.entries[self.order[i]]
    }
}

impl LibraryItems<String> for Items<'_> {
    fn len(&self) -> usize {
        self.order.len()
    }

    fn key(&self, index: usize) -> String {
        self.entry(index).id.clone()
    }

    fn label(&self, index: usize) -> String {
        self.entry(index).name.clone()
    }

    fn marks(&self, index: usize) -> Marks {
        self.entry(index).marks()
    }

    fn stack(&self, index: usize) -> Option<Stack<String>> {
        let s = self.entry(index).stack.as_ref().filter(|_| self.stacks)?;
        Some(Stack {
            top: s.top.clone(),
            count: s.count,
        })
    }

    fn badges(&self, index: usize) -> Vec<Badge> {
        let path = std::path::Path::new(&self.entry(index).name);
        if path.extension().is_some() && helios_load::Kind::of(path) == helios_load::Kind::Raw {
            vec![Badge::new("RAW")]
        } else {
            Vec::new()
        }
    }

    fn revision(&self) -> u64 {
        self.revision
    }
}

/// The thumbnails received so far.
struct Thumbs<'a> {
    textures: &'a HashMap<String, TextureHandle>,
}

impl ThumbSource<String> for Thumbs<'_> {
    fn thumb(&self, key: &String, _size: u32) -> Thumb {
        match self.textures.get(key) {
            Some(t) => {
                let [w, h] = t.size();
                Thumb::Ready {
                    texture: t.id(),
                    size: [w as u32, h as u32],
                }
            }
            None => Thumb::Pending { average: None },
        }
    }
}

impl Library {
    /// The application whose steps the marks run through.
    pub fn set_app(&mut self, app: AppHandle) {
        self.app = Some(app);
    }

    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        let selected = self
            .selected
            .and_then(|i| self.entries.get(i))
            .map(|e| e.id.clone());
        self.entries = entries;
        self.reindex();
        self.selected = selected.and_then(|id| self.index.get(&id).copied());
        self.reorder();
    }

    fn reindex(&mut self) {
        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.clone(), i))
            .collect();
        let mut tags: Vec<String> = self
            .entries
            .iter()
            .flat_map(|e| e.tags.iter().cloned())
            .collect();
        tags.sort_unstable();
        tags.dedup();
        self.known_tags = tags;
    }

    /// The vault's stacks: each member of one whose top image the library
    /// holds shows in it, collapsed or expanded as the vault keeps it.
    pub fn set_stacks(&mut self, stacks: &[helios_vault::Stack]) {
        for e in &mut self.entries {
            e.stack = None;
        }
        for s in stacks {
            if !self.index.contains_key(&s.top) {
                continue;
            }
            let held: Vec<usize> = s
                .members
                .iter()
                .filter_map(|m| self.index.get(m).copied())
                .collect();
            for &i in &held {
                self.entries[i].stack = Some(StackOf {
                    id: s.id.clone(),
                    top: s.top.clone(),
                    count: held.len(),
                });
            }
            self.state.set_expanded(s.top.clone(), s.expanded);
        }
        self.reorder();
    }

    /// Whether an image passes the filter.
    fn passes(&self, e: &Entry) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        self.matches.as_ref().is_none_or(|m| m.contains(&e.id))
            && self.filter.matches(&Candidate {
                name: &e.name,
                marks: e.marks(),
                tags: &e.tags,
                date: e.day.as_deref(),
            })
    }

    /// What the views show, in order: a stack's top image first, its
    /// other members right after it.
    fn reorder(&mut self) {
        let stacks = self.filter.is_empty();
        let mut order = Vec::new();
        let mut placed: HashSet<&str> = HashSet::new();
        for (i, e) in self.entries.iter().enumerate() {
            if !self.passes(e) {
                continue;
            }
            match e.stack.as_ref().filter(|_| stacks) {
                None => order.push(i),
                Some(s) => {
                    if !placed.insert(s.id.as_str()) {
                        continue;
                    }
                    let members: Vec<usize> = self
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.stack.as_ref().is_some_and(|t| t.id == s.id))
                        .map(|(j, _)| j)
                        .collect();
                    order.extend(members.iter().filter(|&&j| self.entries[j].id == s.top));
                    order.extend(members.iter().filter(|&&j| self.entries[j].id != s.top));
                }
            }
        }
        self.order = order;
        self.revision += 1;
    }

    /// The ids the views show, in order (a collapsed stack's members
    /// included: the views fold them).
    pub fn shown(&self) -> Vec<String> {
        self.order
            .iter()
            .map(|&i| self.entries[i].id.clone())
            .collect()
    }

    /// Whether the views show the stack topped by `top` expanded.
    pub fn is_expanded(&self, top: &str) -> bool {
        self.state.is_expanded(&top.to_string())
    }

    /// The search field's filter in force.
    pub fn filter(&self) -> &Filter {
        &self.filter
    }

    /// Filters the library: its vault query waits for the host
    /// ([`Library::take_query`]).
    pub fn set_filter(&mut self, filter: Filter) {
        self.search.set_filter(filter.clone());
        self.filtered(filter);
    }

    fn filtered(&mut self, filter: Filter) {
        self.query = (!filter.is_empty()).then(|| query_of(&filter));
        self.filter = filter;
        self.matches = None;
        self.reorder();
    }

    /// The vault query the filter needs run, once.
    pub fn take_query(&mut self) -> Option<Query> {
        self.query.take()
    }

    /// The images the vault matched for the query last taken.
    pub fn set_matches(&mut self, ids: impl IntoIterator<Item = String>) {
        self.matches = Some(ids.into_iter().collect());
        self.reorder();
    }

    /// Where thumbnails are sent, by image id, from any thread.
    pub fn thumbnails(&self) -> Sender<(String, Image)> {
        self.sender.clone()
    }

    pub fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.changes)
    }

    /// Stacks the user expanded or collapsed since the last call:
    /// (stack id, expanded).
    pub fn take_expansions(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.expansions)
    }

    /// The image the user opened (Enter, a double click), once.
    pub fn take_activated(&mut self) -> Option<String> {
        self.activated.take()
    }

    /// The images selected in the views, in order.
    pub fn selection(&self) -> Vec<String> {
        self.state.selection().keys().to_vec()
    }

    /// Selects one image, by its place in `entries`.
    pub fn select(&mut self, index: usize) {
        if let Some(e) = self.entries.get(index) {
            let key = e.id.clone();
            self.state.set_selection([key.clone()], Some(key));
            self.state.reveal_cursor();
            self.selected = Some(index);
        }
    }

    /// Selects images by id, the cursor on the last.
    pub fn select_ids(&mut self, ids: &[String]) {
        let known: Vec<String> = ids
            .iter()
            .filter(|id| self.index.contains_key(*id))
            .cloned()
            .collect();
        let cursor = known.last().cloned();
        self.selected = cursor.as_ref().and_then(|c| self.index.get(c).copied());
        self.state.set_selection(known, cursor);
        self.state.reveal_cursor();
    }

    /// Moves the cursor by `step` among the images shown, within bounds.
    pub fn step(&mut self, step: i64) {
        if self.order.is_empty() {
            return;
        }
        let last = self.order.len() as i64 - 1;
        let at = self
            .selected
            .and_then(|i| self.order.iter().position(|&o| o == i))
            .map_or(0, |p| (p as i64 + step).clamp(0, last));
        let index = self.order[at as usize];
        self.select(index);
    }

    /// What a mark acts on: the selection, else the cursor's image.
    fn targets(&self) -> Result<Vec<String>, String> {
        let sel = self.state.selection();
        let keys: Vec<String> = if sel.is_empty() {
            sel.cursor().cloned().into_iter().collect()
        } else {
            sel.keys().to_vec()
        };
        let keys: Vec<String> = keys
            .into_iter()
            .filter(|k| self.index.contains_key(k))
            .collect();
        if keys.is_empty() {
            Err("no image selected".into())
        } else {
            Ok(keys)
        }
    }

    /// Shows a change on an image at once, and queues it.
    fn change_at(&mut self, i: usize, field: &str, op: Op) {
        let entry = &mut self.entries[i];
        match &op {
            Op::Set(v) => entry.show(field, v.as_ref()),
            Op::SetIfAbsent(_) => {}
            Op::Add(name) => {
                if field == TAGS {
                    if !entry.tags.contains(name) {
                        entry.tags.push(name.clone());
                        entry.tags.sort_unstable();
                    }
                } else if !entry.labels.contains(name) {
                    entry.labels.push(name.clone());
                    entry
                        .labels
                        .sort_by_key(|l| LABELS.iter().position(|k| k == l));
                }
            }
            Op::Remove(name) => {
                if field == TAGS {
                    entry.tags.retain(|t| t != name);
                } else {
                    entry.labels.retain(|l| l != name);
                }
            }
        }
        let change = Change {
            image: entry.id.clone(),
            field: field.into(),
            op,
        };
        // A later change replaces an earlier one of the same thing.
        self.changes.retain(|c| !change.replaces(c));
        self.changes.push(change);
    }

    fn at(&self, key: &str) -> Option<usize> {
        self.index.get(key).copied()
    }

    /// The places of the images named.
    fn places(&self, keys: &[String]) -> Vec<usize> {
        keys.iter().filter_map(|k| self.at(k)).collect()
    }

    /// Applies what the library's steps ran (a mark, a selection, a stack
    /// opened); whether marks changed.
    fn handle(&mut self, signals: Vec<Signal<String>>) -> bool {
        let mut marked = false;
        for s in signals {
            match s {
                Signal::Select { cursor, .. } => {
                    self.selected = cursor.and_then(|c| self.at(&c));
                }
                Signal::Rate { keys, rating } => {
                    for i in self.places(&keys) {
                        // As in darktable, a rating takes back a reject.
                        if self.entries[i].rejected {
                            self.change_at(i, REJECTED, Op::Set(None));
                        }
                        self.change_at(i, RATING, Op::Set(Some(Value::from(rating))));
                    }
                    marked = true;
                }
                Signal::Label { keys, label, on } => {
                    let name = label.name().to_string();
                    for i in self.places(&keys) {
                        let has = self.entries[i].labels.contains(&name);
                        if on != has {
                            let op = if on {
                                Op::Add(name.clone())
                            } else {
                                Op::Remove(name.clone())
                            };
                            self.change_at(i, LABELS_FIELD, op);
                        }
                    }
                    marked = true;
                }
                Signal::Flag { keys, flag } => {
                    for i in self.places(&keys) {
                        self.flag_at(i, flag);
                    }
                    marked = true;
                }
                Signal::Tag { keys, add, remove } => {
                    for i in self.places(&keys) {
                        for t in &remove {
                            self.change_at(i, TAGS, Op::Remove(t.clone()));
                        }
                        for t in &add {
                            self.change_at(i, TAGS, Op::Add(t.clone()));
                        }
                    }
                    self.reindex();
                    marked = true;
                }
                Signal::Stack { top, expanded } => {
                    if let Some(s) = self.at(&top).and_then(|i| self.entries[i].stack.clone()) {
                        self.expansions.push((s.id, expanded));
                    }
                }
                Signal::Activate { key } => self.activated = Some(key),
                Signal::Zoom { .. } => {}
                Signal::Vetoed { step, reason } => self.notes.push(format!("{step}: {reason}")),
            }
        }
        if marked && !self.filter.is_empty() {
            // The vault answers again; what no longer matches goes now.
            self.query = Some(query_of(&self.filter));
            self.reorder();
        }
        marked
    }

    /// A flag on one image: a pick and a reject exclude each other, the
    /// stars stay either way.
    fn flag_at(&mut self, i: usize, flag: Flag) {
        let (picked, rejected) = (self.entries[i].picked, self.entries[i].rejected);
        match flag {
            Flag::Pick => {
                if rejected {
                    self.change_at(i, REJECTED, Op::Set(None));
                }
                if !picked {
                    self.change_at(i, PICKED, Op::Set(Some(Value::from(true))));
                }
            }
            Flag::Reject => {
                if picked {
                    self.change_at(i, PICKED, Op::Set(None));
                }
                self.change_at(i, REJECTED, Op::Set(Some(Value::from(true))));
            }
            Flag::None => {
                if rejected {
                    self.change_at(i, REJECTED, Op::Set(None));
                }
                if picked {
                    self.change_at(i, PICKED, Op::Set(None));
                }
            }
        }
    }

    /// Runs a mark through the library's step and applies what ran;
    /// refused when a plugin vetoes it.
    fn mark(&mut self, mark: Mark<String>) -> Result<(), String> {
        let signal = change(self.app.as_ref(), LIBRARY, mark);
        if let Signal::Vetoed { step, reason } = &signal {
            return Err(format!("{step}: {reason}"));
        }
        self.handle(vec![signal]);
        Ok(())
    }

    /// A mark asked for by a command (a key): then, culling with
    /// auto-advance on, the cursor moves to the next image.
    fn command_mark(&mut self, mark: Mark<String>) -> Result<(), String> {
        self.mark(mark)?;
        if self.auto_advance {
            self.step(1);
        }
        Ok(())
    }

    /// Rates the selected images, 0 to 5 stars; a rejected image is taken
    /// back from the reject, as in darktable.
    pub fn rate(&mut self, rating: i64) -> Result<(), String> {
        if !(0..=5).contains(&rating) {
            return Err(format!("a rating is 0 to 5 stars, not {rating}"));
        }
        let keys = self.targets()?;
        self.command_mark(Mark::Rate {
            keys,
            rating: rating as u8,
        })
    }

    /// Flags the selected images: picked, rejected or neither.
    pub fn flag(&mut self, flag: Flag) -> Result<(), String> {
        let keys = self.targets()?;
        self.command_mark(Mark::Flag { keys, flag })
    }

    /// Rejects the selected images, or takes the reject back when they
    /// all are; the stars are kept either way.
    pub fn toggle_reject(&mut self) -> Result<(), String> {
        let keys = self.targets()?;
        let all = self
            .places(&keys)
            .into_iter()
            .all(|i| self.entries[i].rejected);
        let flag = if all { Flag::None } else { Flag::Reject };
        self.command_mark(Mark::Flag { keys, flag })
    }

    /// Puts a colour label on the selected images, or takes it off (as
    /// the cursor's image has it): one label added or removed, whatever
    /// else the set holds in the vault.
    pub fn toggle_label(&mut self, label: &str) -> Result<(), String> {
        let parsed = ColorLabel::parse(label)
            .filter(|_| LABELS.contains(&label))
            .ok_or_else(|| format!("`{label}` is not one of {}", LABELS.join(", ")))?;
        let keys = self.targets()?;
        let first = self.selected.or_else(|| self.at(&keys[0]));
        let on = !first
            .and_then(|i| self.entries.get(i))
            .is_some_and(|e| e.labels.iter().any(|l| l == label));
        self.command_mark(Mark::Label {
            keys,
            label: parsed,
            on,
        })
    }

    /// Adds tags to the selected images and takes others off.
    pub fn tag(&mut self, add: &[String], remove: &[String]) -> Result<(), String> {
        let keys = self.targets()?;
        self.mark(Mark::Tag {
            keys,
            add: add.to_vec(),
            remove: remove.to_vec(),
        })
    }

    /// Expands or collapses the stack under the cursor.
    pub fn toggle_stack(&mut self) -> Result<(), String> {
        let i = self.selected.ok_or("no image selected")?;
        let s = self.entries[i]
            .stack
            .clone()
            .ok_or("the image is in no stack")?;
        let expanded = !self.state.is_expanded(&s.top);
        self.state.set_expanded(s.top.clone(), expanded);
        self.expansions.push((s.id, expanded));
        Ok(())
    }

    /// Shows a field as the vault holds it, for an image of the library:
    /// after another writer changed it, or after a write failed.
    pub fn show_field(&mut self, image: &str, field: &str, value: Option<&Value>) {
        if let Some(i) = self.at(image) {
            self.entries[i].show(field, value);
            if field == TAGS {
                self.reindex();
            }
        }
    }

    fn receive(&mut self, ui: &Ui) {
        let mut any = false;
        while let Ok((id, image)) = self.receiver.try_recv() {
            let texture = ui.ctx().load_texture(
                format!("helios.library.{id}"),
                thumbnail(&image),
                egui::TextureOptions::LINEAR,
            );
            self.textures.insert(id, texture);
            any = true;
        }
        if any {
            ui.ctx().request_repaint();
        }
    }

    /// The search field, auto-advance, the count.
    fn toolbar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let search = crate::i18n::text(&ctx, Message::new("gui-library-search", "Search images"));
        let advance = crate::i18n::text(
            &ctx,
            Message::new("gui-library-auto-advance", "Auto-advance"),
        );
        let count = crate::i18n::text(
            &ctx,
            Message::new("gui-library-count", "{ $shown } of { $total }")
                .with("shown", self.order.len())
                .with("total", self.entries.len()),
        );
        let mut asked = None;
        ui.horizontal(|ui| {
            let mut field = SearchField::new(LIBRARY).label(search).width(220.0);
            if let Some(app) = &self.app {
                field = field.app(app);
            }
            asked = field.show(ui, &mut self.search).changed;
            ui.checkbox(&mut self.auto_advance, advance);
            ui.weak(count);
        });
        if let Some(filter) = asked {
            self.filtered(filter);
        }
    }

    /// The marks of the cursor's image, edited for the selection: stars,
    /// labels, flag and tags.
    fn marks_bar(&mut self, ui: &mut Ui) {
        let Some(entry) = self.selected.and_then(|i| self.entries.get(i)).cloned() else {
            ui.weak(crate::i18n::text(
                ui.ctx(),
                Message::new("gui-library-no-cursor", "No image under the cursor"),
            ));
            return;
        };
        let Ok(keys) = self.targets() else {
            return;
        };
        let marks = entry.marks();
        let mut asked = Vec::new();
        ui.horizontal(|ui| {
            ui.label(&entry.name);
            if let Some(rating) = RatingStars::new(marks.rating).show(ui).changed {
                asked.push(Mark::Rate {
                    keys: keys.clone(),
                    rating,
                });
            }
            if let Some(labels) = ColorLabels::new(marks.labels).show(ui).changed {
                for label in ColorLabel::ALL {
                    if labels.has(label) != marks.labels.has(label) {
                        asked.push(Mark::Label {
                            keys: keys.clone(),
                            label,
                            on: labels.has(label),
                        });
                    }
                }
            }
            if let Some(flag) = FlagToggle::new(marks.flag).show(ui).changed {
                asked.push(Mark::Flag {
                    keys: keys.clone(),
                    flag,
                });
            }
        });
        let label = crate::i18n::text(ui.ctx(), Message::new("gui-library-tags", "Tags"));
        let out = TagInput::new("helios.library.tags")
            .label(label)
            .source(&self.known_tags)
            .show(ui, &mut self.tags, &entry.tags);
        if !out.added.is_empty() || !out.removed.is_empty() {
            asked.push(Mark::Tag {
                keys,
                add: out.added,
                remove: out.removed,
            });
        }
        for m in asked {
            if let Err(e) = self.mark(m) {
                self.notes.push(e);
            }
        }
    }

    /// The Vault panel: the search field, the cursor's marks, and the
    /// thumbnail grid with the culling keys.
    pub fn grid(&mut self, ui: &mut Ui) {
        self.receive(ui);
        if self.entries.is_empty() {
            ui.weak(crate::i18n::text(
                ui.ctx(),
                Message::new(
                    "gui-library-empty",
                    "No images: import a folder into the vault",
                ),
            ));
            return;
        }
        self.toolbar(ui);
        self.marks_bar(ui);
        let label = crate::i18n::text(ui.ctx(), Message::new("gui-library-images", "Images"));
        let keymap = CullKeymap::lightroom().auto_advance(self.auto_advance);
        let out = {
            let items = Items {
                entries: &self.entries,
                order: &self.order,
                stacks: self.filter.is_empty(),
                revision: self.revision,
            };
            let thumbs = Thumbs {
                textures: &self.textures,
            };
            let mut grid = ThumbGrid::new("grid").label(label).keymap(keymap);
            if let Some(app) = &self.app {
                grid = grid.app(app);
            }
            grid.show(ui, &mut self.state, &items, &thumbs)
        };
        crate::name_rect(ui.ctx(), "helios.library.grid", out.response.rect);
        self.handle(out.signals);
    }

    /// The Filmstrip panel: one row, the grid's selection.
    pub fn filmstrip(&mut self, ui: &mut Ui) {
        self.receive(ui);
        if self.entries.is_empty() {
            ui.weak(crate::i18n::text(
                ui.ctx(),
                Message::new("gui-library-none", "No images"),
            ));
            return;
        }
        let keymap = CullKeymap::lightroom().auto_advance(self.auto_advance);
        let height = ui.available_height().clamp(64.0, 200.0);
        let out = {
            let items = Items {
                entries: &self.entries,
                order: &self.order,
                stacks: self.filter.is_empty(),
                revision: self.revision,
            };
            let thumbs = Thumbs {
                textures: &self.textures,
            };
            let mut strip = ThumbGrid::filmstrip("strip").keymap(keymap).height(height);
            if let Some(app) = &self.app {
                strip = strip.app(app);
            }
            strip.show(ui, &mut self.state, &items, &thumbs)
        };
        crate::name_rect(ui.ctx(), "helios.library.strip", out.response.rect);
        self.handle(out.signals);
    }

    /// The Compare panel: the two to four images selected side by side,
    /// zoom and pan synced, one marked best; keeping the best picks it
    /// and rejects the others.
    pub fn compare_ui(&mut self, ui: &mut Ui) {
        self.receive(ui);
        let keys = self.selection();
        if !(2..=4).contains(&keys.len()) {
            ui.weak(crate::i18n::text(
                ui.ctx(),
                Message::new(
                    "gui-library-compare-hint",
                    "Select two to four images to compare them",
                ),
            ));
            return;
        }
        let ctx = ui.ctx().clone();
        let sync = crate::i18n::text(&ctx, Message::new("gui-library-sync", "Sync zoom and pan"));
        let keep_text = crate::i18n::text(
            &ctx,
            Message::new(
                "gui-library-keep-best",
                "Keep the best: pick it, reject the others",
            ),
        );
        let best = self.best.clone().filter(|b| keys.contains(b));
        let mut keep = false;
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.synced, sync);
            keep = ui
                .add_enabled(best.is_some(), egui::Button::new(keep_text))
                .clicked();
        });
        let panes: Vec<Pane<String>> = keys
            .iter()
            .filter_map(|k| {
                let i = self.at(k)?;
                Some(Pane::new(k.clone(), self.entries[i].name.clone()))
            })
            .collect();
        let out = {
            let thumbs = Thumbs {
                textures: &self.textures,
            };
            CompareView::new("helios.compare")
                .synced(self.synced)
                .best(best.clone())
                .show(ui, &mut self.compare, &panes, &thumbs)
        };
        crate::name_rect(ui.ctx(), "helios.library.compare", out.response.rect);
        if let Some(k) = out.best {
            self.best = Some(k);
        }
        if keep && let Some(best) = best {
            self.keep_best(&best, &keys);
        }
    }

    /// The image marked best in the comparison.
    pub fn best(&self) -> Option<&str> {
        self.best.as_deref()
    }

    /// Marks an image best in the comparison.
    pub fn set_best(&mut self, id: Option<String>) {
        self.best = id;
    }

    /// Keeps the best of `among`: picks it and rejects the others.
    pub fn keep_best(&mut self, best: &str, among: &[String]) {
        let others: Vec<String> = among.iter().filter(|k| *k != best).cloned().collect();
        let mut marks = vec![Mark::Flag {
            keys: vec![best.to_string()],
            flag: Flag::Pick,
        }];
        if !others.is_empty() {
            marks.push(Mark::Flag {
                keys: others,
                flag: Flag::Reject,
            });
        }
        for m in marks {
            if let Err(e) = self.mark(m) {
                self.notes.push(e);
            }
        }
    }
}
