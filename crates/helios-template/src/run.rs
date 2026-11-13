//! A workshop run on one photo (experimental, see
//! `docs/design/workshops-on-the-fly.md`, section 4, "Run within
//! limits"): what enters the workshop on that photo goes through the
//! workshop's zones, its targets aimed, its junctions answered (from the
//! decision book first, then the AI within the run's [`Budget`]), and
//! the limits' checker ([`limits::check`]): the values to write, each
//! with its provenance, the flags, and Apply, Hold or Stop.
//!
//! Nothing here writes an edit: the caller does, as one step for the run.
//!
//! **The book.** Every junction's answer is recorded by the workshop, the
//! junction and the content of what entered the workshop ([`Book`]): the
//! same photo through the same workshop asks no model again, so a second
//! run of a set makes no call and writes the same values. A dry run says
//! how many photos of a set the book already answers ([`answered`]): the
//! cache's share in `A`.

use std::collections::BTreeMap;

use helios_core::{Engine, Image, Macro, Params};
use serde::{Deserialize, Serialize};

use crate::catalogue::Catalogue;
use crate::format::{Junction, JunctionKind, Limits, Measure, Workshop};
use crate::limits::{self, Action, Answer, Budget, Checked, INPUT_MEASURES};
use crate::workshop::{self, Aimed};

/// A junction's answer the AI gave on a photo, as the book keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recorded {
    /// A value junction's value; none for a zone's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    pub confidence: f64,
}

/// The decision book of a workshop's runs: each junction's answer by
/// [`Book::key`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Book {
    #[serde(default)]
    pub answers: BTreeMap<String, Recorded>,
}

impl Book {
    /// The key of `junction`'s answer for the workshop `workshop`
    /// (id@version) on what enters it, `input`.
    pub fn key(workshop: &str, junction: &str, input: &Image) -> String {
        format!("{workshop}|{junction}|{}", input.content_hash())
    }
}

/// Who answers a run's junctions: the AI (a provider, a plugin, the
/// user's agent). `None`: no AI is available.
pub trait Asker {
    /// `junction` asked on what enters the workshop: a value junction's
    /// value (none for a zone) and the confidence.
    fn ask(&mut self, junction: &Junction, input: &Image) -> Option<Recorded>;
}

/// No AI: every junction takes its fallback.
pub struct NoAi;

impl Asker for NoAi {
    fn ask(&mut self, _: &Junction, _: &Image) -> Option<Recorded> {
        None
    }
}

/// Where a written value comes from.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Provenance {
    /// Aimed at a target: the measure, its goal, what the nearest probe
    /// measured, the renders it took.
    Aimed {
        measure: Measure,
        goal: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        measured: Option<f64>,
        probes: usize,
    },
    /// A junction the AI answered, at this confidence.
    Ai { confidence: f64 },
    /// A junction's fallback.
    Fallback,
    /// The workshop's value, unchanged.
    Fixed,
}

/// The workshop as a run takes it.
pub struct Setup<'a> {
    pub engine: &'a Engine,
    pub catalogue: &'a Catalogue,
    /// id@version: the book's and the edits' workshop.
    pub workshop: &'a str,
    pub built: &'a Macro,
    pub block: &'a Workshop,
    pub limits: &'a Limits,
}

/// One photo through the run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PhotoRun {
    #[serde(flatten)]
    pub checked: Checked,
    pub aimed: Vec<Aimed>,
    /// By exposed parameter written.
    pub provenance: BTreeMap<String, Provenance>,
    /// AI calls answered on this photo.
    pub calls: u64,
    /// Junctions the book answered.
    pub from_book: u64,
}

impl PhotoRun {
    pub fn action(&self) -> Action {
        self.checked.action
    }
}

/// Whether the book answers every junction of the workshop on `input`.
pub fn answered(book: &Book, workshop: &str, junctions: &[Junction], input: &Image) -> bool {
    junctions.iter().all(|j| {
        book.answers
            .contains_key(&Book::key(workshop, &j.name, input))
    })
}

