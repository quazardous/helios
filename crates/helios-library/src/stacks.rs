//! `stacks.suggest`: a library decision that proposes stacks over a
//! selection, for a person to accept.
//!
//! A suggestion is asked of a **resolver**, picked by name
//! ([`Resolvers`]); the default, `rules`, is [`RuleResolver`]: images
//! taken one after the other (the same burst, or capture times within a
//! window, or consecutive frame numbers when there is no time) whose
//! perceptual hashes are close are grouped, with a looser threshold
//! between frames whose exposure differs (a bracket). Each group comes
//! with a proposed best and why ([`best_of`]): the sharpest by the
//! culling focus, a frame with blown highlights only when every measured
//! frame has them. Other resolvers (AI models) plug in under their own
//! names.
//!
//! Nothing is applied on its own: the suggestion is recorded as a
//! proposal ([`Decision`], kept in the vault), and only an accept makes
//! the stacks, all the groups or the ones chosen, the best of each on top.
//! The decision records who answered and how, with the vocabulary of
//! decision nodes ([`helios_decision::Decider`]): accepting the proposal
//! as it is, or correcting it (some groups only). Declining records the
//! answer and makes nothing. Images already in a stack are left out of a
//! suggestion.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use helios_core::{Bus, Event};
use helios_decision::{Decider, Model};
use helios_vault::{ImageId, Store, Vault};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::signals::{Signals, distance};

/// The decision's kind.
pub const KIND: &str = "stacks.suggest";

/// The default resolver's name.
pub const RULES: &str = "rules";

/// Where decisions are kept (`library/decisions`).
const DECISIONS: &str = "decisions";

/// An image a resolver looks at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// The image (hex).
    pub id: String,
    pub signals: Signals,
}

/// A stack proposed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    /// In capture order.
    pub members: Vec<String>,
    /// The one proposed on top.
    pub best: String,
    /// Why that one, in English (`sharpest, not clipped`); empty in
    /// proposals made before it was recorded.
    #[serde(default)]
    pub best_why: String,
    /// `burst` or `bracket`.
    pub kind: String,
    /// Why the members go together, in English.
    pub why: String,
}

/// A resolver's answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub groups: Vec<Group>,
    pub model: Model,
    pub rationale: String,
}

/// Proposes stacks.
pub trait StackResolver: Send + Sync {
    fn suggest(&self, candidates: &[Candidate]) -> Result<Suggestion, String>;
}

/// The thresholds of [`RuleResolver`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    /// Frames this many seconds apart or less may go together.
    pub window: f64,
    /// The most bits two hashes of one burst may differ by (of 64).
    pub distance: u32,
    /// The same, between frames whose exposure differs.
    pub bracket_distance: u32,
    /// Exposures this many stops apart or more make a bracket.
    pub bracket_step: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            window: 2.0,
            distance: 10,
            bracket_distance: 18,
            bracket_step: 0.3,
        }
    }
}

/// The rule-based resolver: no model, the same answer every time.
#[derive(Clone, Debug, Default)]
pub struct RuleResolver {
    pub rules: Rules,
}

/// How two consecutive frames are linked, if they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    Burst,
    Time,
    Frame,
}

/// The version of the rules, recorded with what they propose.
pub const RULES_VERSION: &str = "2";

/// A frame with more than this share of one channel blown white is
/// clipped: the vault's culling threshold, shared with the badges and the
/// `cull` facet.
pub use helios_vault::cull::CLIPPED_WHITE;

