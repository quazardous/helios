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
//! tags; [`query_of`]), run by the host ([`Library::take_query`],
//! [`Library::set_matches`]); its words and dates are matched here, on the
//! names, tags and days the library holds.
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
use egui_pop::views::library::compare::{CompareState, CompareView, Pane};
use egui_pop::views::library::filter::{Candidate, Filter, SearchField, SearchState};
use egui_pop::views::library::marks::{ColorLabels, FlagToggle, RatingStars};
use egui_pop::views::library::tags::{TagInput, TagInputState};
use egui_pop::views::library::{
    Badge, Change as Mark, ColorLabel, CullAction, CullKeymap, Flag, LibraryItems, LibraryState,
    Marks, Signal, Stack, Thumb, ThumbGrid, ThumbSource, Tone as BadgeTone, change,
};
use helios_core::{Image, Message, Region};

use crate::head_render::HeadRender;
pub use helios_vault::fields::Op;
use helios_vault::fields::{LABELS as LABELS_FIELD, PICKED, Query, RATING, REJECTED, TAGS};
use serde_json::Value;

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
    /// The file's size in bytes, when it is on disk.
    pub bytes: Option<u64>,
    /// When the file was last modified, in seconds since 1970 (UTC); shown
    /// in the system's time zone.
    pub modified: Option<i64>,
    /// Its day (`YYYY-MM-DD`), for the search field's dates.
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
}

/// A volume as the library names it: its name, and when and on which
/// node it was last seen (seconds since the epoch).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VolumeNote {
    pub name: String,
    pub last_seen: Option<(u64, String)>,
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

    /// Whether `backup:<word>` in the search field takes it: `met`,
    /// `short` (short, unverified, or none while copies are asked),
    /// `unverified` and `none`; `None` for a word it does not know.
    fn wanted(word: &str, mark: Option<Self>) -> Option<bool> {
        Some(match word {
            "met" | "ok" => mark == Some(Self::Met),
            "short" => matches!(mark, Some(Self::Short | Self::Unverified | Self::None)),
            "unverified" => mark == Some(Self::Unverified),
            "none" => matches!(mark, Some(Self::None) | None),
            _ => return None,
        })
    }
}

/// The `backup:` words of a filter's text, apart: what is left of its
/// text, and the words.
fn backup_words(filter: &Filter) -> (Filter, Vec<String>) {
    let mut rest = filter.clone();
    let mut words = Vec::new();
    let kept: Vec<&str> = filter
        .text
        .split_whitespace()
        .filter(|w| {
            match w
                .to_lowercase()
                .strip_prefix("backup:")
                .filter(|b| BackupMark::wanted(b, None).is_some())
            {
                Some(b) => {
                    words.push(b.to_string());
                    false
                }
                None => true,
            }
        })
        .collect();
    rest.text = kept.join(" ");
    (rest, words)
}

