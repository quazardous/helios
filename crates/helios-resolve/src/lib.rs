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
//! local models and any API through the user's own script), and
//! [`http::Http`] (an OpenAI-compatible or Anthropic vision endpoint, its
//! key read from the environment, or an endpoint speaking the command's
//! JSON). [`Routed`] picks one by the name a decision's question gives
//! (its `resolver` parameter); given the AI service ([`Routed::through`]),
//! it answers the rest through the AI routing map ([`model::ThroughAi`]):
//! a capability's name (`segmentation`) picks that capability's route, no
//! name the `vision` one. None is configured by default: Helios ships no
//! model and calls no network on its own.
//!
//! **A decision with a question** (see `helios_core::Module::question`)
//! sends it with the request ([`Request::question`]): an AI object mask's
//! kind or prompt. The question is part of what identifies the request, so
//! the cache never answers one question with another's answer.
//!
//! **Mask answers.** A decision whose answer declares a text parameter
//! named `mask` (`helios_module_masks::MaskObject`) is answered with an
//! image: a resolver gives the mask as a PNG, which [`mask_answer`] checks
//! and turns into the node's answer (an 8-bit grayscale PNG, base64, over
//! the request's region), with the model's confidence. A mask the node
//! cannot read is refused, and the decision keeps waiting.
//!
//! Every failure is a keyed message ([`Report::failed`]): `resolve-refused`
//! (the model gave no answer), `resolve-failed` (the call failed, or its
//! answer was refused), `resolve-not-asked` (the request could not be
//! built).

pub mod command;
pub mod http;
pub mod model;

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helios_core::{
    Engine, Graph, Image, JobSpec, Message, ParamKind, ParamSpec, Params, Region, Scheduler, Value,
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
    /// The node's question (an object to find, a prompt; empty for a
    /// decision without one).
    pub question: Params,
    /// The region of the image the decision is about, in full-resolution
    /// pixels: what the inputs show, at the request's scale.
    pub region: Region,
}

impl Request {
    /// What identifies the question: the same module, inputs, brief and
    /// question get the same answer.
    pub fn key(&self) -> String {
        let keys: Vec<&str> = self.inputs.iter().map(|i| i.key.as_str()).collect();
        format!(
            "{}|{}|{}|{}|{}",
            self.module,
            keys.join(","),
            self.brief.as_deref().unwrap_or(""),
            plain_json(&self.question),
            self.region,
        )
    }

    /// Whether the answer is a mask image (see [`mask_answer`]).
    pub fn wants_mask(&self) -> bool {
        self.answer
            .iter()
            .any(|p| p.name == MASK && matches!(p.kind, ParamKind::Text { .. }))
    }

    /// The question's text value `name`, empty when it has none.
    pub fn asked(&self, name: &str) -> &str {
        match self.question.get(name) {
            Some(Value::Text(t) | Value::Choice(t)) => t,
            _ => "",
        }
    }
}

/// The answer parameter holding a mask image.
pub const MASK: &str = "mask";

/// A parameter value as plain JSON (a number, a boolean, a string, an
/// array), as external programs read it.
pub fn value_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Float(x) => serde_json::json!(x),
        Value::Int(x) => serde_json::json!(x),
        Value::Bool(x) => serde_json::json!(x),
        Value::Choice(t) | Value::Text(t) => serde_json::json!(t),
        Value::List(l) => serde_json::json!(l),
        Value::Ints(l) => serde_json::json!(l),
    }
}

/// Parameters as a plain JSON object.
pub fn plain_json(params: &Params) -> serde_json::Value {
    serde_json::Value::Object(
        params
            .iter()
            .map(|(k, v)| (k.to_string(), value_json(v)))
            .collect(),
    )
}

/// A mask a resolver found, as the decision's answer: `png` (any PNG, its
/// luminance read, at most `helios_module_masks::object::MAX_SIDE` a side)
/// covering the request's region, and the model's confidence. Refused,
/// with why, when the PNG cannot be read.
pub fn mask_answer(request: &Request, png: &[u8], confidence: f64) -> Result<Params, String> {
    let (width, height, values) = helios_module_masks::object::decode_png(png)?;
    helios_module_masks::object::answer(width, height, &values, request.region, confidence)
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

/// Picks a resolver by the name a decision's question gives (its
/// `resolver` parameter), the default one when it names none. Given the AI
/// service, a request its resolvers do not cover goes through the AI
/// routing map: one naming a capability to that capability's route, one
/// naming none (with no default) to the `vision` route, or `segmentation`
/// for a mask.
#[derive(Default)]
pub struct Routed {
    default: Option<Arc<dyn Resolver>>,
    named: BTreeMap<String, Arc<dyn Resolver>>,
    ai: Option<Arc<helios_ai::Ai>>,
}

impl Routed {
    pub fn new() -> Self {
        Self::default()
    }

    /// The resolver for requests that name none.
    pub fn default_to(mut self, resolver: Arc<dyn Resolver>) -> Self {
        self.default = Some(resolver);
        self
    }

    /// The resolver for requests naming `name`.
    pub fn named(mut self, name: &str, resolver: Arc<dyn Resolver>) -> Self {
        self.named.insert(name.into(), resolver);
        self
    }

    /// Answers what the resolvers above do not cover through the AI
    /// service's routing map.
    pub fn through(mut self, ai: Arc<helios_ai::Ai>) -> Self {
        self.ai = Some(ai);
        self
    }
}

impl Resolver for Routed {
    fn resolve(&self, request: &Request) -> Result<Resolution, String> {
        let name = request.asked("resolver");
        if name.is_empty() {
            if let Some(resolver) = &self.default {
                return resolver.resolve(request);
            }
            let ai = self.ai.as_ref().ok_or("no resolver is configured")?;
            return model::ThroughAi::new(ai.clone(), None).resolve(request);
        }
        if let Some(resolver) = self.named.get(name) {
            return resolver.resolve(request);
        }
        match (&self.ai, helios_ai::Capability::parse(name)) {
            (Some(ai), Some(capability)) => {
                model::ThroughAi::new(ai.clone(), Some(capability)).resolve(request)
            }
            _ => Err(format!("no resolver named `{name}`")),
        }
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
    /// Calls that failed or were refused, with why (a keyed message);
    /// those stay as they were, waiting, nothing recorded.
    pub failed: Vec<(String, Message)>,
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
    let question = graph.question(id).map_err(|e| e.to_string())?;
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
        question,
        region: settings.region,
    })
}

