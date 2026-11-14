//! The library: the vault's images in egui-pop's photo library widgets (the
//! Library panel's thumbnail grid, the Filmstrip, the Compare panel), with
//! their rating, flag (picked or rejected), colour labels and tags.
//!
//! The shell does not hold the vault: the host gives it the images
//! ([`Shell::set_library`](crate::Shell::set_library)) and their stacks
//! ([`Library::set_stacks`]), sends their thumbnails as they are read (from
//! any thread, through [`Library::thumbnails`]: the embedded previews of
//! raws, each burst of them waking an idle window once), and applies what the user changes, collected with
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
//! tags, and dates as capture days; [`query_of`]), run by the host
//! ([`Library::take_query`], [`Library::set_matches`]); its words are
//! matched here, on the names and tags the library holds.
//!
//! Each photo's backups (`helios_storage::strategy::statuses`, read once
//! per refresh: [`Library::set_backups`]) show as a badge when the library
//! has a strategy ([`BackupMark`]: backed up, short, unverified, no
//! backup), and the search field takes `backup:met`, `backup:short`
//! (short or unverified), `backup:unverified` and `backup:none`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SendError, Sender, channel};
use std::sync::{Arc, OnceLock};

use egui::{Color32, ColorImage, TextureHandle, Ui};
use egui_pop::api::AppHandle;

use crate::thumb_textures::Resident;
use egui_pop::views::library::compare::{CompareView, Pane};

use crate::comparison::Comparison;
use egui_pop::views::library::filter::{Filter, SearchField, SearchState};
use egui_pop::views::library::marks::{ColorLabels, FlagToggle, RatingStars};
use egui_pop::views::library::tags::{TagInput, TagInputState};
use egui_pop::views::library::{
    Badge, Change as Mark, ColorLabel, CullAction, CullKeymap, Flag, LibraryItems, LibraryState,
    Marks, Signal, Stack, Thumb, ThumbGrid, ThumbSource, Tone as BadgeTone, change,
};
use helios_core::{Image, Message, Region};

use crate::head_render::HeadRender;
use helios_vault::capture::CAPTURED;
use helios_vault::cull;
pub use helios_vault::fields::Op;
use helios_vault::fields::{
    LABELS as LABELS_FIELD, PICKED, Query, RATING, REJECTED, ROTATION, TAGS, TITLE,
};
use serde_json::Value;

mod batch;
mod versions;
pub use batch::{
    CopiedMarks, DELETE_REJECTED_COMMAND, MARK_KINDS, MARKS_COPY_COMMAND, MARKS_PASTE_COMMAND,
    PasteMode, REMOVE_COMMAND, RESET_ROTATION_COMMAND, ROTATE_LEFT_COMMAND, ROTATE_RIGHT_COMMAND,
    SELECT_INVERT_COMMAND, SELECT_NONE_COMMAND, SELECT_SIMILAR_COMMAND, Similar, TRASH_COMMAND,
    TrashAsk, Turn,
};
pub use batch::{DUPLICATE_COMMAND, DUPLICATE_ORIGINAL_COMMAND};
pub use versions::{REMOVE_VERSION_COMMAND, VersionAsk, VersionHead};

/// The library's name in egui-pop's steps and commands (`library` in
/// their arguments).
pub const LIBRARY: &str = "helios.library";

/// What the search field's box holds beside its text (its icon, its "+"
/// and their spacing), so the bar can give the text what is left.
const SEARCH_BESIDE: f32 = 72.0;

/// The labels, in the order darktable gives them F1 to F5.
/// The keys the grid and the filmstrip read themselves: the thumbnails'
/// size only. The culling keys are the keymap's alone (`crate::keys`),
/// scoped by their `when`, so a key the user takes away stays away.
fn grid_keys() -> CullKeymap {
    let mut keys = CullKeymap::lightroom();
    keys.bindings
        .retain(|(_, a)| matches!(a, CullAction::ZoomIn | CullAction::ZoomOut));
    keys
}

pub const LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

/// How long a frame may spend reading the thumbnail cache for the cells
/// it drew without a texture: about a frame at 60 Hz. A 256 px entry
/// decodes in about a millisecond, so a screen of cells is read in one or
/// two frames.
pub const WARM_FRAME_BUDGET: std::time::Duration = std::time::Duration::from_millis(12);

/// Reads `ids` from `cache` on a few threads (never waiting for its
/// folder's listing), each taking the next id until `budget` is spent:
/// the thumbnails found, the ids the cache has no current entry for (for
/// the pool), and those not reached, in order.
pub fn read_cached(
    cache: &helios_thumbs::ThumbCache,
    ids: Vec<String>,
    budget: std::time::Duration,
) -> (Vec<(String, helios_thumbs::Rgb8)>, Vec<String>, Vec<String>) {
    use std::sync::atomic::AtomicUsize;
    if !cache.listed() {
        // Its listing still runs on the pool's threads: the pool reads them.
        return (Vec::new(), ids, Vec::new());
    }
    let deadline = std::time::Instant::now() + budget;
    let next = AtomicUsize::new(0);
    let threads = helios_thumbs::workers().min(ids.len());
    let mut results: Vec<Option<Option<helios_thumbs::Rgb8>>> = Vec::new();
    std::thread::scope(|scope| {
        let work = || {
            let mut got = Vec::new();
            loop {
                if std::time::Instant::now() >= deadline {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(id) = ids.get(i) else { break };
                got.push((
                    i,
                    cache.try_get(&helios_thumbs::key_of(id), helios_thumbs::SMALL),
                ));
            }
            got
        };
        let helpers: Vec<_> = (1..threads).map(|_| scope.spawn(work)).collect();
        let mut all = work();
        for h in helpers {
            all.extend(h.join().unwrap_or_default());
        }
        results = (0..ids.len()).map(|_| None).collect();
        for (i, r) in all {
            results[i] = Some(r);
        }
    });
    let (mut found, mut missed, mut left) = (Vec::new(), Vec::new(), Vec::new());
    for (id, r) in ids.into_iter().zip(results) {
        match r {
            Some(Some(rgb)) => found.push((id, rgb)),
            Some(None) => missed.push(id),
            None => left.push(id),
        }
    }
    (found, missed, left)
}

/// Where thumbnails are sent, by image id, from any thread: the first one
/// after a frame wakes the window, so previews read while it is idle show
/// without waiting for an input, and a burst of them costs one repaint.
#[derive(Clone)]
pub struct ThumbnailSink {
    sender: Sender<(String, Arrival)>,
    wake: Arc<Wake>,
}

/// What is told the thumbnails wanted (see [`Library::set_on_view`]).
type OnView = dyn Fn(&[String]);

/// What is asked an image's large thumbnail (see [`Library::set_on_large`]).
type OnLarge = dyn Fn(&str);

/// A thumbnail on its way to the window.
enum Arrival {
    /// In the working space, no larger than [`THUMB`] (see [`small`]).
    Linear(Image),
    /// 8-bit sRGB, as the thumbnail cache holds it.
    Srgb(helios_thumbs::Rgb8),
    /// The cache's large entry (Compare's first paint), 8-bit sRGB.
    Large(helios_thumbs::Rgb8),
    /// The file has no readable preview (or no copy at hand and nothing
    /// cached): its cell shows the placeholder.
    NoPreview,
}

/// What a sink needs to wake the window.
#[derive(Default)]
struct Wake {
    /// The window's context, known once the library is first drawn: until
    /// then, its first frame takes whatever was sent.
    ctx: OnceLock<egui::Context>,
    /// A thumbnail was sent since the window last took them: a repaint is
    /// already on its way.
    pending: AtomicBool,
}

impl ThumbnailSink {
    /// Sends a thumbnail; an error once the library is gone.
    ///
    /// # Errors
    /// The thumbnail back when the library no longer exists.
    pub fn send(&self, thumb: (String, Image)) -> Result<(), SendError<(String, Image)>> {
        let (id, image) = thumb;
        self.arrive(id, Arrival::Linear(image))
            .map_err(|SendError((id, a))| match a {
                Arrival::Linear(image) => SendError((id, image)),
                _ => unreachable!("sent as an image"),
            })
    }

    /// Sends an 8-bit sRGB thumbnail (from the thumbnail cache or its
    /// fast read); `false` once the library is gone.
    pub fn send_srgb(&self, id: String, thumb: helios_thumbs::Rgb8) -> bool {
        self.arrive(id, Arrival::Srgb(thumb)).is_ok()
    }

    /// Sends an image's large thumbnail (the cache's 1024 px entry):
    /// Compare draws it until the render of its head version lands;
    /// `false` once the library is gone.
    pub fn send_large(&self, id: String, thumb: helios_thumbs::Rgb8) -> bool {
        self.arrive(id, Arrival::Large(thumb)).is_ok()
    }

    /// Says an image has no thumbnail to show: its cell shows the
    /// placeholder (its file's type, "no preview"); `false` once the
    /// library is gone.
    pub fn send_no_preview(&self, id: String) -> bool {
        self.arrive(id, Arrival::NoPreview).is_ok()
    }

    fn arrive(&self, id: String, arrival: Arrival) -> Result<(), SendError<(String, Arrival)>> {
        self.sender.send((id, arrival))?;
        if !self.wake.pending.swap(true, Ordering::SeqCst)
            && let Some(ctx) = self.wake.ctx.get()
        {
            ctx.request_repaint();
        }
        Ok(())
    }
}

/// Where renders of images' head versions are sent, from any thread
/// (see [`crate::head_render`]): each wakes the window as a thumbnail
/// does.
#[derive(Clone)]
pub struct HeadSink {
    sender: Sender<HeadRender>,
    wake: Arc<Wake>,
}

impl HeadSink {
    /// Sends a render; ignored once the library is gone.
    pub fn send(&self, render: HeadRender) {
        if self.sender.send(render).is_ok()
            && !self.wake.pending.swap(true, Ordering::SeqCst)
            && let Some(ctx) = self.wake.ctx.get()
        {
            ctx.request_repaint();
        }
    }
}

/// A render of an image's head version, as a texture.
struct HeadTexture {
    /// The version it shows (hex); none for an image never edited.
    version: Option<String>,
    /// The longest side it was asked at.
    size: u32,
    texture: TextureHandle,
    /// The edit or the network changed since: rendered again, drawn
    /// until the new one lands.
    stale: bool,
}

/// The renders of images' head versions the Compare panel draws, in
/// place of a raw's embedded preview once each lands.
struct Heads {
    renders: HashMap<String, HeadTexture>,
    /// Renders asked and not back yet, by image: the generation asked at.
    pending: HashMap<String, u64>,
    /// Each image's generation, raised when its edit changes: a render
    /// asked before is stale when it lands.
    generations: HashMap<String, u64>,
    /// Images whose render failed, not asked again until their edit or
    /// the network changes.
    failed: HashSet<String>,
    /// What the host is to render: (image, longest side).
    wanted: Vec<(String, u32)>,
    sender: Sender<HeadRender>,
    receiver: Receiver<HeadRender>,
}

impl Default for Heads {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Heads {
            renders: HashMap::new(),
            pending: HashMap::new(),
            generations: HashMap::new(),
            failed: HashSet::new(),
            wanted: Vec::new(),
            sender,
            receiver,
        }
    }
}

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
    /// Its title, when it has one.
    pub title: Option<String>,
    /// The file's size in bytes, when it is on disk.
    pub bytes: Option<u64>,
    /// When the file was last modified, in seconds since 1970 (UTC); shown
    /// in the system's time zone.
    pub modified: Option<i64>,
    /// When it was taken: the camera's clock read as UTC, in seconds (the
    /// vault's `captured` field, from the file's EXIF, else its time).
    pub captured: Option<f64>,
    /// Its capture day (`YYYY-MM-DD`, by the camera's clock), for the
    /// search field's dates; none until its capture is read.
    pub day: Option<String>,
    pub stack: Option<StackOf>,
    /// Whether a copy of its file is at hand: present, offline (on a
    /// volume not plugged in) or missing: an offline or missing photo
    /// shows a badge in the grid.
    pub presence: helios_vault::Presence,
    /// The volume an offline copy is on (its id), to say which one to
    /// plug in.
    pub offline_on: Option<String>,
    /// When its file was found changed by another application (seconds
    /// since 1970), until the user acknowledges it: a badge in the grid.
    pub changed_outside: Option<u64>,
    /// Quarter turns clockwise the user gave it (the vault's `rotation`
    /// field), on top of its file's orientation: its pictures are shown
    /// turned.
    pub rotation: u8,
    /// Its culling focus (the vault's `focus` field), once measured.
    pub focus: Option<f64>,
    /// The largest share of one channel blown white, once measured.
    pub clip_white: Option<f64>,
    /// The largest share of one channel crushed black, once measured.
    pub clip_black: Option<f64>,
}

/// A volume as the library names it: its name, and when and on which
/// node it was last seen (seconds since the epoch).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VolumeNote {
    pub name: String,
    pub last_seen: Option<(u64, String)>,
    /// Mounted here, now.
    pub online: bool,
    /// Where it is mounted, while it is.
    pub mount: Option<PathBuf>,
}

impl VolumeNote {
    /// Its last-seen day and node, as the messages say them.
    fn seen(&self) -> (String, String) {
        self.last_seen.as_ref().map_or_else(
            || ("?".to_string(), "?".to_string()),
            |(at, node)| {
                (
                    i64::try_from(*at)
                        .ok()
                        .and_then(day_text)
                        .unwrap_or_else(|| "?".into()),
                    node.clone(),
                )
            },
        )
    }
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
            TITLE => {
                self.title = value
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .map(String::from);
            }
            CAPTURED => {
                self.captured = value.and_then(Value::as_f64);
                self.day = self.captured.map(helios_vault::capture::day_text);
            }
            ROTATION => self.rotation = helios_vault::rotation::quarter_turns(value),
            cull::FOCUS => self.focus = value.and_then(Value::as_f64),
            cull::CLIP_WHITE => self.clip_white = value.and_then(Value::as_f64),
            cull::CLIP_BLACK => self.clip_black = value.and_then(Value::as_f64),
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

/// The vault query a search filter makes: all of it runs in the vault
/// (`docs/design/library.md`, L4). Its rating, labels, flags, tags and
/// dates (a date being the capture day by the camera's clock:
/// `date:2024-06-01..2024-06-30` takes both days whole), and in its text
/// Helios's own keys ([`helios_terms`]): `camera:`, `lens:`, `type:`,
/// `folder:`, `collection:`, `presence:`, `cull:` and `backup:`; the other words are looked for
/// in the photo's name and tags. A `backup:` term is measured against no
/// volume until the library sets its rule ([`Library::set_backup_rule`]).
pub fn query_of(filter: &Filter) -> Query {
    use helios_vault::capture::day_start;
    let terms = helios_terms(&filter.text);
    Query {
        min_rating: filter.min_rating.map(i64::from),
        labels: filter.labels.iter().map(|l| l.name().to_string()).collect(),
        flags: filter.flags.iter().map(|f| f.name().to_string()).collect(),
        tags: filter.tags.clone(),
        captured_from: filter.from.as_deref().and_then(day_start),
        captured_before: filter.to.as_deref().and_then(day_start).map(|d| d + 86_400),
        words: terms.words,
        cameras: terms.cameras,
        lenses: terms.lenses,
        file_types: terms.file_types,
        folder: terms.folder,
        album: terms.collection,
        presence: terms.presence,
        backup: terms.backup.map(|states| helios_vault::BackupFilter {
            rule: helios_vault::Backups::default(),
            states,
        }),
        cull: terms.cull,
        ..Query::default()
    }
}

/// The terms of a filter's text: Helios's own keys, and the words left.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Terms {
    pub words: Vec<String>,
    pub cameras: Vec<String>,
    pub lenses: Vec<String>,
    pub file_types: Vec<String>,
    pub folder: Option<String>,
    /// The collection the photos are in (its id): an album for now.
    pub collection: Option<String>,
    pub presence: Vec<helios_vault::Presence>,
    /// The backup states every `backup:` term allows (`None`: no term).
    pub backup: Option<Vec<helios_vault::BackupState>>,
    /// The culling states (`cull:sharp`, `soft`, `clipped`, `unmeasured`),
    /// any of them.
    pub cull: Vec<String>,
}

/// The backup states a `backup:` word takes: `met` (or `ok`), `short`
/// (short, unverified, or no copy at all), `unverified`, `none`.
fn backup_states(word: &str) -> Option<Vec<helios_vault::BackupState>> {
    use helios_vault::BackupState as B;
    Some(match word {
        "met" | "ok" => vec![B::Met],
        "short" => vec![B::Short, B::Unverified, B::None],
        "unverified" => vec![B::Unverified],
        "none" => vec![B::None],
        _ => return None,
    })
}

