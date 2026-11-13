//! The Workshops panel's review of a run (`workshop.review`, experimental,
//! see `docs/design/workshops-on-the-fly.md`, section 4, "The review"):
//! the counts, the photos needing review first, each with its state, its
//! flags, and its values before and after the run with where each comes
//! from; per photo Accept (held), Revert (written), a value set by hand
//! (a fix) and a junction left to its fallback decided; Re-run for the
//! held and decided photos.

use helios_core::Message;
use serde_json::{Value, json};

use super::Panel;

/// A photo's state, in words.
fn state_words(state: &str) -> Message {
    match state {
        "applied" => Message::new("gui-workshops-review-applied", "applied"),
        "held" => Message::new("gui-workshops-review-held", "held"),
        "stopped" => Message::new("gui-workshops-review-stopped", "stopped the run"),
        "unchanged" => Message::new("gui-workshops-review-unchanged", "unchanged"),
        "failed" => Message::new("gui-workshops-review-failed", "failed"),
        "accepted" => Message::new("gui-workshops-review-accepted", "accepted"),
        "reverted" => Message::new("gui-workshops-review-reverted", "reverted"),
        _ => Message::new("gui-workshops-review-fixed", "fixed by hand"),
    }
}

/// A flag, in words.
fn flag_words(flag: &str) -> Message {
    match flag {
        "at_limit" => Message::new("gui-workshops-flag-at-limit", "at its limit"),
        "deviation" => Message::new("gui-workshops-flag-deviation", "far from the test photos"),
        "target_missed" => Message::new("gui-workshops-flag-target-missed", "target missed"),
        "unlike_test_cases" => Message::new("gui-workshops-flag-unlike", "unlike the test photos"),
        "low_confidence" => Message::new("gui-workshops-flag-low-confidence", "AI unsure"),
        "fallback" => Message::new("gui-workshops-flag-fallback", "no AI: fallback used"),
        "not_monotonic" => Message::new("gui-workshops-flag-not-monotonic", "could not aim"),
        _ => Message::new("gui-workshops-flag-ai-budget", "AI budget spent"),
    }
}

/// Where a value comes from, in words.
fn source_words(kind: &str) -> Message {
    match kind {
        "aimed" => Message::new("gui-workshops-source-aimed", "aimed"),
        "ai" => Message::new("gui-workshops-source-ai", "AI"),
        "fallback" => Message::new("gui-workshops-source-fallback", "fallback"),
        "user" => Message::new("gui-workshops-source-user", "set by hand"),
        _ => Message::new("gui-workshops-source-fixed", "fixed"),
    }
}

fn number(v: &Value) -> String {
    v.as_f64().map_or_else(
        || "-".to_string(),
        |f| {
            format!("{:.3}", f)
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        },
    )
}

