//! The Stacks panel: the `stacks.suggest` proposals waiting for an answer,
//! reviewed group by group (see `docs/library.md`, "Suggesting stacks").
//!
//! Each proposal lists its groups (a burst or a bracket, why, its images
//! with the one proposed on top), each checked to be taken. The answer is
//! recorded as a decision's answer: **Accept all** takes every group
//! (`accepted`), **Accept the checked** only some (`corrected`),
//! **Decline** none. **Suggest stacks** asks the resolver again, over the
//! library's selection when it holds two images or more, else over every
//! image.
//!
//! The panel holds no vault: the host gives it the proposals
//! ([`Review::set_decisions`]) and runs what the user asked
//! ([`Review::take_requests`]), as the library's marks are.

use std::collections::{BTreeMap, BTreeSet};

use egui::Ui;
use helios_core::Message;
use helios_library::stacks::{Decision, Status};

use crate::library::Library;

/// The panel's id.
pub const PANEL: &str = "helios.stacks";

/// What the user asked of the proposals, for the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// A new proposal over these images (every image when empty).
    Suggest {
        images: Vec<String>,
    },
    /// Some groups taken (all of them when `None`).
    Accept {
        decision: String,
        groups: Option<Vec<usize>>,
    },
    Decline {
        decision: String,
    },
}

/// The proposals waiting, and which of their groups are checked.
#[derive(Debug, Default)]
pub struct Review {
    decisions: Vec<Decision>,
    /// Per proposal, the groups left unchecked (every group is checked
    /// at first).
    unchecked: BTreeMap<String, BTreeSet<usize>>,
    requests: Vec<Request>,
}

impl Review {
    /// The proposals as the vault keeps them; the answered ones are left
    /// out.
    pub fn set_decisions(&mut self, decisions: Vec<Decision>) {
        self.decisions = decisions
            .into_iter()
            .filter(|d| d.status == Status::Proposed)
            .collect();
        let waiting: BTreeSet<&str> = self.decisions.iter().map(|d| d.id.as_str()).collect();
        self.unchecked.retain(|id, _| waiting.contains(id.as_str()));
    }

    /// The proposals waiting.
    pub fn decisions(&self) -> &[Decision] {
        &self.decisions
    }

    /// Checks or unchecks a group of a proposal.
    pub fn check(&mut self, decision: &str, group: usize, on: bool) {
        let off = self.unchecked.entry(decision.to_string()).or_default();
        if on {
            off.remove(&group);
        } else {
            off.insert(group);
        }
    }

    /// Whether a group of a proposal is checked.
    pub fn checked(&self, decision: &str, group: usize) -> bool {
        !self
            .unchecked
            .get(decision)
            .is_some_and(|off| off.contains(&group))
    }

    /// The groups of a proposal checked, in order.
    fn taken(&self, d: &Decision) -> Vec<usize> {
        (0..d.proposal.groups.len())
            .filter(|g| self.checked(&d.id, *g))
            .collect()
    }

    /// Asks for a new proposal.
    pub fn suggest(&mut self, images: Vec<String>) {
        self.requests.push(Request::Suggest { images });
    }

    /// Accepts every group of a proposal.
    pub fn accept_all(&mut self, decision: &str) {
        self.requests.push(Request::Accept {
            decision: decision.into(),
            groups: None,
        });
    }

    /// Accepts the checked groups of a proposal: every one, or a
    /// correction; refused when none is checked.
    pub fn accept_checked(&mut self, decision: &str) -> Result<(), String> {
        let d = self
            .decisions
            .iter()
            .find(|d| d.id == decision)
            .ok_or_else(|| format!("no proposal {decision} waiting"))?;
        let taken = self.taken(d);
        if taken.is_empty() {
            return Err("no group checked: decline the proposal instead".into());
        }
        let groups = (taken.len() < d.proposal.groups.len()).then_some(taken);
        self.requests.push(Request::Accept {
            decision: decision.into(),
            groups,
        });
        Ok(())
    }

    /// Declines a proposal.
    pub fn decline(&mut self, decision: &str) {
        self.requests.push(Request::Decline {
            decision: decision.into(),
        });
    }