/// A filter's text cut into words, a double-quoted value kept whole
/// (`camera:"Z 8"`, `folder:"/photos/2024 trip"`).
fn tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut open = false;
    for w in text.split_whitespace() {
        if open && let Some(last) = out.last_mut() {
            last.push(' ');
            last.push_str(w);
        } else {
            out.push(w.to_string());
        }
        if w.matches('"').count() % 2 == 1 {
            open = !open;
        }
    }
    out
}

/// A term's value without its quotes.
fn unquoted(value: &str) -> String {
    value.trim_matches('"').to_string()
}

/// A value as a term writes it: quoted when it holds a space.
pub fn quoted(value: &str) -> String {
    if value.contains(char::is_whitespace) {
        format!("\"{value}\"")
    } else {
        value.to_string()
    }
}

/// Helios's own keys in a filter's text (the words egui-pop's grammar
/// leaves): `camera:<model>`, `lens:<lens>`, `type:<cr3>` (each exact;
/// several of one key take any of them), `folder:<path>` (a copy there
/// or below), `collection:<id>` (in that album), `presence:present|offline|missing`,
/// `cull:sharp|soft|clipped|unmeasured` (several: any of them) and
/// `backup:<state>` (several: every one holds). A key with a value it does not know stays
/// a word, as before.
pub fn helios_terms(text: &str) -> Terms {
    let mut t = Terms::default();
    for token in tokens(text) {
        let lower = token.to_lowercase();
        let value = |key: &str| {
            lower
                .starts_with(key)
                .then(|| unquoted(&token[key.len()..]))
                .filter(|v| !v.is_empty())
        };
        if let Some(v) = value("camera:") {
            t.cameras.push(v);
        } else if let Some(v) = value("lens:") {
            t.lenses.push(v);
        } else if let Some(v) = value("type:") {
            t.file_types.push(v.to_lowercase());
        } else if let Some(v) = value("folder:") {
            t.folder = Some(v);
        } else if let Some(v) = value("collection:") {
            t.collection = Some(v);
        } else if let Some(p) = value("presence:").and_then(|v| match v.to_lowercase().as_str() {
            "present" => Some(helios_vault::Presence::Present),
            "offline" => Some(helios_vault::Presence::Offline),
            "missing" => Some(helios_vault::Presence::Missing),
            _ => None,
        }) {
            t.presence.push(p);
        } else if let Some(state) = lower
            .strip_prefix("cull:")
            .filter(|v| cull::STATES.contains(v))
        {
            t.cull.push(state.to_string());
        } else if let Some(states) = lower.strip_prefix("backup:").and_then(backup_states) {
            t.backup = Some(match t.backup.take() {
                None => states,
                Some(before) => before.into_iter().filter(|s| states.contains(s)).collect(),
            });
        } else {
            t.words.push(token);
        }
    }
    t
}

/// The search filter a workspace's library query makes (see
/// `helios_template::QueryDesc`): the library opens on it.
pub fn filter_of(query: &helios_template::QueryDesc) -> Filter {
    Filter {
        min_rating: query.min_rating,
        labels: query
            .labels
            .iter()
            .filter_map(|l| ColorLabel::parse(l))
            .collect(),
        flags: query.flags.iter().filter_map(|f| Flag::parse(f)).collect(),
        tags: query.tags.clone(),
        ..Filter::default()
    }
}

/// A library query typed as in the search field (`flag:pick rating:3`):
/// its words and dates are refused, a workspace keeps criteria only;
/// nothing typed is no query.
pub fn query_typed(text: &str) -> Result<Option<helios_template::QueryDesc>, String> {
    let f = Filter::parse(text);
    if !f.text.is_empty() || f.from.is_some() || f.to.is_some() {
        return Err(format!(
            "the library's query keeps a rating, labels, flags and tags only, not `{text}`"
        ));
    }
    let q = helios_template::QueryDesc {
        min_rating: f.min_rating,
        labels: f.labels.iter().map(|l| l.name().to_string()).collect(),
        flags: f.flags.iter().map(|f| f.name().to_string()).collect(),
        tags: f.tags,
    };
    Ok((!q.is_empty()).then_some(q))
}

/// A culling state's name in the facets bar.
fn cull_value(state: &str) -> Message {
    match state {
        cull::SHARP => Message::new("gui-library-cull-sharp", "sharp"),
        cull::SOFT => Message::new("gui-library-cull-soft", "soft"),
        cull::CLIPPED => Message::new("gui-library-cull-clipped", "clipped"),
        _ => Message::new("gui-library-cull-unmeasured", "not measured"),
    }
}

/// The filter term taking a facet's value (`label:red`, `camera:"Z 8"`).
pub fn facet_term(facet: &str, value: &str) -> String {
    match facet {
        "rating" => format!("rating>={value}"),
        "file_type" => format!("type:{}", quoted(value)),
        other => format!("{other}:{}", quoted(value)),
    }
}

/// The folders tree's counts, as far as they are read.
#[derive(Clone, Debug, Default)]
enum FolderCounts {
    /// Not read since the photos last changed.
    #[default]
    Unread,
    /// The Folders section is open: the host is to count them.
    Wanted,
    /// The host took the ask ([`Library::take_folders_wanted`]).
    Asked,
    Read(helios_vault::VolumeFolders),
}

/// A volume of the folders tree (`docs/design/library.md`, L5): a disk or
/// card the photos' copies are on, its folders below it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeNode {
    /// Its id; `None` for the copies on no known volume.
    pub id: Option<String>,
    /// Its name (the user's, else its label); the id of one the registry
    /// does not know; empty for the copies on no known volume.
    pub name: String,
    /// The photos with a copy on it.
    pub count: u64,
    /// Mounted here, now: an offline volume's folders still filter the
    /// grid (their thumbnails are cached), its row greyed.
    pub online: bool,
    /// When it was last seen (seconds since the epoch), offline.
    pub last_seen: Option<u64>,
    /// Where it is mounted, while it is: its row's menu acts there.
    pub mount: Option<PathBuf>,
    pub folders: Vec<FolderNode>,
}

/// The volumes of the folders tree, each with its folders
/// ([`folder_tree`]): online ones first, then offline ones, by name; the
/// copies on no known volume last.
fn volume_tree(
    counts: &helios_vault::VolumeFolders,
    volumes: &HashMap<String, VolumeNote>,
) -> Vec<VolumeNode> {
    let mut out: Vec<VolumeNode> = counts
        .iter()
        .map(|(id, folders)| {
            let id = id.as_ref().map(|v| v.as_str().to_string());
            let note = id.as_ref().and_then(|id| volumes.get(id));
            VolumeNode {
                name: note.map_or_else(|| id.clone().unwrap_or_default(), |n| n.name.clone()),
                // The volume's root folder counts every photo on it.
                count: folders.values().copied().max().unwrap_or(0),
                // Only the registry says a volume is offline: copies on
                // none, or on one it does not know, are not greyed.
                online: note.is_none_or(|n| n.online),
                last_seen: note.and_then(|n| n.last_seen.as_ref().map(|(at, _)| *at)),
                mount: note.and_then(|n| n.mount.clone()),
                folders: folder_tree(folders),
                id,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (a.id.is_none(), !a.online, &a.name).cmp(&(b.id.is_none(), !b.online, &b.name))
    });
    out
}

/// A folder of the folders tree (`docs/design/library.md`, L5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderNode {
    /// Its absolute path, as a `folder:` term takes it.
    pub path: String,
    /// What the tree shows: its name, or the folders from the one above
    /// it shown (a chain of folders holding nothing but the next one is
    /// one row: `home/me/Photos`).
    pub name: String,
    /// The photos with a copy here or below.
    pub count: u64,
    pub children: Vec<FolderNode>,
}

/// The folders tree of the vault's counts ([`helios_vault::Vault::folders`]):
/// the top folders (none above them counted), each with the folders right
/// below it, by name. A folder whose only content is one folder below
/// (the same photos, nothing beside) is shown in one row with it.
pub fn folder_tree(counts: &std::collections::BTreeMap<String, u64>) -> Vec<FolderNode> {
    let mut below: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut tops = Vec::new();
    for path in counts.keys() {
        let parent = Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy())
            .filter(|p| counts.contains_key(p.as_ref()));
        match parent {
            Some(parent) => {
                let (key, _) = counts.get_key_value(parent.as_ref()).expect("counted");
                below.entry(key.as_str()).or_default().push(path.as_str());
            }
            None => tops.push(path.as_str()),
        }
    }
    let mut out: Vec<FolderNode> = tops
        .into_iter()
        .map(|top| {
            // A top row names its whole path.
            let mut n = folder_node(top, Some(top), counts, &below);
            n.name = n.path.clone();
            n
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A folder of [`folder_tree`] and those below it; `shown_from` is the
/// folder its row starts at when a chain of folders is one row.
fn folder_node(
    path: &str,
    shown_from: Option<&str>,
    counts: &std::collections::BTreeMap<String, u64>,
    below: &HashMap<&str, Vec<&str>>,
) -> FolderNode {
    let count = counts.get(path).copied().unwrap_or(0);
    let kids = below.get(path).map(Vec::as_slice).unwrap_or_default();
    // Nothing here but the one folder below: one row.
    if let [only] = kids
        && counts.get(*only).copied() == Some(count)
    {
        return folder_node(only, shown_from.or(Some(path)), counts, below);
    }
    let name = match shown_from {
        Some(from) => {
            let start = Path::new(from).parent().map(Path::new);
            let rel = start.and_then(|s| Path::new(path).strip_prefix(s).ok());
            match rel {
                Some(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
                _ => path.to_string(),
            }
        }
        None => Path::new(path)
            .file_name()
            .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned()),
    };
    let mut children: Vec<FolderNode> = kids
        .iter()
        .map(|k| folder_node(k, None, counts, below))
        .collect();
    children.sort_by(|a, b| a.name.cmp(&b.name));
    FolderNode {
        path: path.to_string(),
        name,
        count,
        children,
    }
}

/// A volume's row in the Folders section, open first, its folders below:
/// an offline one greyed, with the day it was last seen.
fn volume_row(
    ui: &mut Ui,
    volume: &VolumeNode,
    selected: Option<&str>,
    chosen: &mut Option<String>,
) {
    let name = if volume.id.is_some() {
        volume.name.clone()
    } else {
        crate::i18n::text(
            ui.ctx(),
            Message::new("gui-library-folders-elsewhere", "Other folders"),
        )
    };
    let text = if volume.online {
        format!("{name} {}", volume.count)
    } else {
        let day = volume
            .last_seen
            .and_then(|at| i64::try_from(at).ok())
            .and_then(day_text)
            .unwrap_or_else(|| "?".into());
        crate::i18n::text(
            ui.ctx(),
            Message::new(
                "gui-library-volume-offline",
                "{ $volume } { $count }, offline, last seen { $date }",
            )
            .with("volume", name)
            .with("count", volume.count.to_string())
            .with("date", day),
        )
    };
    let id = ui.make_persistent_id(("helios.library.volume", &volume.id));
    let online = volume.online;
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            let label = egui::RichText::new(text).strong();
            let label = if online { label } else { label.weak() };
            let row = ui.add(egui::Label::new(label).sense(egui::Sense::click()));
            // Known volumes only: "Other folders" is no place on disk.
            if volume.id.is_some() {
                let mount = volume.mount.as_ref().map(|m| m.display().to_string());
                folder_menu(&row, mount.as_deref(), online && mount.is_some());
            }
        })
        .body(|ui| {
            if !online {
                // Its folders greyed too: their photos are not at hand.
                ui.visuals_mut().override_text_color = Some(ui.visuals().weak_text_color());
            }
            for top in &volume.folders {
                folder_row(ui, top, true, online, selected, chosen);
            }
        });
}

/// A folder's row in the Folders section, and the folders below it (open
/// on demand, a top folder open first); `chosen` takes the folder clicked.
fn folder_row(
    ui: &mut Ui,
    node: &FolderNode,
    top: bool,
    online: bool,
    selected: Option<&str>,
    chosen: &mut Option<String>,
) {
    let text = format!("{} {}", node.name, node.count);
    let on = selected == Some(node.path.as_str());
    let mut row = |ui: &mut Ui| {
        let r = ui.selectable_label(on, text.clone());
        folder_menu(&r, Some(&node.path), online);
        if r.clicked() {
            *chosen = Some(node.path.clone());
        }
    };
    if node.children.is_empty() {
        row(ui);
        return;
    }
    let id = ui.make_persistent_id(("helios.library.folder", &node.path));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, top)
        .show_header(ui, row)
        .body(|ui| {
            for child in &node.children {
                folder_row(ui, child, false, online, selected, chosen);
            }
        });
}

/// The command a folder's "Scan this folder" runs (`path`): the image
/// map's job lists the folder and those below it again, recording the
/// files new or changed there.
pub const SCAN_FOLDER_COMMAND: &str = "library.scanFolder";

/// The command that scopes the library to a folder (`path`; null: the
/// whole library), kept with the workspace.
pub const FOLDER_COMMAND: &str = "library.folder";

/// A folder's or a volume's context menu in the folders tree, through the
/// shell's menus: "Show in file manager", "Scan this folder" and "Import
/// new photos here", greyed for a folder not at hand (an offline volume's).
fn folder_menu(row: &egui::Response, path: Option<&str>, at_hand: bool) {
    use crate::dialogs::MenuItem;
    if !row.secondary_clicked() {
        return;
    }
    let ctx = &row.ctx;
    let word = |m: Message| crate::i18n::text(ctx, m);
    let path = path.unwrap_or_default();
    let item = |id: &str, args: serde_json::Value, label: Message| {
        MenuItem::run(id, args)
            .labeled(word(label))
            .enabled(at_hand && !path.is_empty())
    };
    crate::dialogs::context_menu(
        row,
        vec![
            item(
                "library.openFolder",
                serde_json::json!({ "path": path }),
                Message::new("gui-library-folder-show", "Show in file manager"),
            ),
            item(
                SCAN_FOLDER_COMMAND,
                serde_json::json!({ "path": path }),
                Message::new("gui-library-folder-scan", "Scan this folder"),
            ),
            MenuItem::Separator,
            item(
                crate::clipboard::images::WINDOW,
                serde_json::json!({ "source": path }),
                Message::new("gui-library-folder-import", "Import new photos here"),
            ),
        ],
    );
}

/// The command that scopes the library to a collection (`id`; null: the
/// whole library), kept with the workspace.
pub const COLLECTION_COMMAND: &str = "library.collection";
/// Makes an album (`name`; `images`, else the selection), or with
/// `kind: "smart"` a smart album (`rules`, a tree, or the form's keys),
/// or with `kind: "set"` an album set (`set`: the set it goes in);
/// without `name`, asks it.
pub const COLLECTION_CREATE_COMMAND: &str = "library.collection.create";
/// Renames an album (`id`, `name`); without `name`, asks it.
pub const COLLECTION_RENAME_COMMAND: &str = "library.collection.rename";
/// Forgets an album (`id`); its photos stay in the Library. A set's
/// albums move up, or with `members: true` are forgotten too.
pub const COLLECTION_DELETE_COMMAND: &str = "library.collection.delete";
/// Moves an album or a set (`id`) into a set (`set`; null: the top);
/// without `set`, asks it.
pub const COLLECTION_MOVE_COMMAND: &str = "library.collection.move";
/// Puts photos in an album (`id`; `images`, else the selection).
pub const COLLECTION_ADD_COMMAND: &str = "library.collection.add";
/// Takes photos out of an album (`id`; `images`, else the selection).
pub const COLLECTION_REMOVE_COMMAND: &str = "library.collection.remove";
/// Sets a smart album's rules (`id`; `rules`, a tree, or the form's
/// keys); without them, opens the rule editor on them.
pub const COLLECTION_RULES_COMMAND: &str = "library.collection.rules";
/// Edits the rules in the open rule editor, one edit a step
/// ([`crate::rule_editor::Edit::parse`]), or undoes, redoes, applies,
/// cancels; answers the editor's state.
pub const COLLECTION_RULES_EDIT_COMMAND: &str = "library.collection.rules.edit";
/// Lists the albums, with how many photos each holds.
pub const COLLECTION_LIST_COMMAND: &str = "library.collection.list";
/// B: puts the selected photos (`images`, else the selection) in the
/// quick collection, or takes them out when it holds them all; with
/// `add: true` (a drop on its row), only puts in those it lacks.
pub const QUICK_TOGGLE_COMMAND: &str = "library.quick.toggle";
/// Empties the quick collection.
pub const QUICK_CLEAR_COMMAND: &str = "library.quick.clear";
/// Makes an album of the quick collection (`name`; `clear` to empty it
/// too); without `name`, asks it.
pub const QUICK_SAVE_COMMAND: &str = "library.quick.saveAs";

