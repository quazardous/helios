//! Aggregates over a set of photos: one value per group, read from what
//! the vault already stores (`docs/design/diagram-aggregates.md`, the
//! first slice of its plan B).
//!
//! The set is a search ([`helios_vault::rules::parse_text`]: `rating:>=3
//! camera:~Z`) or a list of photos. Its photos are grouped (all
//! together, by stack, or by burst with the grouping of
//! [`crate::stacks`]), then each group gets one value of a stored
//! numeric field (the culling `focus`, `clip_white`, `clip_black`, the
//! `rating`, `iso`…): its `count`, `min`, `max`, `mean`, `median`, a
//! `percentile` (`p` from 0 to 100), the median absolute deviation
//! (`mad`), the photos ranked (`rank`), or the one proposed on top
//! (`best`, the rule of [`crate::stacks::best_of`]). Nothing is rendered
//! or measured here: a photo without the field is skipped and listed as
//! `missing`. Groups may also be shooting sessions (`session`): a new one
//! starts when the time between two consecutive shots exceeds a gap.
//!
//! A diagram's aggregate node will read the same values later; the node
//! answers them now as the read-only command `library.aggregate`.

use std::collections::{BTreeMap, HashMap};

use helios_vault::capture::CAPTURED;
use helios_vault::fields::{self, FieldKind, Query};
use helios_vault::rules::parse_text;
use helios_vault::{ImageId, Store, Vault};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::stacks::{Candidate, RuleResolver, StackResolver, best_of};

/// The node's command answering an aggregate.
pub const COMMAND: &str = "library.aggregate";

/// Which photos.
#[derive(Clone, Debug, PartialEq)]
pub enum Set {
    /// The photos a search finds (an empty search: every photo).
    Query(String),
    /// These photos, in this order.
    Images(Vec<ImageId>),
}

/// What is computed per group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// How many photos carry the measure (every member without one).
    Count,
    Min,
    Max,
    Mean,
    /// The middle value (the mean of the two middle ones for an even
    /// count): the 50th percentile.
    Median,
    /// The `p`th percentile ([`Knobs::percentile`], 0 to 100), linear
    /// between the closest ranks (`p = 0`: the min, `100`: the max).
    Percentile,
    /// The median absolute deviation: the median of each value's distance
    /// to the median (unscaled), a spread outliers barely move.
    Mad,
    /// The measured photos, highest first.
    Rank,
    /// The one proposed on top of a burst ([`best_of`]: the sharpest by
    /// the culling focus, not clipped when one is not); reads the
    /// signals, not the measure.
    Best,
}

impl Op {
    pub const ALL: [&str; 9] = [
        "count",
        "min",
        "max",
        "mean",
        "median",
        "percentile",
        "mad",
        "rank",
        "best",
    ];

    pub fn parse(name: &str) -> Option<Op> {
        serde_json::from_value(Value::from(name)).ok()
    }
}

/// How the set is split.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    /// One group: the whole set.
    #[default]
    All,
    /// Each stack apart, each photo out of a stack alone.
    Stack,
    /// Bursts and brackets as `stacks.suggest`'s rules find them (its
    /// default thresholds), each other photo alone.
    Burst,
    /// Shooting sessions: a new one when the time between two consecutive
    /// capture times exceeds [`Knobs::gap_minutes`]; a photo without a
    /// capture time alone.
    Session,
}

impl GroupBy {
    pub const ALL: [&str; 4] = ["all", "stack", "burst", "session"];

    pub fn parse(name: &str) -> Option<GroupBy> {
        serde_json::from_value(Value::from(name)).ok()
    }
}

/// The gap between two sessions when none is given, in minutes.
pub const DEFAULT_GAP_MINUTES: f64 = 30.0;

/// The settings some ops and groupings take.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Knobs {
    /// The percentile `Op::Percentile` computes, 0 to 100 (required).
    pub percentile: Option<f64>,
    /// The gap that starts a new `GroupBy::Session`, in minutes
    /// (default [`DEFAULT_GAP_MINUTES`]).
    pub gap_minutes: Option<f64>,
}

