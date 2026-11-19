//! The AI service: providers, routing, prompts and the local runtime
//! behind one handle ([`Ai`]), every model call a scheduler job, published
//! on the AI bus and logged.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use helios_core::{Bus, CancelToken, Envelope, JobId, JobSpec, JobState, Scheduler};
use serde::Serialize;
use serde_json::{Value, json};

use crate::Capability;
use crate::images;
use crate::limits::{Clock, Limiter, Permit, RealClock, RunCeiling};
use crate::local::{self, Present, RealSystem, System};
use crate::prompts::{Library, Prompt, PromptRef};
use crate::provider::{
    self, ChatReply, ChatRequest, ImageData, ModelInfo, ModelProvider, Parts, ProviderConfig,
    ProviderKind, SegmentReply, SegmentRequest,
};
use crate::routing::{Overrides, Route, RouteFrom};
use crate::secrets::{KeySource, Secrets, SystemKeyring};
use crate::store::{AiConfig, DEFAULT_PROVIDER, Paths};
use crate::transport::{Transport, Ureq};
use helios_extract::{Adjust, Answer, ExtractorDef};

/// The scheduler resource model calls take.
pub const AI_CALLS: &str = "ai_calls";
/// Model call jobs at once, over every provider (each provider's own
/// limits apply inside).
const AI_JOBS: u64 = 8;
/// One pull at a time.
const AI_PULL: &str = "ai_pull";
/// One bench at a time.
const AI_BENCH: &str = "ai_bench";
/// Where the service's events come from on its bus.
const SOURCE: &str = "ai";
/// The largest model size a pull accepts: above it `size_mib` is a mistake,
/// and the disk check would overflow.
const MAX_MODEL_MIB: u64 = 4 * 1024 * 1024;

/// What a call is for, for the log.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CallMeta {
    /// `id@version` of the prompt, if any.
    pub prompt: Option<String>,
    pub prompt_hash: Option<String>,
    /// Who asked: `try`, `bench`, `decision sky`, a feature's name.
    pub purpose: String,
    /// Where the route came from, when an override chose it or the
    /// caller asked to record it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_from: Option<RouteFrom>,
    /// 1 for the first try, 2 for the corrective retry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
}

/// One model call, as logged and published (`helios.ai.call`). Never the images,
/// the text or the key.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CallRecord {
    /// Seconds since the Unix epoch.
    pub at: u64,
    pub job: Option<u64>,
    pub provider: String,
    pub kind: ProviderKind,
    pub model: String,
    pub capability: Capability,
    #[serde(flatten)]
    pub meta: CallMeta,
    pub remote: bool,
    pub images: usize,
    /// Image bytes sent.
    pub image_bytes: usize,
    pub ms: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The model as the runtime named it in its answer (its exact
    /// version or digest when it gives one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered: Option<String>,
}

/// A prompt run for a feature or a diagram: what to send, how to route
/// it, and how to check the answer.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptCall {
    /// `id` or `id@version` (pin it for reproducible work).
    pub reference: PromptRef,
    pub input: Value,
    pub images: Vec<Vec<u8>>,
    /// Run, node and diagram overrides of the routing map.
    pub overrides: Overrides,
    /// When set, the answer is also checked against the extractor's keys
    /// (values outside a finite list dropped or mapped, and counted).
    pub extractor: Option<ExtractorDef>,
    /// Who asks, for the call log.
    pub purpose: String,
    /// The run the call belongs to: its remote calls count against the
    /// run's ceiling (`limits.run_remote_calls`).
    pub run: Option<String>,
}

impl PromptCall {
    pub fn new(reference: PromptRef, input: Value) -> Self {
        PromptCall {
            reference,
            input,
            images: Vec::new(),
            overrides: Overrides::default(),
            extractor: None,
            purpose: "run".into(),
            run: None,
        }
    }
}

/// How many values the extractor check adjusted instead of refusing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AnswerCounts {
    /// Outside a finite list: dropped.
    pub dropped: usize,
    /// Outside a finite list that accepts `other`: kept as `other`.
    pub mapped_to_other: usize,
    /// Below the extractor's confidence floor: dropped.
    pub below_floor: usize,
    /// Outside a semi-finite list: kept as a new value.
    pub new: usize,
}

impl AnswerCounts {
    fn of(answer: &Answer) -> Self {
        AnswerCounts {
            dropped: answer.count(Adjust::Dropped),
            mapped_to_other: answer.count(Adjust::MappedToOther),
            below_floor: answer.count(Adjust::BelowFloor),
            new: answer.count(Adjust::New),
        }
    }
}

/// A prompt that answered and fitted, with the route that answered.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PromptRun {
    /// `id@version`.
    pub prompt: String,
    pub hash: String,
    pub route: Route,
    pub from: RouteFrom,
    /// The model as the runtime named it (see [`CallRecord::answered`]);
    /// the route's model when the runtime says nothing.
    pub answered: String,
    /// The answer, checked against the prompt's output schema.
    pub value: Value,
    /// The extractor's typed values, when [`PromptCall::extractor`] is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
    pub counts: AnswerCounts,
    /// 1, or 2 when the corrective retry was needed.
    pub attempts: u32,
    pub ms: u64,
}

/// One try of a prompt: the raw reply (if any) and why it did not fit.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Attempt {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    pub errors: Vec<String>,
}

/// A prompt that did not give a fitting answer, with every try.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PromptFailure {
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
    pub attempts: Vec<Attempt>,
}

impl std::fmt::Display for PromptFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: ", self.prompt)?;
        for (i, a) in self.attempts.iter().enumerate() {
            if i > 0 {
                f.write_str("; after the retry: ")?;
            }
            f.write_str(&a.errors.join("; "))?;
        }
        Ok(())
    }
}

/// A model node of a run (a capability, maybe a prompt, its overrides),
/// for the check before the run ([`Ai::check_run`]).
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct Need {
    /// The node's name in the diagram, for the messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    pub capability: Capability,
    /// `id` or `id@version`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default)]
    pub overrides: Overrides,
}

impl Need {
    pub fn new(capability: Capability) -> Self {
        Need {
            node: None,
            capability,
            prompt: None,
            overrides: Overrides::default(),
        }
    }

    /// How the messages name it: its node, else its prompt or capability.
    pub fn label(&self) -> String {
        match (&self.node, &self.prompt) {
            (Some(n), Some(p)) => format!("node `{n}` ({}, {p})", self.capability),
            (Some(n), None) => format!("node `{n}` ({})", self.capability),
            (None, Some(p)) => format!("`{p}` ({})", self.capability),
            (None, None) => self.capability.to_string(),
        }
    }
}

/// What the check found for one [`Need`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeedCheck {
    pub need: Need,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<RouteFrom>,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Whether the route's provider is off this machine (unknown when
    /// there is no route).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<bool>,
    /// Images sent per item (the prompt's, else 1 for vision and
    /// segmentation).
    pub images_per_item: u32,
}

/// What leaves the machine for one node, stated before a run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Leaving {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// To whom: the provider's id, its URL, the model.
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub model: String,
    pub images_per_item: u32,
    /// Over the run, when its item count is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<u64>,
    /// The longest side of an image sent (`privacy.remote_image_side`).
    pub image_side: u32,
}

/// The check before a run: each model node's route, and whether the run
/// may start.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunCheck {
    /// Every node can be served: the run may start.
    pub ok: bool,
    pub nodes: Vec<NeedCheck>,
    /// One line per node nothing serves: the node, the reason.
    pub refused: Vec<String>,
    /// Where to fix it, when something is refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// The nodes whose images go off this machine, and to whom.
    pub leaves: Vec<Leaving>,
    /// Remote calls the run will make at least (one per item and remote
    /// node), when its item count is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_calls: Option<u64>,
    /// The run's ceiling on remote calls (0: none).
    pub remote_ceiling: u64,
}