/// Whether something is to be read again by the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wanted {
    Yes,
    No,
}

/// An album change the Library asks the host to make in the vault, one
/// undo step each (`docs/design/library.md`, L6). Images by id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlbumEdit {
    Create {
        id: String,
        name: String,
        images: Vec<String>,
    },
    Rename {
        id: String,
        name: String,
    },
    /// An album forgotten; a set's members move up, or with `members`
    /// are forgotten too.
    Delete {
        id: String,
        members: bool,
    },
    /// An album set, in the set `set` when given.
    CreateSet {
        id: String,
        name: String,
        set: Option<String>,
    },
    /// An album or a set moved into the set `set` (none: the top).
    Move {
        id: String,
        set: Option<String>,
    },
    Add {
        id: String,
        images: Vec<String>,
    },
    Remove {
        id: String,
        images: Vec<String>,
    },
    /// A smart album: its rules choose its photos.
    CreateSmart {
        id: String,
        name: String,
        rules: helios_vault::rules::Rule,
    },
    /// A smart album's rules.
    Rules {
        id: String,
        rules: helios_vault::rules::Rule,
    },
    /// B: the photos in or out of the quick collection.
    QuickToggle {
        images: Vec<String>,
    },
    /// The quick collection emptied.
    QuickClear,
    /// The quick collection saved as an album (`id`, `name`), emptied
    /// when `clear`.
    QuickSave {
        id: String,
        name: String,
        clear: bool,
    },
}

impl AlbumEdit {
    /// The album it changes.
    pub fn id(&self) -> &str {
        match self {
            AlbumEdit::Create { id, .. }
            | AlbumEdit::Rename { id, .. }
            | AlbumEdit::Delete { id, .. }
            | AlbumEdit::CreateSet { id, .. }
            | AlbumEdit::Move { id, .. }
            | AlbumEdit::Add { id, .. }
            | AlbumEdit::Remove { id, .. }
            | AlbumEdit::CreateSmart { id, .. }
            | AlbumEdit::Rules { id, .. }
            | AlbumEdit::QuickSave { id, .. } => id,
            AlbumEdit::QuickToggle { .. } | AlbumEdit::QuickClear => {
                helios_vault::collections::QUICK
            }
        }
    }
}

/// An album's context menu in the Albums section, through the shell's
/// menus: add or remove the selected photos (greyed with none), rename,
/// move to a set (greyed when there is none to go to), delete. A set's
/// menu makes a set in it, renames, moves it, deletes it keeping its
/// albums or with them.
fn album_menu(
    row: &egui::Response,
    album: &helios_vault::collections::Album,
    selection: bool,
    movable: bool,
) {
    use crate::dialogs::MenuItem;
    let (id, smart) = (album.id.as_str(), album.is_smart());
    if !row.secondary_clicked() {
        return;
    }
    let ctx = &row.ctx;
    let word = |m: Message| crate::i18n::text(ctx, m);
    let args = serde_json::json!({ "id": id });
    let item =
        |command: &str, label: Message| MenuItem::run(command, args.clone()).labeled(word(label));
    let move_to = item(
        COLLECTION_MOVE_COMMAND,
        Message::new("gui-library-album-move", "Move to set…"),
    )
    .enabled(movable);
    if album.is_set() {
        crate::dialogs::context_menu(
            row,
            vec![
                MenuItem::run(
                    COLLECTION_CREATE_COMMAND,
                    serde_json::json!({ "kind": helios_vault::collections::SET, "set": id }),
                )
                .labeled(word(Message::new(
                    "gui-library-set-new-in",
                    "New set inside…",
                ))),
                MenuItem::Separator,
                item(
                    COLLECTION_RENAME_COMMAND,
                    Message::new("gui-library-album-rename", "Rename…"),
                ),
                move_to,
                MenuItem::Separator,
                item(
                    COLLECTION_DELETE_COMMAND,
                    Message::new("gui-library-set-delete", "Delete the set, keep its albums"),
                ),
                MenuItem::run(
                    COLLECTION_DELETE_COMMAND,
                    serde_json::json!({ "id": id, "members": true }),
                )
                .labeled(word(Message::new(
                    "gui-library-set-delete-all",
                    "Delete the set and its albums",
                ))),
            ],
        );
        return;
    }
    // A smart album's photos follow its rules: its menu edits them.
    let fill = if smart {
        vec![item(
            COLLECTION_RULES_COMMAND,
            Message::new("gui-library-album-rules", "Edit the rules…"),
        )]
    } else {
        vec![
            item(
                COLLECTION_ADD_COMMAND,
                Message::new("gui-library-album-add", "Add the selected photos"),
            )
            .enabled(selection),
            item(
                COLLECTION_REMOVE_COMMAND,
                Message::new("gui-library-album-remove", "Remove the selected photos"),
            )
            .enabled(selection),
        ]
    };
    crate::dialogs::context_menu(
        row,
        [
            fill,
            vec![MenuItem::Separator],
            vec![
                item(
                    COLLECTION_RENAME_COMMAND,
                    Message::new("gui-library-album-rename", "Rename…"),
                ),
                move_to,
                item(
                    COLLECTION_DELETE_COMMAND,
                    Message::new("gui-library-album-delete", "Delete the album"),
                ),
            ],
        ]
        .concat(),
    );
}

/// The quick collection's context menu in the Albums section: put the
/// selected photos in or out (greyed with none), empty it, save it as
/// an album (greyed when empty).
fn quick_menu(row: &egui::Response, selection: bool, filled: bool) {
    use crate::dialogs::MenuItem;
    if !row.secondary_clicked() {
        return;
    }
    let ctx = &row.ctx;
    let word = |m: Message| crate::i18n::text(ctx, m);
    let item = |command: &str, label: Message| {
        MenuItem::run(command, serde_json::json!({})).labeled(word(label))
    };
    crate::dialogs::context_menu(
        row,
        vec![
            item(
                QUICK_TOGGLE_COMMAND,
                Message::new(
                    "gui-library-quick-toggle",
                    "Add or remove the selected photos",
                ),
            )
            .enabled(selection),
            MenuItem::Separator,
            item(
                QUICK_SAVE_COMMAND,
                Message::new("gui-library-quick-save", "Save as album…"),
            )
            .enabled(filled),
            item(
                QUICK_CLEAR_COMMAND,
                Message::new("gui-library-quick-clear", "Clear the quick collection"),
            )
            .enabled(filled),
        ],
    );
}

/// Photos dragged out of the grid or the filmstrip (image ids): the
/// payload the Albums section's rows take (`docs/design/library.md`, L6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraggedPhotos(pub Vec<String>);

/// Sets the drag payload when a library view starts carrying photos
/// out.
fn carry_out(ctx: &egui::Context, signals: &[Signal<String>]) {
    for s in signals {
        if let Signal::DragOut { keys } = s {
            egui::DragAndDrop::set_payload(ctx, DraggedPhotos(keys.clone()));
        }
    }
}

/// A row of the Albums section photos may be dropped on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropOn {
    /// The quick collection: they go in.
    Quick,
    /// An album: they go in. A smart album or a set refuses them.
    Album(String),
}

/// Whether `album` takes photos dropped on it, else why not.
fn drop_refusal(album: &helios_vault::collections::Album) -> Option<Message> {
    if album.is_smart() {
        Some(Message::new(
            "gui-library-drop-smart",
            "Not here: a smart album's rules choose its photos",
        ))
    } else if album.is_set() {
        Some(Message::new(
            "gui-library-drop-set",
            "Not here: drop the photos on one of the set's albums",
        ))
    } else {
        None
    }
}

/// A row of the Albums section as a drop target for [`DraggedPhotos`]:
/// while they hover it, its outline (the accent when it takes them, the
/// error colour with a "not allowed" cursor and the reason beside the
/// pointer when not); dropped, they go in (one step of the library's
/// history, through the command an agent runs), or the reason is said.
fn drop_target(
    row: &egui::Response,
    target: DropOn,
    refusal: Option<Message>,
    hover: &std::cell::Cell<Option<(DropOn, bool)>>,
) {
    let ctx = &row.ctx;
    let allowed = refusal.is_none();
    if let Some(photos) = row.dnd_hover_payload::<DraggedPhotos>() {
        hover.set(Some((target.clone(), allowed)));
        let visuals = ctx.global_style().visuals.clone();
        let colour = if allowed {
            visuals.selection.stroke.color
        } else {
            visuals.error_fg_color
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("helios.library.drop"),
        ));
        painter.rect_stroke(
            row.rect.expand(1.0),
            2.0,
            egui::Stroke::new(2.0, colour),
            egui::StrokeKind::Inside,
        );
        let text = match &refusal {
            Some(m) => {
                ctx.set_cursor_icon(egui::CursorIcon::NotAllowed);
                crate::i18n::text(ctx, m.clone())
            }
            None => crate::i18n::text(
                ctx,
                Message::new("gui-library-drop-add", "Add { $count } photos")
                    .with("count", photos.0.len()),
            ),
        };
        // The row's own tooltip, at the pointer: egui hides hover tooltips
        // while something is dragged, so it is shown here, not through
        // `on_hover_text`; it is no window, popup or menu of the panel's.
        egui::Tooltip::for_widget(row).at_pointer().show(|ui| {
            ui.label(text);
        });
    }
    let Some(photos) = row.dnd_release_payload::<DraggedPhotos>() else {
        return;
    };
    if let Some(m) = refusal {
        crate::dialogs::request(
            ctx,
            crate::dialogs::Request::Notify(crate::dialogs::Level::Warning, m),
        );
        return;
    }
    let (command, args) = match target {
        DropOn::Quick => (
            QUICK_TOGGLE_COMMAND,
            serde_json::json!({ "images": photos.0, "add": true }),
        ),
        DropOn::Album(id) => (
            COLLECTION_ADD_COMMAND,
            serde_json::json!({ "id": id, "images": photos.0 }),
        ),
    };
    crate::dialogs::request(
        ctx,
        crate::dialogs::Request::Run(command.into(), args, helios_shell::commands::Origin::User),
    );
}

/// The albums as the Albums section shows them: each set's members, a
/// collection whose set is not there at the top.
struct AlbumTree<'a> {
    albums: &'a [helios_vault::collections::Album],
}

impl<'a> AlbumTree<'a> {
    fn of(albums: &'a [helios_vault::collections::Album]) -> Self {
        Self { albums }
    }

    /// Whether `id` names a set of the list.
    fn is_set(&self, id: &str) -> bool {
        self.albums.iter().any(|a| a.id == id && a.is_set())
    }

    /// The collections in the set `parent` (none: the top), sets first,
    /// each kind by name as the vault lists them.
    fn members(&self, parent: Option<&str>) -> Vec<&'a helios_vault::collections::Album> {
        let mut list: Vec<_> = self
            .albums
            .iter()
            .filter(|a| match (parent, a.set.as_deref()) {
                (None, None) => true,
                (None, Some(set)) => !self.is_set(set),
                (Some(p), Some(set)) => p == set,
                (Some(_), None) => false,
            })
            .collect();
        list.sort_by_key(|a| !a.is_set());
        list
    }

    /// Whether there is somewhere to move `album`: out of its set, or
    /// into a set other than itself.
    fn has_sets_for(&self, album: &helios_vault::collections::Album) -> bool {
        album.set.is_some() || self.albums.iter().any(|a| a.is_set() && a.id != album.id)
    }
}

/// The longest side of a thumbnail, in pixels.
pub const THUMB: u32 = 256;

/// A photo's backups as the grid marks them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BackupMark {
    /// Its verified copies meet the strategy.
    Met,
    /// Some verified copies, fewer than the strategy asks (or none off
    /// site when one must be).
    Short,
    /// Copies on backup disks not verified by hash yet, short otherwise.
    Unverified,
    /// No backup copy at all.
    None,
}

impl BackupMark {
    /// The mark of a photo with `status`.
    pub fn of(status: &helios_storage::strategy::BackupStatus) -> Self {
        if status.have == 0 && status.unverified == 0 {
            Self::None
        } else if status.met() {
            Self::Met
        } else if status.unverified > 0 {
            Self::Unverified
        } else {
            Self::Short
        }
    }
}

pub struct Library {
    pub entries: Vec<Entry>,
    /// The image under the cursor, by its place in `entries`.
    pub selected: Option<usize>,
    /// Culling: after a mark, the cursor moves to the next image.
    pub auto_advance: bool,
    /// Marks a plugin refused, to say.
    pub notes: Vec<String>,
    /// The Albums section's row photos are dragged over this pass, and
    /// whether it takes them.
    drop_hover: std::cell::Cell<Option<(DropOn, bool)>>,
    /// The workspace has an import source: the Library panel's toolbar
    /// offers it.
    pub import_offered: bool,
    /// The user asked to import from it.
    import_asked: bool,
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
    /// What a backup state is measured against (`backup:` terms, the
    /// backup facet): the library's strategy and backup volumes.
    backup_rule: helios_vault::Backups,
    /// How many photos of the filter carry each value of each facet, as
    /// the vault last counted them.
    facets: Option<helios_vault::Facets>,
    /// The folders tree's counts (`docs/design/library.md`, L5): photos
    /// with a copy in each folder or below, for the whole Library, as the
    /// vault last counted them; none until the Folders section is open.
    folders: FolderCounts,
    /// The vault's albums (`docs/design/library.md`, L6), as last read;
    /// none until the host reads them.
    albums: Option<Vec<helios_vault::collections::Album>>,
    /// Whether the albums must be read again
    /// ([`Library::take_albums_wanted`]).
    albums_wanted: Wanted,
    /// Album changes asked, for the host to make in the vault.
    album_edits: Vec<AlbumEdit>,
    /// The quick collection's photos, as last read with the albums.
    quick: Vec<String>,
    /// The same, for the grid's badge.
    quick_set: HashSet<String>,
    /// How many heads each photo with more than one has (its versions:
    /// copies made with [`DUPLICATE_COMMAND`], darktable duplicates), for
    /// the grid's badge.
    head_counts: HashMap<String, usize>,
    /// The heads of the photo under the cursor, as the node listed them,
    /// for the versions strip.
    versions: Option<(String, Vec<VersionHead>)>,
    /// The photo whose heads the strip wants listed, for the host.
    versions_wanted: Option<String>,
    /// The photo whose heads were asked, until they come.
    versions_asked: Option<String>,
    /// What the strip and the commands asked of photos' versions, for
    /// the host.
    version_asks: Vec<VersionAsk>,
    /// The pinned and recent filters (`crate::recent_filters`), as last
    /// read from the vault; none until the host reads them.
    filter_history: Option<crate::recent_filters::FilterHistory>,
    /// Whether the host is to read the history (once, at the start).
    filter_history_wanted: Wanted,
    /// Filters used before the history was read, oldest first.
    used_early: Vec<Filter>,
    /// Whether the history changed, for the host to write.
    filter_history_write: Wanted,
    /// A filter typed in the search field and when (the input's time):
    /// it counts as used once it rests ([`crate::recent_filters::TYPED_REST`]).
    typed: Option<(Filter, f64)>,
    tags: TagInputState,
    /// Every tag the library's images carry, for completion.
    known_tags: Vec<String>,
    app: Option<AppHandle>,
    changes: Vec<Change>,
    /// Stacks expanded or collapsed, for the vault: (stack, expanded).
    expansions: Vec<(String, bool)>,
    /// An image opened (Enter, a double click), for the host.
    activated: Option<String>,
    sink: ThumbnailSink,
    receiver: Receiver<(String, Arrival)>,
    /// The thumbnails' textures, within their budget.
    textures: Resident,
    /// The large thumbnails Compare draws until a render lands.
    large_textures: HashMap<String, TextureHandle>,
    /// The large thumbnails asked for, not arrived yet.
    large_asked: HashSet<String>,
    /// Asked an image's large thumbnail.
    on_large: Option<Box<OnLarge>>,
    /// The grid's cells in view changed since the margin was asked.
    view_changed: std::cell::Cell<bool>,
    /// The placeholders' textures, by file type (`CR2`, `?`).
    placeholders: HashMap<String, TextureHandle>,
    /// The grid's cells in view at its last pass (the thumbnail benchmark
    /// reads them: `examples/thumb_bench.rs`).
    in_view: RefCell<Vec<String>>,
    /// Told the thumbnails wanted (the cells in view without one, then a
    /// margin around them): the thumbnail pool reads those first.
    on_view: Option<Box<OnView>>,
    /// The thumbnail cache, read on the interface's frame for the cells
    /// drawn without a texture (see [`Library::set_thumb_cache`]).
    warm: Option<Arc<helios_thumbs::ThumbCache>>,
    /// How long a frame may spend reading the cache for those cells.
    warm_budget: std::time::Duration,
    heads: Heads,
    /// Where each image's file is, by id: what a copy of images names.
    files: HashMap<String, PathBuf>,
    /// The volumes the vault knows, by id: what an offline photo names.
    volumes: HashMap<String, VolumeNote>,
    /// Each photo's backups, by id, as last read.
    backups: HashMap<String, BackupMark>,
    /// Whether the grid shows the backup badges: the library has a
    /// strategy.
    backup_badges: Shown,
    /// The image map's feed: the thumbnail re-check reads the files of
    /// the cells in view first.
    map_feed: Option<helios_library::map_job::Feed>,
    /// The ids in view in the grid and in Compare, as last fed.
    fed: (Vec<String>, Vec<String>),
    /// The order the vault reads the photos in: this focus's sort.
    sort: helios_vault::Order,
    /// The sort the entries must be read again in, once it changed
    /// ([`Library::take_resort`]).
    resort: Option<helios_vault::Order>,
    /// The marks copied last ([`MARKS_COPY_COMMAND`]), to paste.
    copied: Option<CopiedMarks>,
    /// The photos asked taken out of the Library ([`REMOVE_COMMAND`]),
    /// for the host.
    removals: Vec<Vec<String>>,
    /// The photos asked turned ([`ROTATE_LEFT_COMMAND`] and the others),
    /// for the host.
    rotations: Vec<(Vec<String>, Turn)>,
    /// The photos asked duplicated ([`DUPLICATE_COMMAND`],
    /// [`DUPLICATE_ORIGINAL_COMMAND`]), for the host.
    duplicates: Vec<(Vec<String>, helios_vault::versions::DuplicateFrom)>,
    /// The photos whose files were asked moved to the trash
    /// ([`TRASH_COMMAND`], [`DELETE_REJECTED_COMMAND`]), for the host.
    trash_asks: Vec<TrashAsk>,
}

