//! The record of a workshop session (experimental, see
//! `docs/design/workshops-on-the-fly.md`, section 2): each step of piloting
//! a workshop on a test photo, who made it, what it aimed at, and what the
//! image's zones measured before and after. The vault keeps it as a
//! document of `workshops/sessions`; the desktop writes it.
//!
//! The record refers to the history, it does not replace it: a step names
//! the value store's step (`store_step`), and the versions saved meanwhile
//! carry the session's id in their metadata. An undo is a step of its
//! own; nothing is deleted. [`Session::net`] reads the net outcome per
//! parameter, what generalising starts from.

use std::collections::BTreeMap;

use helios_core::{Engine, Graph, Image, Macro, ModuleRef, Params, Region};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::build::SOURCE_PORT;
use crate::format::{Measure, Zone};
use crate::workshop::{measure_in, zone_masks};

/// What a step was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A workshop designed from the user's words, applied.
    Design,
    /// A workshop applied from the list.
    Apply,
    /// A value set by hand, or by an agent's own request.
    Set,
    /// An AI's proposal, shown as ghost values: nothing set yet.
    Propose,
    /// A proposed value taken as it was.
    Accept,
    /// A proposed value the user replaced by their own.
    Override,
    /// A proposed value turned down: nothing set.
    Reject,
    /// A zone edited.
    Mask,
    /// A module added or removed.
    Structure,
    /// A step taken back.
    Undo,
    /// A step taken back, made again.
    Redo,
}

/// One value a step changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub param: String,
    pub old: Value,
    pub new: Value,
}

/// The zone measures around a step, on the proxy: `<zone>.<measure>`, and
/// the whole image's under `image.<measure>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Measures {
    pub before: BTreeMap<String, f64>,
    pub after: BTreeMap<String, f64>,
}

/// A step of the session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// From 1, in the order made.
    pub seq: u32,
    /// The test image it was made on: an index into [`Session::images`].
    pub image: usize,
    /// `user`, `agent:<id>`, or `model:<provider>/<model>@<version>`.
    pub author: String,
    pub kind: Kind,
    /// The words that came with it (an AI proposal's ask, the user's
    /// note); none for a plain slider move.
    #[serde(default)]
    pub aim: Option<String>,
    /// The workshop's values it changed, by exposed name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Change>,
    /// A proposal's values, by exposed name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub proposal: BTreeMap<String, Value>,
    /// The proposal an accept, an override or a reject answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<u32>,
    /// The value store's step it was, when it set values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_step: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measures: Option<Measures>,
}

impl Record {
    /// A step with nothing but its author and kind.
    pub fn new(author: &str, kind: Kind) -> Self {
        Record {
            seq: 0,
            image: 0,
            author: author.into(),
            kind,
            aim: None,
            changes: Vec::new(),
            proposal: BTreeMap::new(),
            of: None,
            store_step: None,
            measures: None,
        }
    }
}

/// A workshop session's record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// Its id, the document's key.
    pub session: String,
    /// The workshop piloted, `id@version`.
    pub workshop: String,
    /// The workshop's node in the network.
    pub node: String,
    /// The words it was designed for, if it was.
    #[serde(default)]
    pub intent: Option<String>,
    /// The test images, by vault id (or a name when the image is not in
    /// the vault).
    pub images: Vec<String>,
    /// Kept by the user: a session not kept is dropped when it ends,
    /// unless it was generalised.
    #[serde(default)]
    pub kept: bool,
    pub steps: Vec<Record>,
}

impl Session {
    /// Adds a step, numbered after the last; returns its number.
    pub fn push(&mut self, mut record: Record) -> u32 {
        let seq = self.steps.last().map_or(1, |r| r.seq + 1);
        record.seq = seq;
        self.steps.push(record);
        seq
    }

    /// A step by its number.
    pub fn step(&self, seq: u32) -> Option<&Record> {
        self.steps.iter().find(|r| r.seq == seq)
    }

    /// The measures after the last step that has them: where the next
    /// step starts from.
    pub fn last_measures(&self) -> Option<&BTreeMap<String, f64>> {
        self.steps
            .iter()
            .rev()
            .find_map(|r| r.measures.as_ref().map(|m| &m.after))
    }

    /// The measures after the last step made on test image `image` that
    /// has them.
    pub fn last_measures_on(&self, image: usize) -> Option<&BTreeMap<String, f64>> {
        self.steps
            .iter()
            .rev()
            .filter(|r| r.image == image)
            .find_map(|r| r.measures.as_ref().map(|m| &m.after))
    }

    /// The net outcome per parameter: the value each one holds after the
    /// whole session (undos included), and the step that set it.
    pub fn net(&self) -> BTreeMap<String, (Value, u32)> {
        let mut out = BTreeMap::new();
        for r in &self.steps {
            for c in &r.changes {
                out.insert(c.param.clone(), (c.new.clone(), r.seq));
            }
        }
        out
    }