impl RunCheck {
    /// The refusal of a submission, naming each node nothing serves.
    pub fn refusal(&self) -> Option<String> {
        (!self.ok).then(|| {
            format!(
                "the run cannot start: {}. {}",
                self.refused.join("; "),
                self.fix.as_deref().unwrap_or_default()
            )
        })
    }
}

/// Where a refused run's routes are fixed.
const FIX: &str = "Set the routes in the AI settings (ai.toml; helios_ai_routing_set, \
     helios_ai_providers_add, helios_ai_local_pull), and see helios_ai_doctor";

/// What [`Ai::try_prompt`] found: the raw answer and the validated one,
/// or why not. Nothing is applied.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TryResult {
    pub prompt: String,
    pub hash: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub raw: Option<String>,
    pub validated: Option<Value>,
    pub errors: Vec<String>,
    pub ms: u64,
    /// Always false: trying never applies anything.
    pub applied: bool,
}

impl TryResult {
    pub fn ok(&self) -> bool {
        self.validated.is_some() && self.errors.is_empty()
    }
}

/// Who makes a call, for its limits: its job, how it notices a cancel,
/// the run it belongs to, and where it says it is waiting on a limit.
#[derive(Clone, Default)]
struct Slot {
    job: Option<u64>,
    token: Option<CancelToken>,
    run: Option<String>,
    watch: Option<Watch>,
}

impl Slot {
    fn job(job: u64, token: &CancelToken, watch: &Watch) -> Self {
        Slot {
            job: Some(job),
            token: Some(token.clone()),
            run: None,
            watch: Some(watch.clone()),
        }
    }
}

/// Since when a job's call has been going (its timeout runs from there);
/// `None` while it waits on a provider's limit.
type Watch = Arc<Mutex<Option<Instant>>>;

/// How often a job's waiter looks again while the job waits on a limit.
const WATCH_POLL: Duration = Duration::from_millis(500);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Everything but the scheduler: what a job may hold.
struct Core {
    paths: Paths,
    config: Mutex<AiConfig>,
    library: Mutex<Library>,
    transport: Arc<dyn Transport>,
    secrets: Arc<dyn Secrets>,
    system: Arc<dyn System>,
    bus: Arc<Bus>,
    /// A finished job's result, by job id.
    results: Mutex<BTreeMap<u64, Value>>,
    log: Mutex<()>,
    limiter: Arc<Limiter>,
}

/// The AI service.
pub struct Ai {
    core: Arc<Core>,
    scheduler: Scheduler,
}

