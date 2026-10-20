//! Helios's global items in the title bar (egui-pop's `CAPTION` zone,
//! before the window's buttons), which stay in full screen with the bar:
//!
//! - the jobs going on: a ring with their overall estimate (turning
//!   while one cannot estimate), a cross when some failed; a click opens
//!   the Jobs panel;
//! - the decisions waiting for an answer (the stack suggestions): a
//!   count on a bell; a click opens the Stacks panel;
//! - the profile in use: a click opens the Profiles menu, to switch.
//!
//! Each is drawn only when it has something to say (the profile always).
//! The shell sets what the jobs centre does not know (the decisions, the
//! profile) in its status line each frame. Each mark has an id that
//! egui-pop's `indicators` collector reports, so a test or an agent reads
//! its state.

use egui::Ui;
use egui_pop::api::AppHandle;
use egui_pop::api::jobs::{JOBS, status_text};
use egui_pop::api::jobs_centre::PANEL_JOBS;
use egui_pop::api::menus::popup_at;
use egui_pop::core::icon::Icon;
use egui_pop::core::indicator::{Anchor, Indicator, IndicatorExt as _, Pip};
use egui_pop::model::ids::{BlockId, PanelId};
use helios_core::Message;

use crate::i18n::text;
use crate::jobs::Status;

/// The block of the title bar's `CAPTION` zone.
pub const BLOCK: BlockId = BlockId::from_static("helios.frame.caption");

/// The jobs' mark (`indicators` collector).
pub const JOBS_MARK: &str = "helios.caption.jobs";
/// The decisions' count.
pub const DECISIONS_MARK: &str = "helios.caption.decisions";
/// The profile's button (a named rectangle).
pub const PROFILE: &str = "helios.caption.profile";

/// Draws the items, right to left as the zone lays them out: the profile
/// next to the window's buttons, then the decisions, then the jobs.
pub fn block(status: Status) -> impl FnMut(&mut Ui, &AppHandle) + 'static {
    move |ui, app| {
        let (decisions, profile) = {
            let line = status.borrow();
            (line.decisions, line.profile.clone())
        };
        profile_item(ui, app, &profile);
        decisions_item(ui, app, decisions);
        jobs_item(ui, app);
    }
}

fn profile_item(ui: &mut Ui, app: &AppHandle, profile: &str) {
    if profile.is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    let r = ui
        .add(egui::Button::new(egui::RichText::new(profile).small()).frame(false))
        .on_hover_text(text(
            &ctx,
            Message::new("gui-caption-profile", "Profile in use: { $name }")
                .with("name", profile.to_string()),
        ));
    crate::name_rect(&ctx, PROFILE, r.rect);
    if r.clicked() {
        popup_at(app, &crate::plugins::SYSTEM_PROFILES, r.rect);
    }
}

fn decisions_item(ui: &mut Ui, app: &AppHandle, decisions: usize) {
    if decisions == 0 {
        return;
    }
    let ctx = ui.ctx().clone();
    let action = text(
        &ctx,
        Message::new("gui-caption-decisions", "Decisions waiting"),
    );
    let meaning = text(
        &ctx,
        Message::new(
            "gui-caption-decisions-count",
            "{ $count } stack suggestions waiting for an answer",
        )
        .with("count", decisions),
    );
    let r = ui
        .add(Icon::Notification.label(&action).sense_click())
        .on_hover_text(&meaning)
        .with_indicator(
            ui,
            Indicator::count(decisions as u64)
                .id(DECISIONS_MARK)
                .name(meaning),
            Anchor::TopEnd,
        );
    if r.clicked() {
        let _ = app.focus_panel(&PanelId::from_static(crate::stack_review::PANEL));
    }
}

fn jobs_item(ui: &mut Ui, app: &AppHandle) {
    let Some(jobs) = app.service(&JOBS) else {
        return;
    };
    let summary = jobs.summary();
    if summary.active == 0 && summary.failed == 0 {
        return;
    }
    let ctx = ui.ctx().clone();
    let (active, failed) = status_text(&ctx, &summary);
    let pip = if summary.active > 0 {
        summary
            .fraction
            .map_or(Pip::Busy, |f| Pip::Progress(f as f32))
    } else {
        Pip::Failed
    };
    let meaning = match failed {
        Some(failed) if summary.active > 0 => format!("{active}, {failed}"),
        Some(failed) => failed,
        None => active,
    };
    let r = ui.add(
        Indicator::pip(pip)
            .id(JOBS_MARK)
            .name(meaning.clone())
            .tooltip(meaning)
            .interactive(text(
                &ctx,
                Message::new("gui-caption-jobs", "Show the jobs"),
            )),
    );
    if r.clicked() {
        let _ = app.focus_panel(&PANEL_JOBS);
    }
}