/// The library's sort when nothing chose one: by capture time, oldest
/// first (`docs/design/library.md`, L-C1).
pub const DEFAULT_SORT: helios_vault::Order =
    helios_vault::Order::asc(helios_vault::Sort::Captured);

/// The command that sorts the library (`by`, `descending`).
pub const SORT_COMMAND: &str = "library.sort";

/// The sort a template's library names (its default when it names none);
/// `None` for a sort it does not know.
pub fn sort_of(desc: Option<&helios_template::SortDesc>) -> Option<helios_vault::Order> {
    let Some(d) = desc else {
        return Some(DEFAULT_SORT);
    };
    let by = helios_vault::Sort::parse(&d.by).filter(|s| *s != helios_vault::Sort::Id)?;
    Some(helios_vault::Order {
        by,
        descending: d.descending,
    })
}

/// A sort as a template's library writes it (nothing for the default).
pub fn sort_desc(order: helios_vault::Order) -> Option<helios_template::SortDesc> {
    (order != DEFAULT_SORT).then(|| helios_template::SortDesc {
        by: order.by.name().into(),
        descending: order.descending,
    })
}

/// A sort's name in the menu.
fn sort_label(by: helios_vault::Sort) -> Message {
    use helios_vault::Sort;
    match by {
        Sort::Captured => Message::new("gui-library-sort-captured", "Capture time"),
        Sort::Imported => Message::new("gui-library-sort-imported", "Import time"),
        Sort::Name => Message::new("gui-library-sort-name", "File name"),
        Sort::Rating => Message::new("gui-library-sort-rating", "Rating"),
        Sort::Edited => Message::new("gui-library-sort-edited", "Last edit"),
        Sort::Size => Message::new("gui-library-sort-size", "File size"),
        Sort::Focus => Message::new("gui-library-sort-focus", "Sharpness"),
        Sort::Id => Message::new("gui-library-sort-id", "Content"),
    }
}

/// Whether something is drawn (a struct of many flags reads worse).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Shown {
    #[default]
    No,
    Yes,
}

impl Default for Library {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Library {
            entries: Vec::new(),
            selected: None,
            auto_advance: false,
            notes: Vec::new(),
            drop_hover: std::cell::Cell::new(None),
            import_offered: false,
            import_asked: false,
            index: HashMap::new(),
            order: Vec::new(),
            revision: 0,
            state: LibraryState::new(LIBRARY),
            search: SearchState::new(),
            filter: Filter::default(),
            matches: None,
            backup_rule: helios_vault::Backups::default(),
            facets: None,
            folders: FolderCounts::Unread,
            albums: None,
            albums_wanted: Wanted::Yes,
            album_edits: Vec::new(),
            quick: Vec::new(),
            quick_set: HashSet::new(),
            head_counts: HashMap::new(),
            versions: None,
            versions_wanted: None,
            versions_asked: None,
            version_asks: Vec::new(),
            filter_history: None,
            filter_history_wanted: Wanted::Yes,
            used_early: Vec::new(),
            filter_history_write: Wanted::No,
            typed: None,
            query: None,
            tags: TagInputState::default(),
            known_tags: Vec::new(),
            app: None,
            changes: Vec::new(),
            expansions: Vec::new(),
            activated: None,
            sink: ThumbnailSink {
                sender,
                wake: Arc::default(),
            },
            receiver,
            textures: Resident::default(),
            large_textures: HashMap::new(),
            large_asked: HashSet::new(),
            on_large: None,
            view_changed: std::cell::Cell::new(false),
            placeholders: HashMap::new(),
            in_view: RefCell::default(),
            on_view: None,
            warm: None,
            warm_budget: WARM_FRAME_BUDGET,
            heads: Heads::default(),
            files: HashMap::new(),
            volumes: HashMap::new(),
            backups: HashMap::new(),
            backup_badges: Shown::No,
            map_feed: None,
            fed: (Vec::new(), Vec::new()),
            sort: DEFAULT_SORT,
            resort: None,
            copied: None,
            removals: Vec::new(),
            rotations: Vec::new(),
            duplicates: Vec::new(),
            trash_asks: Vec::new(),
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

/// An 8-bit sRGB thumbnail as a picture for a texture.
pub fn rgb_image(rgb: &helios_thumbs::Rgb8) -> ColorImage {
    ColorImage::from_rgb([rgb.width as usize, rgb.height as usize], &rgb.pixels)
}

/// A picture turned `turns` quarters clockwise (L9's rotation): its
/// pixels moved, nothing decoded again.
pub fn turned(image: ColorImage, turns: u8) -> ColorImage {
    let [w, h] = image.size;
    let at = |x: usize, y: usize| image.pixels[y * w + x];
    match turns % 4 {
        0 => image,
        2 => {
            let mut pixels = image.pixels.clone();
            pixels.reverse();
            ColorImage::new([w, h], pixels)
        }
        quarter => {
            // The new picture is h wide and w high; its pixel (x, y)
            // comes from (y, h - 1 - x) turned clockwise, from
            // (w - 1 - y, x) turned counter-clockwise.
            let mut pixels = Vec::with_capacity(w * h);
            for y in 0..w {
                for x in 0..h {
                    pixels.push(if quarter == 1 {
                        at(y, h - 1 - x)
                    } else {
                        at(w - 1 - y, x)
                    });
                }
            }
            ColorImage::new([h, w], pixels)
        }
    }
}

/// An image in the working space (linear Rec.2020) as an sRGB picture no
/// larger than [`THUMB`].
pub fn thumbnail(image: &Image) -> ColorImage {
    srgb(&small(image))
}

/// An image in the working space (linear Rec.2020) as an sRGB picture,
/// at its size.
pub fn srgb(small: &Image) -> ColorImage {
    use helios_photo::color;
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

/// The badges' words for an image not at hand, translated.
struct PresenceWords {
    offline: String,
    missing: String,
    backed_up: String,
    short: String,
    unverified: String,
    no_backup: String,
    changed: String,
    soft: String,
    blown: String,
    crushed: String,
    quick: String,
    versions: String,
}

impl PresenceWords {
    fn of(ctx: &egui::Context) -> Self {
        Self {
            offline: crate::i18n::text(ctx, Message::new("gui-library-badge-offline", "offline")),
            missing: crate::i18n::text(ctx, Message::new("gui-library-badge-missing", "missing")),
            backed_up: crate::i18n::text(
                ctx,
                Message::new("gui-library-badge-backed-up", "backed up"),
            ),
            short: crate::i18n::text(
                ctx,
                Message::new("gui-library-badge-backup-short", "backup short"),
            ),
            unverified: crate::i18n::text(
                ctx,
                Message::new("gui-library-badge-backup-unverified", "unverified"),
            ),
            no_backup: crate::i18n::text(
                ctx,
                Message::new("gui-library-badge-no-backup", "no backup"),
            ),
            changed: crate::i18n::text(
                ctx,
                Message::new("gui-library-badge-changed-outside", "changed outside"),
            ),
            soft: crate::i18n::text(ctx, Message::new("gui-library-badge-soft", "soft focus")),
            blown: crate::i18n::text(
                ctx,
                Message::new("gui-library-badge-blown", "blown highlights"),
            ),
            crushed: crate::i18n::text(
                ctx,
                Message::new("gui-library-badge-crushed", "crushed shadows"),
            ),
            quick: crate::i18n::text(ctx, Message::new("gui-library-badge-quick", "quick")),
            versions: crate::i18n::text(
                ctx,
                Message::new("gui-library-badge-versions", "versions"),
            ),
        }
    }
}

/// The images the views show.
struct Items<'a> {
    entries: &'a [Entry],
    order: &'a [usize],
    /// Stacks shown (a filter flattens them).
    stacks: bool,
    revision: u64,
    words: &'a PresenceWords,
    /// Each photo's backups, when the badges show.
    backups: Option<&'a HashMap<String, BackupMark>>,
    /// The photos in the quick collection.
    quick: &'a HashSet<String>,
    /// How many heads the photos with more than one have.
    heads: &'a HashMap<String, usize>,
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
        let entry = self.entry(index);
        let path = std::path::Path::new(&entry.name);
        let mut badges = Vec::new();
        if path.extension().is_some() && helios_load::Kind::of(path) == helios_load::Kind::Raw {
            badges.push(Badge::new("RAW"));
        }
        // Not at hand: on a disk not plugged in, or gone.
        match entry.presence {
            helios_vault::Presence::Present => {}
            helios_vault::Presence::Offline => {
                badges.push(Badge::new(self.words.offline.clone()).tone(BadgeTone::Warning));
            }
            helios_vault::Presence::Missing => {
                badges.push(Badge::new(self.words.missing.clone()).tone(BadgeTone::Danger));
            }
        }
        // Its file rewritten by another application, not acknowledged.
        if entry.changed_outside.is_some() {
            badges.push(Badge::new(self.words.changed.clone()).tone(BadgeTone::Info));
        }
        if self.quick.contains(&entry.id) {
            badges.push(Badge::new(self.words.quick.clone()).tone(BadgeTone::Info));
        }
        // Its versions (L10, L-C5 B): copies and darktable duplicates,
        // counted, one cell for the photo.
        if let Some(&n) = self.heads.get(&entry.id).filter(|n| **n > 1) {
            badges.push(Badge::new(format!("{n} {}", self.words.versions)).tone(BadgeTone::Info));
        }
        if let Some(mark) = self.backups.and_then(|b| b.get(&entry.id)) {
            let w = self.words;
            badges.push(match mark {
                BackupMark::Met => Badge::new(w.backed_up.clone()).tone(BadgeTone::Success),
                BackupMark::Short => Badge::new(w.short.clone()).tone(BadgeTone::Warning),
                BackupMark::Unverified => Badge::new(w.unverified.clone()).tone(BadgeTone::Info),
                BackupMark::None => Badge::new(w.no_backup.clone()).tone(BadgeTone::Danger),
            });
        }
        // The culling measures, once measured: soft focus, blown
        // highlights, crushed shadows.
        if entry.focus.is_some_and(cull::soft) {
            badges.push(Badge::new(self.words.soft.clone()).tone(BadgeTone::Warning));
        }
        if entry.clip_white.is_some_and(|w| w > cull::CLIPPED_WHITE) {
            badges.push(Badge::new(self.words.blown.clone()).tone(BadgeTone::Warning));
        }
        if entry.clip_black.is_some_and(|b| b > cull::CLIPPED_BLACK) {
            badges.push(Badge::new(self.words.crushed.clone()).tone(BadgeTone::Warning));
        }
        badges
    }

    fn revision(&self) -> u64 {
        self.revision
    }
}

/// What the Compare panel draws: an image's head version rendered once
/// it is, its thumbnail (a raw's embedded preview) until then.
struct CompareThumbs<'a> {
    heads: &'a HashMap<String, HeadTexture>,
    /// The cache's large entries, drawn until a render lands.
    large_textures: &'a HashMap<String, TextureHandle>,
    thumbs: Thumbs<'a>,
}

impl ThumbSource<String> for CompareThumbs<'_> {
    fn thumb(&self, key: &String, size: u32) -> Thumb {
        match self.heads.get(key) {
            Some(h) => {
                let [w, ht] = h.texture.size();
                Thumb::Ready {
                    texture: h.texture.id(),
                    size: [w as u32, ht as u32],
                }
            }
            None => match self.large_textures.get(key) {
                Some(t) => {
                    let [w, h] = t.size();
                    Thumb::Ready {
                        texture: t.id(),
                        size: [w as u32, h as u32],
                    }
                }
                None => self.thumbs.thumb(key, size),
            },
        }
    }
}

/// The thumbnails received so far.
struct Thumbs<'a> {
    textures: &'a Resident,
    /// The pass drawn (what a texture's last use counts).
    pass: u64,
    /// Where the cells the grid builds are recorded, when it does.
    in_view: Option<&'a RefCell<Vec<String>>>,
    /// Set when the cells in view change.
    changed: Option<&'a std::cell::Cell<bool>>,
}

