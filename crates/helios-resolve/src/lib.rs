//! Resolvers: the model calls behind decision nodes (see
//! `helios-decision`, which records answers and never calls a model).
//!
//! A [`Resolver`] gets a [`Request`]: the node, the shape of the answer it
//! expects (its parameters, from its manifest), what it looks at (its
//! inputs rendered at a proxy scale, with their content keys) and the brief.
//! It answers a [`Resolution`]: options best first, a confidence, a short
//! rationale and the model that answered.
//!
//! [`resolve_waiting`] asks for every decision waiting on a model: one
//! scheduler job per node, each needing one `ai_calls`, so the scheduler's
//! limit decides how many run at once. In full-AI mode the answer is
//! recorded and the render goes on; in AI-assisted mode it is a proposal
//! that waits for a human. A failed call leaves the decision waiting and
//! says why.
//!
//! Resolvers here: [`Scripted`] (fixed answers, for tests and replays),
//! [`Cached`] (never asks twice about the same pixels), and
//! [`command::Command`] (an external program speaking JSON, which covers
//! local models and any API through the user's own script).

pub mod command;

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helios_core::{
    Engine, Graph, Image, JobSpec, ParamKind, ParamSpec, Params, Region, Scheduler, Value,
};
use helios_decision::{Book, Mode, Model, Status, input_keys};
use serde::{Deserialize, Serialize};

/// The scheduler resource model calls take.
pub const AI_CALLS: &str = "ai_calls";

/// One input of a decision, as a model sees it.
#[derive(Clone, Debug)]
pub struct Input {
    /// Content key of the input at full size: what the answer is about.
    pub key: String,
    /// The input rendered at the request's scale (linear Rec.2020).
    pub image: Image,
}

/// What a decision asks of a model.
#[derive(Clone, Debug)]
pub struct Request {
    pub node: String,
    /// The decision module and what it says it decides.
    pub module: String,
    pub description: String,
    /// The answer's shape: the module's parameters.
    pub answer: Vec<ParamSpec>,
    pub inputs: Vec<Input>,
    /// What the project wants, in words (a workshop's brief).
    pub brief: Option<String>,
    pub mode: Mode,
}

impl Request {
    /// What identifies the question: the same module, inputs and brief
    /// get the same answer.
    pub fn key(&self) -> String {
        let keys: Vec<&str> = self.inputs.iter().map(|i| i.key.as_str()).collect();
        format!(
            "{}|{}|{}",
            self.module,
            keys.join(","),
            self.brief.as_deref().unwrap_or("")
        )
    }
}

/// A model's answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    /// Best first; one is enough in full-AI mode.
    pub options: Vec<Params>,
    /// From 0 (a guess) to 1 (certain).
    pub confidence: f64,
    pub rationale: String,
    pub model: Model,
}

/// Calls a model. Runs on scheduler workers, several at once up to the
/// `ai_calls` limit.
pub trait Resolver: Send + Sync {
    fn resolve(&self, request: &Request) -> Result<Resolution, String>;
}

/// Fixed answers by decision module, for tests and replays.
pub struct Scripted {
    answers: HashMap<String, Resolution>,
    calls: Mutex<Vec<String>>,
}

impl Scripted {
    pub fn new() -> Self {
        Scripted {
            answers: HashMap::new(),
            calls: Mutex::default(),
        }
    }

    /// Answers `resolution` to every request from `module`.
    pub fn answer(mut self, module: &str, resolution: Resolution) -> Self {
        self.answers.insert(module.into(), resolution);
        self
    }

    /// The nodes asked so far, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls lock").clone()
    }
}

impl Default for Scripted {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver for Scripted {
    fn resolve(&self, request: &Request) -> Result<Resolution, String> {
        self.calls
            .lock()
            .expect("calls lock")
            .push(request.node.clone());
        self.answers
            .get(&request.module)
            .cloned()
            .ok_or_else(|| format!("no scripted answer for `{}`", request.module))
    }
}

/// Remembers answers by [`Request::key`]: a model is not asked again about
/// the same pixels and brief (asking again after a `reask` included: the
/// question has not changed).
pub struct Cached<R> {
    inner: R,
    memo: Mutex<HashMap<String, Resolution>>,
}

impl<R: Resolver> Cached<R> {
    pub fn new(inner: R) -> Self {
        Cached {
            inner,
            memo: Mutex::default(),
        }
    }

