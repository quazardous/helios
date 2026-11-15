//! A value that arrives late (a size, a count served by a slow read),
//! drawn in its state, so the labels around it never wait for the data.

use std::fmt::Display;

use egui_pop::core::icon::Icon;
use egui_pop::core::indicator::Indicator;
use egui_pop::core::progress::Spinner;
use egui_pop::core::theme::Tokens;
use egui_pop::core::ui::{TextRole, Tone, UiPop as _};
use egui_pop::egui::{Response, Ui};

/// A value and where it stands. `since` is when the work started, in
/// motion clock seconds (`egui_pop::core::motion::now`): the spinner
/// shows only once the work has run longer than the theme's delay.
#[derive(Clone, Debug, PartialEq)]
pub enum Value<T> {
    /// Known and current.
    Fresh(T),
    /// Not known yet, being fetched.
    Pending { since: f64 },
    /// Known, being fetched again: the old value stays, muted.
    Refreshing { value: T, since: f64 },
    /// The fetch failed, for `reason`.
    Failed { reason: String },
}

impl<T> Value<T> {
    /// Its state's name, for the gallery and agents.
    pub fn state(&self) -> &'static str {
        match self {
            Value::Fresh(_) => "fresh",
            Value::Pending { .. } => "pending",
            Value::Refreshing { .. } => "refreshing",
            Value::Failed { .. } => "failed",
        }
    }
}

/// `value` in its state, `label` its name (the failed indicator's, and
/// the spinner's id; the screen draws the label itself):
/// the value in the body text role when fresh; a delayed spinner
/// (egui-pop's, at the inline icon size `size.icon-inline`) while pending; the old value in
/// the muted role and a delayed spinner while refreshing; a warning
/// indicator with the reason as its tooltip when failed.
pub fn value<T: Display>(ui: &mut Ui, label: &str, value: &Value<T>) -> Response {
    let id = ui.make_persistent_id(("helios_ui.value", label));
    let size = Tokens::of(ui.ctx()).size.icon_inline;
    let busy = |since: f64| Spinner::new().size(size).delayed(id, since, true);
    match value {
        Value::Fresh(v) => ui.text_role(TextRole::Body, v.to_string()),
        Value::Pending { since } => ui.add(busy(*since)),
        Value::Refreshing { value, since } => {
            ui.horizontal(|ui| {
                let r = ui.text_role(TextRole::Muted, value.to_string());
                ui.add(busy(*since));
                r
            })
            .inner
        }
        Value::Failed { reason } => Indicator::icon(Icon::Warning)
            .tone(Tone::Warning)
            .name(format!("{label}: failed"))
            .tooltip(reason.as_str())
            .show(ui),
    }
}
