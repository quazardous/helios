//! A place with nothing to show yet: empty (with why, and what to do),
//! loading (skeletons in the content's shape), failed (the reason, and a
//! retry).

use egui_pop::core::icon::Icon;
use egui_pop::core::placeholder::{EmptyState, Skeleton};
use egui_pop::egui::{Response, Ui};

/// The empty places of Helios's screens. Screens pick a case; they do not
/// write the sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Empty {
    /// The Library holds no photo.
    NoPhotos,
    /// A search matched no photo.
    NoMatch,
    /// No background job.
    NoJobs,
    /// Nothing waits in the queue.
    NothingQueued,
    /// No source to import from.
    NoSource,
}

impl Empty {
    pub const ALL: [Empty; 5] = [
        Empty::NoPhotos,
        Empty::NoMatch,
        Empty::NoJobs,
        Empty::NothingQueued,
        Empty::NoSource,
    ];

    /// Its name, for the gallery and agents.
    pub fn name(self) -> &'static str {
        match self {
            Empty::NoPhotos => "no-photos",
            Empty::NoMatch => "no-match",
            Empty::NoJobs => "no-jobs",
            Empty::NothingQueued => "nothing-queued",
            Empty::NoSource => "no-source",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Empty::NoPhotos | Empty::NoSource => Icon::Folder,
            Empty::NoMatch => Icon::Search,
            Empty::NoJobs | Empty::NothingQueued => Icon::History,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Empty::NoPhotos => "No photos yet",
            Empty::NoMatch => "No photo matches",
            Empty::NoJobs => "No background jobs",
            Empty::NothingQueued => "Nothing queued",
            Empty::NoSource => "No source yet",
        }
    }

    /// The sentence under the title: why, and what to do.
    pub fn text(self) -> &'static str {
        match self {
            Empty::NoPhotos => "Import photos to see them here.",
            Empty::NoMatch => "Try fewer words, or clear the search.",
            Empty::NoJobs => "Imports, exports and previews show here while they run.",
            Empty::NothingQueued => "Edits waiting to be rendered show here.",
            Empty::NoSource => "Add a folder, a card or an account to import from.",
        }
    }

    /// Its action's label, when it has one.
    pub fn action(self) -> Option<&'static str> {
        match self {
            Empty::NoPhotos => Some("Import photos…"),
            Empty::NoMatch => Some("Clear the search"),
            Empty::NoSource => Some("Add a source…"),
            Empty::NoJobs | Empty::NothingQueued => None,
        }
    }

    fn state(self) -> EmptyState {
        let state = EmptyState::new(self.icon(), self.title()).text(self.text());
        match self.action() {
            Some(label) => state.action(label),
            None => state,
        }
    }
}

/// The empty state of `case`: egui-pop's empty state (icon, title,
/// sentence, action). Whether its action was clicked; the screen runs
/// the command.
///
/// Need (state): an empty state, nothing here yet.
/// Need (state): no results for a search or a filter.
pub fn empty(ui: &mut Ui, case: Empty) -> bool {
    case.state().show(ui)
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
/// `title`, the `reason` and a Retry action. Whether Retry was clicked.
///
/// Need (state): a place that failed to load, with Retry.
pub fn failed(ui: &mut Ui, title: &str, reason: &str) -> bool {
    EmptyState::new(Icon::Error, title)
        .text(reason)
        .action("Retry")
        .danger()
        .show(ui)
}