pub struct Library {
    pub entries: Vec<Entry>,
    /// The image under the cursor, by its place in `entries`.
    pub selected: Option<usize>,
    /// Culling: after a mark, the cursor moves to the next image.
    pub auto_advance: bool,
    /// Marks a plugin refused, to say.
    pub notes: Vec<String>,
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
    tags: TagInputState,
    /// Every tag the library's images carry, for completion.
    known_tags: Vec<String>,
    compare: Comparison,
    /// The image marked best in the comparison.
    best: Option<String>,
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
            import_offered: false,
            import_asked: false,
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
            compare: Comparison {
                state: CompareState::new(),
                synced: true,
            },
            best: None,
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

/// The comparison's zoom and pan, and whether they follow each other.
struct Comparison {
    state: CompareState,
    synced: bool,
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
        if let Some(mark) = self.backups.and_then(|b| b.get(&entry.id)) {
            let w = self.words;
            badges.push(match mark {
                BackupMark::Met => Badge::new(w.backed_up.clone()).tone(BadgeTone::Success),
                BackupMark::Short => Badge::new(w.short.clone()).tone(BadgeTone::Warning),
                BackupMark::Unverified => Badge::new(w.unverified.clone()).tone(BadgeTone::Info),
                BackupMark::None => Badge::new(w.no_backup.clone()).tone(BadgeTone::Danger),
            });
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
        self.reorder();
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
                    last_seen: None,
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
    /// language: RAW, offline, missing, changed outside Helios, and its
    /// backups when the library has a strategy.
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

    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        let selected = self
            .selected
            .and_then(|i| self.entries.get(i))
            .map(|e| e.id.clone());
        // A photo found changed outside Helios has a new thumbnail under
        // its key: its texture is read again.
        for e in &entries {
            let before = self.entry(&e.id).and_then(|old| old.changed_outside);
            if e.changed_outside.is_some() && e.changed_outside != before {
                self.textures.forget(&e.id);
                self.large_textures.remove(&e.id);
                self.large_asked.remove(&e.id);
            }
        }
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

    /// Whether an image passes the filter (`filter`: its criteria but
    /// the `backup:` words, which `backup` holds).
    fn passes(&self, e: &Entry, filter: &Filter, backup: &[String]) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        let mark = self.backups.get(&e.id).copied();
        self.matches.as_ref().is_none_or(|m| m.contains(&e.id))
            && backup
                .iter()
                .all(|w| BackupMark::wanted(w, mark).unwrap_or(true))
            && filter.matches(&Candidate {
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
        let (filter, backup) = backup_words(&self.filter);
        let mut order = Vec::new();
        let mut placed: HashSet<&str> = HashSet::new();
        for (i, e) in self.entries.iter().enumerate() {
            if !self.passes(e, &filter, &backup) {
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
                rgb_image(&rgb),
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
                        srgb(&image),
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
                            rgb_image(&rgb),
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
                picture,
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
        let import = self
            .import_offered
            .then(|| crate::i18n::text(&ctx, Message::new("gui-library-import", "Import…")));
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
            };
            let thumbs = Thumbs {
                textures: &self.textures,
                pass: ui.ctx().cumulative_pass_nr(),
                in_view: Some(&self.in_view),
                changed: Some(&self.view_changed),
            };
            let mut grid = ThumbGrid::new("grid").label(label).keymap(keymap);
            if let Some(app) = &self.app {
                grid = grid.app(app);
            }
            grid.show(ui, &mut self.state, &items, &thumbs)
        };
        self.after_pass(ui.ctx());
        let in_view = self.in_view.borrow().clone();
        self.feed_in_view(in_view, true);
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
            };
            let thumbs = Thumbs {
                textures: &self.textures,
                pass: ui.ctx().cumulative_pass_nr(),
                in_view: None,
                changed: None,
            };
            let mut strip = ThumbGrid::filmstrip("strip").keymap(keymap).height(height);
            if let Some(app) = &self.app {
                strip = strip.app(app);
            }
            strip.show(ui, &mut self.state, &items, &thumbs)
        };
        self.after_pass(ui.ctx());
        crate::name_rect(ui.ctx(), "helios.library.strip", out.response.rect);
        self.presence_hover(ui, "strip");
        self.bind_clipboard(ui.ctx(), out.response.rect);
        self.handle(out.signals);
    }

    /// The Compare panel: the two to four images selected side by side,
    /// zoom and pan synced, one marked best; keeping the best picks it
    /// and rejects the others.
    pub fn compare_ui(&mut self, ui: &mut Ui) {
        self.receive(ui.ctx());
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
        let best = self.best.clone().filter(|b| keys.contains(b));
        let mut keep = false;
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.compare.synced, sync);
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
                .synced(self.compare.synced)
                .best(best.clone())
                .show(ui, &mut self.compare.state, &panes, &thumbs)
        };
        crate::name_rect(ui.ctx(), "helios.library.compare", out.response.rect);
        self.after_pass(ui.ctx());
        if let Some(k) = out.best {
            self.best = Some(k);
        }
        if keep && let Some(best) = best {
            self.keep_best(&best, &keys);
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

#[cfg(test)]
mod tests {
    use super::*;

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
