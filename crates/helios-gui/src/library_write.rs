//! What the Library asks written in the vault, on the bus: a request
//! asked while the Library draws or runs a command, heard after the
//! drawing in the same pass.
//!
//! The Library holds no vault. A photo removed, turned, duplicated or
//! trashed, a stack grouped, a keyword renamed, metadata set, files
//! paired, a version switched or removed, an album changed, a stack
//! expanded, the metadata templates changed: each is asked on
//! [`LibraryWrite`] (`library.write`) with the undo batch it was asked
//! in, through bus-contract's `Defer` ([`ask`]): the
//! Library is borrowed while it draws, so its listener runs after the
//! drawing (egui-pop drains the asks once per pass, and before it
//! quits). The listener, the host's (`persist.library`), adds each to
//! the writes the node runs in turn, in ask order, so an undo batch made
//! of several asks stays in one queue in the order it was made.
//!
//! The asker gets nothing back: what the write did is said by the
//! library's events (`library.changed`) and the history's steps.

use egui_pop::api::bus::bus_contract::{self, Bus, Point, PointDecl, PointKind, Refused};

use crate::library::{
    AlbumEdit, Batch, KeywordAsk, MetaEdit, PairAsk, StackAsk, Template, TrashAsk, Turn, VersionAsk,
};

/// The request a Library write is asked on.
pub const WRITE: &str = "library.write";

/// A write the Library asks of the vault.
#[derive(Clone, Debug, PartialEq)]
pub enum WriteAsk {
    /// Photos taken out of the Library (`library.images.remove`).
    Remove(Vec<String>, Batch),
    /// Photos turned (`library.images.rotate`).
    Rotate(Vec<String>, Turn, Batch),
    /// Photos duplicated (`library.images.duplicate`).
    Duplicate(Vec<String>, helios_vault::versions::DuplicateFrom),
    /// Photos' files to the trash, confirmed or to ask first.
    Trash(TrashAsk),
    /// Photos grouped, ungrouped, a stack's top set.
    Stack(StackAsk, Batch),
    /// A keyword branch renamed or merged.
    Keyword(KeywordAsk, Batch),
    /// Photos' metadata set as a group.
    Meta(MetaEdit, Batch),
    /// RAW+JPEG files paired or unpaired.
    Pair(PairAsk, Batch),
    /// A version switched or removed (confirmed, or to ask first).
    Version(VersionAsk, Batch),
    /// An album changed.
    Album(AlbumEdit, Batch),
    /// A stack expanded or collapsed.
    Expand(String, bool),
    /// The metadata templates, written back.
    Templates(Vec<Template>),
}

/// The request a Library write is asked on: heard after the drawing.
pub struct LibraryWrite;

impl Point for LibraryWrite {
    const ID: &'static str = WRITE;
    type Input = WriteAsk;
    type Value = ();
    const KIND: PointKind = PointKind::Request;
}

/// The Library's write point, its contract with the application's bus:
/// declared by [`declare`].
pub const POINTS: &[PointDecl] = &[PointDecl::of::<LibraryWrite>(
    "A write the Library asks of the vault (remove, turn, duplicate, trash, stacks, keywords, metadata, pairs, versions, albums, expansions, templates), with its undo batch: heard after the drawing, sent to the node in ask order",
)
.fail_closed()];

/// Declares [`POINTS`] on `bus` (once).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "library: {refused}");
        }
    }
}

/// Asks `write` on `bus`, heard after the drawing; on a bus without
/// `Defer`, at once. Fail-closed (the point is declared so): a listener
/// that fails refuses the write, and egui-pop's drain logs it.
pub fn ask(bus: &dyn Bus, write: WriteAsk) {
    declare(bus);
    match bus.defer() {
        Some(defer) => {
            defer.ask::<LibraryWrite>(write, ());
        }
        None => {
            let out = bus.alter::<LibraryWrite>(write, ());
            if let Some(r) = out.refused {
                log::warn!(target: "helios", "{r}");
            }
        }
    }
}
