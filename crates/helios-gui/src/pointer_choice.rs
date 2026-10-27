//! The pointer profile Helios starts with, and the first run's offer of
//! darktable's.
//!
//! Helios starts on Photoshop's profile: the wheel over a slider
//! scrolls the panel and a slider takes it only once clicked or focused,
//! so scrolling past sliders never changes a value (darktable's pitfall
//! 7, `docs/darktable-pitfalls/ui-platforms.md`). A profile the user
//! saved is kept; Settings › Input picks any.
//!
//! The first time the desktop starts with no profile saved, a small
//! notice at the bottom of the window asks "Coming from darktable?": one
//! button sets darktable's profile, the other keeps Photoshop's habits,
//! and the close button dismisses it. It never blocks the window, and
//! whatever the answer it is remembered with the interface's settings
//! ([`ASKED`]), so it shows once. Only the desktop offers it
//! (`Shell::offer_pointer_choice`; the shell draws it, as it draws its
//! popups): tests and tools never see it unless they ask.

use std::sync::PoisonError;

use egui_pop::api::AppHandle;
use egui_pop::model::commands::Origin;
use egui_pop::model::ids::OwnerId;
use egui_pop::model::values::{Change, Descriptor, Set, Value};
use helios_core::Message;

/// The pointer profile by default (egui-pop's `input.pointer_profile`).
pub const DEFAULT_PROFILE: &str = "photoshop";

/// darktable's profile, faithful to darktable.
pub const DARKTABLE_PROFILE: &str = "darktable";

/// Whether the first run's question was answered or dismissed: kept
/// with the interface's settings (saved once true), not shown in the
/// settings window.
pub const ASKED: &str = "settings/helios.pointer_profile_asked";

/// egui-pop's pointer profile setting.
const PROFILE_SETTING: &str = "settings/input.pointer_profile";

/// Who declares [`ASKED`].
const OWNER: OwnerId = OwnerId::from_static("helios.pointer_choice");

/// The notice's name, for its rectangle (`target_rect`).
pub const RECT: &str = "helios.pointer_choice";

/// Declares [`ASKED`] in the application's store (a value saved by an
/// earlier run, read before it was declared, is taken then).
pub fn declare(app: &AppHandle) -> Result<(), String> {
    let store = app.store();
    let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
    store
        .declare(
            &OWNER,
            ASKED,
            Descriptor::bool("Pointer profile asked", false),
        )
        .map_err(|e| e.to_string())
}

/// Whether the question is to be asked: never answered, and no profile
/// saved (the setting still at its default).
pub fn wanted(app: &AppHandle) -> bool {
    let store = app.store();
    let store = store.lock().unwrap_or_else(PoisonError::into_inner);
    let asked = store
        .get(ASKED)
        .is_none_or(|e| e.value.as_bool().unwrap_or(true));
    let saved = store
        .get(PROFILE_SETTING)
        .is_none_or(|e| e.value != e.descriptor.default);
    !asked && !saved
}

/// The answer: darktable's profile, or the default kept. Either way the
/// question is not asked again.
pub fn answer(app: &AppHandle, darktable: bool) -> Result<(), String> {
    if darktable {
        egui_pop::api::pointer::set_profile(app, DARKTABLE_PROFILE)?;
    }
    dismiss(app)
}

/// The question closed unanswered: not asked again.
pub fn dismiss(app: &AppHandle) -> Result<(), String> {
    let store = app.store();
    let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
    store
        .set(Set::new(ASKED, Change::To(Value::Bool(true)), Origin::User).untracked())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Draws the notice's body in `ui`: the shell places it, in an area of
/// its own over the panels that blocks nothing (`Shell::ui`, shown while
/// [`wanted`]). Applies what was clicked.
pub fn body(ui: &mut egui::Ui, app: &AppHandle) {
    let ctx = ui.ctx().clone();
    let ctx = &ctx;
    let title = crate::i18n::text(
        ctx,
        Message::new("gui-pointer-choice-title", "Coming from darktable?"),
    );
    let text = crate::i18n::text(
        ctx,
        Message::new(
            "gui-pointer-choice-text",
            "Use darktable's mouse and wheel habits: the wheel changes the slider under the pointer and zooms the image. You can change this later in Settings › Input.",
        ),
    );
    let use_darktable = crate::i18n::text(
        ctx,
        Message::new("gui-pointer-choice-darktable", "Use darktable's"),
    );
    let keep = crate::i18n::text(
        ctx,
        Message::new("gui-pointer-choice-keep", "Keep Photoshop-style"),
    );
    let close = crate::i18n::text(
        ctx,
        Message::new(
            "gui-pointer-choice-close",
            "Close the pointer profile notice",
        ),
    );
    let mut chosen: Option<bool> = None;
    let mut closed = false;
    ui.set_max_width(440.0);
    ui.horizontal(|ui| {
        ui.strong(&title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.small_button("✕").on_hover_text(&close);
            egui_pop::core::a11y::name(&r, egui::WidgetType::Button, &close);
            closed = r.clicked();
        });
    });
    ui.label(&text);
    ui.horizontal(|ui| {
        if ui.button(&use_darktable).clicked() {
            chosen = Some(true);
        }
        if ui.button(&keep).clicked() {
            chosen = Some(false);
        }
    });
    let done = match chosen {
        Some(darktable) => answer(app, darktable),
        None if closed => dismiss(app),
        None => Ok(()),
    };
    if let Err(e) = done {
        log::warn!("pointer choice: {e}");
    }
}