impl RuleResolver {
    /// Whether `b`, taken after `a`, continues its group: the link and
    /// whether their exposures differ.
    fn link(&self, a: &Signals, b: &Signals) -> Option<(Link, bool)> {
        let r = &self.rules;
        let different = |x: Option<f64>, y: Option<f64>| {
            x.zip(y)
                .is_some_and(|(x, y)| (x - y).abs() >= r.bracket_step)
        };
        let bracket =
            different(a.exposure(), b.exposure()) || different(a.exposure_bias, b.exposure_bias);
        if let (Some(x), Some(y)) = (a.camera(), b.camera())
            && x != y
        {
            return None;
        }
        if a.burst.is_some() && a.burst == b.burst {
            return Some((Link::Burst, bracket));
        }
        let link = match (a.taken, b.taken) {
            (Some(x), Some(y)) if (y - x).abs() <= r.window => Link::Time,
            (Some(_), Some(_)) => return None,
            _ => match (a.frame, b.frame) {
                (Some(x), Some(y)) if x.abs_diff(y) == 1 => Link::Frame,
                _ => return None,
            },
        };
        let (x, y) = (a.hash()?, b.hash()?);
        let limit = if bracket {
            r.bracket_distance
        } else {
            r.distance
        };
        (distance(x, y) <= limit).then_some((link, bracket))
    }
}

impl StackResolver for RuleResolver {
    fn suggest(&self, candidates: &[Candidate]) -> Result<Suggestion, String> {
        let mut order: Vec<&Candidate> = candidates.iter().collect();
        order.sort_by(|a, b| {
            let time = |c: &Candidate| c.signals.taken.unwrap_or(f64::INFINITY);
            time(a)
                .total_cmp(&time(b))
                .then(a.signals.frame.cmp(&b.signals.frame))
                .then(a.id.cmp(&b.id))
        });
        let mut groups = Vec::new();
        let mut current: Vec<&Candidate> = Vec::new();
        let mut links: Vec<(Link, bool)> = Vec::new();
        let mut close = |current: &mut Vec<&Candidate>, links: &mut Vec<(Link, bool)>| {
            if current.len() >= 2 {
                groups.push(group(current, links));
            }
            current.clear();
            links.clear();
        };
        for c in order {
            let linked = current
                .last()
                .and_then(|last| self.link(&last.signals, &c.signals));
            match linked {
                Some(l) => links.push(l),
                None => close(&mut current, &mut links),
            }
            current.push(c);
        }
        close(&mut current, &mut links);
        let rationale = format!(
            "{} group(s): frames within {} s (or the same burst, or consecutive frame numbers without a time) whose hashes differ by at most {} bits ({} across exposures {} stops apart or more)",
            groups.len(),
            self.rules.window,
            self.rules.distance,
            self.rules.bracket_distance,
            self.rules.bracket_step
        );
        Ok(Suggestion {
            groups,
            model: Model {
                name: RULES.into(),
                version: RULES_VERSION.into(),
            },
            rationale,
        })
    }
}

/// A group of frames and how each followed the one before.
fn group(members: &[&Candidate], links: &[(Link, bool)]) -> Group {
    let bracket = links.iter().any(|(_, b)| *b);
    let mut ways: Vec<&str> = links
        .iter()
        .map(|(l, _)| match l {
            Link::Burst => "the same burst",
            Link::Time => "taken within the window",
            Link::Frame => "consecutive frame numbers",
        })
        .collect();
    ways.sort_unstable();
    ways.dedup();
    let (best, best_why) = best_of(members);
    Group {
        members: members.iter().map(|c| c.id.clone()).collect(),
        best,
        best_why,
        kind: (if bracket { "bracket" } else { "burst" }).into(),
        why: format!("{}, look alike; the best proposed on top", ways.join(", ")),
    }
}

/// How a frame ranks for the top of its group: only frames with a
/// culling focus are ranked against each other (the focus is a raw
/// energy, comparable within one burst only).
struct Rank {
    focus: f64,
    /// The most of any channel blown to white.
    white: f64,
    /// The most of any channel crushed to black.
    black: f64,
}

impl Rank {
    fn of(c: &Candidate) -> Option<Rank> {
        let cull = &c.signals.cull;
        let max = |v: Option<[f64; 3]>| v.map_or(0.0, |v| v.into_iter().fold(0.0, f64::max));
        Some(Rank {
            focus: cull.focus?,
            white: max(cull.clip_white),
            black: max(cull.clip_black),
        })
    }

    fn clipped(&self) -> bool {
        self.white > CLIPPED_WHITE
    }
}

