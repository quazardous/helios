//! A place with nothing to show yet: empty (with why, and what to do),
//! loading (skeletons in the content's shape), failed (the reason, and a
//! retry).

use egui_pop::core::icon::Icon;
use egui_pop::core::placeholder::{EmptyState, Skeleton};
use egui_pop::core::strings::{self, StrKey};
use egui_pop::egui::{Context, Response, Ui};

/// The empty places of Helios's screens. Screens pick a case; they do not
/// write the sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Empty {
    /// The Library holds no photo.
    NoPhotos,
    /// A search matched no photo.
    NoMatch,
    /// The album shown holds no photo (a workspace may open on one).
    EmptyAlbum,
    /// No background job.
    NoJobs,
    /// Nothing waits in the queue.
    NothingQueued,
    /// No source to import from.
    NoSource,
}

impl Empty {
    pub const ALL: [Empty; 6] = [
        Empty::NoPhotos,
        Empty::NoMatch,
        Empty::EmptyAlbum,
        Empty::NoJobs,
        Empty::NothingQueued,
        Empty::NoSource,
    ];

    /// Its name, for the gallery and agents.
    pub fn name(self) -> &'static str {
        match self {
            Empty::NoPhotos => "no-photos",
            Empty::NoMatch => "no-match",
            Empty::EmptyAlbum => "empty-album",
            Empty::NoJobs => "no-jobs",
            Empty::NothingQueued => "nothing-queued",
            Empty::NoSource => "no-source",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Empty::NoPhotos | Empty::NoSource | Empty::EmptyAlbum => Icon::Folder,
            Empty::NoMatch => Icon::Search,
            Empty::NoJobs | Empty::NothingQueued => Icon::History,
        }
    }

    /// Its title, in English ([`Empty::title_key`] translates it).
    pub fn title(self) -> &'static str {
        self.title_key().fallback
    }

    /// The sentence under the title, in English: why, and what to do
    /// ([`Empty::text_key`] translates it).
    pub fn text(self) -> &'static str {
        self.text_key().fallback
    }

    /// Its action's label in English, when it has one
    /// ([`Empty::action_key`] translates it).
    pub fn action(self) -> Option<&'static str> {
        self.action_key().map(|k| k.fallback)
    }

    /// Its title's key in the catalogues (`messages.ftl`), with its
    /// English.
    pub fn title_key(self) -> StrKey {
        match self {
            Empty::NoPhotos => StrKey::new("ui-empty-no-photos", "No photos yet"),
            Empty::NoMatch => StrKey::new("ui-empty-no-match", "No photo matches"),
            Empty::EmptyAlbum => StrKey::new("ui-empty-empty-album", "This album is empty"),
            Empty::NoJobs => StrKey::new("ui-empty-no-jobs", "No background jobs"),
            Empty::NothingQueued => StrKey::new("ui-empty-nothing-queued", "Nothing queued"),
            Empty::NoSource => StrKey::new("ui-empty-no-source", "No source yet"),
        }
    }

    /// Its sentence's key in the catalogues, with its English.
    pub fn text_key(self) -> StrKey {
        match self {
            Empty::NoPhotos => {
                StrKey::new("ui-empty-no-photos-text", "Import photos to see them here.")
            }
            Empty::NoMatch => StrKey::new(
                "ui-empty-no-match-text",
                "Try fewer words, or clear the search.",
            ),
            Empty::EmptyAlbum => StrKey::new(
                "ui-empty-empty-album-text",
                "The Library shows this album only, and it holds no photo.",
            ),
            Empty::NoJobs => StrKey::new(
                "ui-empty-no-jobs-text",
                "Imports, exports and previews show here while they run.",
            ),
            Empty::NothingQueued => StrKey::new(
                "ui-empty-nothing-queued-text",
                "Edits waiting to be rendered show here.",
            ),
            Empty::NoSource => StrKey::new(
                "ui-empty-no-source-text",
                "Add a folder, a card or an account to import from.",
            ),
        }
    }

    /// Its action's key in the catalogues, with its English, when it has
    /// one.
    pub fn action_key(self) -> Option<StrKey> {
        match self {
            Empty::NoPhotos => Some(StrKey::new("ui-empty-no-photos-action", "Import photos…")),
            Empty::NoMatch => Some(StrKey::new("ui-empty-no-match-action", "Clear the search")),
            Empty::EmptyAlbum => Some(StrKey::new(
                "ui-empty-empty-album-action",
                "Show all photos",
            )),
            Empty::NoSource => Some(StrKey::new("ui-empty-no-source-action", "Add a source…")),
            Empty::NoJobs | Empty::NothingQueued => None,
        }
    }

    /// Its empty state, in the language `ctx` shows (egui-pop's strings:
    /// the translation plugin when the application installs one, else
    /// English).
    fn state(self, ctx: &Context) -> EmptyState {
        let state = EmptyState::new(self.icon(), strings::tr(ctx, &self.title_key()))
            .text(strings::tr(ctx, &self.text_key()));
        match self.action_key() {
            Some(key) => state.action(strings::tr(ctx, &key)),
            None => state,
        }
    }
}

/// The empty state of `case`: egui-pop's empty state (icon, title,
/// sentence, action), in the language the application shows. Whether its
/// action was clicked; the screen runs the command.
///
/// Need (state): an empty state, nothing here yet.
/// Need (state): no results for a search or a filter.
/// Need (state): an empty album, with the way back to all photos.
pub fn empty(ui: &mut Ui, case: Empty) -> bool {
    let state = case.state(ui.ctx());
    state.show(ui)
}

/// The shape of what is loading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Loading {
    /// Lines of text (a description, a detail panel).
    Text(usize),
    /// Rows of a list or a table.
    Rows(usize),
}

/// What loads, shown in its shape: egui-pop's skeleton, busy for
/// assistive technologies and named "Loading".
///
/// Need (state): a value or a list still loading.
/// Need (state): placeholder shapes while content loads.
pub fn loading(ui: &mut Ui, shape: Loading) -> Response {
    let skeleton = match shape {
        Loading::Text(lines) => Skeleton::text(lines),
        Loading::Rows(rows) => Skeleton::rows(rows),
    };
    ui.add(skeleton)
}

/// A place that failed to load: egui-pop's empty state in danger, with
/// `title`, the `reason` and a Retry action (egui-pop's translated
/// "Retry"). Whether Retry was clicked.
///
/// Need (state): a place that failed to load, with Retry.
pub fn failed(ui: &mut Ui, title: &str, reason: &str) -> bool {
    EmptyState::new(Icon::Error, title)
        .text(reason)
        .action(strings::tr(ui.ctx(), &strings::RETRY))
        .danger()
        .show(ui)
}