/// One group's value.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Group {
    /// `all`, a stack's id, `burst-<n>`, or the photo's id when alone.
    pub key: String,
    /// Every member, in capture order (then by id).
    pub members: Vec<String>,
    /// The members the value was computed from.
    pub measured: usize,
    /// The members skipped: without the measure (or, for `best`, without
    /// signals).
    pub missing: Vec<String>,
    /// The value: none for an empty group, for `rank` and for `best`.
    pub value: Option<f64>,
    /// The photos holding it: every one at the `min` or `max` (ties
    /// included, in capture order), the measured photos highest first
    /// for `rank` (ties by capture time, then id), the one on top for
    /// `best`; empty otherwise.
    pub holders: Vec<String>,
    /// Why the one on top (`best` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// An aggregate's answer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Aggregate {
    pub measure: Option<String>,
    pub op: Op,
    pub by: GroupBy,
    /// The percentile computed (`percentile` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p: Option<f64>,
    /// The gap between sessions, in minutes (`session` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap_minutes: Option<f64>,
    /// How many photos the set holds.
    pub images: usize,
    pub groups: Vec<Group>,
}

/// The photos of a set.
fn photos<S: Store>(vault: &Vault<S>, set: &Set) -> Result<Vec<ImageId>, String> {
    match set {
        Set::Images(ids) => {
            let mut seen = std::collections::HashSet::new();
            Ok(ids.iter().copied().filter(|id| seen.insert(*id)).collect())
        }
        Set::Query(text) => {
            let rule = parse_text(text).map_err(|e| format!("search `{text}`: {e}"))?;
            let cursor = vault.open_cursor(Query {
                rules: Some(rule),
                ..Query::default()
            })?;
            let ids = vault.cursor_ids(cursor.id, 0, cursor.count);
            vault.close_cursor(cursor.id);
            ids
        }
    }
}

/// Checks that `measure` is a stored numeric field.
fn numeric(measure: &str) -> Result<(), String> {
    match fields::spec(measure)?.kind {
        FieldKind::Number | FieldKind::Int { .. } => Ok(()),
        _ => Err(format!("`{measure}` is not a number: no aggregate over it")),
    }
}

/// Computes `op` of `measure` over `set`, split `by`. `measure` may be
/// left out for `count` (the members counted) and `best` (which ranks by
/// the signals).
pub fn aggregate<S: Store>(
    vault: &Vault<S>,
    set: &Set,
    measure: Option<&str>,
    op: Op,
    by: GroupBy,
) -> Result<Aggregate, String> {
    aggregate_with(vault, set, measure, op, by, Knobs::default())
}