/// The member proposed on top of a group and why, deterministically:
///
/// - frames with culling measures are ranked: a frame not clipped
///   (at most [`CLIPPED_WHITE`] blown white on every channel) before a
///   clipped one, then the higher focus, then the less blown white, then
///   the less crushed black, then the earlier in capture order;
/// - frames without measures are never chosen over a measured one;
/// - when no frame is measured, the highest import sharpness wins, the
///   earlier on a tie (the rule before the culling measures).
pub fn best_of(members: &[&Candidate]) -> (String, String) {
    let ranked: Vec<(usize, Rank)> = members
        .iter()
        .enumerate()
        .filter_map(|(i, c)| Rank::of(c).map(|r| (i, r)))
        .collect();
    let unmeasured = members.len() - ranked.len();
    let Some((top, rank)) = ranked.iter().min_by(|(i, a), (j, b)| {
        a.clipped()
            .cmp(&b.clipped())
            .then(b.focus.total_cmp(&a.focus))
            .then(a.white.total_cmp(&b.white))
            .then(a.black.total_cmp(&b.black))
            .then(i.cmp(j))
    }) else {
        let best = members
            .iter()
            .enumerate()
            .max_by(|(i, a), (j, b)| {
                let s = |c: &Candidate| c.signals.sharpness.unwrap_or(f64::NEG_INFINITY);
                // The first of equals wins.
                s(a).total_cmp(&s(b)).then(j.cmp(i))
            })
            .map(|(_, c)| c.id.clone())
            .unwrap_or_default();
        return (best, "sharpest (no culling measures)".into());
    };
    let percent = |share: f64| format!("{:.1}%", share * 100.0);
    let sharper_clipped = ranked
        .iter()
        .filter(|(_, r)| r.clipped() && r.focus > rank.focus)
        .map(|(_, r)| r.white)
        .fold(None, |m: Option<f64>, w| Some(m.map_or(w, |m| m.min(w))));
    // The others as sharp: all are at least as clipped as the top.
    let tied: Vec<&Rank> = ranked
        .iter()
        .filter(|(i, r)| i != top && r.clipped() == rank.clipped() && r.focus == rank.focus)
        .map(|(_, r)| r)
        .collect();
    let mut why = match (rank.clipped(), sharper_clipped) {
        (true, _) => format!(
            "sharpest; every measured frame is clipped ({} blown white)",
            percent(rank.white)
        ),
        (false, Some(white)) => format!(
            "sharpest not clipped (a sharper frame has {} blown white)",
            percent(white)
        ),
        (false, None) => "sharpest, not clipped".to_string(),
    };
    if tied
        .iter()
        .any(|r| r.white > rank.white || r.black > rank.black)
    {
        why.push_str("; less clipped than one as sharp");
    } else if !tied.is_empty() {
        why.push_str("; the first of frames as sharp");
    }
    if unmeasured > 0 {
        let _ = write!(why, "; {unmeasured} frame(s) not measured");
    }
    (members[*top].id.clone(), why)
}

/// Resolvers by name.
#[derive(Clone)]
pub struct Resolvers {
    named: BTreeMap<String, Arc<dyn StackResolver>>,
}

impl Default for Resolvers {
    /// `rules`, with its default thresholds.
    fn default() -> Self {
        Resolvers::new().with(RULES, Arc::new(RuleResolver::default()))
    }
}

impl Resolvers {
    /// None at all.
    pub fn new() -> Self {
        Resolvers {
            named: BTreeMap::new(),
        }
    }

    pub fn with(mut self, name: &str, resolver: Arc<dyn StackResolver>) -> Self {
        self.named.insert(name.into(), resolver);
        self
    }

    pub fn get(&self, name: &str) -> Result<&Arc<dyn StackResolver>, String> {
        self.named.get(name).ok_or_else(|| {
            let known: Vec<&str> = self.named.keys().map(String::as_str).collect();
            format!("no stack resolver `{name}` (known: {})", known.join(", "))
        })
    }
}

/// Where a decision stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Proposed,
    Accepted,
    Declined,
}

/// What a resolver proposed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub groups: Vec<Group>,
    pub model: Model,
    pub rationale: String,
    pub at: u64,
}

