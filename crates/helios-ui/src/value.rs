//! A value worked out late (a volume's free space, a folder's size, a
//! count from the catalogue): egui-pop's late-value cell, sized by what
//! kind of value it is.

use std::fmt::Display;

pub use egui_pop::core::late::Late;
use egui_pop::core::late::LateValue;
use egui_pop::egui::Ui;

/// What kind of value a cell shows: it sets the room kept while the
/// value is pending, so nothing moves when it lands. Screens pick a kind;
/// they do not size the cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A size in bytes, written short ("412.7 GB").
    Size,
    /// A count ("12 480").
    Count,
    /// A date ("12 Oct 2026").
    Date,
    /// A duration ("01:12:30").
    Duration,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Size, Kind::Count, Kind::Date, Kind::Duration];

    /// The text as wide as the kind's values: the cell's room.
    pub fn sample(self) -> &'static str {
        match self {
            Kind::Size => "000.0 GB",
            Kind::Count => "000 000",
            Kind::Date => "00 Mmm 0000",
            Kind::Duration => "00:00:00",
        }
    }
}

/// A late value: its name (also its accessible name), its kind and where
/// it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Value<'a, T> {
    pub name: &'a str,
    pub kind: Kind,
    pub late: Late<T>,
}

impl<'a, T> Value<'a, T> {
    pub fn new(name: &'a str, kind: Kind, late: Late<T>) -> Self {
        Self { name, kind, late }
    }
}

/// The value in egui-pop's late-value cell: a skeleton sized for its kind
/// while pending, the value, the last value with a small spinner while
/// refreshing, the reason with Retry once failed. Whether Retry was
/// clicked; the screen runs the work again.
///
/// Need (state): a value worked out late (a size, a count), with its waiting states.
/// Need (state): a value that refreshes, its last value kept meanwhile.
pub fn value<T: Display>(ui: &mut Ui, value: Value<'_, T>) -> bool {
    LateValue::new(value.name, value.late)
        .sample(value.kind.sample())
        .show(ui)
        .retry
}
