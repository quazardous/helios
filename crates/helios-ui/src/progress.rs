//! How far a piece of work is: a bar when it says, a spinner when it
//! does not (never a fake bar).

use egui_pop::core::progress::{ProgressBar, Spinner};
use egui_pop::egui::{Response, TextStyle, Ui};

/// Where a progress shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Progress {
    /// In a row of a list, beside the row's other words: a short bar.
    Row,
    /// On its own line in a detail: the whole width, its name and its
    /// percentage written above the bar.
    Block,
}

impl Progress {
    pub const ALL: [Progress; 2] = [Progress::Row, Progress::Block];

    /// A row's bar, in lines of body text.
    const ROW_EM: f32 = 10.0;
}

/// The progress of the work named `name` (its accessible name): egui-pop's
/// progress bar at `fraction` (0 to 1), or its spinner when the work does
/// not say how far it is.
///
/// Need (state): a progress bar.
/// Need (state): a job's progress, a bar when it counts and a spinner when it does not.
pub fn progress(ui: &mut Ui, place: Progress, name: &str, fraction: Option<f32>) -> Response {
    let Some(fraction) = fraction else {
        return ui.add(Spinner::new().name(name));
    };
    let bar = ProgressBar::new(Some(fraction)).label(name);
    ui.add(match place {
        Progress::Row => bar.width(Progress::ROW_EM * ui.text_style_height(&TextStyle::Body)),
        Progress::Block => bar.caption(true),
    })
}