/// How a person answered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    #[serde(flatten)]
    pub decider: Decider,
    /// The groups taken, by their place in the proposal.
    pub groups: Vec<usize>,
    /// The stacks made.
    pub stacks: Vec<String>,
    pub at: u64,
}

/// A `stacks.suggest` decision, as the vault keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub kind: String,
    pub status: Status,
    /// The images asked about.
    pub selection: Vec<String>,
    /// Images left out, with why (already stacked, no signals).
    pub left_out: Vec<(String, String)>,
    pub resolver: String,
    pub proposal: Proposal,
    pub answer: Option<Answer>,
}

/// A library decision was proposed or answered.
#[derive(Serialize)]
pub struct DecisionChanged {
    pub decision: String,
    pub kind: &'static str,
    pub status: Status,
    pub groups: usize,
}

impl Event for DecisionChanged {
    const KIND: &'static str = "library.decision";
}

fn announce(bus: Option<&Bus>, d: &Decision) {
    if let Some(bus) = bus {
        bus.publish(
            "library",
            None,
            &DecisionChanged {
                decision: d.id.clone(),
                kind: KIND,
                status: d.status,
                groups: d.proposal.groups.len(),
            },
        );
    }
}

fn save<S: Store>(vault: &Vault<S>, d: &Decision) -> Result<(), String> {
    let doc = serde_json::to_value(d).expect("decisions serialize");
    vault.set_library_document(DECISIONS, &d.id, Some(&doc))
}

/// Every `stacks.suggest` decision, oldest first.
pub fn decisions<S: Store>(vault: &Vault<S>) -> Result<Vec<Decision>, String> {
    let mut all: Vec<Decision> = vault
        .library_documents(DECISIONS)?
        .into_iter()
        .map(|(_, doc)| serde_json::from_value(doc).map_err(|e| format!("a decision: {e}")))
        .collect::<Result<_, _>>()?;
    all.retain(|d| d.kind == KIND);
    all.sort_by_key(|d| (d.proposal.at, number(&d.id)));
    Ok(all)
}