impl Core {
    fn config(&self) -> MutexGuard<'_, AiConfig> {
        self.config.lock().expect("config lock")
    }

    fn parts(&self) -> Parts {
        Parts {
            transport: self.transport.clone(),
            secrets: self.secrets.clone(),
        }
    }

    fn entry(&self, id: &str) -> Result<ProviderConfig, String> {
        self.config()
            .providers()
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("no provider `{id}`"))
    }

    fn provider(&self, id: &str) -> Result<Arc<dyn ModelProvider>, String> {
        provider::build(&self.entry(id)?, &self.parts())
    }

    fn image_side(&self, remote: bool) -> u32 {
        let config = self.config();
        if remote {
            config.privacy.remote_image_side
        } else {
            config.privacy.local_image_side
        }
    }

    /// A slot for a call of `capability` to `provider`: waits while the
    /// provider is at its limits; refused past the run's remote ceiling.
    fn permit(
        &self,
        provider: &dyn ModelProvider,
        capability: Capability,
        slot: &Slot,
    ) -> Result<Permit, String> {
        let (rule, ceiling) = {
            let config = self.config();
            (
                config
                    .limits
                    .rule(provider.id(), provider.remote(), capability),
                config.limits.run_remote_calls,
            )
        };
        let run = slot
            .run
            .as_deref()
            .filter(|_| provider.remote())
            .map(|run| RunCeiling { run, ceiling });
        let token = slot.token.clone();
        let watch = slot.watch.clone();
        self.limiter.acquire(
            provider.id(),
            rule,
            run,
            &|| token.as_ref().is_some_and(CancelToken::is_cancelled),
            &|waiting| {
                if let Some(w) = &watch {
                    *w.lock().expect("watch lock") = (!waiting).then(Instant::now);
                }
            },
        )
    }

    fn record(&self, record: &CallRecord) {
        let line = serde_json::to_value(record).expect("records serialize");
        self.bus
            .publish_value(SOURCE, None, "helios.ai.call", line.clone());
        let _guard = self.log.lock().expect("log lock");
        if let Some(dir) = self.paths.log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.paths.log)
        {
            let _ = writeln!(file, "{line}");
        }
    }

    /// A chat call, logged; images downscaled and re-encoded first.
    fn chat(
        &self,
        route: &Route,
        mut request: ChatRequest,
        meta: CallMeta,
        slot: &Slot,
    ) -> Result<ChatReply, String> {
        let provider = self.provider(&route.provider)?;
        let capability = if request.images.is_empty() {
            if request.schema.is_some() {
                Capability::Structured
            } else {
                Capability::Chat
            }
        } else {
            Capability::Vision
        };
        let declared = provider.capabilities();
        let needed_ok = declared.contains(&capability)
            || (capability == Capability::Structured && declared.contains(&Capability::Chat));
        if !needed_ok {
            return Err(provider::unsupported(provider.id(), capability));
        }
        let side = self.image_side(provider.remote());
        request.images = request
            .images
            .iter()
            .map(|i| images::prepare(&i.bytes, side))
            .collect::<Result<_, _>>()?;
        request.model.clone_from(&route.model);
        let _permit = self.permit(provider.as_ref(), capability, slot)?;
        let started = Instant::now();
        let result = provider.chat(&request);
        self.record(&CallRecord {
            at: now(),
            job: slot.job,
            provider: provider.id().to_string(),
            kind: provider.kind(),
            model: route.model.clone(),
            capability,
            meta,
            remote: provider.remote(),
            images: request.images.len(),
            image_bytes: request.images.iter().map(|i| i.bytes.len()).sum(),
            ms: started.elapsed().as_millis() as u64,
            ok: result.is_ok(),
            error: result.as_ref().err().cloned(),
            answered: result
                .as_ref()
                .ok()
                .map(|r| r.model.clone())
                .filter(|m| !m.is_empty()),
        });
        result
    }

    fn embed(
        &self,
        route: &Route,
        texts: &[String],
        meta: CallMeta,
        slot: &Slot,
    ) -> Result<Vec<Vec<f32>>, String> {
        let provider = self.provider(&route.provider)?;
        let _permit = self.permit(provider.as_ref(), Capability::Embeddings, slot)?;
        let started = Instant::now();
        let result = provider.embed(&route.model, texts);
        self.record(&CallRecord {
            at: now(),
            job: slot.job,
            provider: provider.id().to_string(),
            kind: provider.kind(),
            model: route.model.clone(),
            capability: Capability::Embeddings,
            meta,
            remote: provider.remote(),
            images: 0,
            image_bytes: 0,
            ms: started.elapsed().as_millis() as u64,
            ok: result.is_ok(),
            error: result.as_ref().err().cloned(),
            answered: None,
        });
        result
    }

    fn segment(
        &self,
        route: &Route,
        mut request: SegmentRequest,
        meta: CallMeta,
        slot: &Slot,
    ) -> Result<SegmentReply, String> {
        let provider = self.provider(&route.provider)?;
        request.image = images::prepare(&request.image.bytes, self.image_side(provider.remote()))?;
        request.model.clone_from(&route.model);
        let _permit = self.permit(provider.as_ref(), Capability::Segmentation, slot)?;
        let started = Instant::now();
        let result = provider.segment(&request);
        self.record(&CallRecord {
            at: now(),
            job: slot.job,
            provider: provider.id().to_string(),
            kind: provider.kind(),
            model: route.model.clone(),
            capability: Capability::Segmentation,
            meta,
            remote: provider.remote(),
            images: 1,
            image_bytes: request.image.bytes.len(),
            ms: started.elapsed().as_millis() as u64,
            ok: result.is_ok(),
            error: result.as_ref().err().cloned(),
            answered: None,
        });
        result
    }

    /// The route for a capability (or a prompt), or why there is none.
    fn route(&self, capability: Capability, prompt: Option<&str>) -> Result<Route, String> {
        self.resolve(capability, prompt, &Overrides::default())
            .map(|(r, _)| r)
    }

    /// The route once `overrides` are applied, and where it came from.
    fn resolve(
        &self,
        capability: Capability,
        prompt: Option<&str>,
        overrides: &Overrides,
    ) -> Result<(Route, RouteFrom), String> {
        let routing = self.config().routing.clone();
        routing.resolve(capability, prompt, overrides).map_err(|e| {
            if e.starts_with("nothing is routed") {
                format!(
                    "{e}: set a route (helios_ai_routing_set) or let the bench fill them \
                         (helios_ai_local_bench with apply)"
                )
            } else {
                e
            }
        })
    }

    /// The prompt's text and images, or why it cannot be sent.
    fn prepare(
        prompt: &Prompt,
        input: &Value,
        images: &[Vec<u8>],
    ) -> Result<(String, Vec<ImageData>), String> {
        let text = prompt.render(input)?;
        if prompt.capability == Capability::Embeddings
            || prompt.capability == Capability::Segmentation
        {
            return Err(format!(
                "a {} prompt is not a chat: it is not tried",
                prompt.capability
            ));
        }
        let wanted = prompt.images as usize;
        if images.len() < wanted {
            return Err(format!(
                "{} takes {wanted} image(s), {} given",
                prompt.reference(),
                images.len()
            ));
        }
        let images = images[..wanted]
            .iter()
            .map(|b| ImageData {
                mime: "image/png".into(),
                bytes: b.clone(),
            })
            .collect();
        Ok((text, images))
    }

    fn request(
        prompt: &Prompt,
        route: &Route,
        text: String,
        images: Vec<ImageData>,
    ) -> ChatRequest {
        ChatRequest {
            model: route.model.clone(),
            system: (!prompt.system.is_empty()).then(|| prompt.system.clone()),
            prompt: text,
            images,
            schema: prompt.output.clone(),
            json: prompt.output.is_some(),
            temperature: Some(prompt.temperature),
            seed: prompt.seed,
            max_tokens: 0,
        }
    }

    /// A reply checked against the prompt's schema, then the extractor's
    /// keys.
    fn check(
        prompt: &Prompt,
        extractor: Option<&ExtractorDef>,
        raw: &str,
    ) -> Result<(Value, Option<Answer>), Vec<String>> {
        let value = prompt.validate(raw)?;
        match extractor {
            None => Ok((value, None)),
            Some(def) => helios_extract::validate(def, &value).map(|a| (value, Some(a))),
        }
    }

    /// Runs a prompt through `route`; an answer that does not fit is sent
    /// back once with its errors (helios-extract's `retry_note`).
    fn run_prompt(
        &self,
        prompt: &Prompt,
        call: &PromptCall,
        (route, from): (Route, RouteFrom),
        slot: &Slot,
    ) -> Result<PromptRun, PromptFailure> {
        let mut failure = PromptFailure {
            prompt: prompt.reference(),
            route: Some(route.clone()),
            attempts: Vec::new(),
        };
        let (text, images) = match Self::prepare(prompt, &call.input, &call.images) {
            Ok(p) => p,
            Err(e) => {
                failure.attempts.push(Attempt {
                    raw: None,
                    errors: vec![e],
                });
                return Err(failure);
            }
        };
        let started = Instant::now();
        for attempt in 1..=2u32 {
            let text = match failure.attempts.last() {
                Some(last) => format!("{text}\n\n{}", helios_extract::retry_note(&last.errors)),
                None => text.clone(),
            };
            let meta = CallMeta {
                prompt: Some(prompt.reference()),
                prompt_hash: Some(prompt.hash()),
                purpose: call.purpose.clone(),
                route_from: Some(from.clone()),
                attempt: Some(attempt),
            };
            let request = Self::request(prompt, &route, text, images.clone());
            let reply = match self.chat(&route, request, meta, slot) {
                Ok(r) => r,
                Err(e) => {
                    // Not a misfit: the provider failed, nothing to correct.
                    failure.attempts.push(Attempt {
                        raw: None,
                        errors: vec![e],
                    });
                    return Err(failure);
                }
            };
            match Self::check(prompt, call.extractor.as_ref(), &reply.text) {
                Ok((value, answer)) => {
                    return Ok(PromptRun {
                        prompt: prompt.reference(),
                        hash: prompt.hash(),
                        answered: if reply.model.is_empty() {
                            route.model.clone()
                        } else {
                            reply.model
                        },
                        route,
                        from,
                        value,
                        counts: answer.as_ref().map(AnswerCounts::of).unwrap_or_default(),
                        answer,
                        attempts: attempt,
                        ms: started.elapsed().as_millis() as u64,
                    });
                }
                Err(errors) => failure.attempts.push(Attempt {
                    raw: Some(reply.text),
                    errors,
                }),
            }
        }
        Err(failure)
    }

    fn try_prompt(
        &self,
        prompt: &Prompt,
        input: Option<Value>,
        images: &[Vec<u8>],
        route: Option<Route>,
        purpose: &str,
        slot: &Slot,
    ) -> TryResult {
        let mut result = TryResult {
            prompt: prompt.reference(),
            hash: prompt.hash(),
            ..TryResult::default()
        };
        let input = input
            .or_else(|| prompt.example.clone())
            .unwrap_or_else(|| json!({}));
        let (text, images) = match Self::prepare(prompt, &input, images) {
            Ok(p) => p,
            Err(e) => {
                result.errors.push(e);
                return result;
            }
        };
        let route = match route.map_or_else(|| self.route(prompt.capability, Some(&prompt.id)), Ok)
        {
            Ok(r) => r,
            Err(e) => {
                result.errors.push(e);
                return result;
            }
        };
        result.provider = Some(route.provider.clone());
        result.model = Some(route.model.clone());
        let request = Self::request(prompt, &route, text, images);
        let meta = CallMeta {
            prompt: Some(prompt.reference()),
            prompt_hash: Some(prompt.hash()),
            purpose: purpose.into(),
            ..CallMeta::default()
        };
        let started = Instant::now();
        match self.chat(&route, request, meta, slot) {
            Ok(reply) => {
                result.ms = started.elapsed().as_millis() as u64;
                match prompt.validate(&reply.text) {
                    Ok(v) => result.validated = Some(v),
                    Err(errors) => result.errors = errors,
                }
                result.raw = Some(reply.text);
            }
            Err(e) => {
                result.ms = started.elapsed().as_millis() as u64;
                result.errors.push(e);
            }
        }
        result
    }

    fn progress(&self, job: u64, name: &str, details: Value) {
        let mut payload = json!({ "job": job, "name": name });
        if let (Some(p), Some(d)) = (payload.as_object_mut(), details.as_object()) {
            p.extend(d.clone());
        }
        self.bus
            .publish_value(SOURCE, None, "helios.job.progress", payload);
    }

    fn finish(&self, job: u64, result: Value) {
        self.results
            .lock()
            .expect("results lock")
            .insert(job, result);
    }
}