impl Panel {
    /// The review of the run last started or reviewed here.
    pub(super) fn review_ui(&mut self, ui: &mut egui::Ui, run: &mut Option<(&str, Value)>) {
        let ctx = ui.ctx().clone();
        let text = |m: Message| crate::i18n::text(&ctx, m);
        let Some(review) = self.review.clone() else {
            if let Some(last) = &self.last_run {
                ui.separator();
                if ui
                    .button(text(Message::new(
                        "gui-workshops-review-last",
                        "Review the last run",
                    )))
                    .clicked()
                {
                    *run = Some(("workshop.review", json!({ "run": last })));
                }
            }
            return;
        };
        let id = review["run"].clone();
        let node = review["node"].clone();
        ui.separator();
        ui.strong(text(
            Message::new(
                "gui-workshops-review-title",
                "Review: { $name }, run { $n }",
            )
            .with("name", review["name"].as_str().unwrap_or_default())
            .with("n", review["n"].to_string()),
        ));
        let c = &review["counts"];
        ui.weak(text(
            Message::new(
                "gui-workshops-review-counts",
                "{ $photos } photos, { $flagged } to review, { $held } held; AI calls: { $calls }",
            )
            .with("photos", c["photos"].to_string())
            .with("flagged", c["flagged"].to_string())
            .with(
                "held",
                (c["held"].as_u64().unwrap_or(0) + c["stopped"].as_u64().unwrap_or(0)).to_string(),
            )
            .with("calls", c["calls"].to_string()),
        ));
        let rerun = review["rerun"].as_array().map_or(0, Vec::len);
        let mut close = false;
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut self.review_flagged_only,
                text(Message::new("gui-workshops-review-flagged-only", "Only photos to review")),
            );
            if rerun > 0
                && ui
                    .button(text(
                        Message::new("gui-workshops-review-rerun", "Re-run held and decided ({ $count })")
                            .with("count", rerun.to_string()),
                    ))
                    .on_hover_text(text(Message::new(
                        "gui-workshops-review-rerun-hint",
                        "Run the workshop again on the held photos and those with a junction decided: a new run, one undo step",
                    )))
                    .clicked()
            {
                *run = Some(("workshop.review", json!({ "run": id, "node": node, "action": "rerun" })));
            }
            if ui
                .small_button(text(Message::new("gui-workshops-review-refresh", "Refresh")))
                .clicked()
            {
                *run = Some(("workshop.review", json!({ "run": id })));
            }
            close = ui
                .small_button(text(Message::new("gui-workshops-review-close", "Close")))
                .clicked();
        });
        if close {
            self.review = None;
            return;
        }
        let pending = review["pending"].as_array().cloned().unwrap_or_default();
        for p in review["photos"].as_array().into_iter().flatten() {
            let state = p["state"].as_str().unwrap_or_default();
            let flags = p["flags"].as_array().cloned().unwrap_or_default();
            let to_review = !flags.is_empty() || matches!(state, "held" | "stopped" | "failed");
            if self.review_flagged_only && !to_review {
                continue;
            }
            let image = p["image"].as_str().unwrap_or_default().to_string();
            let name = p["name"]
                .as_str()
                .map_or_else(|| image.chars().take(8).collect(), String::from);
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&name);
                    ui.label(text(state_words(state)));
                    for f in &flags {
                        ui.label(
                            egui::RichText::new(text(flag_words(f["flag"].as_str().unwrap_or_default())))
                                .color(ui.visuals().warn_fg_color),
                        )
                        .on_hover_text(f["note"].as_str().unwrap_or_default());
                    }
                });
                for (slider, after) in p["after"].as_object().into_iter().flatten() {
                    let source = p["provenance"][slider]["kind"].as_str().unwrap_or("fixed");
                    let junction = pending
                        .iter()
                        .any(|j| j["image"] == image.as_str() && j["junction"] == slider.as_str());
                    ui.horizontal_wrapped(|ui| {
                        let l = ui.label(text(
                            Message::new(
                                "gui-workshops-review-value",
                                "{ $slider }: { $before } before, { $after } after ({ $source })",
                            )
                            .with("slider", slider.as_str())
                            .with("before", number(&p["before"][slider]))
                            .with("after", number(after))
                            .with("source", text(source_words(source))),
                        ));
                        if matches!(state, "failed" | "reverted") {
                            return;
                        }
                        let key = (image.clone(), slider.clone());
                        let v = self
                            .review_edits
                            .entry(key)
                            .or_insert_with(|| after.as_f64().unwrap_or(0.0));
                        ui.add(egui::DragValue::new(v).speed(0.01)).labelled_by(l.id);
                        let v = *v;
                        if junction {
                            if ui
                                .small_button(text(Message::new("gui-workshops-review-decide", "Decide")))
                                .on_hover_text(text(Message::new(
                                    "gui-workshops-review-decide-hint",
                                    "Answer this junction for this photo: recorded in the workshop's decision book, a re-run takes it",
                                )))
                                .clicked()
                            {
                                *run = Some((
                                    "workshop.review",
                                    json!({ "run": id, "node": node, "action": "decide", "image": image, "junction": slider, "value": v }),
                                ));
                            }
                        } else if ui
                            .small_button(text(Message::new("gui-workshops-review-set", "Set")))
                            .on_hover_text(text(Message::new(
                                "gui-workshops-review-set-hint",
                                "Fix this value on this photo by hand: a new version, its flag cleared",
                            )))
                            .clicked()
                        {
                            *run = Some((
                                "workshop.review",
                                json!({ "run": id, "node": node, "action": "fix", "image": image, "values": { slider.as_str(): v } }),
                            ));
                        }
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    if matches!(state, "held" | "stopped")
                        && ui
                            .small_button(text(Message::new("gui-workshops-review-accept", "Accept")))
                            .on_hover_text(text(Message::new(
                                "gui-workshops-review-accept-hint",
                                "Write this photo with the values the run found, flags and all",
                            )))
                            .clicked()
                    {
                        *run = Some((
                            "workshop.review",
                            json!({ "run": id, "node": node, "action": "accept", "image": image }),
                        ));
                    }
                    if matches!(state, "applied" | "accepted" | "fixed")
                        && ui
                            .small_button(text(Message::new("gui-workshops-review-revert", "Revert")))
                            .on_hover_text(text(Message::new(
                                "gui-workshops-review-revert-hint",
                                "Take the run's version of this photo out: its edit before the run is current again",
                            )))
                            .clicked()
                    {
                        *run = Some((
                            "workshop.review",
                            json!({ "run": id, "action": "revert", "image": image }),
                        ));
                    }
                });
            });
        }
    }
}