/// [`aggregate`] with the settings of `percentile` and `session`.
pub fn aggregate_with<S: Store>(
    vault: &Vault<S>,
    set: &Set,
    measure: Option<&str>,
    op: Op,
    by: GroupBy,
    knobs: Knobs,
) -> Result<Aggregate, String> {
    let p = match (op, knobs.percentile) {
        (Op::Percentile, Some(p)) if (0.0..=100.0).contains(&p) => Some(p),
        (Op::Percentile, Some(p)) => return Err(format!("percentile {p} is not within 0 to 100")),
        (Op::Percentile, None) => return Err("`percentile` needs `p` (0 to 100)".into()),
        _ => None,
    };
    let gap_minutes = match (by, knobs.gap_minutes) {
        (GroupBy::Session, Some(g)) if g.is_finite() && g >= 0.0 => Some(g),
        (GroupBy::Session, Some(g)) => return Err(format!("gap {g} is not a number of minutes")),
        (GroupBy::Session, None) => Some(DEFAULT_GAP_MINUTES),
        _ => None,
    };
    let measure = match (measure, op) {
        (Some(m), _) => {
            numeric(m)?;
            Some(m)
        }
        (None, Op::Count | Op::Best) => None,
        (None, _) => return Err(format!("`{op:?}` needs a measure").to_lowercase()),
    };
    let ids = photos(vault, set)?;
    let names: Vec<&str> = std::iter::once(CAPTURED).chain(measure).collect();
    let rows = vault.fields_of(&ids, &names)?;
    let number = |row: &BTreeMap<String, Value>, k: &str| {
        row.get(k).and_then(Value::as_f64).filter(|v| v.is_finite())
    };
    let facts: HashMap<ImageId, (Option<f64>, Option<f64>)> = ids
        .iter()
        .zip(&rows)
        .map(|(id, row)| {
            (
                *id,
                (number(row, CAPTURED), measure.and_then(|m| number(row, m))),
            )
        })
        .collect();
    // Capture order, then id: the order of members, of ties and of ranks.
    let mut ordered = ids.clone();
    ordered.sort_by(|a, b| {
        let t = |id: &ImageId| facts[id].0.unwrap_or(f64::INFINITY);
        t(a).total_cmp(&t(b)).then(a.hex().cmp(&b.hex()))
    });
    let need_signals = op == Op::Best || by == GroupBy::Burst;
    let mut signals = HashMap::new();
    if need_signals {
        for id in &ordered {
            if let Some(doc) = vault.signals(id)? {
                let s: crate::Signals = serde_json::from_value(doc)
                    .map_err(|e| format!("signals of {}: {e}", id.hex()))?;
                signals.insert(*id, s);
            }
        }
    }
    let groups = match gap_minutes {
        Some(gap) => sessions(&ordered, |id| facts[id].0, gap * 60.0),
        None => split(vault, &ordered, by, &signals)?,
    };
    let groups = groups
        .into_iter()
        .map(|(key, members)| {
            let values: Vec<(ImageId, f64)> = members
                .iter()
                .filter_map(|id| facts[id].1.map(|v| (*id, v)))
                .collect();
            value_of(key, &members, &values, measure, (op, p), &signals)
        })
        .collect();
    Ok(Aggregate {
        measure: measure.map(str::to_string),
        op,
        by,
        p,
        gap_minutes,
        images: ids.len(),
        groups,
    })
}

/// The groups of photos in capture order, each with its key.
fn split<S: Store>(
    vault: &Vault<S>,
    ordered: &[ImageId],
    by: GroupBy,
    signals: &HashMap<ImageId, crate::Signals>,
) -> Result<Vec<(String, Vec<ImageId>)>, String> {
    match by {
        // `Session` is grouped by `sessions`, which needs the capture times.
        GroupBy::All | GroupBy::Session => Ok(vec![("all".into(), ordered.to_vec())]),
        GroupBy::Stack => {
            let mut of: HashMap<String, String> = HashMap::new();
            for s in vault.stacks()? {
                for m in &s.members {
                    of.insert(m.clone(), s.id.clone());
                }
            }
            Ok(grouped(ordered, |id| of.get(&id.hex()).cloned()))
        }
        GroupBy::Burst => {
            let candidates: Vec<Candidate> = ordered
                .iter()
                .filter_map(|id| {
                    signals.get(id).map(|s| Candidate {
                        id: id.hex(),
                        signals: s.clone(),
                    })
                })
                .collect();
            let suggestion = RuleResolver::default().suggest(&candidates)?;
            let mut of: HashMap<String, String> = HashMap::new();
            for (n, g) in suggestion.groups.iter().enumerate() {
                for m in &g.members {
                    of.insert(m.clone(), format!("burst-{}", n + 1));
                }
            }
            Ok(grouped(ordered, |id| of.get(&id.hex()).cloned()))
        }
    }
}

/// Splits `ordered` (capture order) into sessions: a new one when the
/// time to the previous shot exceeds `gap` seconds. Keys `session-<n>`;
/// a photo without a capture time is alone, keyed by its id.
fn sessions(
    ordered: &[ImageId],
    time_of: impl Fn(&ImageId) -> Option<f64>,
    gap: f64,
) -> Vec<(String, Vec<ImageId>)> {
    let mut groups: Vec<(String, Vec<ImageId>)> = Vec::new();
    let mut last: Option<f64> = None;
    let mut n = 0;
    for id in ordered {
        let Some(t) = time_of(id) else {
            groups.push((id.hex(), vec![*id]));
            continue;
        };
        match last {
            Some(prev) if t - prev <= gap => groups.last_mut().expect("open").1.push(*id),
            _ => {
                n += 1;
                groups.push((format!("session-{n}"), vec![*id]));
            }
        }
        last = Some(t);
    }
    groups
}

