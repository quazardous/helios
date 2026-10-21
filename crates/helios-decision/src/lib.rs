//! Decision nodes: where a network needs a judgment (see
//! `docs/module-contract.md`, "Decision nodes").
//!
//! The core knows a decision node only as a module whose parameters are an
//! answer: pending until one is recorded, then rendered from it like any
//! parameters. This crate is everything around the answer:
//!
//! - the **mode** of each node (full AI, AI-assisted, human), which a
//!   project or a run can override ([`effective_mode`]);
//! - the **book** of answers ([`Book`]): who decided (the model, a human, or
//!   a human accepting or correcting the model), the model and its version,
//!   the inputs it saw by content key, when. Answers are versioned: asking
//!   again ([`Book::reask`]) never rewrites an answer, the next one is a new
//!   version;
//! - **replay** ([`Book::replay`]): a loaded network renders from the
//!   recorded answers, offline and deterministically. No model is called
//!   here: resolvers that call models come on top and hand their answers in;
//! - the **question** of a node whose module has one (an object to find, a
//!   prompt: see `helios_core::Module::question`): kept with its record,
//!   recorded with each answer, replayed before it. An answer holds for the
//!   question it was given to: another question set on the graph
//!   ([`Book::follow`]) or through the book ([`Book::pose`]) makes the node
//!   pending again.
//!
//! Not yet: a pending decision as a waiting item in the workflow journal
//! (it needs the journal's waits and signals).

use std::collections::BTreeMap;
use std::sync::Arc;

use helios_config::{Config, Layer, Value as Setting};
use helios_core::{Bus, Engine, Event, Graph, NodeId, Params, Region};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The config key holding the project's default mode.
pub const MODE_SETTING: &str = "project.decisions.mode";

/// How a decision node is resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The model decides and the network goes on; a human may override later.
    FullAi,
    /// The model proposes, a human accepts, picks or corrects.
    AiAssisted,
    /// A human decides, with no AI involved.
    Human,
}

impl Mode {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "full_ai" => Ok(Mode::FullAi),
            "ai_assisted" => Ok(Mode::AiAssisted),
            "human" => Ok(Mode::Human),
            other => Err(format!(
                "`{other}` is not a decision mode (full_ai, ai_assisted, human)"
            )),
        }
    }
}

/// The mode a node runs in: a project or a run (or a user, for themselves)
/// overrides the node's own setting; without one, the node's setting holds,
/// and a node with none takes the configured default.
pub fn effective_mode(node: Option<Mode>, config: &Config) -> Result<Mode, String> {
    let (value, layer) = config
        .get(MODE_SETTING)
        .ok_or_else(|| format!("`{MODE_SETTING}` is not declared"))?;
    let Setting::Choice(text) = value else {
        return Err(format!("`{MODE_SETTING}` is not a choice"));
    };
    let configured = Mode::parse(&text)?;
    Ok(match node {
        Some(own) if layer < Layer::Project => own,
        _ => configured,
    })
}

/// A model and its version, as recorded with what it decided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub version: String,
}

/// Who decided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Decider {
    Model {
        model: Model,
    },
    Human {
        who: String,
    },
    /// A human took the model's proposal as it was.
    Accepted {
        who: String,
        model: Model,
    },
    /// A human changed the model's proposal.
    Corrected {
        who: String,
        model: Model,
    },
}

/// A recorded answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// 1 for the first answer of a node, then one more per answer.
    pub version: u32,
    /// The node's parameters, as decided.
    pub values: Params,
    #[serde(flatten)]
    pub decider: Decider,
    /// Content keys of the node's inputs when it was decided.
    pub inputs: Vec<String>,
    pub at: u64,
}

/// What a model proposed to a human, in AI-assisted mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    /// Ranked, best first.
    pub options: Vec<Params>,
    pub model: Model,
    pub inputs: Vec<String>,
    pub at: u64,
    /// How sure the model is, from 0 (a guess) to 1 (certain), when it
    /// said ([`Book::explain`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Why, in the model's words (empty when it gave no reason).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rationale: String,
}

/// Where a node stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Waiting for the model (full AI) or a human (human mode).
    Pending,
    /// Waiting for a human to accept, pick or correct a proposal.
    Proposed,
    Answered,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Record {
    mode: Mode,
    /// Every answer ever recorded, oldest first.
    answers: Vec<Answer>,
    /// Whether the last answer is in force (false after asking again).
    current: bool,
    proposal: Option<Proposal>,
    /// The question in force (empty for a node without one).
    #[serde(default, skip_serializing_if = "is_empty")]
    question: Params,
}