/// What [`Ai::bench`] measures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BenchRequest {
    /// Prompt references; every shipped and user prompt's latest version
    /// when empty.
    pub prompts: Vec<String>,
    /// The image vision prompts look at; a test card when none.
    pub image: Option<Vec<u8>>,
    /// Route each prompt (and each unrouted capability) to the fastest
    /// model that answered correctly.
    pub apply: bool,
    /// Runs per prompt and model (the median is kept).
    pub runs: u32,
    /// Only these providers; every local one when empty.
    pub providers: Vec<String>,
}

impl Ai {
    /// The service on this machine: the real network, keyring and system.
    pub fn open(paths: Paths) -> Result<Ai, String> {
        Ai::with_parts(
            paths,
            Arc::new(Ureq),
            Arc::new(SystemKeyring),
            Arc::new(RealSystem),
        )
    }

    /// The service over given parts (fakes, in tests).
    pub fn with_parts(
        paths: Paths,
        transport: Arc<dyn Transport>,
        secrets: Arc<dyn Secrets>,
        system: Arc<dyn System>,
    ) -> Result<Ai, String> {
        Ai::with_clock(paths, transport, secrets, system, Arc::new(RealClock))
    }

    /// The service over given parts and a clock (limits per minute, in
    /// tests).
    pub fn with_clock(
        paths: Paths,
        transport: Arc<dyn Transport>,
        secrets: Arc<dyn Secrets>,
        system: Arc<dyn System>,
        clock: Arc<dyn Clock>,
    ) -> Result<Ai, String> {
        let config = AiConfig::load(&paths.config)?;
        let library = Library::load(Some(&paths.prompts));
        // Calls at once over every provider; each provider's own limits
        // ([`crate::limits`]) are kept by the limiter inside the jobs.
        let calls = AI_JOBS.max(config.limits.calls);
        let bus = Arc::new(Bus::new());
        let workers = usize::try_from(calls).unwrap_or(1) + 2;
        let scheduler = Scheduler::with_resources(
            bus.clone(),
            workers,
            &[(AI_CALLS, calls), (AI_PULL, 1), (AI_BENCH, 1)],
        );
        Ok(Ai {
            core: Arc::new(Core {
                paths,
                config: Mutex::new(config),
                library: Mutex::new(library),
                transport,
                secrets,
                system,
                bus,
                results: Mutex::default(),
                log: Mutex::new(()),
                limiter: Limiter::new(clock),
            }),
            scheduler,
        })
    }

    pub fn bus(&self) -> &Arc<Bus> {
        &self.core.bus
    }

    pub fn paths(&self) -> &Paths {
        &self.core.paths
    }

    pub fn config(&self) -> AiConfig {
        self.core.config().clone()
    }

    fn save(&self, config: &AiConfig) -> Result<(), String> {
        config.save(&self.core.paths.config)
    }

    /// The provider with id `id`.
    pub fn provider(&self, id: &str) -> Result<Arc<dyn ModelProvider>, String> {
        self.core.provider(id)
    }