/// The `p`th percentile (0 to 100) of `sorted` (ascending, not empty),
/// linear between the closest ranks.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let at = p / 100.0 * (sorted.len() - 1) as f64;
    let (lo, hi) = (at.floor() as usize, at.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (at - lo as f64)
}

/// `values` sorted ascending.
fn sorted(values: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut v: Vec<f64> = values.collect();
    v.sort_by(f64::total_cmp);
    v
}

/// Groups `ordered` by the key `key_of` gives, a photo without one alone;
/// groups in the order of their first member.
fn grouped(
    ordered: &[ImageId],
    key_of: impl Fn(&ImageId) -> Option<String>,
) -> Vec<(String, Vec<ImageId>)> {
    let mut groups: Vec<(String, Vec<ImageId>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for id in ordered {
        let key = key_of(id).unwrap_or_else(|| id.hex());
        if let Some(&i) = at.get(&key) {
            groups[i].1.push(*id);
        } else {
            at.insert(key.clone(), groups.len());
            groups.push((key, vec![*id]));
        }
    }
    groups
}

/// One group's value. `values` are the measured members, in capture
/// order.
fn value_of(
    key: String,
    members: &[ImageId],
    values: &[(ImageId, f64)],
    measure: Option<&str>,
    (op, p): (Op, Option<f64>),
    signals: &HashMap<ImageId, crate::Signals>,
) -> Group {
    let hex = |ids: &mut dyn Iterator<Item = ImageId>| ids.map(|i| i.hex()).collect::<Vec<_>>();
    let mut group = Group {
        key,
        members: hex(&mut members.iter().copied()),
        measured: values.len(),
        missing: hex(&mut members
            .iter()
            .copied()
            .filter(|id| !values.iter().any(|(v, _)| v == id))),
        value: None,
        holders: Vec::new(),
        why: None,
    };
    let extreme = |pick: fn(f64, f64) -> f64| {
        let v = values.iter().map(|(_, v)| *v).reduce(pick)?;
        Some((
            v,
            hex(&mut values.iter().filter(|(_, x)| *x == v).map(|(i, _)| *i)),
        ))
    };
    match op {
        Op::Count => {
            if measure.is_none() {
                group.measured = members.len();
                group.missing.clear();
            }
            group.value = Some(group.measured as f64);
        }
        Op::Min | Op::Max => {
            let pick = if op == Op::Min { f64::min } else { f64::max };
            if let Some((v, holders)) = extreme(pick) {
                group.value = Some(v);
                group.holders = holders;
            }
        }
        Op::Mean => {
            if !values.is_empty() {
                group.value =
                    Some(values.iter().map(|(_, v)| v).sum::<f64>() / values.len() as f64);
            }
        }
        Op::Median | Op::Percentile | Op::Mad if !values.is_empty() => {
            let all = sorted(values.iter().map(|(_, v)| *v));
            let median = percentile(&all, 50.0);
            group.value = Some(match op {
                Op::Median => median,
                Op::Percentile => percentile(&all, p.unwrap_or(50.0)),
                _ => percentile(&sorted(all.iter().map(|v| (v - median).abs())), 50.0),
            });
        }
        Op::Median | Op::Percentile | Op::Mad => {}
        Op::Rank => {
            // A stable sort keeps the capture order between equals.
            let mut ranked = values.to_vec();
            ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
            group.holders = hex(&mut ranked.into_iter().map(|(i, _)| i));
        }
        Op::Best => {
            let candidates: Vec<Candidate> = members
                .iter()
                .filter_map(|id| {
                    signals.get(id).map(|s| Candidate {
                        id: id.hex(),
                        signals: s.clone(),
                    })
                })
                .collect();
            group.measured = candidates.len();
            group.missing = hex(&mut members
                .iter()
                .copied()
                .filter(|id| !signals.contains_key(id)));
            if !candidates.is_empty() {
                let refs: Vec<&Candidate> = candidates.iter().collect();
                let (best, why) = best_of(&refs);
                group.holders = vec![best];
                group.why = Some(why);
            }
        }
    }
    group
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cull::Cull;
    use helios_vault::MemoryStore;
    use std::path::PathBuf;

    struct Fixture {
        vault: Vault<MemoryStore>,
        dir: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(name: &str) -> Fixture {
        let dir =
            std::env::temp_dir().join(format!("helios-aggregate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Fixture {
            vault: Vault::new(MemoryStore::default()),
            dir,
        }
    }

    /// A photo taken at `taken` with this culling focus (none: not
    /// measured).
    fn photo(f: &Fixture, n: u32, taken: f64, focus: Option<f64>) -> ImageId {
        let file = f.dir.join(format!("{n}.raw"));
        std::fs::write(&file, format!("frame {n}")).unwrap();
        let id = f.vault.import_file(&file, 1).unwrap();
        let signals = crate::Signals {
            taken: Some(taken),
            phash: Some("0000000000000000".into()),
            cull: Cull {
                focus,
                clip_white: focus.map(|_| [0.0; 3]),
                clip_black: focus.map(|_| [0.0; 3]),
                ..Cull::default()
            },
            ..crate::Signals::default()
        };
        let doc = serde_json::to_value(&signals).unwrap();
        f.vault.set_signals(&id, Some(&doc)).unwrap();
        let capture = helios_vault::capture::Capture {
            captured: Some(taken),
            ..Default::default()
        };
        f.vault.set_captures(&[(id, capture)], None).unwrap();
        id
    }

    fn one(a: &Aggregate) -> &Group {
        assert_eq!(a.groups.len(), 1, "{:?}", a.groups);
        &a.groups[0]
    }

    #[test]
    fn mean_min_max_and_count_over_stored_focus() {
        let f = fixture("stats");
        let ids = [
            photo(&f, 1, 10.0, Some(2.0)),
            photo(&f, 2, 11.0, Some(6.0)),
            photo(&f, 3, 12.0, Some(4.0)),
        ];
        let set = Set::Images(ids.to_vec());
        let run = |op| aggregate(&f.vault, &set, Some("focus"), op, GroupBy::All).unwrap();
        let mean = run(Op::Mean);
        assert_eq!(one(&mean).value, Some(4.0));
        assert_eq!(one(&mean).measured, 3);
        let min = run(Op::Min);
        assert_eq!(one(&min).value, Some(2.0));
        assert_eq!(one(&min).holders, [ids[0].hex()]);
        let max = run(Op::Max);
        assert_eq!(one(&max).value, Some(6.0));
        assert_eq!(one(&max).holders, [ids[1].hex()]);
        assert_eq!(one(&run(Op::Count)).value, Some(3.0));
        let rank = run(Op::Rank);
        assert_eq!(
            one(&rank).holders,
            [ids[1].hex(), ids[2].hex(), ids[0].hex()]
        );
        assert_eq!(one(&rank).members.len(), 3);
    }

    #[test]
    fn ties_all_hold_the_value_and_rank_in_capture_order() {
        let f = fixture("ties");
        // Imported out of capture order: the later shot first.
        let late = photo(&f, 1, 20.0, Some(5.0));
        let early = photo(&f, 2, 10.0, Some(5.0));
        let low = photo(&f, 3, 15.0, Some(1.0));
        let set = Set::Images(vec![late, early, low]);
        let max = aggregate(&f.vault, &set, Some("focus"), Op::Max, GroupBy::All).unwrap();
        assert_eq!(one(&max).holders, [early.hex(), late.hex()]);
        let rank = aggregate(&f.vault, &set, Some("focus"), Op::Rank, GroupBy::All).unwrap();
        assert_eq!(one(&rank).holders, [early.hex(), late.hex(), low.hex()]);
        assert_eq!(one(&rank).members, [early.hex(), low.hex(), late.hex()]);
    }

    #[test]
    fn a_photo_without_the_measure_is_skipped_and_counted() {
        let f = fixture("missing");
        let a = photo(&f, 1, 10.0, Some(2.0));
        let b = photo(&f, 2, 11.0, None);
        let c = photo(&f, 3, 12.0, Some(4.0));
        let set = Set::Images(vec![a, b, c]);
        let mean = aggregate(&f.vault, &set, Some("focus"), Op::Mean, GroupBy::All).unwrap();
        let g = one(&mean);
        assert_eq!(g.value, Some(3.0));
        assert_eq!(g.measured, 2);
        assert_eq!(g.missing, [b.hex()]);
        let count = aggregate(&f.vault, &set, Some("focus"), Op::Count, GroupBy::All).unwrap();
        assert_eq!(one(&count).value, Some(2.0));
        let all = aggregate(&f.vault, &set, None, Op::Count, GroupBy::All).unwrap();
        assert_eq!(one(&all).value, Some(3.0));
        assert!(one(&all).missing.is_empty());
        // Only the unmeasured one: no value, every member missing.
        let alone = Set::Images(vec![b]);
        let max = aggregate(&f.vault, &alone, Some("focus"), Op::Max, GroupBy::All).unwrap();
        assert_eq!(one(&max).value, None);
        assert!(one(&max).holders.is_empty());
        assert_eq!(one(&max).missing, [b.hex()]);
    }

    #[test]
    fn an_empty_set_has_no_value() {
        let f = fixture("empty");
        photo(&f, 1, 10.0, Some(2.0));
        let empty = Set::Images(Vec::new());
        for op in [Op::Min, Op::Max, Op::Mean, Op::Rank, Op::Best] {
            let a = aggregate(&f.vault, &empty, Some("focus"), op, GroupBy::All).unwrap();
            assert_eq!(a.images, 0);
            let g = one(&a);
            assert_eq!(g.value, None, "{op:?}");
            assert!(g.holders.is_empty() && g.members.is_empty(), "{op:?}");
        }
        let count = aggregate(&f.vault, &empty, None, Op::Count, GroupBy::All).unwrap();
        assert_eq!(one(&count).value, Some(0.0));
        // A search that finds nothing.
        let none = Set::Query("rating:>=5".into());
        let a = aggregate(&f.vault, &none, Some("focus"), Op::Max, GroupBy::Stack).unwrap();
        assert_eq!(a.images, 0);
        assert!(a.groups.is_empty());
    }

    #[test]
    fn a_search_picks_the_set_and_stacks_split_it() {
        let f = fixture("stacks");
        let a = photo(&f, 1, 10.0, Some(2.0));
        let b = photo(&f, 2, 11.0, Some(6.0));
        let c = photo(&f, 3, 50.0, Some(3.0));
        let stack = f.vault.create_stack(&[a, b], Some(a), "user", 2).unwrap();
        let every = Set::Query(String::new());
        let max = aggregate(&f.vault, &every, Some("focus"), Op::Max, GroupBy::Stack).unwrap();
        assert_eq!(max.images, 3);
        let keys: Vec<&str> = max.groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, [stack.id.clone(), c.hex()]);
        assert_eq!(max.groups[0].holders, [b.hex()]);
        assert_eq!(max.groups[1].value, Some(3.0));
    }

    #[test]
    fn bursts_split_the_set_and_best_reuses_the_stacks_rule() {
        let f = fixture("burst");
        let a = photo(&f, 1, 10.0, Some(2.0));
        let b = photo(&f, 2, 10.5, Some(6.0));
        let c = photo(&f, 3, 100.0, Some(3.0));
        let set = Set::Images(vec![a, b, c]);
        let best = aggregate(&f.vault, &set, None, Op::Best, GroupBy::Burst).unwrap();
        assert_eq!(best.groups.len(), 2, "{:?}", best.groups);
        assert_eq!(best.groups[0].key, "burst-1");
        assert_eq!(best.groups[0].holders, [b.hex()]);
        assert_eq!(best.groups[0].why.as_deref(), Some("sharpest, not clipped"));
        assert_eq!(best.groups[1].key, c.hex());
    }

    fn knobs(p: Option<f64>, gap: Option<f64>) -> Knobs {
        Knobs {
            percentile: p,
            gap_minutes: gap,
        }
    }

    /// `op` of the focus over `set`, one group.
    fn focus_of(f: &Fixture, set: &Set, op: Op, p: Option<f64>) -> Option<f64> {
        let a = aggregate_with(
            &f.vault,
            set,
            Some("focus"),
            op,
            GroupBy::All,
            knobs(p, None),
        );
        one(&a.unwrap()).value
    }

    #[test]
    fn median_of_an_odd_and_an_even_count() {
        let f = fixture("median");
        let ids = [
            photo(&f, 1, 10.0, Some(9.0)),
            photo(&f, 2, 11.0, Some(1.0)),
            photo(&f, 3, 12.0, Some(4.0)),
            photo(&f, 4, 13.0, Some(2.0)),
        ];
        let odd = Set::Images(ids[..3].to_vec());
        assert_eq!(focus_of(&f, &odd, Op::Median, None), Some(4.0));
        let even = Set::Images(ids.to_vec());
        assert_eq!(focus_of(&f, &even, Op::Median, None), Some(3.0));
    }

    #[test]
    fn percentiles_run_from_the_min_to_the_max() {
        let f = fixture("percentile");
        let ids = [
            photo(&f, 1, 10.0, Some(30.0)),
            photo(&f, 2, 11.0, Some(10.0)),
            photo(&f, 3, 12.0, Some(20.0)),
            photo(&f, 4, 13.0, Some(40.0)),
            photo(&f, 5, 14.0, Some(50.0)),
        ];
        let set = Set::Images(ids.to_vec());
        let at = |p| focus_of(&f, &set, Op::Percentile, Some(p));
        assert_eq!(at(0.0), Some(10.0));
        assert_eq!(at(50.0), Some(30.0));
        assert_eq!(at(100.0), Some(50.0));
        // Between ranks: 10% of the way from rank 0 (10) to 4 (50) is 14.
        assert_eq!(at(10.0), Some(14.0));
        let refused = |p: Option<f64>| {
            aggregate_with(
                &f.vault,
                &set,
                Some("focus"),
                Op::Percentile,
                GroupBy::All,
                knobs(p, None),
            )
            .is_err()
        };
        assert!(refused(None));
        assert!(refused(Some(101.0)));
        assert!(refused(Some(-1.0)));
    }

    #[test]
    fn mad_is_the_median_distance_to_the_median_and_resists_an_outlier() {
        let f = fixture("mad");
        let values = [1.0, 1.0, 2.0, 2.0, 4.0, 6.0, 9.0];
        let ids: Vec<ImageId> = values
            .iter()
            .enumerate()
            .map(|(n, v)| photo(&f, n as u32, 10.0 + n as f64, Some(*v)))
            .collect();
        // Median 2; distances 1 1 0 0 2 4 7; their median 1.
        let set = Set::Images(ids.clone());
        assert_eq!(focus_of(&f, &set, Op::Mad, None), Some(1.0));
        assert_eq!(focus_of(&f, &set, Op::Median, None), Some(2.0));
        // Ties only: no spread.
        let ties = Set::Images(ids[..2].to_vec());
        assert_eq!(focus_of(&f, &ties, Op::Mad, None), Some(0.0));
        assert_eq!(focus_of(&f, &ties, Op::Median, None), Some(1.0));
    }

    #[test]
    fn median_percentile_and_mad_skip_and_count_a_photo_without_the_measure() {
        let f = fixture("robust-missing");
        let a = photo(&f, 1, 10.0, Some(2.0));
        let b = photo(&f, 2, 11.0, None);
        let c = photo(&f, 3, 12.0, Some(4.0));
        let set = Set::Images(vec![a, b, c]);
        for (op, p, want) in [
            (Op::Median, None, 3.0),
            (Op::Percentile, Some(100.0), 4.0),
            (Op::Mad, None, 1.0),
        ] {
            let agg = aggregate_with(
                &f.vault,
                &set,
                Some("focus"),
                op,
                GroupBy::All,
                knobs(p, None),
            )
            .unwrap();
            let g = one(&agg);
            assert_eq!(g.value, Some(want), "{op:?}");
            assert_eq!(g.measured, 2, "{op:?}");
            assert_eq!(g.missing, [b.hex()], "{op:?}");
        }
        let alone = Set::Images(vec![b]);
        assert_eq!(focus_of(&f, &alone, Op::Median, None), None);
        assert_eq!(focus_of(&f, &alone, Op::Mad, None), None);
    }

    #[test]
    fn sessions_split_on_a_gap_and_join_within_it() {
        let f = fixture("sessions");
        let min = 60.0;
        // Two shots 10 minutes apart, then 45 minutes later two more.
        let a = photo(&f, 1, 0.0, Some(1.0));
        let b = photo(&f, 2, 10.0 * min, Some(3.0));
        let c = photo(&f, 3, 55.0 * min, Some(5.0));
        let d = photo(&f, 4, 60.0 * min, None);
        let set = Set::Images(vec![d, c, b, a]);
        let run = |gap| {
            aggregate_with(
                &f.vault,
                &set,
                Some("focus"),
                Op::Mean,
                GroupBy::Session,
                knobs(None, gap),
            )
            .unwrap()
        };
        let split = run(None);
        assert_eq!(split.gap_minutes, Some(DEFAULT_GAP_MINUTES));
        let keys: Vec<&str> = split.groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, ["session-1", "session-2"]);
        assert_eq!(split.groups[0].members, [a.hex(), b.hex()]);
        assert_eq!(split.groups[0].value, Some(2.0));
        assert_eq!(split.groups[1].members, [c.hex(), d.hex()]);
        assert_eq!(split.groups[1].value, Some(5.0));
        assert_eq!(split.groups[1].missing, [d.hex()]);
        // A wider gap joins them all.
        let joined = run(Some(45.0));
        assert_eq!(one(&joined).members.len(), 4);
        assert_eq!(one(&joined).value, Some(3.0));
        // A narrower one splits each shot apart.
        assert_eq!(run(Some(4.0)).groups.len(), 4);
        assert!(
            aggregate_with(
                &f.vault,
                &set,
                None,
                Op::Count,
                GroupBy::Session,
                knobs(None, Some(-1.0))
            )
            .is_err()
        );
    }

    #[test]
    fn sessions_leave_a_photo_without_a_capture_time_alone() {
        let order = [
            ImageId::from_hex(&"1".repeat(64)),
            ImageId::from_hex(&"2".repeat(64)),
        ];
        let order: Vec<ImageId> = order.into_iter().map(Result::unwrap).collect();
        let groups = sessions(&order, |id| (*id == order[0]).then_some(0.0), 60.0);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "session-1");
        assert_eq!(groups[1].0, order[1].hex());
    }

    #[test]
    fn a_text_field_or_a_missing_measure_is_refused() {
        let f = fixture("refused");
        let set = Set::Images(Vec::new());
        assert!(aggregate(&f.vault, &set, Some("lens"), Op::Max, GroupBy::All).is_err());
        assert!(aggregate(&f.vault, &set, Some("nope"), Op::Max, GroupBy::All).is_err());
        assert!(aggregate(&f.vault, &set, None, Op::Mean, GroupBy::All).is_err());
        assert_eq!(Op::parse("rank"), Some(Op::Rank));
        assert_eq!(GroupBy::parse("burst"), Some(GroupBy::Burst));
        assert_eq!(Op::parse("sum"), None);
        assert_eq!(Op::parse("mad"), Some(Op::Mad));
        assert_eq!(GroupBy::parse("session"), Some(GroupBy::Session));
        for name in Op::ALL {
            assert!(Op::parse(name).is_some(), "{name}");
        }
        for name in GroupBy::ALL {
            assert!(GroupBy::parse(name).is_some(), "{name}");
        }
    }
}