    /// What the user asked since the last call.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// The panel. Clicking a group's images selects them in the library
    /// (returned, for the caller to select), to look at them in the grid
    /// or the Compare panel.
    pub fn ui(&mut self, ui: &mut Ui, library: &Library) -> Option<Vec<String>> {
        let ctx = ui.ctx().clone();
        let text = |m: Message| crate::i18n::text(&ctx, m);
        let mut show = None;
        let selection = library.selection();
        if ui
            .button(text(Message::new("gui-stacks-suggest", "Suggest stacks")))
            .on_hover_text(text(Message::new(
                "gui-stacks-suggest-hint",
                "Over the images selected (two or more), else over every image",
            )))
            .clicked()
        {
            let images = if selection.len() >= 2 {
                selection.clone()
            } else {
                Vec::new()
            };
            self.suggest(images);
        }
        if self.decisions.is_empty() {
            ui.weak(text(Message::new(
                "gui-stacks-none",
                "No stack suggestion waiting",
            )));
            return None;
        }
        let name = |id: &str| {
            library
                .entries
                .iter()
                .find(|e| e.id == id)
                .map_or_else(|| id.chars().take(12).collect(), |e| e.name.clone())
        };
        let decisions = self.decisions.clone();
        egui::ScrollArea::vertical()
            .id_salt("helios.stacks.scroll")
            .show(ui, |ui| {
                for d in &decisions {
                    ui.separator();
                    ui.strong(text(
                        Message::new(
                            "gui-stacks-proposal",
                            "{ $groups } stacks proposed by { $resolver }",
                        )
                        .with("groups", d.proposal.groups.len())
                        .with("resolver", d.resolver.clone()),
                    ));
                    if !d.left_out.is_empty() {
                        let why: Vec<String> = d
                            .left_out
                            .iter()
                            .map(|(id, why)| format!("{}: {why}", name(id)))
                            .collect();
                        ui.weak(text(
                            Message::new("gui-stacks-left-out", "{ $count } images left out")
                                .with("count", d.left_out.len()),
                        ))
                        .on_hover_text(why.join("\n"));
                    }
                    for (g, group) in d.proposal.groups.iter().enumerate() {
                        let mut on = self.checked(&d.id, g);
                        ui.horizontal(|ui| {
                            let title = match group.kind.as_str() {
                                "bracket" => text(
                                    Message::new("gui-stacks-bracket", "Bracket of { $count }")
                                        .with("count", group.members.len()),
                                ),
                                _ => text(
                                    Message::new("gui-stacks-burst", "Burst of { $count }")
                                        .with("count", group.members.len()),
                                ),
                            };
                            let r = ui.checkbox(&mut on, title).on_hover_text(&group.why);
                            crate::name_rect(
                                ui.ctx(),
                                &format!("helios.stacks.{}.{g}", d.id),
                                r.rect,
                            );
                            if ui
                                .small_button(text(Message::new("gui-stacks-show", "Show")))
                                .clicked()
                            {
                                show = Some(group.members.clone());
                            }
                        });
                        if on != self.checked(&d.id, g) {
                            self.check(&d.id, g, on);
                        }
                        ui.horizontal_wrapped(|ui| {
                            for m in &group.members {
                                let top = *m == group.best;
                                let label = if top {
                                    format!("★ {}", name(m))
                                } else {
                                    name(m)
                                };
                                let r = ui.selectable_label(selection.contains(m), label);
                                let r = if top {
                                    r.on_hover_text(text(Message::new(
                                        "gui-stacks-top",
                                        "Proposed on top: the sharpest",
                                    )))
                                } else {
                                    r
                                };
                                if r.clicked() {
                                    show = Some(vec![m.clone()]);
                                }
                            }
                        });
                    }
                    let taken = self.taken(d);
                    ui.horizontal(|ui| {
                        if ui
                            .button(text(Message::new("gui-stacks-accept-all", "Accept all")))
                            .clicked()
                        {
                            self.accept_all(&d.id);
                        }
                        let partial = !taken.is_empty() && taken.len() < d.proposal.groups.len();
                        if ui
                            .add_enabled(
                                partial,
                                egui::Button::new(text(Message::new(
                                    "gui-stacks-accept-checked",
                                    "Accept the checked",
                                ))),
                            )
                            .clicked()
                        {
                            let _ = self.accept_checked(&d.id);
                        }
                        if ui
                            .button(text(Message::new("gui-stacks-decline", "Decline")))
                            .clicked()
                        {
                            self.decline(&d.id);
                        }
                    });
                }
            });
        show
    }
}