    /// Runs `work` as a scheduler job needing `needs`, and waits for it.
    fn job<T: Send + 'static>(
        &self,
        name: &str,
        needs: &[(&str, u64)],
        timeout: Duration,
        work: impl FnOnce(&CancelToken, u64, &Watch) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (tx, rx) = mpsc::channel();
        let watch: Watch = Arc::new(Mutex::new(Some(Instant::now())));
        let held = watch.clone();
        let id = self.submit(name, needs, move |token, id| {
            let result = work(token, id, &held);
            let failed = result.as_ref().err().cloned();
            let _ = tx.send(result);
            failed.map_or(Ok(()), Err)
        });
        // The timeout runs while the job is queued or calling, not while it
        // waits on a provider's limit.
        loop {
            let since = *watch.lock().expect("watch lock");
            let wait = match since {
                None => WATCH_POLL,
                Some(t) => timeout.saturating_sub(t.elapsed()),
            };
            if wait.is_zero() {
                self.scheduler.cancel(id);
                return Err(format!("no answer in {} s", timeout.as_secs()));
            }
            match rx.recv_timeout(wait) {
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!(
                        "the job did not run ({:?})",
                        self.scheduler.job(id)
                    ));
                }
            }
        }
    }

    /// Queues `work` as a job; it is handed its own id.
    fn submit(
        &self,
        name: &str,
        needs: &[(&str, u64)],
        work: impl FnOnce(&CancelToken, u64) -> Result<(), String> + Send + 'static,
    ) -> JobId {
        let mut spec = JobSpec::new(name);
        for (resource, amount) in needs {
            spec = spec.needs(resource, *amount);
        }
        let slot: Arc<Mutex<Option<JobId>>> = Arc::default();
        let held = slot.clone();
        let mut guard = slot.lock().expect("slot lock");
        let id = self.scheduler.submit_spec(spec, move |token| {
            let id = held.lock().expect("slot lock").map_or(0, |j| j.0);
            if token.is_cancelled() {
                return Err("cancelled".into());
            }
            work(token, id)
        });
        *guard = Some(id);
        id
    }

    fn timeout_of(&self, route: &Route) -> Duration {
        self.core
            .entry(&route.provider)
            .map_or(Duration::from_mins(5), |e| e.timeout())
            + Duration::from_secs(30)
    }

    /// A chat call through `route`, as a job.
    pub fn chat(
        &self,
        route: &Route,
        request: ChatRequest,
        meta: CallMeta,
    ) -> Result<ChatReply, String> {
        let core = self.core.clone();
        let route_owned = route.clone();
        self.job(
            &format!("ai chat {}/{}", route.provider, route.model),
            &[(AI_CALLS, 1)],
            self.timeout_of(route),
            move |token, job, watch| {
                core.chat(&route_owned, request, meta, &Slot::job(job, token, watch))
            },
        )
    }

    /// Embeddings through `route`, as a job.
    pub fn embed(
        &self,
        route: &Route,
        texts: Vec<String>,
        meta: CallMeta,
    ) -> Result<Vec<Vec<f32>>, String> {
        let core = self.core.clone();
        let route_owned = route.clone();
        self.job(
            &format!("ai embed {}/{}", route.provider, route.model),
            &[(AI_CALLS, 1)],
            self.timeout_of(route),
            move |token, job, watch| {
                core.embed(&route_owned, &texts, meta, &Slot::job(job, token, watch))
            },
        )
    }

    /// A mask through `route`, as a job.
    pub fn segment(
        &self,
        route: &Route,
        request: SegmentRequest,
        meta: CallMeta,
    ) -> Result<SegmentReply, String> {
        let core = self.core.clone();
        let route_owned = route.clone();
        self.job(
            &format!("ai segment {}/{}", route.provider, route.model),
            &[(AI_CALLS, 1)],
            self.timeout_of(route),
            move |token, job, watch| {
                core.segment(&route_owned, request, meta, &Slot::job(job, token, watch))
            },
        )
    }

    /// The route for a capability, or a prompt of it; why not otherwise.
    pub fn route(&self, capability: Capability, prompt: Option<&str>) -> Result<Route, String> {
        self.core.route(capability, prompt)
    }

    /// The route a call would take once `overrides` are applied (run,
    /// node, diagram, then the map), and where it came from.
    pub fn resolve(
        &self,
        capability: Capability,
        prompt: Option<&str>,
        overrides: &Overrides,
    ) -> Result<(Route, RouteFrom), String> {
        self.core.resolve(capability, prompt, overrides)
    }

    // Providers.

    fn key_state(&self, key: Option<&KeySource>) -> Value {
        match key {
            None => json!(null),
            Some(source) => {
                let found = source.resolve(self.core.secrets.as_ref());
                json!({ "where": source.describe(), "found": found.is_ok() })
            }
        }
    }

    /// Every provider in effect.
    pub fn providers_list(&self) -> Value {
        let config = self.core.config().clone();
        Value::Array(
            config
                .providers()
                .iter()
                .map(|p| {
                    json!({
                        "id": p.id,
                        "kind": p.kind,
                        "url": p.url(),
                        "command": p.command,
                        "remote": p.remote(),
                        "capabilities": p.capabilities(),
                        "limits": config.limits.providers.get(&p.id),
                        "key": self.key_state(p.key.as_ref()),
                        "listed": config.listed.iter().any(|l| l.id == p.id),
                    })
                })
                .collect(),
        )
    }

    /// Adds (or replaces) a provider. `api_key` goes to the keyring, under
    /// the provider's id, and the entry names it there.
    pub fn add_provider(
        &self,
        mut entry: ProviderConfig,
        api_key: Option<&str>,
    ) -> Result<Value, String> {
        if let Some(key) = api_key {
            if entry.key.is_some() {
                return Err("give `api_key` or `key_env`, not both".into());
            }
            self.core.secrets.set(&entry.id, key)?;
            entry.key = Some(KeySource::Keyring(entry.id.clone()));
        }
        entry.check()?;
        let mut config = self.core.config();
        config.put(entry.clone());
        self.save(&config)?;
        Ok(json!({
            "added": entry.id,
            "remote": entry.remote(),
            "capabilities": entry.capabilities(),
            "key": entry.key.as_ref().map(KeySource::describe),
            "note": if entry.remote() {
                "a remote provider: images and text routed to it leave this machine (images downscaled)"
            } else {
                "a local provider: nothing leaves this machine"
            },
        }))
    }

    /// Removes a provider and every route to it.
    pub fn remove_provider(&self, id: &str) -> Result<Value, String> {
        let mut config = self.core.config();
        let Some(at) = config.listed.iter().position(|p| p.id == id) else {
            return Err(if id == DEFAULT_PROVIDER {
                format!(
                    "`{id}` is not listed: it is the built-in local provider, which calls nothing until a route names it"
                )
            } else {
                format!("no provider `{id}`")
            });
        };
        let removed = config.listed.remove(at);
        let routes = config.routing.forget(id);
        self.save(&config)?;
        drop(config);
        if let Some(KeySource::Keyring(name)) = &removed.key {
            self.core.secrets.delete(name)?;
        }
        Ok(json!({ "removed": id, "routes_removed": routes }))
    }

    /// Whether a provider answers, its models, and (with a model) a short
    /// call to it.
    pub fn test_provider(&self, id: &str, model: Option<&str>) -> Result<Value, String> {
        let provider = self.provider(id)?;
        let started = Instant::now();
        let health = provider.health();
        let health_ms = started.elapsed().as_millis() as u64;
        let models = provider.models();
        let mut report = json!({
            "provider": id,
            "remote": provider.remote(),
            "reachable": health.is_ok(),
            "health": health.as_ref().ok(),
            "error": health.as_ref().err(),
            "ms": health_ms,
            "models": models.as_ref().map(|m| m.iter().map(|i| i.name.clone()).collect::<Vec<_>>()).ok(),
        });
        if let Some(model) = model {
            let request = ChatRequest {
                prompt: "Reply with the single word: ok".into(),
                temperature: Some(0.0),
                max_tokens: 16,
                ..ChatRequest::default()
            };
            let started = Instant::now();
            let reply = self.chat(
                &Route::new(id, model),
                request,
                CallMeta {
                    purpose: "provider test".into(),
                    ..CallMeta::default()
                },
            );
            report["call"] = json!({
                "model": model,
                "ok": reply.is_ok(),
                "reply": reply.as_ref().ok().map(|r| r.text.clone()),
                "error": reply.as_ref().err(),
                "ms": started.elapsed().as_millis() as u64,
            });
        }
        Ok(report)
    }

    /// Each provider's models (or one provider's), with their capabilities.
    pub fn models_list(&self, only: Option<&str>) -> Result<Value, String> {
        let config = self.core.config().clone();
        let providers: Vec<ProviderConfig> = config
            .providers()
            .into_iter()
            .filter(|p| only.is_none_or(|o| o == p.id))
            .collect();
        if let Some(o) = only
            && providers.is_empty()
        {
            return Err(format!("no provider `{o}`"));
        }
        let routed = |provider: &str, model: &str| -> Vec<String> {
            let r = &config.routing;
            r.capabilities
                .iter()
                .filter(|(_, route)| route.provider == provider && route.model == model)
                .map(|(c, _)| c.name().to_string())
                .chain(
                    r.prompts
                        .iter()
                        .filter(|(_, route)| route.provider == provider && route.model == model)
                        .map(|(p, _)| p.clone()),
                )
                .collect()
        };
        let mut out = Vec::new();
        for entry in providers {
            let listed = provider::build(&entry, &self.core.parts()).and_then(|p| p.models());
            out.push(match listed {
                Ok(models) => json!({
                    "provider": entry.id,
                    "models": models.iter().map(|m| json!({
                        "name": m.name,
                        "size_bytes": m.size_bytes,
                        "capabilities": m.capabilities,
                        "routed_for": routed(&entry.id, &m.name),
                    })).collect::<Vec<_>>(),
                }),
                Err(e) => json!({ "provider": entry.id, "error": e }),
            });
        }
        Ok(Value::Array(out))
    }

    // Routing.

    /// Why a route cannot serve `capability`, if it cannot. `cache` keeps
    /// each provider's model list across checks.
    fn check_route(
        &self,
        capability: Capability,
        route: &Route,
        cache: &mut BTreeMap<String, Result<Vec<ModelInfo>, String>>,
    ) -> Result<(), String> {
        let entry = self.core.entry(&route.provider)?;
        let declared = entry.capabilities();
        let can = declared.contains(&capability)
            || (capability == Capability::Structured && declared.contains(&Capability::Chat));
        if !can {
            return Err(format!("`{}` does not do {capability}", entry.id));
        }
        if let Some(key) = &entry.key {
            key.resolve(self.core.secrets.as_ref())?;
        }
        let models = cache
            .entry(entry.id.clone())
            .or_insert_with(|| provider::build(&entry, &self.core.parts()).and_then(|p| p.models()))
            .clone()
            .map_err(|e| format!("`{}` does not answer: {e}", entry.id))?;
        if entry.kind == ProviderKind::Command {
            return Ok(());
        }
        let Some(model) = models
            .iter()
            .find(|m| m.name == route.model || m.name == format!("{}:latest", route.model))
        else {
            return Err(format!(
                "`{}` has no model `{}`{}",
                entry.id,
                route.model,
                if entry.kind == ProviderKind::Ollama {
                    " (pull it: helios_ai_local_pull)"
                } else {
                    ""
                }
            ));
        };
        let can = model.capabilities.contains(&capability)
            || (capability == Capability::Structured
                && model.capabilities.contains(&Capability::Chat));
        if can {
            Ok(())
        } else {
            Err(format!(
                "`{}` does not do {capability}, as far as `{}` says",
                route.model, entry.id
            ))
        }
    }

    /// The routing map, each route checked.
    pub fn routing_get(&self) -> Value {
        let routing = self.core.config().routing.clone();
        let mut cache = BTreeMap::new();
        let mut capabilities = serde_json::Map::new();
        for c in Capability::ALL {
            let entry = match routing.route(c, None) {
                None => json!({ "route": null, "available": false,
                    "reason": format!("nothing is routed for {c}") }),
                Some((route, from)) => {
                    let check = self.check_route(c, &route, &mut cache);
                    json!({ "route": route, "from": from, "available": check.is_ok(),
                        "reason": check.err() })
                }
            };
            capabilities.insert(c.name().into(), entry);
        }
        let library = self.core.library.lock().expect("library lock").clone();
        let prompts: Vec<Value> = library
            .latest()
            .map(|p| match routing.route(p.capability, Some(&p.id)) {
                None => json!({ "prompt": p.id, "capability": p.capability, "route": null,
                    "available": false, "reason": format!("nothing is routed for {} or `{}`", p.capability, p.id) }),
                Some((route, from)) => {
                    let check = self.check_route(p.capability, &route, &mut cache);
                    json!({ "prompt": p.id, "capability": p.capability, "route": route,
                        "from": from, "available": check.is_ok(), "reason": check.err() })
                }
            })
            .collect();
        json!({ "capabilities": capabilities, "prompts": prompts })
    }

    /// Sets (or with no route, clears) the route of a capability or of a
    /// prompt.
    pub fn routing_set(
        &self,
        capability: Option<Capability>,
        prompt: Option<&str>,
        route: Option<Route>,
    ) -> Result<Value, String> {
        if let Some(r) = &route {
            self.core.entry(&r.provider)?;
        }
        let mut config = self.core.config();
        let key = match (capability, prompt) {
            (Some(c), None) => {
                match &route {
                    Some(r) => config.routing.capabilities.insert(c, r.clone()),
                    None => config.routing.capabilities.remove(&c),
                };
                c.name().to_string()
            }
            (None, Some(p)) => {
                PromptRef::parse(p)?;
                match &route {
                    Some(r) => config.routing.prompts.insert(p.to_string(), r.clone()),
                    None => config.routing.prompts.remove(p),
                };
                p.to_string()
            }
            _ => return Err("give a capability or a prompt, not both".into()),
        };
        self.save(&config)?;
        Ok(json!({ "for": key, "route": route }))
    }

    // Prompts.

    pub fn prompts_list(&self) -> Value {
        let library = self.core.library.lock().expect("library lock");
        json!({ "prompts": library.list(), "problems": library.problems })
    }

    pub fn prompt(&self, reference: &PromptRef) -> Result<Prompt, String> {
        self.core
            .library
            .lock()
            .expect("library lock")
            .get(reference)
            .cloned()
    }

    /// Writes a new user version of a prompt.
    pub fn set_prompt(&self, prompt: Prompt) -> Result<Prompt, String> {
        self.core.library.lock().expect("library lock").set(prompt)
    }

    /// Runs a prompt (as a job) and returns the raw and the validated
    /// answer. Nothing is applied.
    pub fn try_prompt(
        &self,
        reference: &PromptRef,
        input: Option<Value>,
        images: Vec<Vec<u8>>,
        route: Option<Route>,
    ) -> Result<TryResult, String> {
        let prompt = self.prompt(reference)?;
        let core = self.core.clone();
        let timeout = route
            .as_ref()
            .map_or(Duration::from_mins(5), |r| self.timeout_of(r));
        self.job(
            &format!("ai try {}", prompt.reference()),
            &[(AI_CALLS, 1)],
            timeout + Duration::from_secs(30),
            move |token, job, watch| {
                Ok(core.try_prompt(
                    &prompt,
                    input,
                    &images,
                    route,
                    "try",
                    &Slot::job(job, token, watch),
                ))
            },
        )
    }

    /// Runs a prompt for a feature (`stacks.suggest`, `template.design`):
    /// the validated answer, or why there is none. Pin a version
    /// (`id@version`) for reproducible work.
    ///
    /// Routes from the map; an answer that does not fit is retried once
    /// with its errors. [`Ai::run_prompt_with`] takes overrides and tells
    /// which route answered.
    pub fn run_prompt(
        &self,
        reference: &PromptRef,
        input: Value,
        images: Vec<Vec<u8>>,
    ) -> Result<Value, String> {
        let mut call = PromptCall::new(reference.clone(), input);
        call.images = images;
        call.purpose = "feature".into();
        self.run_prompt_with(call)
            .map(|r| r.value)
            .map_err(|f| f.to_string())
    }

    /// Runs a prompt (as a job) with its run, node and diagram overrides
    /// (in that order, before the routing map). The answer is checked
    /// against the prompt's schema, and the extractor's keys when one is
    /// given; one that does not fit is sent back once with its errors.
    /// The result says which route answered and how many values were
    /// adjusted; both tries are in the call log with their attempt.
    pub fn run_prompt_with(&self, call: PromptCall) -> Result<PromptRun, PromptFailure> {
        let prompt = self.prompt(&call.reference).map_err(|e| PromptFailure {
            prompt: call.reference.id.clone(),
            route: None,
            attempts: vec![Attempt {
                raw: None,
                errors: vec![e],
            }],
        })?;
        let fail = |e: String, route: Option<Route>| PromptFailure {
            prompt: prompt.reference(),
            route,
            attempts: vec![Attempt {
                raw: None,
                errors: vec![e],
            }],
        };
        let resolved = self
            .core
            .resolve(prompt.capability, Some(&prompt.id), &call.overrides)
            .map_err(|e| fail(e, None))?;
        let core = self.core.clone();
        let route = resolved.0.clone();
        let name = format!("ai run {}", prompt.reference());
        let held = prompt.clone();
        match self.job(
            &name,
            &[(AI_CALLS, 1)],
            self.timeout_of(&route) * 2,
            move |token, job, watch| {
                let slot = Slot {
                    run: call.run.clone(),
                    ..Slot::job(job, token, watch)
                };
                Ok(core.run_prompt(&held, &call, resolved, &slot))
            },
        ) {
            Ok(run) => run,
            Err(e) => Err(fail(e, Some(route))),
        }
    }

    /// Before a run: for each capability (and prompt) it will call, with
    /// its overrides, the route it resolves to and whether that route can
    /// serve (the prompt known, the provider configured and answering, its
    /// key found, the model present and doing the capability). Each
    /// provider is asked for its models once.
    pub fn check_needs(&self, needs: &[Need]) -> Vec<NeedCheck> {
        let mut cache = BTreeMap::new();
        let library = self.core.library.lock().expect("library lock").clone();
        let providers = self.core.config().providers();
        needs
            .iter()
            .map(|need| {
                let default_images = u32::from(matches!(
                    need.capability,
                    Capability::Vision | Capability::Segmentation
                ));
                let failed = |reason: String, images: u32| NeedCheck {
                    need: need.clone(),
                    route: None,
                    from: None,
                    available: false,
                    reason: Some(reason),
                    remote: None,
                    images_per_item: images,
                };
                let prompt = match need.prompt.as_deref().map(PromptRef::parse) {
                    Some(Err(e)) => return failed(e, default_images),
                    Some(Ok(r)) => match library.get(&r) {
                        Ok(p) => Some(p.clone()),
                        Err(e) => return failed(e, default_images),
                    },
                    None => None,
                };
                let images = prompt.as_ref().map_or(default_images, |p| p.images);
                match self.core.resolve(
                    need.capability,
                    prompt.as_ref().map(|p| p.id.as_str()),
                    &need.overrides,
                ) {
                    Err(e) => failed(e, images),
                    Ok((route, from)) => {
                        let check = self.check_route(need.capability, &route, &mut cache);
                        NeedCheck {
                            need: need.clone(),
                            remote: providers
                                .iter()
                                .find(|p| p.id == route.provider)
                                .map(ProviderConfig::remote),
                            route: Some(route),
                            from: Some(from),
                            available: check.is_ok(),
                            reason: check.err(),
                            images_per_item: images,
                        }
                    }
                }
            })
            .collect()
    }

    /// The check a run's submission calls before anything runs: every
    /// model node checked ([`Ai::check_needs`]), the nodes nothing serves
    /// named with the reason and where to fix it, what leaves the machine
    /// and to whom, and the run's remote calls against its ceiling.
    /// `items`: how many images the run takes, when known.
    pub fn check_run(&self, needs: &[Need], items: Option<u64>) -> RunCheck {
        let nodes = self.check_needs(needs);
        let config = self.core.config().clone();
        let providers = config.providers();
        let refused: Vec<String> = nodes
            .iter()
            .filter(|n| !n.available)
            .map(|n| {
                format!(
                    "{}: {}",
                    n.need.label(),
                    n.reason.as_deref().unwrap_or("not available")
                )
            })
            .collect();
        let leaves: Vec<Leaving> = nodes
            .iter()
            .filter(|n| n.remote == Some(true))
            .filter_map(|n| {
                let route = n.route.as_ref()?;
                Some(Leaving {
                    node: n.need.node.clone(),
                    provider: route.provider.clone(),
                    url: providers
                        .iter()
                        .find(|p| p.id == route.provider)
                        .and_then(ProviderConfig::url),
                    model: route.model.clone(),
                    images_per_item: n.images_per_item,
                    images: items.map(|i| i * u64::from(n.images_per_item)),
                    image_side: config.privacy.remote_image_side,
                })
            })
            .collect();
        let remote_nodes = nodes.iter().filter(|n| n.remote == Some(true)).count() as u64;
        RunCheck {
            ok: refused.is_empty(),
            fix: (!refused.is_empty()).then(|| FIX.to_string()),
            refused,
            nodes,
            leaves,
            remote_calls: items.map(|i| i * remote_nodes),
            remote_ceiling: config.limits.run_remote_calls,
        }
    }

    /// [`Ai::check_run`] as a submission calls it: the check, or the
    /// refusal naming each node nothing serves. Nothing has run either way.
    pub fn check_submission(&self, needs: &[Need], items: Option<u64>) -> Result<RunCheck, String> {
        let check = self.check_run(needs, items);
        match check.refusal() {
            Some(refusal) => Err(refusal),
            None => Ok(check),
        }
    }

    /// Remote calls `run` has made so far (see `limits.run_remote_calls`).
    pub fn run_remote_calls(&self, run: &str) -> u64 {
        self.core.limiter.run_calls(run)
    }

    /// Forgets a run's remote call count: the run ended, or the user let
    /// it go on past its ceiling.
    pub fn forget_run(&self, run: &str) {
        self.core.limiter.forget_run(run);
    }

    // The local runtime.

    pub fn probe(&self) -> local::Probe {
        local::probe(self.core.system.as_ref())
    }

    /// Whether Ollama is installed for this user, running, and its models.
    pub fn status(&self) -> Value {
        let system = self.core.system.as_ref();
        let on_path = system.which("ollama");
        let user_install = local::home(system).join(".local/bin/ollama");
        let installed = on_path.or_else(|| system.exists(&user_install).then_some(user_install));
        let entry = self.core.entry(DEFAULT_PROVIDER);
        let provider = self.provider(DEFAULT_PROVIDER);
        let health = provider
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|p| p.health());
        let models = match (&provider, &health) {
            (Ok(p), Ok(_)) => p.models().ok(),
            _ => None,
        };
        json!({
            "runtime": "ollama",
            "installed": installed.is_some(),
            "path": installed,
            "url": entry.ok().and_then(|e| e.url()),
            "running": health.is_ok(),
            "version": health.as_ref().ok(),
            "error": health.as_ref().err(),
            "models": models,
            "models_dir": local::models_dir(system),
        })
    }

    fn present(&self) -> Present {
        let status = self.status();
        Present {
            installed: status["installed"] == true,
            running: status["running"] == true,
        }
    }

    /// Models per capability that fit this machine.
    pub fn recommend(&self) -> Value {
        let probe = self.probe();
        let installed: Vec<String> = self
            .provider(DEFAULT_PROVIDER)
            .and_then(|p| p.models())
            .map(|m| m.into_iter().map(|i| i.name).collect())
            .unwrap_or_default();
        local::recommend(&probe, &installed)
    }

    /// The setup steps for this OS, running those whose id is in `confirm`.
    pub fn setup(&self, confirm: &[String]) -> Value {
        let system = self.core.system.as_ref();
        let present = self.present();
        let steps = local::plan(system, &self.core.paths.cache, present);
        let root =
            system.os() != "windows" && system.run("id", &["-u"]).is_ok_and(|u| u.trim() == "0");
        let outcomes = local::setup(system, steps, confirm, root);
        let ran = outcomes.iter().any(|o| o.outcome == "ran");
        let pending: Vec<&str> = outcomes
            .iter()
            .filter(|o| o.outcome == "not_confirmed")
            .map(|o| o.step.id.as_str())
            .collect();
        json!({
            "os": system.os(),
            "root": root,
            "ran": ran,
            "steps": outcomes,
            "next": if pending.is_empty() {
                "nothing left to confirm: check helios_ai_local_status".to_string()
            } else {
                format!("read the steps, then call again with confirm = {pending:?} (all or some)")
            },
            "status": if ran { self.status() } else { json!(null) },
        })
    }

    /// Starts pulling `model` through `provider` (Ollama by default) as a
    /// job, after checking the disk has room for it. `size_mib` gives the
    /// size of a model the catalogue does not know.
    pub fn pull(
        &self,
        provider_id: Option<&str>,
        model: &str,
        size_mib: Option<u64>,
    ) -> Result<Value, String> {
        if let Some(size) = size_mib.filter(|&s| s > MAX_MODEL_MIB) {
            return Err(format!(
                "`size_mib` = {size} is not a model's size: at most {MAX_MODEL_MIB} MiB (4 TiB)"
            ));
        }
        let provider = self.provider(provider_id.unwrap_or(DEFAULT_PROVIDER))?;
        if provider.kind() != ProviderKind::Ollama {
            return Err(format!("`{}` cannot download models", provider.id()));
        }
        let needed = size_mib.or_else(|| local::known(model).map(|k| k.size_mib));
        let mut disk = json!(null);
        if !provider.remote() {
            let system = self.core.system.as_ref();
            let dir = local::models_dir(system);
            let free = local::disk_free(system, &dir);
            // A model the catalogue does not know needs at least this much.
            let wanted = needed.unwrap_or(4096).saturating_mul(11) / 10;
            match free {
                Some(f) if f < wanted => {
                    return Err(format!(
                        "not enough disk under {}: {f} MiB free, {wanted} MiB needed{}",
                        dir.display(),
                        if needed.is_none() {
                            " (the model's size is not known: give `size_mib`)"
                        } else {
                            ""
                        }
                    ));
                }
                _ => disk = json!({ "dir": dir, "free_mib": free, "needed_mib": wanted }),
            }
        }
        let core = self.core.clone();
        let name = format!("ai pull {model}");
        let model_owned = model.to_string();
        let job_name = name.clone();
        let id = self.submit(&name, &[(AI_PULL, 1)], move |token, job| {
            let mut last = (String::new(), u64::MAX);
            let result = provider.pull(&model_owned, &mut |p| {
                let percent = if p.status == "success" {
                    100
                } else {
                    p.completed.saturating_mul(100).checked_div(p.total).unwrap_or(0)
                };
                if p.status != last.0 || percent != last.1 {
                    core.progress(
                        job,
                        &job_name,
                        json!({ "status": p.status, "completed": p.completed, "total": p.total, "percent": percent }),
                    );
                    last = (p.status.clone(), percent);
                }
                !token.is_cancelled()
            });
            core.finish(
                job,
                match &result {
                    Ok(()) => json!({ "pulled": model_owned }),
                    Err(e) => json!({ "error": e }),
                },
            );
            result
        });
        Ok(json!({ "job": id.0, "name": name, "disk": disk,
            "follow": "helios_ai_jobs_events (helios.job.progress), helios_ai_jobs_wait, helios_ai_jobs_cancel" }))
    }

    /// Measures prompts on local models, as a job; with `apply`, routes
    /// each prompt (and each unrouted capability) to the fastest model
    /// that answered correctly.
    pub fn bench(&self, request: BenchRequest) -> Result<Value, String> {
        let library = self.core.library.lock().expect("library lock").clone();
        let prompts: Vec<Prompt> = if request.prompts.is_empty() {
            library.latest().cloned().collect()
        } else {
            request
                .prompts
                .iter()
                .map(|r| PromptRef::parse(r).and_then(|r| library.get(&r).cloned()))
                .collect::<Result<_, _>>()?
        };
        let config = self.core.config().clone();
        let entries: Vec<ProviderConfig> = config
            .providers()
            .into_iter()
            .filter(|p| {
                if request.providers.is_empty() {
                    !p.remote()
                } else {
                    request.providers.contains(&p.id)
                }
            })
            .collect();
        let core = self.core.clone();
        let image = request
            .image
            .clone()
            .unwrap_or_else(|| images::test_card(512));
        let runs = request.runs.max(1);
        let apply = request.apply;
        let id = self.submit("ai bench", &[(AI_BENCH, 1), (AI_CALLS, 1)], move |token, job| {
            let mut candidates: Vec<(Route, Vec<Capability>)> = Vec::new();
            let mut unreachable = Vec::new();
            for entry in &entries {
                match provider::build(entry, &core.parts()).and_then(|p| p.models()) {
                    Ok(models) => candidates.extend(
                        models
                            .into_iter()
                            .map(|m| (Route::new(&entry.id, &m.name), m.capabilities)),
                    ),
                    Err(e) => unreachable.push(json!({ "provider": entry.id, "error": e })),
                }
            }
            let plan: Vec<(&Prompt, &Route)> = prompts
                .iter()
                .flat_map(|p| {
                    candidates
                        .iter()
                        .filter(move |(_, caps)| {
                            caps.contains(&p.capability)
                                || (p.capability == Capability::Structured
                                    && caps.contains(&Capability::Chat))
                        })
                        .map(move |(r, _)| (p, r))
                })
                .collect();
            let total = plan.len() as u64 * u64::from(runs);
            let mut done = 0;
            let mut rows = Vec::new();
            for (prompt, route) in &plan {
                let mut times = Vec::new();
                let mut errors = Vec::new();
                let mut valid = 0;
                for _ in 0..runs {
                    if token.is_cancelled() {
                        return Err("cancelled".into());
                    }
                    let shown = std::slice::from_ref(&image);
                    let slot = Slot {
                        job: Some(job),
                        token: Some(token.clone()),
                        ..Slot::default()
                    };
                    let t = core.try_prompt(prompt, None, shown, Some((*route).clone()), "bench", &slot);
                    done += 1;
                    core.progress(
                        job,
                        "ai bench",
                        json!({ "status": format!("{} on {}/{}", prompt.reference(), route.provider, route.model),
                            "completed": done, "total": total, "percent": done * 100 / total.max(1) }),
                    );
                    if t.ok() {
                        valid += 1;
                    } else if errors.len() < 3 {
                        errors.extend(t.errors.iter().take(3).cloned());
                    }
                    times.push(t.ms);
                }
                times.sort_unstable();
                rows.push(json!({
                    "prompt": prompt.id,
                    "version": prompt.version,
                    "capability": prompt.capability,
                    "provider": route.provider,
                    "model": route.model,
                    "runs": runs,
                    "valid": valid,
                    "ok": valid == runs,
                    "median_ms": times[times.len() / 2],
                    "errors": errors,
                }));
            }
            let applied = if apply { apply_bench(&core, &rows)? } else { Vec::new() };
            core.finish(
                job,
                json!({ "results": rows, "unreachable": unreachable, "applied": applied }),
            );
            Ok(())
        });
        Ok(json!({ "job": id.0, "name": "ai bench",
            "follow": "helios_ai_jobs_events (helios.job.progress), helios_ai_jobs_wait" }))
    }

    // Jobs.

    /// The AI bus's events after `since`.
    pub fn events(&self, since: u64) -> Vec<Envelope> {
        self.core.bus.since(since)
    }

    /// Waits for a job to end (at most `timeout`); its state and result.
    pub fn job_wait(&self, job: u64, timeout: Duration) -> Value {
        let id = JobId(job);
        let events = self.core.bus.subscribe();
        let deadline = Instant::now() + timeout;
        loop {
            let state = self.scheduler.job(id);
            match state {
                None => return json!({ "job": job, "state": null, "error": "no such job" }),
                Some(JobState::Queued | JobState::Running) => {}
                Some(s) => {
                    let result = self
                        .core
                        .results
                        .lock()
                        .expect("results lock")
                        .get(&job)
                        .cloned();
                    return json!({ "job": job, "state": s, "result": result });
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return json!({ "job": job, "state": state, "timed_out": true });
            }
            let _ = events.recv_timeout(left.min(Duration::from_millis(250)));
        }
    }

    pub fn job_cancel(&self, job: u64) -> bool {
        self.scheduler.cancel(JobId(job))
    }

    /// The report of `helios doctor ai`.
    pub fn doctor(&self) -> Value {
        let config = self.config();
        let mut providers = Vec::new();
        for entry in config.providers() {
            let built = provider::build(&entry, &self.core.parts());
            let health = built
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|p| p.health());
            providers.push(json!({
                "id": entry.id,
                "kind": entry.kind,
                "url": entry.url(),
                "remote": entry.remote(),
                "capabilities": entry.capabilities(),
                "key": self.key_state(entry.key.as_ref()),
                "reachable": health.is_ok(),
                "health": health.as_ref().ok(),
                "error": health.as_ref().err(),
            }));
        }
        let library = self.core.library.lock().expect("library lock");
        let problems = library.problems.clone();
        drop(library);
        json!({
            "providers": providers,
            "routing": self.routing_get(),
            "runtime": self.status(),
            "privacy": config.privacy,
            "limits": config.limits,
            "files": {
                "config": self.core.paths.config,
                "prompts": self.core.paths.prompts,
                "log": self.core.paths.log,
            },
            "problems": problems,
        })
    }
}

