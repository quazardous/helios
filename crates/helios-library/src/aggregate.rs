//! Aggregates over a set of photos: one value per group, read from what
//! the vault already stores (`docs/design/diagram-aggregates.md`, the
//! first slice of its plan B).
//!
//! The set is a search ([`helios_vault::rules::parse_text`]: `rating:>=3
//! camera:~Z`) or a list of photos. Its photos are grouped (all
//! together, by stack, or by burst with the grouping of
//! [`crate::stacks`]), then each group gets one value of a stored
//! numeric field (the culling `focus`, `clip_white`, `clip_black`, the
//! `rating`, `iso`…): its `count`, `min`, `max`, `mean`, the photos
//! ranked (`rank`), or the one proposed on top (`best`, the rule of
//! [`crate::stacks::best_of`]). Nothing is rendered or measured here:
//! a photo without the field is skipped and listed as `missing`.
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
    /// The measured photos, highest first.
    Rank,
    /// The one proposed on top of a burst ([`best_of`]: the sharpest by
    /// the culling focus, not clipped when one is not); reads the
    /// signals, not the measure.
    Best,
}

impl Op {
    pub const ALL: [&str; 6] = ["count", "min", "max", "mean", "rank", "best"];

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
}

impl GroupBy {
    pub const ALL: [&str; 3] = ["all", "stack", "burst"];

    pub fn parse(name: &str) -> Option<GroupBy> {
        serde_json::from_value(Value::from(name)).ok()
    }
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
    let groups = split(vault, &ordered, by, &signals)?;
    let groups = groups
        .into_iter()
        .map(|(key, members)| {
            let values: Vec<(ImageId, f64)> = members
                .iter()
                .filter_map(|id| facts[id].1.map(|v| (*id, v)))
                .collect();
            value_of(key, &members, &values, measure, op, &signals)
        })
        .collect();
    Ok(Aggregate {
        measure: measure.map(str::to_string),
        op,
        by,
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
        GroupBy::All => Ok(vec![("all".into(), ordered.to_vec())]),
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
        match at.get(&key) {
            Some(&i) => groups[i].1.push(*id),
            None => {
                at.insert(key.clone(), groups.len());
                groups.push((key, vec![*id]));
            }
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
    op: Op,
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
    }
}