/// The number at the end of a decision's id.
fn number(id: &str) -> u64 {
    id.rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

pub fn decision<S: Store>(vault: &Vault<S>, id: &str) -> Result<Decision, String> {
    let doc = vault
        .library_document(DECISIONS, id)?
        .ok_or_else(|| format!("no decision {id}"))?;
    serde_json::from_value(doc).map_err(|e| format!("decision {id}: {e}"))
}

/// Asks `resolver` for stacks over `selection` (every image of the vault
/// when empty) and records its proposal: nothing is applied.
pub fn suggest<S: Store>(
    vault: &Vault<S>,
    bus: Option<&Bus>,
    resolvers: &Resolvers,
    resolver: &str,
    selection: &[ImageId],
    now: u64,
) -> Result<Decision, String> {
    let chosen = resolvers.get(resolver)?;
    let selection: Vec<ImageId> = if selection.is_empty() {
        vault.images()?.into_iter().map(|i| i.id).collect()
    } else {
        selection.to_vec()
    };
    let stacked: Vec<ImageId> = selection
        .iter()
        .filter_map(|id| match vault.stack_of(id) {
            Ok(Some(_)) => Some(Ok(*id)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        })
        .collect::<Result<_, _>>()?;
    let mut candidates = Vec::new();
    let mut left_out = Vec::new();
    for id in &selection {
        if vault.image(id)?.is_none() {
            return Err(format!("no image {}", id.hex()));
        }
        if stacked.contains(id) {
            left_out.push((id.hex(), "already in a stack".to_string()));
            continue;
        }
        match vault.signals(id)? {
            Some(doc) => candidates.push(Candidate {
                id: id.hex(),
                signals: serde_json::from_value(doc)
                    .map_err(|e| format!("signals of {}: {e}", id.hex()))?,
            }),
            None => left_out.push((id.hex(), "no signals (import it again)".to_string())),
        }
    }
    let answer = chosen.suggest(&candidates)?;
    let n = vault.library_documents(DECISIONS)?.len() + 1;
    let d = Decision {
        id: format!("{KIND}-{n}"),
        kind: KIND.into(),
        status: Status::Proposed,
        selection: selection.iter().map(ImageId::hex).collect(),
        left_out,
        resolver: resolver.into(),
        proposal: Proposal {
            groups: answer.groups,
            model: answer.model,
            rationale: answer.rationale,
            at: now,
        },
        answer: None,
    };
    save(vault, &d)?;
    announce(bus, &d);
    Ok(d)
}

/// Accepts a proposal: the groups chosen (all when `None`) become stacks,
/// the best of each on top, and the answer is recorded (accepted as it
/// is, or corrected when only some groups are taken).
pub fn accept<S: Store>(
    vault: &Vault<S>,
    bus: Option<&Bus>,
    id: &str,
    groups: Option<&[usize]>,
    who: &str,
    now: u64,
) -> Result<Decision, String> {
    let mut d = decision(vault, id)?;
    if d.status != Status::Proposed {
        return Err(format!("decision {id} was already answered"));
    }
    let all: Vec<usize> = (0..d.proposal.groups.len()).collect();
    let mut taken: Vec<usize> = groups.map_or_else(|| all.clone(), <[usize]>::to_vec);
    taken.sort_unstable();
    taken.dedup();
    if let Some(bad) = taken.iter().find(|g| **g >= all.len()) {
        return Err(format!(
            "decision {id} has no group {bad} (it has {})",
            all.len()
        ));
    }
    // Every group is checked before any stack is made.
    let parse = |hex: &String| ImageId::from_hex(hex);
    let mut planned = Vec::new();
    for g in &taken {
        let group = &d.proposal.groups[*g];
        let members: Vec<ImageId> = group.members.iter().map(parse).collect::<Result<_, _>>()?;
        for m in &members {
            if let Some(s) = vault.stack_of(m)? {
                return Err(format!(
                    "group {g}: {} went into stack {} meanwhile",
                    m.hex(),
                    s.id
                ));
            }
        }
        planned.push((members, parse(&group.best)?));
    }
    let who_made = format!("decision:{id}");
    let mut stacks = Vec::new();
    for (members, best) in planned {
        stacks.push(vault.create_stack(&members, Some(best), &who_made, now)?.id);
    }
    let model = d.proposal.model.clone();
    let decider = if taken == all {
        Decider::Accepted {
            who: who.into(),
            model,
        }
    } else {
        Decider::Corrected {
            who: who.into(),
            model,
        }
    };
    d.status = Status::Accepted;
    d.answer = Some(Answer {
        decider,
        groups: taken,
        stacks,
        at: now,
    });
    save(vault, &d)?;
    announce(bus, &d);
    Ok(d)
}

/// Declines a proposal: recorded, nothing made.
pub fn decline<S: Store>(
    vault: &Vault<S>,
    bus: Option<&Bus>,
    id: &str,
    who: &str,
    now: u64,
) -> Result<Decision, String> {
    let mut d = decision(vault, id)?;
    if d.status != Status::Proposed {
        return Err(format!("decision {id} was already answered"));
    }
    d.status = Status::Declined;
    d.answer = Some(Answer {
        decider: Decider::Human { who: who.into() },
        groups: Vec::new(),
        stacks: Vec::new(),
        at: now,
    });
    save(vault, &d)?;
    announce(bus, &d);
    Ok(d)
}

/// A decision as plain JSON.
pub fn to_json(d: &Decision) -> Value {
    serde_json::to_value(d).expect("decisions serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cull::Cull;

    /// A frame of one burst, `n` in capture order.
    fn frame(n: u32, sharpness: f64, cull: Option<(f64, f64)>) -> Candidate {
        Candidate {
            id: format!("{n:02}"),
            signals: Signals {
                phash: Some("f0f0f0f00f0f0f0f".into()),
                taken: Some(f64::from(n) * 0.2),
                burst: Some("b1".into()),
                sharpness: Some(sharpness),
                cull: cull.map_or_else(Cull::default, |(focus, white)| Cull {
                    version: Some(crate::cull::VERSION),
                    focus: Some(focus),
                    clip_white: Some([white / 2.0, white, 0.0]),
                    clip_black: Some([0.001, 0.0, 0.0]),
                    ..Cull::default()
                }),
                ..Signals::default()
            },
        }
    }

    fn one_group(candidates: &[Candidate]) -> Group {
        let s = RuleResolver::default().suggest(candidates).unwrap();
        assert_eq!(s.groups.len(), 1, "{:?}", s.groups);
        s.groups.into_iter().next().unwrap()
    }

    #[test]
    fn the_sharp_unclipped_frame_goes_on_top() {
        // Sharp, blurred, and sharper still but 6% blown on green; the
        // import sharpness would pick the blurred one.
        let burst = [
            frame(1, 1.0, Some((40.0, 0.001))),
            frame(2, 9.0, Some((5.0, 0.0))),
            frame(3, 2.0, Some((55.0, 0.06))),
        ];
        let g = one_group(&burst);
        assert_eq!(g.best, "01");
        assert_eq!(
            g.best_why,
            "sharpest not clipped (a sharper frame has 6.0% blown white)"
        );
        // Without the clipped one: plainly the sharpest.
        let g = one_group(&burst[..2]);
        assert_eq!(g.best, "01");
        assert_eq!(g.best_why, "sharpest, not clipped");
    }

    #[test]
    fn when_every_frame_is_clipped_the_sharpest_wins() {
        let burst = [
            frame(1, 1.0, Some((40.0, 0.05))),
            frame(2, 1.0, Some((50.0, 0.30))),
        ];
        let g = one_group(&burst);
        assert_eq!(g.best, "02");
        assert!(
            g.best_why
                .starts_with("sharpest; every measured frame is clipped")
        );
    }

    #[test]
    fn as_sharp_frames_go_to_the_less_clipped_then_the_first() {
        let g = one_group(&[
            frame(1, 1.0, Some((40.0, 0.01))),
            frame(2, 1.0, Some((40.0, 0.005))),
        ]);
        assert_eq!(g.best, "02");
        assert_eq!(
            g.best_why,
            "sharpest, not clipped; less clipped than one as sharp"
        );
        let g = one_group(&[
            frame(1, 1.0, Some((40.0, 0.01))),
            frame(2, 1.0, Some((40.0, 0.01))),
        ]);
        assert_eq!(g.best, "01");
        assert_eq!(
            g.best_why,
            "sharpest, not clipped; the first of frames as sharp"
        );
    }

    #[test]
    fn a_burst_without_measures_keeps_the_import_sharpness() {
        let g = one_group(&[
            frame(1, 1.0, None),
            frame(2, 3.0, None),
            frame(3, 2.0, None),
        ]);
        assert_eq!(g.best, "02");
        assert_eq!(g.best_why, "sharpest (no culling measures)");
        // Equals: the first.
        let g = one_group(&[frame(1, 3.0, None), frame(2, 3.0, None)]);
        assert_eq!(g.best, "01");
    }

    #[test]
    fn an_unmeasured_frame_never_beats_a_measured_one() {
        // The unmeasured frame has by far the highest import sharpness,
        // and the measured one is even clipped.
        let g = one_group(&[
            frame(1, 99.0, None),
            frame(2, 1.0, Some((3.0, 0.5))),
            frame(3, 50.0, None),
        ]);
        assert_eq!(g.best, "02");
        assert!(
            g.best_why.ends_with("; 2 frame(s) not measured"),
            "{}",
            g.best_why
        );
    }

    #[test]
    fn the_same_input_gives_the_same_proposal() {
        let burst = [
            frame(1, 1.0, Some((40.0, 0.001))),
            frame(2, 9.0, None),
            frame(3, 2.0, Some((55.0, 0.06))),
            frame(4, 2.0, Some((40.0, 0.001))),
        ];
        let first = RuleResolver::default().suggest(&burst).unwrap();
        for _ in 0..5 {
            assert_eq!(RuleResolver::default().suggest(&burst).unwrap(), first);
        }
        // The order the candidates come in does not matter either.
        let mut reversed = burst.to_vec();
        reversed.reverse();
        assert_eq!(RuleResolver::default().suggest(&reversed).unwrap(), first);
        assert_eq!(first.groups[0].best, "01");
    }
}
