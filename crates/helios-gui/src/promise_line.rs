//! A promise as one line (see `docs/protocol.md`, "Promises"): what it
//! stands for, its progress while it runs, then its outcome. Cancel while
//! it is unsettled; Retry when it failed and may be retried.
//!
//! The line draws a [`Promise`] as the node answers it (`promises.get`, or
//! a `promise.progress` / `promise.done` event's payload) and says which
//! button was clicked; the caller sends `promises.cancel` or
//! `promises.retry`. It holds no state, so any screen can show the
//! promises it holds: the import window, the Library, a job's console.
//!
//! Helios's own rather than egui-pop's: egui-pop's jobs centre follows
//! jobs, and a promise is the node's record of a result, which egui-pop
//! does not know.

use egui::Ui;
use helios_core::Message;
use helios_vault::promises::{Promise, State};

/// What the person asked of the promise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// `promises.cancel`.
    Cancel,
    /// `promises.retry`.
    Retry,
}

/// How far `done` of `total` is, from 0 to 1.
#[allow(clippy::cast_precision_loss)]
fn fraction(done: u64, total: u64) -> f32 {
    (done as f64 / total as f64).min(1.0) as f32
}

/// Draws promise `p` on one line; the button clicked, if any.
pub fn show(ui: &mut Ui, p: &Promise) -> Option<Action> {
    let ctx = ui.ctx().clone();
    let text = |m: Message| crate::i18n::text(&ctx, m);
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(&p.label);
        match &p.state {
            State::Pending => {
                ui.spinner();
                ui.weak(text(Message::new(
                    "gui-promise-pending",
                    "Waiting to start",
                )));
            }
            State::Running { done, total, note } => {
                let counts = match total {
                    Some(total) => text(
                        Message::new("gui-promise-counts", "{ $done } of { $total }")
                            .with("done", *done)
                            .with("total", *total),
                    ),
                    None => text(
                        Message::new("gui-promise-so-far", "{ $done } so far").with("done", *done),
                    ),
                };
                let bar = match total {
                    Some(total) if *total > 0 => egui::ProgressBar::new(fraction(*done, *total)),
                    _ => egui::ProgressBar::new(0.0).animate(true),
                };
                ui.add(bar.desired_width(160.0));
                ui.label(counts);
                if let Some(note) = note {
                    ui.weak(note);
                }
            }
            State::Done { .. } => {
                ui.label(text(Message::new("gui-promise-done", "Done")));
            }
            State::Failed { reason, retryable } => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    text(
                        Message::new("gui-promise-failed", "Failed: { $reason }")
                            .with("reason", reason.as_str()),
                    ),
                );
                if *retryable {
                    if ui
                        .button(text(Message::new("gui-promise-retry", "Retry")))
                        .clicked()
                    {
                        action = Some(Action::Retry);
                    }
                }
            }
            State::Cancelled => {
                ui.weak(text(Message::new("gui-promise-cancelled", "Cancelled")));
            }
        }
        if !p.state.is_settled() {
            if p.cancel_asked {
                ui.weak(text(Message::new("gui-promise-cancelling", "Cancelling…")));
            } else {
                if ui
                    .button(text(Message::new("gui-promise-cancel", "Cancel")))
                    .clicked()
                {
                    action = Some(Action::Cancel);
                }
            }
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    use super::*;

    fn promise(state: State) -> Promise {
        Promise {
            id: "promise-1".into(),
            owner: "import-promise-1".into(),
            kind: "import".into(),
            label: "Import 3 photos".into(),
            request: json!(null),
            state,
            cancel_asked: false,
            attempts: 1,
            created: 1,
            updated: 1,
        }
    }

    /// The line drawn for `p`, and the action its last frame returned.
    fn harness(p: Promise) -> Harness<'static, (Promise, Option<Action>)> {
        Harness::builder()
            .with_size(egui::vec2(600.0, 40.0))
            .build_ui_state(
                |ui, (p, action): &mut (Promise, Option<Action>)| {
                    if let Some(a) = show(ui, p) {
                        *action = Some(a);
                    }
                },
                (p, None),
            )
    }

    #[test]
    fn a_promise_line_shows_progress_then_its_outcome_with_cancel_and_retry() {
        // Running: its counts and note, and Cancel.
        let mut h = harness(promise(State::Running {
            done: 1,
            total: Some(3),
            note: Some("a.cr3".into()),
        }));
        h.run();
        h.get_by_label("Import 3 photos");
        h.get_by_label("1 of 3");
        h.get_by_label("a.cr3");
        h.get_by_label("Cancel").click();
        h.run();
        assert_eq!(h.state().1, Some(Action::Cancel));
        // A cancel asked: no second Cancel.
        let mut asked = promise(State::Pending);
        asked.cancel_asked = true;
        let mut h = harness(asked);
        h.run();
        h.get_by_label("Cancelling…");
        assert!(h.query_by_label("Cancel").is_none());
        // Failed, retryable: the reason and Retry, no Cancel.
        let mut h = harness(promise(State::Failed {
            reason: "interrupted".into(),
            retryable: true,
        }));
        h.run();
        h.get_by_label("Failed: interrupted");
        assert!(h.query_by_label("Cancel").is_none());
        h.get_by_label("Retry").click();
        h.run();
        assert_eq!(h.state().1, Some(Action::Retry));
        // Failed for good: no Retry. Done: its word only.
        let mut h = harness(promise(State::Failed {
            reason: "gone".into(),
            retryable: false,
        }));
        h.run();
        assert!(h.query_by_label("Retry").is_none());
        let mut h = harness(promise(State::Done {
            result: json!({ "imported": 3 }),
        }));
        h.run();
        h.get_by_label("Done");
        assert!(h.query_by_label("Cancel").is_none());
    }
}