    /// The proposals not answered yet: (proposal step, parameter, value).
    /// A proposal's value is answered by an accept, an override or a
    /// reject naming it and the parameter.
    pub fn open_proposals(&self) -> Vec<(u32, String, Value)> {
        let mut out = Vec::new();
        for r in self.steps.iter().filter(|r| r.kind == Kind::Propose) {
            for (param, value) in &r.proposal {
                let answered = self.steps.iter().any(|a| {
                    a.of == Some(r.seq)
                        && (a.changes.iter().any(|c| c.param == *param)
                            || a.proposal.contains_key(param))
                });
                if !answered {
                    out.push((r.seq, param.clone(), value.clone()));
                }
            }
        }
        out
    }
}

/// The measures a session records: the whole image's median, highlights
/// and clipping, and each zone's median, highlights and contrast, rounded
/// to a thousandth of a stop (or of a %). A zone without pixels has none.
pub fn measures(image: &Image, zones: &BTreeMap<String, Image>) -> BTreeMap<String, f64> {
    let round = |v: f64| (v * 1000.0).round() / 1000.0;
    let mut out = BTreeMap::new();
    for m in [Measure::LumaMedian, Measure::LumaHigh, Measure::ClipHigh] {
        if let Some(v) = measure_in(image, m, None, None) {
            out.insert(format!("image.{}", m.name()), round(v));
        }
    }
    for (name, mask) in zones {
        for m in [
            Measure::LumaMedian,
            Measure::LumaHigh,
            Measure::ZoneContrast,
        ] {
            if let Some(v) = measure_in(image, m, Some(mask), None) {
                out.insert(format!("{name}.{}", m.name()), round(v));
            }
        }
    }
    out
}

/// What `built` (a workshop) gives `input` (what enters it, the proxy)
/// with its exposed parameters at `values`, measured in its `zones`
/// (drawn from `input`, never from its sliders) as [`measures`].
pub fn measured(
    engine: &Engine,
    catalogue: &crate::catalogue::Catalogue,
    built: &Macro,
    zones: &[Zone],
    input: &Image,
    values: &Params,
) -> Result<BTreeMap<String, f64>, String> {
    let r = input.region();
    let region = Region::sized(r.width, r.height);
    let input = Image::new(region, input.channels(), input.to_vec());
    let masks = zone_masks(engine, catalogue, zones, &input)?;
    let module = ModuleRef::composite(built.clone());
    let manifest = module.manifest().clone();
    let (Some(entry), Some(exit)) = (manifest.inputs.first(), manifest.outputs.first()) else {
        return Err(format!("`{}` has no input or no output", manifest.name));
    };
    let mut graph = Graph::new();
    let src = graph.add_source("input", input);
    let node = graph
        .add_module("workshop", module, values)
        .map_err(|e| e.to_string())?;
    graph
        .connect(src, SOURCE_PORT, node, &entry.name)
        .map_err(|e| e.to_string())?;
    let out = engine
        .render(&graph, node, &exit.name, region)
        .map_err(|e| e.to_string())?;
    Ok(measures(&out, &masks))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn set(author: &str, param: &str, old: f64, new: f64) -> Record {
        Record {
            changes: vec![Change {
                param: param.into(),
                old: json!(old),
                new: json!(new),
            }],
            ..Record::new(author, Kind::Set)
        }
    }

    #[test]
    fn the_net_outcome_follows_undos_and_proposals_close_when_answered() {
        let mut s = Session::default();
        s.push(Record::new("agent:mcp", Kind::Design));
        s.push(set("user", "lift", 0.4, 0.65));
        s.push(set("user", "lift", 0.65, 0.9));
        let mut undo = set("user", "lift", 0.9, 0.65);
        undo.kind = Kind::Undo;
        s.push(undo);
        let p = s.push(Record {
            aim: Some("the face is too dark".into()),
            proposal: BTreeMap::from([("lift".into(), json!(0.8)), ("sky".into(), json!(-0.3))]),
            ..Record::new("agent:mcp", Kind::Propose)
        });
        assert_eq!(s.open_proposals().len(), 2);
        let mut accept = set("user", "lift", 0.65, 0.8);
        accept.kind = Kind::Accept;
        accept.of = Some(p);
        s.push(accept);
        s.push(Record {
            of: Some(p),
            proposal: BTreeMap::from([("sky".into(), json!(-0.3))]),
            ..Record::new("user", Kind::Reject)
        });
        assert!(s.open_proposals().is_empty());
        let net = s.net();
        assert_eq!(net["lift"], (json!(0.8), 6));
        assert!(!net.contains_key("sky"));
        // The record reads back as written.
        let back: Session = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