fn is_empty(p: &Params) -> bool {
    p.iter().next().is_none()
}

/// Whether an answer was given to this question: every value of the
/// question it names is the same.
fn answers(values: &Params, question: &Params) -> bool {
    question
        .iter()
        .all(|(name, v)| values.get(name).is_none_or(|a| a == v))
}

/// A decision was answered.
#[derive(Serialize)]
pub struct Answered {
    pub node: String,
    pub version: u32,
    #[serde(flatten)]
    pub decider: Decider,
}

impl Event for Answered {
    const KIND: &'static str = "decision.answered";
}

/// A model proposed answers that wait for a human.
#[derive(Serialize)]
pub struct Proposed {
    pub node: String,
    pub options: usize,
    pub model: Model,
}

impl Event for Proposed {
    const KIND: &'static str = "decision.proposed";
}

/// A decision waits for an answer.
#[derive(Serialize)]
pub struct Waiting {
    pub node: String,
    pub mode: Mode,
}

impl Event for Waiting {
    const KIND: &'static str = "decision.pending";
}

/// The decisions of an edit: modes and answers, by node name. Serializable:
/// it is stored with the edit, and a loaded edit replays it.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Book {
    nodes: BTreeMap<String, Record>,
    #[serde(skip)]
    bus: Option<Arc<Bus>>,
}

fn node_id(graph: &Graph, node: &str) -> Result<NodeId, String> {
    graph
        .find(node)
        .ok_or_else(|| format!("no node named `{node}`"))
}

/// The content keys of a node's inputs over a region: what a decision sees.
pub fn input_keys(
    engine: &Engine,
    graph: &Graph,
    node: &str,
    region: Region,
) -> Result<Vec<String>, String> {
    let id = node_id(graph, node)?;
    let inputs = graph.input_nodes(id).map_err(|e| e.to_string())?;
    inputs
        .into_iter()
        .flatten()
        .map(|src| {
            engine
                .key(graph, src, region)
                .map(|k| k.to_string())
                .map_err(|e| e.to_string())
        })
        .collect()
}

impl Book {
    pub fn new() -> Self {
        Self::default()
    }