/// A request that could not be built, as a keyed message.
fn not_asked(node: &str, reason: String) -> Message {
    Message::new("resolve-not-asked", "{ $node } was not asked: { $reason }")
        .with("node", node)
        .with("reason", reason)
}

/// Phase one: the requests for every decision waiting on a model
/// (full-AI and AI-assisted nodes that are pending), their inputs
/// rendered. Needs the graph only to read; the requests that could not be
/// built are reported, with why.
pub fn ask(
    book: &Book,
    graph: &Graph,
    engine: &Engine,
    settings: &Settings,
) -> (Vec<Request>, Vec<(String, Message)>) {
    let mut requests = Vec::new();
    let mut failed = Vec::new();
    for (node, status) in book.waiting() {
        if status != Status::Pending {
            continue;
        }
        let mode = match book.mode(node) {
            Ok(Mode::Human) => continue,
            Ok(m) => m,
            Err(e) => {
                failed.push((node.to_string(), not_asked(node, e)));
                continue;
            }
        };
        match request(engine, graph, node, mode, settings) {
            Ok(r) => requests.push(r),
            Err(e) => failed.push((node.to_string(), not_asked(node, e))),
        }
    }
    (requests, failed)
}

/// What a model call gave, with the request it answers.
pub type Called = (Request, Result<Resolution, String>);

/// Phase two: calls the resolver for each request, as scheduler jobs
/// needing one `ai_calls` each, and waits for them. Needs neither the
/// graph nor the book, so a node keeps serving while models think.
pub fn call(
    scheduler: &Scheduler,
    resolver: Arc<dyn Resolver>,
    requests: Vec<Request>,
    timeout: Duration,
) -> Result<Vec<Called>, String> {
    let (tx, rx) = mpsc::channel();
    let asked = requests.len();
    for req in requests {
        let (tx, resolver) = (tx.clone(), resolver.clone());
        scheduler.submit_spec(
            JobSpec::new(&format!("decide {}", req.node)).needs(AI_CALLS, 1),
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
    }
    drop(tx);
    let mut called = Vec::with_capacity(asked);
    for _ in 0..asked {
        match rx.recv_timeout(timeout) {
            Ok(c) => called.push(c),
            Err(_) => {
                return Err(format!(
                    "{} of {asked} model calls did not finish in {timeout:?}",
                    asked - called.len()
                ));
            }
        }
    }
    Ok(called)
}

/// Phase three: records what came back: full-AI answers in force,
/// AI-assisted ones as proposals. A failed or refused call leaves its
/// decision as it was (waiting, nothing recorded) and says why by key. An
/// answer to another question than the node's now is not recorded either:
/// the question changed while the model thought.
pub fn record(
    book: &mut Book,
    graph: &mut Graph,
    called: Vec<Called>,
    now: u64,
    report: &mut Report,
) {
    book.follow(graph);
    for (req, result) in called {
        let keys: Vec<String> = req.inputs.iter().map(|i| i.key.clone()).collect();
        let node = req.node.clone();
        let failed = |reason: String| {
            Message::new("resolve-failed", "{ $node }: { $reason }")
                .with("node", node.as_str())
                .with("reason", reason)
        };
        let recorded = match result {
            Err(e) => Err(failed(e)),
            Ok(r) if r.options.is_empty() => Err(Message::new(
                "resolve-refused",
                "{ $node }: the model gave no answer ({ $reason })",
            )
            .with("node", node.as_str())
            .with("reason", r.rationale)),
            Ok(_) if book.question(&node).is_some_and(|q| *q != req.question) => {
                Err(failed("the question changed meanwhile".into()))
            }
            Ok(r) => match req.mode {
                Mode::FullAi => book
                    .model_answer(graph, &node, r.options[0].clone(), r.model, keys, now)
                    .map(|_| true),
                _ => book
                    .propose(graph, &node, r.options, r.model, keys, now)
                    .map(|()| false),
            }
            .map_err(failed),
        };
        match recorded {
            Ok(true) => report.answered.push(req.node),
            Ok(false) => report.proposed.push(req.node),
            Err(m) => report.failed.push((req.node, m)),
        }
    }
}

/// Asks `resolver` about every decision waiting on a model and records
/// the answers: [`ask`], [`call`] and [`record`] in a row.
pub fn resolve_waiting(
    book: &mut Book,
    graph: &mut Graph,
    engine: &Engine,
    scheduler: &Scheduler,
    resolver: Arc<dyn Resolver>,
    settings: &Settings,
) -> Result<Report, String> {
    book.follow(graph);
    let (requests, failed) = ask(book, graph, engine, settings);
    let mut report = Report {
        failed,
        ..Report::default()
    };
    let called = call(scheduler, resolver, requests, settings.timeout)?;
    record(book, graph, called, settings.now, &mut report);
    Ok(report)
}
