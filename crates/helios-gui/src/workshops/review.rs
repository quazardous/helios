//! The Workshops panel's review of a run (`workshop.review`, experimental,
//! see `docs/design/workshops-on-the-fly.md`, section 4, "The review"):
//! the counts, the photos needing review first, each with its state, its
//! flags, and its values before and after the run with where each comes
//! from; per photo Accept (held), Revert (written), a value set by hand
//! (a fix) and a junction left to its fallback decided; Re-run for the
//! held and decided photos.

use egui_pop::core::number::NumberInput;
use egui_pop::core::tooltip::TooltipExt as _;
use egui_pop::core::ui::{Space, TextRole, Tone, UiPop as _};
use egui_pop::core::widgets;
use helios_core::Message;
use helios_ui::{Action, Scroll};
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

/// How wide a value set by hand is typed, in points: a few digits.
const VALUE_WIDTH: f32 = 64.0;

fn number(v: &Value) -> String {
    v.as_f64().map_or_else(
        || "-".to_string(),
        |f| {
            format!("{f:.3}")
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
                ui.divider();
                if helios_ui::action(
                    ui,
                    Action::Secondary,
                    &text(Message::new(
                        "gui-workshops-review-last",
                        "Review the last run",
                    )),
                )
                .clicked()
                {
                    *run = Some(("workshop.review", json!({ "run": last })));
                }
            }
            return;
        };
        let id = review["run"].clone();
        let node = review["node"].clone();
        ui.divider();
        ui.text_role(
            TextRole::Section,
            text(
                Message::new(
                    "gui-workshops-review-title",
                    "Review: { $name }, run { $n }",
                )
                .with("name", review["name"].as_str().unwrap_or_default())
                .with("n", review["n"].to_string()),
            ),
        );
        let c = &review["counts"];
        ui.muted(text(
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
            widgets::checkbox(
                ui,
                &mut self.review_flagged_only,
                text(Message::new("gui-workshops-review-flagged-only", "Only photos to review")),
            );
            if rerun > 0
                && helios_ui::action(
                    ui,
                    Action::Secondary,
                    &text(
                        Message::new("gui-workshops-review-rerun", "Re-run held and decided ({ $count })")
                            .with("count", rerun.to_string()),
                    ),
                )
                .tip(text(Message::new(
                    "gui-workshops-review-rerun-hint",
                    "Run the workshop again on the held photos and those with a junction decided: a new run, one undo step",
                )))
                .clicked()
            {
                *run = Some(("workshop.review", json!({ "run": id, "node": node, "action": "rerun" })));
            }
            if helios_ui::row_action(
                ui,
                Action::Secondary,
                &text(Message::new("gui-workshops-review-refresh", "Refresh")),
            )
            .clicked()
            {
                *run = Some(("workshop.review", json!({ "run": id })));
            }
            close = helios_ui::row_action(
                ui,
                Action::Secondary,
                &text(Message::new("gui-workshops-review-close", "Close")),
            )
            .clicked();
        });
        if close {
            self.review = None;
            return;
        }
        let pending = review["pending"].as_array().cloned().unwrap_or_default();
        helios_ui::scroll(ui, "workshop-review", Scroll::Form, |ui| {
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
                ui.space_role(Space::Group);
                ui.card(Tone::Neutral, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.text_role(TextRole::Section, &name);
                        ui.text_role(TextRole::Body, text(state_words(state)));
                        for f in &flags {
                            let r = ui.text_tone(
                                Tone::Warning,
                                text(flag_words(f["flag"].as_str().unwrap_or_default())),
                            );
                            if let Some(note) = f["note"].as_str().filter(|n| !n.is_empty()) {
                                let _ = r.tip(note);
                            }
                        }
                    });
                    for (slider, after) in p["after"].as_object().into_iter().flatten() {
                        let source = p["provenance"][slider]["kind"].as_str().unwrap_or("fixed");
                        let junction = pending
                            .iter()
                            .any(|j| j["image"] == image.as_str() && j["junction"] == slider.as_str());
                        let l = ui.text_role(
                            TextRole::Body,
                            text(
                                Message::new(
                                    "gui-workshops-review-value",
                                    "{ $slider }: { $before } before, { $after } after ({ $source })",
                                )
                                .with("slider", slider.as_str())
                                .with("before", number(&p["before"][slider]))
                                .with("after", number(after))
                                .with("source", text(source_words(source))),
                            ),
                        );
                        if matches!(state, "failed" | "reverted") {
                            continue;
                        }
                        // The value under its line, and its button.
                        ui.horizontal(|ui| {
                            let key = (image.clone(), slider.clone());
                            let v = self
                                .review_edits
                                .entry(key)
                                .or_insert_with(|| after.as_f64().unwrap_or(0.0));
                            // The value typed; the node refuses one outside
                            // the slider's range. The kit's number field is
                            // as wide as its range's end, so an unbounded one
                            // is held to a few digits here.
                            let field = ui
                                .push_id((&image, slider), |ui| {
                                    ui.allocate_ui(egui::vec2(VALUE_WIDTH, ui.spacing().interact_size.y), |ui| {
                                        NumberInput::new(v, slider, f64::MIN..=f64::MAX)
                                            .step(0.01)
                                            .decimals(2)
                                            .show(ui)
                                    })
                                    .inner
                                })
                                .inner;
                            let _ = field.labelled_by(l.id);
                            let v = *v;
                            if junction {
                                if helios_ui::row_action(
                                    ui,
                                    Action::Secondary,
                                    &text(Message::new("gui-workshops-review-decide", "Decide")),
                                )
                                .tip(text(Message::new(
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
                            } else if helios_ui::row_action(
                                ui,
                                Action::Secondary,
                                &text(Message::new("gui-workshops-review-set", "Set")),
                            )
                            .tip(text(Message::new(
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
                            && helios_ui::row_action(
                                ui,
                                Action::Secondary,
                                &text(Message::new("gui-workshops-review-accept", "Accept")),
                            )
                            .tip(text(Message::new(
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
                            && helios_ui::row_action(
                                ui,
                                Action::Secondary,
                                &text(Message::new("gui-workshops-review-revert", "Revert")),
                            )
                            .tip(text(Message::new(
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
        });
    }
}