    pub fn inner(&self) -> &R {
        &self.inner
    }
}

impl<R: Resolver> Resolver for Cached<R> {
    fn resolve(&self, request: &Request) -> Result<Resolution, String> {
        let key = request.key();
        if let Some(r) = self.memo.lock().expect("memo lock").get(&key) {
            return Ok(r.clone());
        }
        let r = self.inner.resolve(request)?;
        self.memo.lock().expect("memo lock").insert(key, r.clone());
        Ok(r)
    }
}

/// A parameter value from plain JSON (a number, a boolean, a string, an
/// array), typed by the parameter's declaration: what external models write.
pub fn value_from_json(spec: &ParamSpec, v: &serde_json::Value) -> Result<Value, String> {
    let wrong = || format!("`{}`: `{v}` does not fit its kind", spec.name);
    Ok(match &spec.kind {
        ParamKind::Float { .. } => Value::Float(v.as_f64().ok_or_else(wrong)?),
        ParamKind::Int { .. } => Value::Int(v.as_i64().ok_or_else(wrong)?),
        ParamKind::Bool { .. } => Value::Bool(v.as_bool().ok_or_else(wrong)?),
        ParamKind::Choice { .. } => Value::Choice(v.as_str().ok_or_else(wrong)?.into()),
        ParamKind::Text { .. } => Value::Text(v.as_str().ok_or_else(wrong)?.into()),
        ParamKind::Floats { .. } => Value::List(
            v.as_array()
                .ok_or_else(wrong)?
                .iter()
                .map(|x| x.as_f64().ok_or_else(wrong))
                .collect::<Result<_, _>>()?,
        ),
        ParamKind::Ints { .. } => Value::Ints(
            v.as_array()
                .ok_or_else(wrong)?
                .iter()
                .map(|x| x.as_i64().ok_or_else(wrong))
                .collect::<Result<_, _>>()?,
        ),
    })
}

/// An answer's parameters from a plain JSON object, by the declarations;
/// unknown names are refused.
pub fn params_from_json(specs: &[ParamSpec], v: &serde_json::Value) -> Result<Params, String> {
    let obj = v.as_object().ok_or("an answer is a JSON object")?;
    let mut params = Params::new();
    for (name, value) in obj {
        let spec = specs
            .iter()
            .find(|s| &s.name == name)
            .ok_or_else(|| format!("no parameter `{name}`"))?;
        params = params.with(name, value_from_json(spec, value)?);
    }
    Ok(params)
}

/// How [`resolve_waiting`] asks.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The region decisions are made over, at full size.
    pub region: Region,
    /// The scale inputs are rendered at for the model (a proxy).
    pub scale: f64,
    pub brief: Option<String>,
    /// How long to wait for every call.
    pub timeout: Duration,
    /// The time recorded with answers.
    pub now: u64,
}

/// What [`resolve_waiting`] did, by node name.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    pub answered: Vec<String>,
    pub proposed: Vec<String>,
    /// Calls that failed or were refused, with why; those stay waiting.
    pub failed: Vec<(String, String)>,
}

/// The request for a waiting decision.
fn request(
    engine: &Engine,
    graph: &Graph,
    node: &str,
    mode: Mode,
    settings: &Settings,
) -> Result<Request, String> {
    let id = graph
        .find(node)
        .ok_or_else(|| format!("no node named `{node}`"))?;
    let manifest = graph
        .manifest(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("`{node}` is not a module"))?;
    let keys = input_keys(engine, graph, node, settings.region)?;
    let ports = graph.input_ports(id).map_err(|e| e.to_string())?;
    let mut inputs = Vec::new();
    for ((src, port), key) in ports.into_iter().flatten().zip(keys) {
        let image = engine
            .render_at(graph, src, &port, settings.region, settings.scale)
            .map_err(|e| e.to_string())?;
        inputs.push(Input { key, image });
    }
    Ok(Request {
        node: node.into(),
        module: manifest.name,
        description: manifest.description,
        answer: manifest.params,
        inputs,
        brief: settings.brief.clone(),
        mode,
    })
}

/// Asks `resolver` about every decision waiting on a model (full-AI and
/// AI-assisted nodes that are pending), as scheduler jobs needing one
/// `ai_calls` each, waits for them, and records what came back.
pub fn resolve_waiting(
    book: &mut Book,
    graph: &mut Graph,
    engine: &Engine,
    scheduler: &Scheduler,
    resolver: Arc<dyn Resolver>,
    settings: &Settings,
) -> Result<Report, String> {
    let mut report = Report::default();
    let waiting: Vec<String> = book
        .waiting()
        .into_iter()
        .filter(|(_, s)| *s == Status::Pending)
        .map(|(n, _)| n.to_string())
        .collect();
    let (tx, rx) = mpsc::channel();
    let mut asked = 0;
    for node in waiting {
        let mode = book.mode(&node)?;
        if mode == Mode::Human {
            continue;
        }
        let req = match request(engine, graph, &node, mode, settings) {
            Ok(r) => r,
            Err(e) => {
                report.failed.push((node, e));
                continue;
            }
        };
        let (tx, resolver) = (tx.clone(), resolver.clone());
        scheduler.submit_spec(
            JobSpec::new(&format!("decide {node}")).needs(AI_CALLS, 1),
            move |token| {
                let result = if token.is_cancelled() {
                    Err("cancelled".to_string())
                } else {
                    resolver.resolve(&req)
                };
                let _ = tx.send((req, result));
                Ok(())
            },
        );
        asked += 1;
    }
    drop(tx);
    for _ in 0..asked {
        let Ok((req, result)) = rx.recv_timeout(settings.timeout) else {
            return Err(format!(
                "{} of {asked} model calls did not finish in {:?}",
                asked - report.answered.len() - report.proposed.len() - report.failed.len(),
                settings.timeout
            ));
        };
        let keys: Vec<String> = req.inputs.iter().map(|i| i.key.clone()).collect();
        let recorded = result.and_then(|r| match req.mode {
            Mode::FullAi => {
                let best = r
                    .options
                    .first()
                    .cloned()
                    .ok_or("the model gave no option")?;
                book.model_answer(graph, &req.node, best, r.model, keys, settings.now)
                    .map(|_| true)
            }
            _ => book
                .propose(graph, &req.node, r.options, r.model, keys, settings.now)
                .map(|_| false),
        });
        match recorded {
            Ok(true) => report.answered.push(req.node),
            Ok(false) => report.proposed.push(req.node),
            Err(e) => report.failed.push((req.node, e)),
        }
    }
    Ok(report)
}