impl ThumbSource<String> for Thumbs<'_> {
    fn thumb(&self, key: &String, _size: u32) -> Thumb {
        match self.textures.draw(key, self.pass) {
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

    fn wanted(&self, keys: &[String], _size: u32) {
        if let Some(in_view) = self.in_view {
            let mut in_view = in_view.borrow_mut();
            if in_view.as_slice() != keys {
                *in_view = keys.to_vec();
                if let Some(changed) = self.changed {
                    changed.set(true);
                }
            }
        }
    }
}

impl Library {
    /// The application whose steps the marks run through.
    pub fn set_app(&mut self, app: AppHandle) {
        self.app = Some(app);
    }

    /// Where the images' files are, by id (their first present copy).
    pub fn set_files(&mut self, files: impl IntoIterator<Item = (String, PathBuf)>) {
        self.files = files.into_iter().collect();
    }

    /// Where an image's file is, when it is on disk.
    pub fn file(&self, id: &str) -> Option<&Path> {
        self.files.get(id).map(PathBuf::as_path)
    }

    /// The volumes the vault knows: what an offline photo names.
    pub fn set_volumes(&mut self, volumes: &[helios_storage::Volume]) {
        self.volumes = volumes
            .iter()
            .map(|v| {
                (
                    v.id.as_str().to_string(),
                    VolumeNote {
                        name: v.name.clone(),
                        last_seen: v.last_seen.as_ref().map(|s| (s.at, s.node.clone())),
                        online: v.online(),
                        mount: v.mount.clone(),
                    },
                )
            })
            .collect();
    }

    /// Each photo's backups, by id, read once per refresh; `badges`: the
    /// library has a strategy, so the grid shows them. The `backup:`
    /// words of the search field match on them.
    pub fn set_backups(&mut self, backups: HashMap<String, BackupMark>, badges: bool) {
        let badges = if badges { Shown::Yes } else { Shown::No };
        if self.backups == backups && self.backup_badges == badges {
            return;
        }
        self.backups = backups;
        self.backup_badges = badges;
        // Backups changed: a `backup:` term may take other photos.
        if self.filter.text.to_lowercase().contains("backup:") {
            self.ask_query();
        }
        self.reorder();
    }

    /// What a backup state is measured against: the library's strategy
    /// and backup volumes (`helios_storage::strategy::rule`).
    pub fn set_backup_rule(&mut self, rule: helios_vault::Backups) {
        if self.backup_rule != rule {
            self.backup_rule = rule;
            self.ask_query();
        }
    }

    /// The backup rule the facets count against: only when the library
    /// has a strategy (its badges show).
    pub fn facet_rule(&self) -> Option<&helios_vault::Backups> {
        (self.backup_badges == Shown::Yes).then_some(&self.backup_rule)
    }

    /// The facet counts of the filter, as the vault counted them.
    pub fn set_facets(&mut self, facets: helios_vault::Facets) {
        self.facets = Some(facets);
    }

    /// The facet counts shown, once counted.
    pub fn facets(&self) -> Option<&helios_vault::Facets> {
        self.facets.as_ref()
    }

    /// Whether the host must count the folders ([`Library::set_folders`]),
    /// once: asked while the Folders section is open and its counts are
    /// not read since the photos last changed.
    pub fn take_folders_wanted(&mut self) -> bool {
        let wanted = matches!(self.folders, FolderCounts::Wanted);
        if wanted {
            self.folders = FolderCounts::Asked;
        }
        wanted
    }

    /// The folders tree's counts, as the vault counted them
    /// ([`helios_vault::Vault::folders`] for the whole Library).
    pub fn set_folders(&mut self, folders: helios_vault::VolumeFolders) {
        self.folders = FolderCounts::Read(folders);
    }

    /// The folders tree, once counted: the volumes, each with its top
    /// folders and theirs below.
    pub fn folder_tree(&self) -> Option<Vec<VolumeNode>> {
        match &self.folders {
            FolderCounts::Read(counts) => Some(volume_tree(counts, &self.volumes)),
            _ => None,
        }
    }

    /// The folder the filter is scoped to (its `folder:` term).
    pub fn folder(&self) -> Option<String> {
        helios_terms(&self.filter.text).folder
    }

    /// Scopes the library to a folder and below (its `folder:` term in
    /// the filter, in place of any other), or to no folder; the rest of
    /// the filter is kept.
    pub fn set_folder(&mut self, folder: Option<&str>) {
        let mut words: Vec<String> = tokens(&self.filter.text)
            .into_iter()
            .filter(|t| !t.to_lowercase().starts_with("folder:"))
            .collect();
        if let Some(folder) = folder {
            words.push(format!("folder:{}", quoted(folder)));
        }
        let filter = Filter {
            text: words.join(" "),
            ..self.filter.clone()
        };
        self.set_filter(filter);
    }

    /// The collection the filter is scoped to (its `collection:` term).
    pub fn collection(&self) -> Option<String> {
        helios_terms(&self.filter.text).collection
    }

    /// Scopes the library to a collection (its `collection:` term in the
    /// filter, in place of any other), or to none; the rest of the filter
    /// is kept.
    pub fn set_collection(&mut self, id: Option<&str>) {
        let mut words: Vec<String> = tokens(&self.filter.text)
            .into_iter()
            .filter(|t| !t.to_lowercase().starts_with("collection:"))
            .collect();
        if let Some(id) = id {
            words.push(format!("collection:{}", quoted(id)));
        }
        let filter = Filter {
            text: words.join(" "),
            ..self.filter.clone()
        };
        self.set_filter(filter);
    }

    /// The vault's albums, as last read (none before the host reads them).
    pub fn albums(&self) -> Option<&[helios_vault::collections::Album]> {
        self.albums.as_deref()
    }

    /// How many photos of the Library an album holds (a photo taken out
    /// of the Library since does not count); a smart album, how many its
    /// rules selected when the albums were read; a set, how many its
    /// members held together then.
    pub fn album_count(&self, album: &helios_vault::collections::Album) -> usize {
        if album.is_smart() || album.is_set() {
            return album.count.unwrap_or(0);
        }
        album
            .images
            .iter()
            .filter(|i| self.index.contains_key(*i))
            .count()
    }

    /// The quick collection's photos, by id, as last read (those taken
    /// out of the Library since among them).
    pub fn quick(&self) -> &[String] {
        &self.quick
    }

    /// The Albums section's row photos were dragged over last pass, and
    /// whether it takes them.
    pub fn drop_hover(&self) -> Option<(DropOn, bool)> {
        let h = self.drop_hover.take();
        self.drop_hover.set(h.clone());
        h
    }

    /// How many photos of the Library the quick collection holds.
    pub fn quick_count(&self) -> usize {
        self.quick
            .iter()
            .filter(|i| self.index.contains_key(*i))
            .count()
    }

    /// How many heads the photos with more than one have, as the node
    /// counts them (`library.versions.heads`): the grid's badge follows.
    pub fn set_heads(&mut self, heads: HashMap<String, usize>) {
        if self.head_counts == heads {
            return;
        }
        self.head_counts = heads;
        self.versions_changed();
        self.revision += 1;
    }

    /// Sets one photo's head count, as a duplicate's answer gives it.
    pub fn set_heads_of(&mut self, image: &str, heads: usize) {
        let before = if heads > 1 {
            self.head_counts.insert(image.to_string(), heads)
        } else {
            self.head_counts.remove(image)
        };
        if before != Some(heads).filter(|n| *n > 1) {
            self.revision += 1;
        }
    }

    /// How many heads a photo has, when more than one.
    pub fn heads_of(&self, image: &str) -> Option<usize> {
        self.head_counts.get(image).copied()
    }

    /// The quick collection as the vault holds it: the grid's badges
    /// follow, and its photos are asked again when it is shown.
    pub fn set_quick(&mut self, images: Vec<String>) {
        if self.quick == images {
            return;
        }
        self.quick_set = images.iter().cloned().collect();
        self.quick = images;
        if self.collection().as_deref() == Some(helios_vault::collections::QUICK) {
            self.ask_query();
        }
        self.reorder();
    }

    /// Whether the host must read the albums ([`Library::set_albums`]),
    /// once.
    pub fn take_albums_wanted(&mut self) -> bool {
        std::mem::replace(&mut self.albums_wanted, Wanted::No) == Wanted::Yes
    }

    /// The albums as the vault holds them; the photos of the album shown
    /// are asked again.
    pub fn set_albums(&mut self, albums: Vec<helios_vault::collections::Album>) {
        self.albums = Some(albums);
        if self.collection().is_some() {
            self.ask_query();
        }
    }

    /// Asks the host to read the albums again (an undo, a redo).
    pub fn ask_albums(&mut self) {
        self.albums_wanted = Wanted::Yes;
    }

    /// Queues an album change for the host ([`Library::take_album_edits`]).
    pub fn edit_album(&mut self, edit: AlbumEdit) {
        self.album_edits.push(edit);
    }

    /// The album changes asked since, in order.
    pub fn take_album_edits(&mut self) -> Vec<AlbumEdit> {
        std::mem::take(&mut self.album_edits)
    }

    /// The vault query of the filter in force, its backup rule set.
    fn query_for(&self, filter: &Filter) -> Query {
        let mut query = query_of(filter);
        if let Some(b) = &mut query.backup {
            b.rule = self.backup_rule.clone();
        }
        query
    }

    /// Asks the host to run the filter's query again (the photos or what
    /// it measures changed); nothing without a filter.
    fn ask_query(&mut self) {
        if !self.filter.is_empty() {
            self.query = Some(self.query_for(&self.filter));
        }
    }

    /// A photo's backups, as last read.
    pub fn backup_of(&self, id: &str) -> Option<BackupMark> {
        self.backups.get(id).copied()
    }

    /// The volume an offline image is on, as the library names it.
    pub fn offline_volume(&self, id: &str) -> Option<VolumeNote> {
        let entry = self.entry(id)?;
        if entry.presence != helios_vault::Presence::Offline {
            return None;
        }
        let volume = entry.offline_on.as_deref()?;
        Some(
            self.volumes
                .get(volume)
                .cloned()
                .unwrap_or_else(|| VolumeNote {
                    name: volume.to_string(),
                    ..VolumeNote::default()
                }),
        )
    }

    /// What hovering an offline or missing image says: the volume it is
    /// on and when that was last seen, or that it is missing.
    pub fn presence_note(&self, id: &str) -> Option<Message> {
        match self.entry(id)?.presence {
            helios_vault::Presence::Present => None,
            helios_vault::Presence::Offline => Some(match self.offline_volume(id) {
                Some(v) => {
                    let (date, node) = v.seen();
                    Message::new(
                        "gui-library-offline-on",
                        "On { $volume }, last seen { $date } on { $node }",
                    )
                    .with("volume", v.name)
                    .with("date", date)
                    .with("node", node)
                }
                None => Message::new(
                    "gui-library-offline-unknown",
                    "On a disk that is not plugged in",
                ),
            }),
            helios_vault::Presence::Missing => Some(Message::new(
                "gui-library-missing-note",
                "Missing: its file is not where it was, and no other copy is known",
            )),
        }
    }

    /// Why an image cannot be opened, when a disk would help: "Plug in
    /// <volume> to open this photo", with its last-seen date. `None` for
    /// a present or missing image.
    pub fn unavailable(&self, id: &str) -> Option<Message> {
        let v = self.offline_volume(id)?;
        let (date, node) = v.seen();
        Some(
            Message::new(
                "gui-library-plug-in",
                "Plug in { $volume } to open this photo (last seen { $date } on { $node })",
            )
            .with("volume", v.name)
            .with("date", date)
            .with("node", node),
        )
    }

    /// The badges an image shows in the grid, in the interface's
    /// language: RAW, offline, missing, changed outside Helios, in the
    /// quick collection, its versions counted, its backups when the library has a strategy,
    /// and its culling measures (soft focus, blown highlights, crushed
    /// shadows) once measured.
    pub fn badges_of(&self, ctx: &egui::Context, id: &str) -> Vec<String> {
        let Some(&i) = self.index.get(id) else {
            return Vec::new();
        };
        let words = PresenceWords::of(ctx);
        let order = [i];
        let items = Items {
            entries: &self.entries,
            order: &order,
            stacks: false,
            revision: 0,
            words: &words,
            backups: (self.backup_badges == Shown::Yes).then_some(&self.backups),
            quick: &self.quick_set,
            heads: &self.head_counts,
        };
        items.badges(0).into_iter().map(|b| b.text).collect()
    }

    /// Hovering an offline or missing image in a view says why: the
    /// volume it is on, or that it is missing. (A cell's id is the view's
    /// own: egui-pop's library views name them so.)
    fn presence_hover(&self, ui: &Ui, view: &str) {
        let grid = egui::Id::unique(("egui_pop_views::library", LIBRARY, view));
        for e in &self.entries {
            if e.presence == helios_vault::Presence::Present {
                continue;
            }
            let Some(r) = ui.ctx().read_response(grid.with(("cell", &e.id))) else {
                continue;
            };
            if r.hovered()
                && let Some(note) = self.presence_note(&e.id)
            {
                r.on_hover_text(crate::i18n::text(ui.ctx(), note));
                return;
            }
        }
    }

    /// An image the library holds, by id.
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.index.get(id).and_then(|&i| self.entries.get(i))
    }

    /// The ids of the images in view, in order.
    pub fn ids(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.id.clone()).collect()
    }

    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        let selected = self
            .selected
            .and_then(|i| self.entries.get(i))
            .map(|e| e.id.clone());
        // A photo found changed outside Helios has a new thumbnail under
        // its key: its texture is read again.
        // A photo turned (an undo, another writer) shows its pictures
        // turned again.
        for e in &entries {
            let before = self.entry(&e.id).and_then(|old| old.changed_outside);
            let turned = self
                .entry(&e.id)
                .is_some_and(|old| old.rotation != e.rotation);
            if (e.changed_outside.is_some() && e.changed_outside != before) || turned {
                self.forget_pictures(&e.id);
            }
        }
        self.entries = entries;
        self.reindex();
        self.selected = selected.and_then(|id| self.index.get(&id).copied());
        // Photos came or went: the folders are counted again when shown.
        self.folders = FolderCounts::Unread;
        // Photos came or went: the vault answers the filter again.
        self.ask_query();
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

    /// Whether an image passes the filter: the vault matched it (every
    /// term runs in the vault's query). Until the vault answers a new
    /// filter, the last answer stands.
    fn passes(&self, e: &Entry) -> bool {
        self.filter.is_empty() || self.matches.as_ref().is_none_or(|m| m.contains(&e.id))
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

    /// The images the views show, and of them those picked and those
    /// rejected (a reject wins over a pick).
    pub fn counts(&self) -> (usize, usize, usize) {
        let (mut picked, mut rejected) = (0, 0);
        for &i in &self.order {
            match self.entries[i].flag() {
                Flag::Pick => picked += 1,
                Flag::Reject => rejected += 1,
                Flag::None => {}
            }
        }
        (self.order.len(), picked, rejected)
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
        self.query = (!filter.is_empty()).then(|| self.query_for(&filter));
        if filter.is_empty() {
            self.matches = None;
            self.facets = None;
        }
        self.filter = filter;
        self.reorder();
    }

    /// The order the photos are read in: this focus's sort.
    pub fn sort(&self) -> helios_vault::Order {
        self.sort
    }

    /// Sorts the library: the host reads it again in that order
    /// ([`Library::take_resort`]); the selection follows its photos.
    pub fn set_sort(&mut self, sort: helios_vault::Order) {
        if sort != self.sort {
            self.sort = sort;
            self.resort = Some(sort);
        }
    }

    /// Asks the host to read the photos again in the sort (they were read
    /// in another).
    pub fn ask_resort(&mut self) {
        self.resort = Some(self.sort);
    }

    /// Whether the photos must be read again in the sort, once.
    pub fn take_resort(&mut self) -> bool {
        self.resort.take().is_some()
    }

    /// The filter in force used (applied, a facet, a folder or an album
    /// chosen): first of the recent filters.
    pub fn record_filter(&mut self) {
        self.typed = None;
        let filter = self.filter.clone();
        if let Some(h) = &mut self.filter_history {
            if h.record(&filter) {
                self.filter_history_write = Wanted::Yes;
            }
        } else if !filter.is_empty() {
            self.used_early.retain(|f| *f != filter);
            self.used_early.push(filter);
        }
    }

    /// Filters the library with `filter`, used: recorded as recent.
    pub fn apply_filter(&mut self, filter: Filter) {
        self.set_filter(filter);
        self.record_filter();
    }

    /// The pinned and recent filters, once read.
    pub fn filter_history(&self) -> Option<&crate::recent_filters::FilterHistory> {
        self.filter_history.as_ref()
    }

    /// Whether the host is to read the history: once, at the start.
    pub fn take_filter_history_wanted(&mut self) -> bool {
        std::mem::replace(&mut self.filter_history_wanted, Wanted::No) == Wanted::Yes
    }

    /// The history as the vault keeps it; what was used before it was
    /// read goes on top.
    pub fn set_filter_history(&mut self, history: crate::recent_filters::FilterHistory) {
        let early = std::mem::take(&mut self.used_early);
        let merged = history.clone().merged(&early);
        if merged != history {
            self.filter_history_write = Wanted::Yes;
        }
        self.filter_history = Some(merged);
    }

    /// Pins a filter, or unpins it; whether that changed anything.
    pub fn pin_filter(&mut self, filter: &Filter, pinned: bool) -> bool {
        let history = self.filter_history.get_or_insert_with(Default::default);
        let changed = history.pin(filter, pinned);
        if changed {
            self.filter_history_write = Wanted::Yes;
        }
        changed
    }

    /// The history to write, once after it changed.
    pub fn take_filter_history_write(&mut self) -> Option<crate::recent_filters::FilterHistory> {
        (std::mem::replace(&mut self.filter_history_write, Wanted::No) == Wanted::Yes)
            .then(|| self.filter_history.clone())
            .flatten()
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
    pub fn thumbnails(&self) -> ThumbnailSink {
        self.sink.clone()
    }

    /// Told the thumbnails wanted, in the order wanted: the cells in view
    /// without a texture (one never received, or freed beyond the
    /// textures' budget), then a margin of a screen around them. The
    /// thumbnail pool reads those first, from its cache.
    /// The thumbnail cache the cells drawn without a texture are read from
    /// at once, on the frame that drew them (decoded on a few threads,
    /// within [`WARM_FRAME_BUDGET`]): a cached thumbnail shows the next
    /// frame, never after the pool's queue. Misses go to the pool
    /// ([`Library::set_on_view`]).
    pub fn set_thumb_cache(&mut self, cache: Option<Arc<helios_thumbs::ThumbCache>>) {
        self.warm = cache;
    }

    /// How long a frame may spend reading the cache for the cells drawn
    /// without a texture ([`WARM_FRAME_BUDGET`] by default).
    pub fn set_warm_budget(&mut self, budget: std::time::Duration) {
        self.warm_budget = budget;
    }

    pub fn set_on_view(&mut self, on_view: impl Fn(&[String]) + 'static) {
        self.on_view = Some(Box::new(on_view));
        // A new pool: what the last one was asked is asked again.
        self.textures.clear_requests();
        self.large_asked.clear();
    }

    /// The image map's feed: the files of the grid's and Compare's cells
    /// in view are told it, so the thumbnail re-check reads them first.
    pub fn set_map_feed(&mut self, feed: helios_library::map_job::Feed) {
        self.map_feed = Some(feed);
    }

    /// Asks the image map's job to list `folder` and those below it again
    /// ([`SCAN_FOLDER_COMMAND`]); false without the job (no vault).
    pub fn scan_folder(&self, folder: &Path) -> bool {
        let Some(feed) = &self.map_feed else {
            return false;
        };
        feed.scan_folder(folder.to_path_buf());
        true
    }

    /// Tells the map's feed the files in view when they changed: the
    /// grid's cells (`grid`) or Compare's panes; each image's file is the
    /// sighting the library reads it from.
    fn feed_in_view(&mut self, ids: Vec<String>, grid: bool) {
        let Some(feed) = &self.map_feed else {
            return;
        };
        let last = if grid {
            &mut self.fed.0
        } else {
            &mut self.fed.1
        };
        if *last == ids {
            return;
        }
        *last = ids;
        let paths: Vec<PathBuf> = self
            .fed
            .0
            .iter()
            .chain(&self.fed.1)
            .filter_map(|id| self.files.get(id).cloned())
            .collect();
        feed.on_screen(paths);
    }

    /// Asked an image's large thumbnail (the cache's 1024 px entry):
    /// Compare's first paint, before the render of its head version.
    pub fn set_on_large(&mut self, on_large: impl Fn(&str) + 'static) {
        self.on_large = Some(Box::new(on_large));
        self.large_asked.clear();
    }

    /// The thumbnails' textures budget, in bytes
    /// (`system.cache.textures`).
    pub fn set_texture_budget(&mut self, bytes: u64) {
        self.textures.set_budget(bytes);
    }

    /// The loupe's first frame: the large thumbnail (the cache's 1024 px
    /// entry) of an image being opened, handed over once it arrived (the
    /// library keeps no copy), asked of the pool the first time. The
    /// render of its head version Compare holds, when it does.
    pub fn take_large(&mut self, ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
        self.receive(ctx);
        if let Some(h) = self.heads.renders.get(id) {
            return Some(h.texture.clone());
        }
        if let Some(t) = self.large_textures.remove(id) {
            return Some(t);
        }
        self.ask_large(id);
        None
    }

    /// Asks the pool an image's large thumbnail, once (until it arrives).
    pub fn ask_large(&mut self, id: &str) {
        if let Some(on_large) = &self.on_large
            && self.large_asked.insert(id.to_string())
        {
            on_large(id);
        }
    }

    /// The large thumbnail Compare draws for an image until its render
    /// lands: its size, when one is held.
    pub fn large_thumbnail(&self, id: &str) -> Option<[usize; 2]> {
        self.large_textures.get(id).map(TextureHandle::size)
    }

    /// After a pass drawing thumbnails: asks those drawn without one (and,
    /// when the grid's cells in view changed, a margin around them), then
    /// frees what the budget cannot hold.
    fn after_pass(&mut self, ctx: &egui::Context) {
        let mut wanted = self.textures.take_missing();
        if let Some(cache) = self.warm.clone() {
            wanted = self.read_warm(ctx, &cache, wanted);
        }
        if self.view_changed.replace(false) {
            let margin = self.margin();
            wanted.extend(self.textures.ask(margin));
        }
        if !wanted.is_empty()
            && let Some(on_view) = &self.on_view
        {
            on_view(&wanted);
        }
        self.textures.evict(ctx.cumulative_pass_nr());
    }

    /// Reads the cells drawn without a texture from the cache, on a few
    /// threads, until the frame's budget is spent: each found becomes its
    /// texture now; those not reached are asked again on the next frame
    /// (a repaint is requested). The misses are returned, for the pool.
    fn read_warm(
        &mut self,
        ctx: &egui::Context,
        cache: &helios_thumbs::ThumbCache,
        ids: Vec<String>,
    ) -> Vec<String> {
        if ids.is_empty() {
            return ids;
        }
        let (found, missed, left) = read_cached(cache, ids, self.warm_budget);
        let pass = ctx.cumulative_pass_nr();
        for (id, rgb) in found {
            let texture = ctx.load_texture(
                format!("helios.library.{id}"),
                turned(rgb_image(&rgb), self.turns(&id)),
                egui::TextureOptions::LINEAR,
            );
            self.textures.insert(id, texture, false, pass);
        }
        for id in &left {
            self.textures.dropped(id);
        }
        ctx.request_repaint();
        missed
    }

    /// The images around the grid's cells in view, a screen's worth on
    /// each side, those after first.
    fn margin(&self) -> Vec<String> {
        let in_view = self.in_view.borrow();
        if in_view.is_empty() {
            return Vec::new();
        }
        let shown: HashSet<usize> = in_view
            .iter()
            .filter_map(|k| self.index.get(k).copied())
            .collect();
        let mut first = usize::MAX;
        let mut last = 0;
        for (pos, e) in self.order.iter().enumerate() {
            if shown.contains(e) {
                first = first.min(pos);
                last = last.max(pos);
            }
        }
        if first == usize::MAX {
            return Vec::new();
        }
        let m = in_view.len();
        let after = self.order.iter().skip(last + 1).take(m);
        let before = self.order[first.saturating_sub(m)..first].iter().rev();
        after
            .chain(before)
            .map(|&e| self.entries[e].id.clone())
            .collect()
    }

    /// An image's file type, for its placeholder: its file's extension,
    /// upper case (`?` when unknown).
    fn file_type(&self, id: &str) -> String {
        let name = self.index.get(id).map(|&i| self.entries[i].name.as_str());
        let from_file = self.files.get(id).and_then(|p| p.extension());
        from_file
            .or_else(|| name.and_then(|n| Path::new(n).extension()))
            .and_then(|e| e.to_str())
            .map_or_else(|| "?".to_string(), str::to_ascii_uppercase)
    }

    /// The size of the thumbnail an image's cell draws, turned as the
    /// photo is (width, height), once received.
    pub fn thumb_size(&self, id: &str) -> Option<[usize; 2]> {
        self.textures.get(id).map(egui::TextureHandle::size)
    }

    /// Whether this image's cell shows the placeholder (no preview).
    pub fn has_placeholder(&self, id: &str) -> bool {
        self.textures.is_shared(id)
            && self
                .textures
                .get(id)
                .is_some_and(|t| self.placeholders.values().any(|p| p.id() == t.id()))
    }

    /// Where renders of images' head versions are sent, from any thread.
    pub fn head_sink(&self) -> HeadSink {
        HeadSink {
            sender: self.heads.sender.clone(),
            wake: self.sink.wake.clone(),
        }
    }

    /// The renders of head versions the library wants since the last
    /// call: (image, longest side), for the host to run off the window's
    /// thread ([`crate::head_render::spawn`]) and send to
    /// [`Library::head_sink`].
    pub fn take_head_requests(&mut self) -> Vec<(String, u32)> {
        std::mem::take(&mut self.heads.wanted)
    }

    /// Asks a render of each raw among `keys` with no current one at
    /// `size` (its longest side, kept between 256 and 4096), unless one
    /// is on its way. A picture is drawn from its thumbnail, as before.
    pub fn ask_heads(&mut self, keys: &[String], size: u32) {
        let size = size.clamp(256, 4096);
        for k in keys {
            let raw = self.entry(k).is_some_and(|e| {
                let path = Path::new(&e.name);
                path.extension().is_some() && helios_load::Kind::of(path) == helios_load::Kind::Raw
            });
            let h = &mut self.heads;
            if !raw || h.pending.contains_key(k) || h.failed.contains(k) {
                continue;
            }
            let current = h.renders.get(k).is_some_and(|r| !r.stale && r.size >= size);
            if current {
                continue;
            }
            let generation = h.generations.get(k).copied().unwrap_or(0);
            h.pending.insert(k.clone(), generation);
            h.wanted.push((k.clone(), size));
        }
    }

    /// Takes the renders that landed; whether any did.
    pub fn receive_heads(&mut self, ctx: &egui::Context) -> bool {
        let mut any = false;
        while let Ok(r) = self.heads.receiver.try_recv() {
            any = true;
            let asked = self.heads.pending.remove(&r.id);
            let now = self.heads.generations.get(&r.id).copied().unwrap_or(0);
            match r.image {
                Ok(image) => {
                    let texture = ctx.load_texture(
                        format!("helios.library.head.{}", r.id),
                        turned(srgb(&image), self.turns(&r.id)),
                        egui::TextureOptions::LINEAR,
                    );
                    self.heads.renders.insert(
                        r.id,
                        HeadTexture {
                            version: r.version,
                            size: r.size,
                            texture,
                            stale: asked != Some(now),
                        },
                    );
                }
                Err(e) => {
                    self.notes.push(e);
                    self.heads.failed.insert(r.id);
                }
            }
        }
        any
    }

    /// The image's edit changed: its render is drawn until a new one of
    /// the new version lands.
    pub fn edit_changed(&mut self, id: &str) {
        *self.heads.generations.entry(id.to_string()).or_default() += 1;
        self.heads.failed.remove(id);
        if let Some(r) = self.heads.renders.get_mut(id) {
            r.stale = true;
        }
    }

    /// The workspace's network changed: every render is made again.
    pub fn network_changed(&mut self) {
        let ids: Vec<String> = self
            .heads
            .renders
            .keys()
            .chain(self.heads.pending.keys())
            .cloned()
            .collect();
        for id in ids {
            self.edit_changed(&id);
        }
        self.heads.failed.clear();
    }

    /// The render of an image's head version the Compare panel draws:
    /// the version it shows and its size, and whether it is stale.
    pub fn head_render(&self, id: &str) -> Option<(Option<&str>, [usize; 2], bool)> {
        self.heads
            .renders
            .get(id)
            .map(|r| (r.version.as_deref(), r.texture.size(), r.stale))
    }

    /// The grid's cells in view at its last pass, by id.
    pub fn in_view(&self) -> Vec<String> {
        self.in_view.borrow().clone()
    }

    /// The thumbnails' textures held (the placeholders, shared, left
    /// out), and their bytes.
    pub fn thumbnail_textures(&self) -> (usize, u64) {
        self.textures.usage()
    }

    /// The thumbnails' textures budget, in bytes.
    pub fn texture_budget(&self) -> u64 {
        self.textures.budget()
    }

    /// Whether this image's thumbnail is held (received, and not freed
    /// since).
    pub fn has_thumbnail(&self, id: &str) -> bool {
        self.textures.contains(id)
    }

    pub fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.changes)
    }

    /// Stacks the user expanded or collapsed since the last call:
    /// (stack id, expanded).
    pub fn take_expansions(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.expansions)
    }

    /// Whether the user asked to import from the workspace's source,
    /// once.
    pub fn take_import_asked(&mut self) -> bool {
        std::mem::take(&mut self.import_asked)
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

    /// Whether the Library holds the image `id`.
    pub fn holds(&self, id: &str) -> bool {
        self.index.contains_key(id)
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

    /// What a mark or a copy acts on: the selection, else the cursor's
    /// image.
    pub fn targets(&self) -> Result<Vec<String>, String> {
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
                Signal::Zoom { .. } | Signal::DragOut { .. } => {}
                Signal::Vetoed { step, reason } => self.notes.push(format!("{step}: {reason}")),
            }
        }
        if marked && !self.filter.is_empty() {
            // The vault answers again; what no longer matches goes now.
            self.query = Some(self.query_for(&self.filter));
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
            let turns = self.entries[i].rotation;
            self.entries[i].show(field, value);
            if self.entries[i].rotation != turns {
                self.forget_pictures(image);
            }
            if field == TAGS {
                self.reindex();
            }
        }
        if field == helios_vault::copies::CURRENT_VERSION {
            self.versions_changed();
            self.edit_changed(image);
        }
    }

    /// Drops an image's pictures (its thumbnail, its large preview, its
    /// head's render): asked again when drawn, from the thumbnails' cache.
    fn forget_pictures(&mut self, id: &str) {
        self.textures.forget(id);
        self.large_textures.remove(id);
        self.large_asked.remove(id);
        if self.heads.renders.remove(id).is_some() {
            let generation = self.heads.generations.entry(id.to_string()).or_default();
            *generation += 1;
        }
    }

    /// An image's quarter turns clockwise, as its pictures are shown.
    fn turns(&self, id: &str) -> u8 {
        self.entry(id).map_or(0, |e| e.rotation)
    }

    fn receive(&mut self, ctx: &egui::Context) {
        let wake = &self.sink.wake;
        if wake.ctx.get().is_none() {
            let _ = wake.ctx.set(ctx.clone());
        }
        // Cleared before the queue is drained: a thumbnail sent after the
        // drain sets it again and wakes the window.
        wake.pending.store(false, Ordering::SeqCst);
        let mut any = false;
        let pass = ctx.cumulative_pass_nr();
        while let Ok((id, arrival)) = self.receiver.try_recv() {
            // RGBA, as the texture holds it.
            let bytes = |w: u32, h: u32| u64::from(w) * u64::from(h) * 4;
            let picture = match arrival {
                Arrival::Linear(image) => thumbnail(&image),
                Arrival::Srgb(rgb) => {
                    if self.textures.contains(&id) {
                        // Read from the cache on the frame that drew it
                        // already: the pool's copy is the same picture.
                        self.textures.dropped(&id);
                        continue;
                    }
                    if !self.textures.accepts(&id, bytes(rgb.width, rgb.height)) {
                        // Out of view, beyond the budget: read again from
                        // the cache when its cell comes in view.
                        self.textures.dropped(&id);
                        continue;
                    }
                    rgb_image(&rgb)
                }
                Arrival::Large(rgb) => {
                    self.large_asked.remove(&id);
                    if !self.heads.renders.contains_key(&id) {
                        let texture = ctx.load_texture(
                            format!("helios.library.large.{id}"),
                            turned(rgb_image(&rgb), self.turns(&id)),
                            egui::TextureOptions::LINEAR,
                        );
                        self.large_textures.insert(id, texture);
                        any = true;
                    }
                    continue;
                }
                Arrival::NoPreview => {
                    let kind = self.file_type(&id);
                    let texture = self
                        .placeholders
                        .entry(kind.clone())
                        .or_insert_with(|| {
                            ctx.load_texture(
                                format!("helios.library.placeholder.{kind}"),
                                rgb_image(&helios_thumbs::placeholder(&kind)),
                                egui::TextureOptions::LINEAR,
                            )
                        })
                        .clone();
                    self.textures.insert(id, texture, true, pass);
                    any = true;
                    continue;
                }
            };
            let texture = ctx.load_texture(
                format!("helios.library.{id}"),
                turned(picture, self.turns(&id)),
                egui::TextureOptions::LINEAR,
            );
            self.textures.insert(id, texture, false, pass);
            any = true;
        }
        any |= self.receive_heads(ctx);
        if any {
            ctx.request_repaint();
        }
    }

    /// The search field; the sort and the count; auto-advance and
    /// Compare. Each group on a line of its own, so a narrow panel does
    /// not wrap the count away from what it counts.
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
        let import = self
            .import_offered
            .then(|| crate::i18n::text(&ctx, Message::new("gui-library-import", "Import…")));
        let compare = crate::i18n::text(&ctx, Message::new("gui-library-compare", "Compare"));
        let compare_hint = crate::i18n::text(
            &ctx,
            Message::new(
                "gui-library-compare-tip",
                "The selected photos side by side (C)",
            ),
        );
        let can_compare = self.state.selection().len() >= 2;
        let mut asked = None;
        // Wrapped, the field no wider than what is left of the line: a
        // narrow panel takes the bar on several lines rather than widen
        // its content past its edge (and cut what comes under it).
        ui.horizontal_wrapped(|ui| {
            if let Some(import) = import
                && ui.button(import).clicked()
            {
                self.import_asked = true;
            }
            // The field's box holds its icon and its "+" beside the text.
            let width = (ui.available_width() - SEARCH_BESIDE).clamp(80.0, 220.0);
            let mut field = SearchField::new(LIBRARY).label(search).width(width);
            if let Some(app) = &self.app {
                field = field.app(app);
            }
            asked = field.show(ui, &mut self.search).changed;
        });
        ui.horizontal_wrapped(|ui| {
            self.sort_menu(ui);
            ui.weak(count);
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.auto_advance, advance);
            if ui
                .add_enabled(can_compare, egui::Button::new(compare))
                .on_hover_text(compare_hint)
                .clicked()
            {
                crate::dialogs::request(
                    ui.ctx(),
                    crate::dialogs::Request::Run(
                        crate::comparison::COMMAND.into(),
                        Value::Null,
                        helios_shell::commands::Origin::User,
                    ),
                );
            }
        });
        let now = ctx.input(|i| i.time);
        if let Some(filter) = asked {
            self.typed = Some((filter.clone(), now));
            self.filtered(filter);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                crate::recent_filters::TYPED_REST,
            ));
        }
        // A filter typed counts as used once it rests.
        if self.typed.as_ref().is_some_and(|(f, at)| {
            now - at >= crate::recent_filters::TYPED_REST && *f == self.filter
        }) {
            self.record_filter();
        }
    }

    /// The facet counts of the filter in force (`docs/design/library.md`,
    /// L4), a line per facet: each value with how many of the photos
    /// shown carry it; a click adds it to the filter. Shown while a filter
    /// is in force, once the vault counted.
    fn facets_bar(&mut self, ui: &mut Ui) {
        if self.filter.is_empty() {
            return;
        }
        let Some(facets) = self.facets.clone() else {
            return;
        };
        let ctx = ui.ctx().clone();
        let mut add = None;
        for (facet, title) in [
            ("rating", Message::new("gui-library-facet-rating", "Rating")),
            ("label", Message::new("gui-library-facet-label", "Label")),
            ("flag", Message::new("gui-library-facet-flag", "Flag")),
            ("camera", Message::new("gui-library-facet-camera", "Camera")),
            ("lens", Message::new("gui-library-facet-lens", "Lens")),
            ("file_type", Message::new("gui-library-facet-type", "Type")),
            (
                "presence",
                Message::new("gui-library-facet-presence", "Presence"),
            ),
            ("backup", Message::new("gui-library-facet-backup", "Backup")),
            ("cull", Message::new("gui-library-facet-cull", "Culling")),
        ] {
            let values = facets.of(facet);
            if values.is_empty() {
                continue;
            }
            ui.horizontal_wrapped(|ui| {
                ui.weak(crate::i18n::text(&ctx, title));
                for (value, n) in values {
                    let shown = if facet == "rating" {
                        format!("{value}★")
                    } else if facet == "cull" {
                        crate::i18n::text(&ctx, cull_value(value))
                    } else {
                        value.to_string()
                    };
                    if ui.small_button(format!("{shown} {n}")).clicked() {
                        add = Some(facet_term(facet, value));
                    }
                }
            });
        }
        if let Some(term) = add {
            let filter = self.filter.merged(&Filter::parse(&term));
            self.apply_filter(filter);
        }
    }

    /// The Folders section (`docs/design/library.md`, L5): the folders of
    /// the Library's copies with how many photos are there or below,
    /// counted by the vault when the section opens; a click scopes the
    /// grid to a folder (its `folder:` term), a click on it again drops
    /// the scope.
    fn folders_section(&mut self, ui: &mut Ui) {
        if self.entries.is_empty() {
            return;
        }
        let title = crate::i18n::text(ui.ctx(), Message::new("gui-library-folders", "Folders"));
        let selected = self.folder();
        let mut chosen = None;
        egui::CollapsingHeader::new(title)
            .id_salt("helios.library.folders")
            .show(ui, |ui| {
                let Some(tree) = self.folder_tree() else {
                    // Counted once by the host, next frame.
                    // One frame more for the host's turn (no spinner: it
                    // would repaint on and on).
                    if matches!(self.folders, FolderCounts::Unread) {
                        self.folders = FolderCounts::Wanted;
                        ui.ctx().request_repaint();
                    }
                    return;
                };
                egui::ScrollArea::vertical()
                    .id_salt("helios.library.folders.scroll")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        for volume in &tree {
                            volume_row(ui, volume, selected.as_deref(), &mut chosen);
                        }
                    });
            });
        if let Some(path) = chosen {
            // Through the shell: the folder is kept with the workspace.
            let same = selected.as_deref() == Some(path.as_str());
            crate::dialogs::request(
                ui.ctx(),
                crate::dialogs::Request::Run(
                    FOLDER_COMMAND.into(),
                    serde_json::json!({ "path": (!same).then_some(path) }),
                    helios_shell::commands::Origin::User,
                ),
            );
        }
    }

    /// The Albums section (`docs/design/library.md`, L6): the vault's
    /// albums by name with how many photos each holds, "New album…"
    /// (from the selection, when there is one); a click scopes the grid to
    /// an album (its `collection:` term), a click on it again drops the
    /// scope; each row's menu adds or removes the selected photos,
    /// renames or deletes the album. The quick collection is pinned
    /// first (⚡, its count), its menu emptying it or saving it as an
    /// album.
    fn albums_section(&mut self, ui: &mut Ui) {
        self.drop_hover.set(None);
        if self.entries.is_empty() {
            return;
        }
        let ctx = ui.ctx().clone();
        let word = |m: Message| crate::i18n::text(&ctx, m);
        let selected = self.collection();
        let selection = self.targets().is_ok();
        let mut chosen = None;
        let mut create = false;
        let mut create_smart = false;
        let mut create_set = false;
        egui::CollapsingHeader::new(word(Message::new("gui-library-albums", "Albums")))
            .id_salt("helios.library.albums")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    create = ui
                        .button(word(Message::new("gui-library-album-new", "New album…")))
                        .clicked();
                    create_smart = ui
                        .button(word(Message::new(
                            "gui-library-album-new-smart",
                            "New smart album…",
                        )))
                        .clicked();
                    create_set = ui
                        .button(word(Message::new("gui-library-set-new", "New set…")))
                        .clicked();
                });
                // The quick collection, always there, pinned first.
                let quick = helios_vault::collections::QUICK;
                let count = self.quick_count();
                let name = word(Message::new("gui-library-quick", "Quick collection"));
                let r = ui
                    .selectable_label(
                        selected.as_deref() == Some(quick),
                        format!("⚡ {name} {count}"),
                    )
                    .on_hover_text(word(Message::new(
                        "gui-library-quick-tip",
                        "B adds the selected photos or takes them out",
                    )));
                quick_menu(&r, selection, count > 0);
                drop_target(&r, DropOn::Quick, None, &self.drop_hover);
                if r.clicked() {
                    chosen = Some(quick.to_string());
                }
                let Some(albums) = self.albums.as_deref() else {
                    return;
                };
                if albums.is_empty() {
                    ui.weak(word(Message::new(
                        "gui-library-albums-none",
                        "No album yet",
                    )));
                    return;
                }
                let tree = AlbumTree::of(albums);
                egui::ScrollArea::vertical()
                    .id_salt("helios.library.albums.scroll")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        self.album_rows(
                            ui,
                            &tree,
                            None,
                            selected.as_deref(),
                            selection,
                            &mut chosen,
                        );
                    });
            });
        let run = |command: &str, args: serde_json::Value| {
            crate::dialogs::request(
                ui.ctx(),
                crate::dialogs::Request::Run(
                    command.into(),
                    args,
                    helios_shell::commands::Origin::User,
                ),
            );
        };
        if create {
            run(COLLECTION_CREATE_COMMAND, serde_json::json!({}));
        }
        if create_set {
            run(
                COLLECTION_CREATE_COMMAND,
                serde_json::json!({ "kind": helios_vault::collections::SET }),
            );
        }
        if create_smart {
            run(
                COLLECTION_CREATE_COMMAND,
                serde_json::json!({ "kind": helios_vault::collections::SMART }),
            );
        }
        if let Some(id) = chosen {
            // Through the shell: the album is kept with the workspace.
            let same = selected.as_deref() == Some(id.as_str());
            run(
                COLLECTION_COMMAND,
                serde_json::json!({ "id": (!same).then_some(id) }),
            );
        }
    }

    /// The rows of the collections in the set `parent` (none: the top),
    /// a set as a group that opens and closes, its members indented
    /// below it.
    fn album_rows(
        &self,
        ui: &mut Ui,
        tree: &AlbumTree<'_>,
        parent: Option<&str>,
        selected: Option<&str>,
        selection: bool,
        chosen: &mut Option<String>,
    ) {
        let ctx = ui.ctx().clone();
        let word = |m: Message| crate::i18n::text(&ctx, m);
        for album in tree.members(parent) {
            let mark = if album.is_smart() { "⚙ " } else { "" };
            let text = format!("{mark}{} {}", album.name, self.album_count(album));
            let on = selected == Some(album.id.as_str());
            let movable = tree.has_sets_for(album);
            if !album.is_set() {
                let mut r = ui.selectable_label(on, text);
                if album.is_smart() {
                    r = r.on_hover_text(word(Message::new(
                        "gui-library-album-smart-tip",
                        "Smart album: its rules choose its photos",
                    )));
                }
                album_menu(&r, album, selection, movable);
                drop_target(
                    &r,
                    DropOn::Album(album.id.clone()),
                    drop_refusal(album),
                    &self.drop_hover,
                );
                if r.clicked() {
                    *chosen = Some(album.id.clone());
                }
                continue;
            }
            // A set: its toggle (named for screen readers), its row, its
            // members below when open.
            let key = ui.make_persistent_id(("helios.library.set", &album.id));
            let mut state =
                egui::collapsing_header::CollapsingState::load_with_default_open(&ctx, key, true);
            let open = state.is_open();
            ui.horizontal(|ui| {
                let toggle =
                    state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
                let name = word(
                    Message::new(
                        "gui-library-set-toggle",
                        "Show or hide the albums of { $name }",
                    )
                    .with("name", album.name.as_str()),
                );
                toggle.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::DisclosureTriangle, true, &name)
                });
                let r = ui
                    .selectable_label(on, text.clone())
                    .on_hover_text(word(Message::new(
                        "gui-library-set-tip",
                        "Album set: its albums' photos together",
                    )));
                album_menu(&r, album, selection, movable);
                drop_target(
                    &r,
                    DropOn::Album(album.id.clone()),
                    drop_refusal(album),
                    &self.drop_hover,
                );
                if r.clicked() {
                    *chosen = Some(album.id.clone());
                }
            });
            state.store(&ctx);
            if open {
                ui.indent(key, |ui| {
                    self.album_rows(ui, tree, Some(&album.id), selected, selection, chosen);
                });
            }
        }
    }

    /// The sort's menu and its direction: a change runs
    /// [`SORT_COMMAND`], which the workspace keeps.
    fn sort_menu(&mut self, ui: &mut Ui) {
        use helios_vault::{Order, Sort};
        let ctx = ui.ctx().clone();
        let word = |m: Message| crate::i18n::text(&ctx, m);
        let mut chosen = self.sort;
        let current = word(sort_label(self.sort.by));
        let tip = word(Message::new("gui-library-sort-tip", "Sort by"));
        let menu = egui::ComboBox::from_id_salt("helios.library.sort")
            .selected_text(current.clone())
            .show_ui(ui, |ui| {
                for by in Sort::ALL.into_iter().filter(|s| *s != Sort::Id) {
                    // Sharpness starts with the sharpest.
                    let descending = if by == Sort::Focus && self.sort.by != Sort::Focus {
                        true
                    } else {
                        self.sort.descending
                    };
                    ui.selectable_value(
                        &mut chosen,
                        Order { by, descending },
                        word(sort_label(by)),
                    );
                }
            })
            .response
            .on_hover_text(&tip);
        // Named for screen readers (and tests): "Sort by", its value the
        // sort chosen.
        menu.widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), &tip);
            info.current_text_value = Some(current.clone());
            info
        });
        let (arrow, tip) = if self.sort.descending {
            (
                "↓",
                Message::new(
                    "gui-library-sort-descending",
                    "Descending: newest, largest or Z first",
                ),
            )
        } else {
            (
                "↑",
                Message::new(
                    "gui-library-sort-ascending",
                    "Ascending: oldest, smallest or A first",
                ),
            )
        };
        let tip = word(tip);
        let flip = ui.add(egui::Button::new(arrow)).on_hover_text(&tip);
        flip.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &tip));
        if flip.clicked() {
            chosen.descending = !chosen.descending;
        }
        if chosen != self.sort {
            crate::dialogs::request(
                ui.ctx(),
                crate::dialogs::Request::Run(
                    SORT_COMMAND.into(),
                    serde_json::json!({ "by": chosen.by.name(), "descending": chosen.descending }),
                    helios_shell::commands::Origin::User,
                ),
            );
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
        ui.horizontal_wrapped(|ui| {
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
        self.versions_strip(ui, &entry.id);
        for m in asked {
            if let Err(e) = self.mark(m) {
                self.notes.push(e);
            }
        }
    }

    /// The Library panel: the search field, the cursor's marks, and the
    /// thumbnail grid with the culling keys.
    pub fn grid(&mut self, ui: &mut Ui) {
        self.receive(ui.ctx());
        // The search field (with the import button) and the marks bar
        // stay in place while the library is empty.
        self.toolbar(ui);
        self.facets_bar(ui);
        self.folders_section(ui);
        self.albums_section(ui);
        self.marks_bar(ui);
        if self.entries.is_empty() {
            let text = crate::i18n::text(
                ui.ctx(),
                Message::new(
                    "gui-library-empty",
                    "No photos yet: import a card or a folder, or drop photos on the window",
                ),
            );
            ui.add(egui::Label::new(egui::RichText::new(text).weak()).wrap());
            self.import_button(ui);
            return;
        }
        let label = crate::i18n::text(ui.ctx(), Message::new("gui-library-images", "Images"));
        let keymap = grid_keys();
        let words = PresenceWords::of(ui.ctx());
        let out = {
            let items = Items {
                entries: &self.entries,
                order: &self.order,
                stacks: self.filter.is_empty(),
                revision: self.revision,
                words: &words,
                backups: (self.backup_badges == Shown::Yes).then_some(&self.backups),
                quick: &self.quick_set,
                heads: &self.head_counts,
            };
            let thumbs = Thumbs {
                textures: &self.textures,
                pass: ui.ctx().cumulative_pass_nr(),
                in_view: Some(&self.in_view),
                changed: Some(&self.view_changed),
            };
            let mut grid = ThumbGrid::new("grid")
                .label(label)
                .keymap(keymap)
                .drag_out(true);
            if let Some(app) = &self.app {
                grid = grid.app(app);
            }
            grid.show(ui, &mut self.state, &items, &thumbs)
        };
        self.after_pass(ui.ctx());
        let in_view = self.in_view.borrow().clone();
        self.feed_in_view(in_view, true);
        carry_out(ui.ctx(), &out.signals);
        crate::name_rect(ui.ctx(), "helios.library.grid", out.response.rect);
        self.presence_hover(ui, "grid");
        // Its context menu, a secondary click anywhere on it (a cell
        // takes the click, the grid only its focus): what acts on the
        // images, the selection else what the library shows.
        let area = out.response.rect;
        if ui.rect_contains_pointer(area) && ui.input(|i| i.pointer.secondary_clicked()) {
            let at = ui
                .input(|i| i.pointer.interact_pos())
                .unwrap_or(area.center());
            crate::dialogs::request(
                ui.ctx(),
                crate::dialogs::Request::Menu {
                    at,
                    items: vec![
                        crate::dialogs::MenuItem::command("library.export"),
                        crate::dialogs::MenuItem::command("library.copyFiles"),
                        crate::dialogs::MenuItem::command("library.fitCameraLook"),
                        crate::dialogs::MenuItem::Separator,
                        crate::dialogs::MenuItem::command(ROTATE_LEFT_COMMAND).labeled(
                            crate::i18n::text(
                                ui.ctx(),
                                Message::new("gui-library-rotate-left", "Rotate Left"),
                            ),
                        ),
                        crate::dialogs::MenuItem::command(ROTATE_RIGHT_COMMAND).labeled(
                            crate::i18n::text(
                                ui.ctx(),
                                Message::new("gui-library-rotate-right", "Rotate Right"),
                            ),
                        ),
                        crate::dialogs::MenuItem::command(RESET_ROTATION_COMMAND).labeled(
                            crate::i18n::text(
                                ui.ctx(),
                                Message::new("gui-library-rotate-reset", "Reset Rotation"),
                            ),
                        ),
                        crate::dialogs::MenuItem::Separator,
                        crate::dialogs::MenuItem::command(DUPLICATE_COMMAND).labeled(
                            crate::i18n::text(
                                ui.ctx(),
                                Message::new("gui-library-duplicate", "Duplicate"),
                            ),
                        ),
                        crate::dialogs::MenuItem::command(DUPLICATE_ORIGINAL_COMMAND).labeled(
                            crate::i18n::text(
                                ui.ctx(),
                                Message::new(
                                    "gui-library-duplicate-original",
                                    "Duplicate Original",
                                ),
                            ),
                        ),
                        crate::dialogs::MenuItem::command(REMOVE_VERSION_COMMAND).labeled(
                            crate::i18n::text(
                                ui.ctx(),
                                Message::new("gui-library-remove-version", "Remove Version…"),
                            ),
                        ),
                        crate::dialogs::MenuItem::Separator,
                        crate::dialogs::MenuItem::command(REMOVE_COMMAND).labeled(
                            crate::i18n::text(
                                ui.ctx(),
                                Message::new("gui-library-remove", "Remove from Library"),
                            ),
                        ),
                        crate::dialogs::MenuItem::command(TRASH_COMMAND).labeled(
                            crate::i18n::text(
                                ui.ctx(),
                                Message::new("gui-library-trash", "Move to Trash…"),
                            ),
                        ),
                        crate::dialogs::MenuItem::command(DELETE_REJECTED_COMMAND).labeled(
                            crate::i18n::text(
                                ui.ctx(),
                                Message::new("gui-library-delete-rejected", "Delete Rejected…"),
                            ),
                        ),
                    ],
                },
            );
        }
        self.bind_clipboard(ui.ctx(), out.response.rect);
        self.handle(out.signals);
    }

    /// While the grid or the filmstrip has the focus, the Edit commands
    /// copy its images and paste into the library
    /// (`crate::clipboard::images`).
    fn bind_clipboard(&self, ctx: &egui::Context, area: egui::Rect) {
        if let Some(app) = &self.app
            && let Some(id) = crate::clipboard::focused_within(ctx, area)
        {
            crate::clipboard::images::focus(app, id);
        }
    }

    /// An empty library's way in, whether or not the workspace has an
    /// import source: "Import photos…" asks for a card or a folder.
    fn import_button(&mut self, ui: &mut Ui) {
        let label = crate::i18n::text(
            ui.ctx(),
            Message::new("gui-library-import-photos", "Import photos…"),
        );
        if ui.button(label).clicked() {
            self.import_asked = true;
        }
    }

    /// The Filmstrip panel: one row, the grid's selection.
    pub fn filmstrip(&mut self, ui: &mut Ui) {
        self.receive(ui.ctx());
        if self.entries.is_empty() {
            ui.horizontal_wrapped(|ui| {
                self.import_button(ui);
                ui.weak(crate::i18n::text(
                    ui.ctx(),
                    Message::new("gui-library-none", "No images"),
                ));
            });
            return;
        }
        let keymap = grid_keys();
        let height = ui.available_height().clamp(64.0, 200.0);
        let words = PresenceWords::of(ui.ctx());
        let out = {
            let items = Items {
                entries: &self.entries,
                order: &self.order,
                stacks: self.filter.is_empty(),
                revision: self.revision,
                words: &words,
                backups: (self.backup_badges == Shown::Yes).then_some(&self.backups),
                quick: &self.quick_set,
                heads: &self.head_counts,
            };
            let thumbs = Thumbs {
                textures: &self.textures,
                pass: ui.ctx().cumulative_pass_nr(),
                in_view: None,
                changed: None,
            };
            let mut strip = ThumbGrid::filmstrip("strip")
                .keymap(keymap)
                .height(height)
                .drag_out(true);
            if let Some(app) = &self.app {
                strip = strip.app(app);
            }
            strip.show(ui, &mut self.state, &items, &thumbs)
        };
        self.after_pass(ui.ctx());
        carry_out(ui.ctx(), &out.signals);
        crate::name_rect(ui.ctx(), "helios.library.strip", out.response.rect);
        self.presence_hover(ui, "strip");
        self.bind_clipboard(ui.ctx(), out.response.rect);
        self.handle(out.signals);
    }

    /// The Compare panel: the comparison's photos side by side (the
    /// selection's two to four when it has none chosen), each drawn from
    /// its edit, zoom and pan synchronised (a toggle), one active and
    /// outlined, its marks at hand, one marked best; keeping the best
    /// picks it and rejects the others.
    ///
    /// The comparison is the focus's ([`Comparison`]); with photos chosen
    /// (the Compare place), the selection is the active photo, so the
    /// culling keys mark it and a click in the filmstrip on one of them
    /// makes it active, as in Lightroom's Survey view.
    pub fn compare_ui(&mut self, ui: &mut Ui, cmp: &mut Comparison) {
        self.receive(ui.ctx());
        let selection = self.selection();
        let keys: Vec<String> = cmp
            .shown(&selection)
            .into_iter()
            .filter(|k| self.index.contains_key(k))
            .collect();
        if keys.len() < 2 {
            ui.weak(crate::i18n::text(
                ui.ctx(),
                Message::new(
                    "gui-library-compare-hint",
                    "Select two to four images to compare them",
                ),
            ));
            return;
        }
        let chosen = !cmp.photos().is_empty();
        // A photo of the comparison selected elsewhere becomes active.
        if chosen
            && let [one] = selection.as_slice()
            && let Some(i) = keys.iter().position(|k| k == one)
        {
            cmp.set_active(i);
        }
        let before = cmp.active_index(keys.len());
        self.feed_in_view(keys.clone(), false);
        let ctx = ui.ctx().clone();
        let sync = crate::i18n::text(&ctx, Message::new("gui-library-sync", "Sync zoom and pan"));
        let keep_text = crate::i18n::text(
            &ctx,
            Message::new(
                "gui-library-keep-best",
                "Keep the best: pick it, reject the others",
            ),
        );
        let best = cmp.best().map(String::from).filter(|b| keys.contains(b));
        let mut keep = false;
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut cmp.synced, sync);
            keep = ui
                .add_enabled(best.is_some(), egui::Button::new(keep_text))
                .clicked();
            ui.separator();
            // The active photo, chosen by its name (or the arrow keys).
            for (i, k) in keys.iter().enumerate() {
                let name = self.at(k).map_or(k.as_str(), |i| &self.entries[i].name);
                if ui
                    .selectable_label(i == cmp.active_index(keys.len()), name)
                    .clicked()
                {
                    cmp.set_active(i);
                }
            }
        });
        self.active_marks(ui, cmp.active(&keys).cloned());
        let panes: Vec<Pane<String>> = keys
            .iter()
            .filter_map(|k| {
                let i = self.at(k)?;
                Some(Pane::new(k.clone(), self.entries[i].name.clone()))
            })
            .collect();
        // Each raw drawn from its head version, rendered at about the
        // size of a pane.
        let pane = ui.available_size() * ui.ctx().pixels_per_point();
        let side = pane.x / keys.len() as f32;
        self.ask_heads(&keys, side.max(pane.y).ceil() as u32);
        self.ask_compare_large(&keys);
        let out = {
            let thumbs = CompareThumbs {
                heads: &self.heads.renders,
                large_textures: &self.large_textures,
                thumbs: Thumbs {
                    textures: &self.textures,
                    pass: ui.ctx().cumulative_pass_nr(),
                    in_view: None,
                    changed: None,
                },
            };
            CompareView::new("helios.compare")
                .synced(cmp.synced)
                .best(best.clone())
                .show(ui, &mut cmp.view, &panes, &thumbs)
        };
        let rect = out.response.rect;
        crate::name_rect(ui.ctx(), "helios.library.compare", rect);
        // The active photo outlined; a click in a pane makes it active.
        let gap = egui_pop::core::theme::Tokens::of(ui.ctx()).space.s;
        let areas = pane_rects(rect, panes.len(), gap);
        let clicked = ui.input(|i| {
            i.pointer
                .primary_clicked()
                .then(|| i.pointer.interact_pos())
                .flatten()
        });
        if let Some(at) = clicked
            && let Some(i) = areas.iter().position(|a| a.contains(at))
        {
            cmp.set_active(i);
        }
        let active = cmp.active_index(keys.len());
        if let Some(area) = areas.get(active) {
            let ring = egui_pop::core::theme::Tokens::of(ui.ctx()).focus_ring;
            ui.painter().rect_stroke(
                area.shrink(1.0),
                0.0,
                egui::Stroke::new(2.0, ring),
                egui::StrokeKind::Inside,
            );
            crate::name_rect(ui.ctx(), "helios.library.compare.active", *area);
        }
        self.after_pass(ui.ctx());
        if let Some(k) = out.best {
            cmp.set_best(Some(k));
        }
        if keep && let Some(best) = best {
            self.keep_best(&best, &keys);
        }
        // The selection follows the active photo of a chosen comparison.
        if chosen && active != before {
            self.select_ids(std::slice::from_ref(&keys[active]));
        }
    }

    /// The active photo's marks in the Compare panel: its name, stars,
    /// colour labels and flag, each changed on it alone.
    fn active_marks(&mut self, ui: &mut Ui, active: Option<String>) {
        let Some(entry) = active
            .and_then(|k| self.at(&k))
            .map(|i| self.entries[i].clone())
        else {
            return;
        };
        let marks = entry.marks();
        let keys = vec![entry.id.clone()];
        let rating = crate::i18n::text(
            ui.ctx(),
            Message::new("gui-compare-rating", "Active photo's rating"),
        );
        let mut asked = Vec::new();
        ui.horizontal_wrapped(|ui| {
            ui.strong(&entry.name);
            if let Some(rating) = RatingStars::new(marks.rating)
                .label(rating)
                .show(ui)
                .changed
            {
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
        for m in asked {
            if let Err(e) = self.mark(m) {
                self.notes.push(e);
            }
        }
    }

    /// Compare's first paint: the cache's large entry of each image shown
    /// with no render of its head version yet, asked once. The large
    /// entries of images no longer shown, or rendered, are freed.
    fn ask_compare_large(&mut self, keys: &[String]) {
        let renders = &self.heads.renders;
        self.large_textures
            .retain(|k, _| keys.contains(k) && !renders.contains_key(k));
        let Some(on_large) = &self.on_large else {
            return;
        };
        for k in keys {
            if !renders.contains_key(k)
                && !self.large_textures.contains_key(k)
                && self.large_asked.insert(k.clone())
            {
                on_large(k);
            }
        }
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

/// Where the Compare view puts `n` panes in `rect`, `gap` apart: side by
/// side up to three, four in a square (egui-pop's `CompareView` layout).
fn pane_rects(rect: egui::Rect, n: usize, gap: f32) -> Vec<egui::Rect> {
    let (cols, rows) = match n {
        0 | 1 => (1, 1),
        2 => (2, 1),
        3 => (3, 1),
        _ => (2, 2),
    };
    let w = (rect.width() - gap * (cols as f32 - 1.0)) / cols as f32;
    let h = (rect.height() - gap * (rows as f32 - 1.0)) / rows as f32;
    (0..n.min(cols * rows))
        .map(|i| {
            let (c, r) = ((i % cols) as f32, (i / cols) as f32);
            egui::Rect::from_min_size(
                rect.min + egui::vec2(c * (w + gap), r * (h + gap)),
                egui::vec2(w, h),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_pop::views::library::filter::Candidate;

    /// A picture 3 wide and 2 high turned each way: its corners land
    /// where a quarter turn puts them, and four turns give it back.
    #[test]
    fn a_picture_turns_by_quarters() {
        let px = |n: u8| Color32::from_rgb(n, 0, 0);
        // 1 2 3
        // 4 5 6
        let picture = ColorImage::new([3, 2], (1..=6).map(px).collect());
        let reds = |p: &ColorImage| p.pixels.iter().map(Color32::r).collect::<Vec<_>>();
        let right = turned(picture.clone(), 1);
        assert_eq!(right.size, [2, 3]);
        assert_eq!(reds(&right), [4, 1, 5, 2, 6, 3]);
        let left = turned(picture.clone(), 3);
        assert_eq!(left.size, [2, 3]);
        assert_eq!(reds(&left), [3, 6, 2, 5, 1, 4]);
        assert_eq!(reds(&turned(picture.clone(), 2)), [6, 5, 4, 3, 2, 1]);
        let back = (0..4).fold(picture.clone(), |p, _| turned(p, 1));
        assert_eq!(reds(&back), reds(&picture));
    }

    /// A date in the search field is a capture day: the vault query takes
    /// both ends whole, and an entry's day is its capture's by the
    /// camera's clock, not its file's.
    /// Helios's own keys come out of the search text into the vault
    /// query, quoted values whole, unknown values left as words; several
    /// `backup:` terms keep the states every one allows.
    #[test]
    fn helios_keys_become_query_terms() {
        use helios_vault::{BackupState as B, Presence};
        let q = query_of(&Filter::parse(
            "camera:\"Z 8\" lens:L1 TYPE:NEF folder:\"/a b/c\" presence:offline presence:soon sea backup:short backup:unverified",
        ));
        assert_eq!(q.cameras, ["Z 8"]);
        assert_eq!(q.lenses, ["L1"]);
        assert_eq!(q.file_types, ["nef"]);
        assert_eq!(q.folder.as_deref(), Some("/a b/c"));
        assert_eq!(q.presence, [Presence::Offline]);
        assert_eq!(q.words, ["presence:soon", "sea"]);
        assert_eq!(q.backup.unwrap().states, [B::Unverified]);
        assert_eq!(facet_term("camera", "Z 8"), "camera:\"Z 8\"");
        assert_eq!(facet_term("file_type", "nef"), "type:nef");
        assert_eq!(
            query_of(&Filter::parse(&facet_term("camera", "Z 8"))).cameras,
            ["Z 8"]
        );
    }

    #[test]
    fn cull_terms_and_badges_follow_the_measures() {
        let q = query_of(&Filter::parse("cull:soft cull:Clipped cull:blurry"));
        assert_eq!(q.cull, ["soft", "clipped"]);
        assert_eq!(q.words, ["cull:blurry"]);
        assert_eq!(facet_term("cull", "sharp"), "cull:sharp");
        let entry = |id: &str, focus: Option<f64>, white: Option<f64>, black: Option<f64>| {
            let mut e = Entry {
                id: id.into(),
                name: format!("{id}.jpg"),
                ..Entry::default()
            };
            e.show(cull::FOCUS, focus.map(Value::from).as_ref());
            e.show(cull::CLIP_WHITE, white.map(Value::from).as_ref());
            e.show(cull::CLIP_BLACK, black.map(Value::from).as_ref());
            e
        };
        let mut library = Library::default();
        library.set_entries(vec![
            entry("sharp", Some(0.02), Some(0.0), Some(0.0)),
            entry("soft", Some(0.0002), Some(0.0), Some(0.0)),
            entry("blown", Some(0.02), Some(0.06), Some(0.0)),
            entry("crushed", Some(0.02), Some(0.0), Some(0.2)),
            entry("all", Some(0.0001), Some(0.5), Some(0.5)),
            entry("none", None, None, None),
        ]);
        let ctx = egui::Context::default();
        let badges = |id: &str| library.badges_of(&ctx, id);
        assert!(badges("sharp").is_empty());
        assert_eq!(badges("soft"), ["soft focus"]);
        assert_eq!(badges("blown"), ["blown highlights"]);
        assert_eq!(badges("crushed"), ["crushed shadows"]);
        assert_eq!(
            badges("all"),
            ["soft focus", "blown highlights", "crushed shadows"]
        );
        assert!(badges("none").is_empty(), "not measured: no badge");
    }

    #[test]
    fn search_dates_are_capture_days_queried_in_the_vault() {
        use helios_vault::capture::day_start;
        let q = query_of(&Filter::parse("date:2024-06-01..2024-06-02 rating>=2"));
        assert_eq!(q.captured_from, day_start("2024-06-01"));
        assert_eq!(q.captured_before, day_start("2024-06-03"));
        assert_eq!(q.min_rating, Some(2));
        let open = query_of(&Filter::parse("from:2024-06-01"));
        assert_eq!(open.captured_before, None);
        let mut e = Entry {
            modified: Some(1_800_000_000),
            ..Entry::default()
        };
        assert_eq!(e.day, None, "no capture read: no day");
        let late = day_start("2024-06-01").unwrap() as f64 + 23.5 * 3600.0;
        e.show(CAPTURED, Some(&serde_json::json!(late)));
        assert_eq!(e.day.as_deref(), Some("2024-06-01"));
        assert!(
            Filter::parse("date:2024-06-01..2024-06-01").matches(&Candidate {
                name: &e.name,
                marks: e.marks(),
                tags: &e.tags,
                date: e.day.as_deref(),
            })
        );
    }

    #[test]
    fn the_files_in_view_feed_the_map_once_per_change() {
        let feed = helios_library::map_job::Feed::default();
        let mut library = Library::default();
        library.set_files([
            ("a".to_string(), PathBuf::from("/p/a.jpg")),
            ("b".to_string(), PathBuf::from("/p/b.jpg")),
            ("c".to_string(), PathBuf::from("/p/c.jpg")),
        ]);
        library.set_map_feed(feed.clone());
        library.feed_in_view(vec!["a".into(), "b".into()], true);
        let (changes, shown) = feed.shown();
        assert_eq!(
            shown,
            [PathBuf::from("/p/a.jpg"), PathBuf::from("/p/b.jpg")]
        );
        // The same cells again: nothing told.
        library.feed_in_view(vec!["a".into(), "b".into()], true);
        assert_eq!(feed.shown().0, changes);
        // Compare's panes join the grid's; an image with no file is left.
        library.feed_in_view(vec!["c".into(), "gone".into()], false);
        assert_eq!(
            feed.shown().1,
            [
                PathBuf::from("/p/a.jpg"),
                PathBuf::from("/p/b.jpg"),
                PathBuf::from("/p/c.jpg"),
            ]
        );
    }
}