    /// Announces answers and pending decisions on a bus.
    pub fn with_bus(mut self, bus: Arc<Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    fn publish<E: Event>(&self, event: &E) {
        if let Some(bus) = &self.bus {
            bus.publish("decision", None, event);
        }
    }

    /// Puts a decision node in the book with its mode, or changes its mode
    /// (answers are kept). A node never answered is announced as pending.
    pub fn declare(&mut self, node: &str, mode: Mode) {
        let record = self.nodes.entry(node.to_string()).or_insert(Record {
            mode,
            answers: Vec::new(),
            current: false,
            proposal: None,
            question: Params::new(),
        });
        record.mode = mode;
        if !record.current && record.proposal.is_none() {
            self.publish(&Waiting {
                node: node.into(),
                mode,
            });
        }
    }

    fn record(&self, node: &str) -> Result<&Record, String> {
        self.nodes
            .get(node)
            .ok_or_else(|| format!("`{node}` is not a declared decision"))
    }

    fn record_mut(&mut self, node: &str) -> Result<&mut Record, String> {
        self.nodes
            .get_mut(node)
            .ok_or_else(|| format!("`{node}` is not a declared decision"))
    }

    pub fn mode(&self, node: &str) -> Result<Mode, String> {
        Ok(self.record(node)?.mode)
    }

    pub fn status(&self, node: &str) -> Result<Status, String> {
        let record = self.record(node)?;
        Ok(if record.current {
            Status::Answered
        } else if record.proposal.is_some() {
            Status::Proposed
        } else {
            Status::Pending
        })
    }

    /// The answer in force, if any.
    pub fn answer(&self, node: &str) -> Option<&Answer> {
        let record = self.nodes.get(node)?;
        record.current.then(|| record.answers.last()).flatten()
    }

    /// Every answer recorded for a node, oldest first.
    pub fn history(&self, node: &str) -> &[Answer] {
        self.nodes.get(node).map_or(&[], |r| &r.answers)
    }

    pub fn proposal(&self, node: &str) -> Option<&Proposal> {
        self.nodes.get(node)?.proposal.as_ref()
    }

    /// The question of a node, as the book keeps it.
    pub fn question(&self, node: &str) -> Option<&Params> {
        self.nodes.get(node).map(|r| &r.question)
    }

    /// Records an answer, applies it to the graph, announces it. The graph
    /// checks the values first: a refused answer records nothing. The
    /// answer is recorded with the question it answers.
    fn settle(
        &mut self,
        graph: &mut Graph,
        node: &str,
        values: Params,
        decider: Decider,
        inputs: Vec<String>,
        now: u64,
    ) -> Result<u32, String> {
        let id = node_id(graph, node)?;
        self.record(node)?;
        let question = graph.question(id).map_err(|e| e.to_string())?;
        let values = values
            .iter()
            .fold(question.clone(), |q, (k, v)| q.with(k, v.clone()));
        graph.answer(id, &values).map_err(|e| e.to_string())?;
        let question = graph.question(id).map_err(|e| e.to_string())?;
        let record = self.record_mut(node)?;
        record.question = question;
        let version = record.answers.last().map_or(1, |a| a.version + 1);
        record.answers.push(Answer {
            version,
            values,
            decider: decider.clone(),
            inputs,
            at: now,
        });
        record.current = true;
        record.proposal = None;
        self.publish(&Answered {
            node: node.into(),
            version,
            decider,
        });
        Ok(version)
    }

    /// What a model decided. In full AI mode it is the answer; in
    /// AI-assisted mode it becomes a proposal with one option, for a human
    /// to accept or correct; in human mode it is refused.
    pub fn model_answer(
        &mut self,
        graph: &mut Graph,
        node: &str,
        values: Params,
        model: Model,
        inputs: Vec<String>,
        now: u64,
    ) -> Result<Status, String> {
        match self.mode(node)? {
            Mode::FullAi => {
                self.settle(graph, node, values, Decider::Model { model }, inputs, now)?;
                Ok(Status::Answered)
            }
            Mode::AiAssisted => {
                self.propose(graph, node, vec![values], model, inputs, now)?;
                Ok(Status::Proposed)
            }
            Mode::Human => Err(format!("`{node}` is decided by a human: no model answer")),
        }
    }

    /// Ranked options from a model, waiting for a human (AI-assisted mode).
    /// Each option is checked now, so a human only picks among valid ones.
    pub fn propose(
        &mut self,
        graph: &Graph,
        node: &str,
        options: Vec<Params>,
        model: Model,
        inputs: Vec<String>,
        now: u64,
    ) -> Result<(), String> {
        if self.mode(node)? != Mode::AiAssisted {
            return Err(format!("`{node}` is not AI-assisted: nothing to propose"));
        }
        if options.is_empty() {
            return Err(format!("`{node}`: a proposal needs at least one option"));
        }
        let id = node_id(graph, node)?;
        for values in &options {
            graph.check_answer(id, values).map_err(|e| e.to_string())?;
        }
        let event = Proposed {
            node: node.into(),
            options: options.len(),
            model: model.clone(),
        };
        self.record_mut(node)?.proposal = Some(Proposal {
            options,
            model,
            inputs,
            at: now,
            confidence: None,
            rationale: String::new(),
        });
        self.publish(&event);
        Ok(())
    }

    /// Says how sure the model is of its proposal (from 0 to 1, clamped)
    /// and why, for the human who answers it.
    pub fn explain(&mut self, node: &str, confidence: f64, rationale: &str) -> Result<(), String> {
        let proposal = self
            .record_mut(node)?
            .proposal
            .as_mut()
            .ok_or_else(|| format!("`{node}` has no proposal to explain"))?;
        proposal.confidence = Some(confidence.clamp(0.0, 1.0));
        proposal.rationale = rationale.to_string();
        Ok(())
    }

    /// A human takes one of the proposed options as it is.
    pub fn accept(
        &mut self,
        graph: &mut Graph,
        node: &str,
        who: &str,
        option: usize,
        now: u64,
    ) -> Result<u32, String> {
        let proposal = self
            .record(node)?
            .proposal
            .clone()
            .ok_or_else(|| format!("`{node}` has no proposal to accept"))?;
        let values = proposal.options.get(option).cloned().ok_or_else(|| {
            format!(
                "`{node}`: no option {option} (there are {})",
                proposal.options.len()
            )
        })?;
        let decider = Decider::Accepted {
            who: who.into(),
            model: proposal.model,
        };
        self.settle(graph, node, values, decider, proposal.inputs, now)
    }

    /// A human decides: corrects a proposal if there is one, decides alone
    /// otherwise. A human may always decide, whatever the mode: in full AI
    /// mode this is the review that overrides the model.
    pub fn decide(
        &mut self,
        graph: &mut Graph,
        node: &str,
        who: &str,
        values: Params,
        inputs: Vec<String>,
        now: u64,
    ) -> Result<u32, String> {
        let decider = match &self.record(node)?.proposal {
            Some(p) => Decider::Corrected {
                who: who.into(),
                model: p.model.clone(),
            },
            None => Decider::Human { who: who.into() },
        };
        self.settle(graph, node, values, decider, inputs, now)
    }

    /// Asks again: the answer in force is withdrawn (it stays in the
    /// history) and the node is pending; the next answer is a new version.
    pub fn reask(&mut self, graph: &mut Graph, node: &str) -> Result<(), String> {
        let id = node_id(graph, node)?;
        let record = self.record_mut(node)?;
        record.current = false;
        record.proposal = None;
        let mode = record.mode;
        graph.withdraw(id).map_err(|e| e.to_string())?;
        self.publish(&Waiting {
            node: node.into(),
            mode,
        });
        Ok(())
    }

    /// Sets one value of a node's question, on the graph and in the book.
    /// Another value than the one in force withdraws the answer: the node
    /// is pending again, and the answer stays in the history.
    pub fn pose(
        &mut self,
        graph: &mut Graph,
        node: &str,
        name: &str,
        value: helios_core::Value,
    ) -> Result<(), String> {
        let id = node_id(graph, node)?;
        self.record(node)?;
        graph
            .set_param(id, name, value)
            .map_err(|e| e.to_string())?;
        self.follow(graph);
        Ok(())
    }

    /// Takes in the questions as the graph holds them, set there
    /// directly (a parameter edited by a surface): a node whose answer was
    /// given to another question is pending again, and announced so.
    /// Returns the nodes made pending.
    pub fn follow(&mut self, graph: &Graph) -> Vec<String> {
        let mut withdrawn = Vec::new();
        for (node, record) in &mut self.nodes {
            let Some(id) = graph.find(node) else { continue };
            let Ok(question) = graph.question(id) else {
                continue;
            };
            if question == record.question {
                continue;
            }
            let stale = record.current
                && record
                    .answers
                    .last()
                    .is_some_and(|a| !answers(&a.values, &question));
            record.question = question;
            if stale {
                record.current = false;
                record.proposal = None;
                withdrawn.push(node.clone());
            } else if record
                .proposal
                .as_ref()
                .is_some_and(|p| p.options.iter().any(|o| !answers(o, &record.question)))
            {
                record.proposal = None;
                withdrawn.push(node.clone());
            }
        }
        for node in &withdrawn {
            let mode = self.nodes[node].mode;
            self.publish(&Waiting {
                node: node.clone(),
                mode,
            });
        }
        withdrawn
    }

    /// Applies every answer in force to a graph: a loaded edit renders from
    /// its recorded answers, with no model and no human involved. A node's
    /// question is set first; an answer given to another question is not
    /// applied (the node stays pending).
    pub fn replay(&self, graph: &mut Graph) -> Result<usize, String> {
        let mut applied = 0;
        for (node, record) in &self.nodes {
            let id = node_id(graph, node)?;
            for (name, value) in record.question.iter() {
                graph
                    .set_param(id, name, value.clone())
                    .map_err(|e| format!("replaying `{node}`: {e}"))?;
            }
            match record
                .current
                .then(|| record.answers.last())
                .flatten()
                .filter(|a| answers(&a.values, &record.question))
            {
                Some(answer) => {
                    graph
                        .answer(id, &answer.values)
                        .map_err(|e| format!("replaying `{node}`: {e}"))?;
                    applied += 1;
                }
                None => graph.withdraw(id).map_err(|e| e.to_string())?,
            }
        }
        Ok(applied)
    }

    /// Whether the inputs changed since the answer in force was given: the
    /// answer still renders (replay never re-asks), but a surface can flag
    /// it for review.
    pub fn is_stale(&self, node: &str, inputs_now: &[String]) -> bool {
        self.answer(node).is_some_and(|a| a.inputs != inputs_now)
    }

    /// Decisions waiting for someone, by node name.
    pub fn waiting(&self) -> Vec<(&str, Status)> {
        self.nodes
            .keys()
            .filter_map(|n| match self.status(n) {
                Ok(Status::Answered) | Err(_) => None,
                Ok(s) => Some((n.as_str(), s)),
            })
            .collect()
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("decisions serialize")
    }

    pub fn from_json(value: Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|e| format!("decisions: {e}"))
    }
}

impl helios_core::Describe for Book {
    fn describe(&self) -> Value {
        let nodes: serde_json::Map<String, Value> = self
            .nodes
            .iter()
            .map(|(name, r)| {
                let status = self.status(name).expect("a node of the book");
                (
                    name.clone(),
                    json!({
                        "mode": r.mode,
                        "status": status,
                        "answer": self.answer(name),
                        "versions": r.answers.len(),
                        "proposal": r.proposal,
                    }),
                )
            })
            .collect();
        json!({ "nodes": nodes })
    }
}