/// `input` (what enters the workshop on a photo) run through `setup`
/// from the values the photo holds now (`current`): zones drawn,
/// junctions answered by the book or by `asker` while `budget` lasts
/// (each answer recorded in `book`), targets aimed with the junctions'
/// values in, then the limits' check.
pub fn photo(
    setup: &Setup,
    input: &Image,
    current: &Params,
    budget: &mut Budget,
    book: &mut Book,
    asker: &mut dyn Asker,
) -> Result<PhotoRun, String> {
    let masks = workshop::zone_masks(setup.engine, setup.catalogue, &setup.block.zones, input)?;
    // The junctions first: a value junction's value is in what the
    // targets are aimed with, as the check will keep it.
    let (mut calls, mut from_book) = (0, 0);
    let mut answers = BTreeMap::new();
    let mut start = current.clone();
    for j in &setup.block.junctions {
        let key = Book::key(setup.workshop, &j.name, input);
        let answer = if let Some(r) = book.answers.get(&key) {
            from_book += 1;
            Answer::Answered {
                value: r.value,
                confidence: r.confidence,
            }
        } else if !budget.ask() {
            Answer::OverBudget
        } else {
            match asker.ask(j, input) {
                Some(r) => {
                    calls += 1;
                    book.answers.insert(key, r);
                    Answer::Answered {
                        value: r.value,
                        confidence: r.confidence,
                    }
                }
                None => Answer::NoAi,
            }
        };
        let min = setup
            .limits
            .junctions
            .get(&j.name)
            .map_or(limits::MIN_CONFIDENCE, |l| l.min_confidence);
        let value = match &answer {
            Answer::Answered {
                value: Some(v),
                confidence,
            } if *confidence >= min => Some(*v),
            _ => j.fallback,
        };
        if let (JunctionKind::Value, Some(v)) = (j.kind, value) {
            start.set(&j.name, helios_core::Value::Float(v));
        }
        answers.insert(j.name.clone(), answer);
    }
    let (aimed, params) = if setup.block.targets.is_empty() {
        (Vec::new(), start)
    } else {
        workshop::aim_rendered(
            setup.engine,
            setup.built,
            input,
            &masks,
            &start,
            &setup.block.targets,
        )?
    };
    let ranges = workshop::ranges(setup.built);
    let mut found = limits::Photo {
        answers,
        ..limits::Photo::default()
    };
    for name in ranges.keys() {
        if let Some(v) = params.get(name).and_then(helios_core::Value::as_f64) {
            found.values.insert(name.clone(), v);
        }
    }
    for a in &aimed {
        match a.measured {
            Some(m) => {
                found.missed_by.insert(a.param.clone(), m - a.goal);
            }
            None if !a.reached => found.not_monotonic.push(a.param.clone()),
            None => {}
        }
    }
    found.input = INPUT_MEASURES
        .iter()
        .filter_map(|m| workshop::measure(input, *m).map(|v| (*m, v)))
        .collect();
    let checked = limits::check(setup.limits, &setup.block.junctions, &found);
    let mut provenance = BTreeMap::new();
    for name in checked.values.keys() {
        let p = if let Some(j) = setup.block.junctions.iter().find(|j| j.name == *name) {
            match found.answers.get(&j.name) {
                _ if checked.fallbacks.contains(&j.name) => Provenance::Fallback,
                Some(Answer::Answered { confidence, .. }) if j.kind == JunctionKind::Value => {
                    Provenance::Ai {
                        confidence: *confidence,
                    }
                }
                _ => Provenance::Fixed,
            }
        } else if let Some(a) = aimed.iter().find(|a| a.param == *name) {
            Provenance::Aimed {
                measure: a.measure,
                goal: a.goal,
                measured: a.measured,
                probes: a.probes,
            }
        } else {
            Provenance::Fixed
        };
        provenance.insert(name.clone(), p);
    }
    Ok(PhotoRun {
        checked,
        aimed,
        provenance,
        calls,
        from_book,
    })
}

/// The checked values as the workshop's parameters: each exposed one
/// rounded to its step within its range, on top of `current`.
pub fn params_of(built: &Macro, current: &Params, values: &BTreeMap<String, f64>) -> Params {
    let ranges = workshop::ranges(built);
    let mut out = current.clone();
    for (name, v) in values {
        if let Some(r) = ranges.get(name) {
            out.set(name, r.value(r.round(*v)));
        }
    }
    out
}
