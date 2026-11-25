//! Helios's global items in the title bar (egui-pop's `CAPTION` zone,
//! before the window's buttons), which stay in full screen with the bar:
//!
//! - the background tasks going on, as the node's promises (the
//!   status bar's Activity summary, `crate::activity`): a ring with the
//!   main one's progress (turning while it cannot say), a still pause
//!   mark when every task left is held (a service's quota, a refused
//!   sign-in), a cross when some failed unseen; a click opens the Activity focus on that task, as the
//!   status bar's item does;
//! - the decisions waiting for an answer (the stack suggestions): a
//!   count on a bell; a click opens the Stacks panel;
//! - the profile in use: a click opens the Profiles menu, to switch.
//!
//! Each is drawn only when it has something to say (the profile always).
//! The shell sets what they show (the activity, the decisions, the
//! profile) in its status line each frame. Each mark has an id that
//! egui-pop's `indicators` collector reports, so a test or an agent reads
//! its state.

use egui::Ui;
use egui_pop::api::AppHandle;
use egui_pop::api::menus::popup_at;
use egui_pop::core::icon::Icon;
use egui_pop::core::indicator::{Anchor, Indicator, IndicatorExt as _, Pip};
use egui_pop::core::tooltip::TooltipExt as _;
use egui_pop::core::widgets;
use egui_pop::model::ids::{BlockId, CommandId, PanelId};
use helios_core::Message;
use helios_vault::promises::State;
use serde_json::json;

use crate::i18n::text;
use crate::jobs::Status;

/// The block of the title bar's `CAPTION` zone.
pub const BLOCK: BlockId = BlockId::from_static("helios.frame.caption");

/// The background tasks' mark (`indicators` collector).
pub const JOBS_MARK: &str = "helios.caption.jobs";
/// The decisions' count.
pub const DECISIONS_MARK: &str = "helios.caption.decisions";
/// The profile's button (a named rectangle).
pub const PROFILE: &str = "helios.caption.profile";

/// Draws the items, right to left as the zone lays them out: the profile
/// next to the window's buttons, then the decisions, then the jobs.
pub fn block(status: Status) -> impl FnMut(&mut Ui, &AppHandle) + 'static {
    move |ui, app| {
        let (decisions, profile, activity) = {
            let line = status.borrow();
            (line.decisions, line.profile.clone(), line.activity.clone())
        };
        profile_item(ui, app, &profile);
        decisions_item(ui, app, decisions);
        jobs_item(ui, app, activity.as_ref());
    }
}

fn profile_item(ui: &mut Ui, app: &AppHandle, profile: &str) {
    if profile.is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    let r = widgets::bar_text_button(ui, profile).tip(text(
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
        .tip(meaning.as_str())
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

/// The background tasks' mark, from the status bar's Activity summary:
/// nothing while none runs and none failed unseen.
fn jobs_item(ui: &mut Ui, app: &AppHandle, activity: Option<&crate::activity::Summary>) {
    let Some(summary) = activity else {
        return;
    };
    let ctx = ui.ctx().clone();
    let main = summary.running.first();
    let held = summary.paused.first();
    let mark = match (main.map(|p| &p.state), held) {
        (
            Some(State::Running {
                done,
                total: Some(total),
                ..
            }),
            _,
        ) if *total > 0 => Indicator::pip(Pip::Progress((*done as f32 / *total as f32).min(1.0))),
        (Some(_), _) => Indicator::pip(Pip::Busy),
        // Every task left is held: no turning ring.
        (None, Some(_)) => {
            Indicator::icon("phosphor:pause").tone(egui_pop::core::indicator::Tone::Warning)
        }
        (None, None) => Indicator::pip(Pip::Failed),
    };
    let running = (!summary.running.is_empty()).then(|| {
        text(
            &ctx,
            Message::new(
                "gui-activity-running",
                "{ $count } background tasks running",
            )
            .with("count", summary.running.len()),
        )
    });
    let paused = (!summary.paused.is_empty()).then(|| {
        let said = text(
            &ctx,
            Message::new("gui-activity-paused", "{ $count } background tasks paused")
                .with("count", summary.paused.len()),
        );
        let lines: Vec<String> = summary
            .paused
            .iter()
            .map(|p| crate::activity::paused_line(&ctx, p))
            .collect();
        (said, lines.join("\n"))
    });
    let failed = (summary.failed > 0).then(|| {
        text(
            &ctx,
            Message::new("gui-activity-failed", "{ $count } background tasks failed")
                .with("count", summary.failed),
        )
    });
    let (paused, why) = paused.unzip();
    let meaning = [running, paused, failed]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    if meaning.is_empty() {
        return;
    }
    let tip = match why {
        Some(why) => format!("{meaning}\n{why}"),
        None => meaning.clone(),
    };
    let r = ui.add(
        mark.id(JOBS_MARK)
            .name(meaning)
            .tooltip(tip)
            .interactive(text(
                &ctx,
                Message::new("gui-caption-jobs", "Show the jobs"),
            )),
    );
    if r.clicked() {
        let id = main
            .or(held)
            .map(|p| p.id.clone())
            .or_else(|| summary.newest_failed.clone());
        app.run(
            CommandId::from_static(crate::activity::COMMAND),
            id.map_or(serde_json::Value::Null, |id| json!({ "id": id })),
        );
    }
}