/// Routes each prompt to its fastest valid model, and each capability not
/// routed yet to the fastest valid model of its first prompt.
fn apply_bench(core: &Core, rows: &[Value]) -> Result<Vec<Value>, String> {
    let mut best: BTreeMap<String, (u64, Route, Capability)> = BTreeMap::new();
    for row in rows.iter().filter(|r| r["ok"] == true) {
        let (Some(prompt), Some(ms)) = (row["prompt"].as_str(), row["median_ms"].as_u64()) else {
            continue;
        };
        let Ok(capability) = serde_json::from_value::<Capability>(row["capability"].clone()) else {
            continue;
        };
        let route = Route::new(
            row["provider"].as_str().unwrap_or_default(),
            row["model"].as_str().unwrap_or_default(),
        );
        if best.get(prompt).is_none_or(|(b, _, _)| ms < *b) {
            best.insert(prompt.to_string(), (ms, route, capability));
        }
    }
    let mut config = core.config();
    let mut applied = Vec::new();
    for (prompt, (_, route, capability)) in &best {
        config.routing.prompts.insert(prompt.clone(), route.clone());
        applied.push(json!({ "for": prompt, "route": route }));
        if !config.routing.capabilities.contains_key(capability) {
            config
                .routing
                .capabilities
                .insert(*capability, route.clone());
            applied.push(json!({ "for": capability, "route": route }));
        }
    }
    config.save(&core.paths.config)?;
    Ok(applied)
}

/// The file `path` read whole, for a prompt's image.
pub fn read_image(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}
