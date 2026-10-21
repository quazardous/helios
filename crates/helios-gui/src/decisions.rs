//! The Decisions panel: a card for each decision of the open image that
//! waits for an answer (see `docs/module-contract.md`, "Decision nodes",
//! and `docs/gui.md`, "Decision prompts").
//!
//! A card says what the decision asks (its module's description and its
//! question), and in AI-assisted mode what the model proposes, how sure it
//! is and why. **Accept** takes the model's best option; **Correct** opens
//! the answer's form, generated from the parameters the decision module
//! declares, filled with the proposal; a decision with no proposal (human
//! mode, or a model that has not answered yet) is answered through the
//! same form. With several proposals waiting, **Accept all above** takes
//! every best option the model is at least that sure of.
//!
//! The panel holds the edit's decisions ([`Book`]), not the graph: what
//! the user asks is queued ([`Decisions::take_requests`]) and the shell
//! answers it through the book on the viewport's graph, as the
//! `decision.*` commands agents run do, so the render goes on once the
//! last decision on its way is answered. Cards are made again from the
//! book each frame ([`Decisions::refresh`]): an answer from anywhere (an
//! agent, a script) removes its card.
//!
//! [`Decisions::card_ui`] draws one card on its own, for the graph view
//! to show next to its node once it exists.

use std::collections::BTreeMap;

use egui::Ui;
use helios_core::{Graph, Message, ModuleKind, ParamKind, ParamSpec, Params, Value};
use helios_decision::{Book, Mode, Proposal, Status};
use helios_values::Descriptor;

/// The panel's id.
pub const PANEL: &str = "helios.decisions";

/// The pending badge's mark in the status bar (egui-pop's `indicators`
/// collector).
pub const MARK: &str = "helios.status.decisions";

/// The confidence "Accept all above" starts at.
pub const THRESHOLD: f64 = 0.8;

/// What the user asked of the decisions, for the shell.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Takes one option of a proposal (0: the model's best).
    Accept { node: String, option: usize },
    /// Answers with these values: a correction of the proposal when
    /// there is one, a human answer otherwise.
    Decide { node: String, values: Params },
    /// Takes the best option of every proposal at least this sure.
    AcceptAbove { confidence: f64 },
}

/// A decision waiting, as its card shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub node: String,
    /// The decision module, and what it says it decides.
    pub module: String,
    pub description: String,
    /// What it is asked (an object to find, a prompt): empty for most.
    pub question: Params,
    pub mode: Mode,
    pub status: Status,
    pub proposal: Option<Proposal>,
    /// The answer's fields: the module's parameters, its question left out.
    pub answer: Vec<ParamSpec>,
    /// The values the graph holds now, which an answer form starts from
    /// when there is no proposal.
    pub current: Params,
}

impl Card {
    /// Whether it waits for a person: a proposal to accept or correct, or
    /// a decision in human mode. The others wait for the model (a person
    /// may still answer them).
    pub fn for_a_person(&self) -> bool {
        self.status == Status::Proposed || self.mode == Mode::Human
    }

    /// The model's confidence in its proposal, if it said.
    pub fn confidence(&self) -> Option<f64> {
        self.proposal.as_ref().and_then(|p| p.confidence)
    }
}

/// The decisions of the open image, their cards and open answer forms.
pub struct Decisions {
    book: Book,
    cards: Vec<Card>,
    /// The answer forms open, by node: the values as edited.
    forms: BTreeMap<String, Params>,
    threshold: f64,
    requests: Vec<Request>,
}

impl Default for Decisions {
    fn default() -> Self {
        Decisions {
            book: Book::new(),
            cards: Vec::new(),
            forms: BTreeMap::new(),
            threshold: THRESHOLD,
            requests: Vec::new(),
        }
    }
}

impl std::fmt::Debug for Decisions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decisions")
            .field("cards", &self.cards)
            .field("forms", &self.forms)
            .field("threshold", &self.threshold)
            .finish_non_exhaustive()
    }
}

/// The card of a decision waiting, `None` when the graph does not hold
/// it (the book may know nodes of another network).
fn card(book: &Book, graph: &Graph, node: &str, status: Status) -> Option<Card> {
    let id = graph.find(node)?;
    let manifest = graph.manifest(id).ok()??;
    let question = graph.question(id).ok()?;
    let answer = manifest
        .params
        .into_iter()
        .filter(|p| question.get(&p.name).is_none())
        .collect();
    Some(Card {
        node: node.to_string(),
        module: manifest.name,
        description: manifest.description,
        question,
        mode: book.mode(node).ok()?,
        status,
        proposal: book.proposal(node).cloned(),
        answer,
        current: graph.params(id).ok()?.unwrap_or_default(),
    })
}

impl Decisions {
    /// The edit's decisions.
    pub fn book(&self) -> &Book {
        &self.book
    }

    /// The edit's decisions, to answer them (the shell does, on the
    /// viewport's graph).
    pub fn book_mut(&mut self) -> &mut Book {
        &mut self.book
    }

    /// Another edit's decisions: the forms open are closed.
    pub fn set_book(&mut self, book: Book) {
        self.book = book;
        self.forms.clear();
    }

    /// Puts the graph's decision nodes the book does not know in it, in
    /// human mode: with no model configured in the desktop, a person
    /// answers them.
    pub fn adopt(&mut self, graph: &Graph) {
        for id in graph.ids() {
            let decision = graph
                .manifest(id)
                .ok()
                .flatten()
                .is_some_and(|m| m.kind == ModuleKind::Decision);
            let Ok(name) = graph.name(id) else { continue };
            if decision && self.book.mode(name).is_err() {
                self.book.declare(name, Mode::Human);
            }
        }
    }

    /// Makes the cards again from the book and the graph shown (none:
    /// no image open, no card). A question changed on the graph makes
    /// its decision pending again first.
    pub fn refresh(&mut self, graph: Option<&Graph>) {
        let Some(graph) = graph else {
            self.cards.clear();
            self.forms.clear();
            return;
        };
        self.book.follow(graph);
        let book = &self.book;
        self.cards = book
            .waiting()
            .into_iter()
            .filter_map(|(node, status)| card(book, graph, node, status))
            .collect();
        let cards = &self.cards;
        self.forms
            .retain(|node, _| cards.iter().any(|c| c.node == *node));
    }

    /// The decisions waiting, as of the last [`Decisions::refresh`].
    pub fn cards(&self) -> &[Card] {
        &self.cards
    }

    /// How many decisions wait.
    pub fn pending(&self) -> usize {
        self.cards.len()
    }

    /// How many of them wait for a person (see [`Card::for_a_person`]).
    pub fn for_a_person(&self) -> usize {
        self.cards.iter().filter(|c| c.for_a_person()).count()
    }

    /// The confidence "Accept all above" takes.
    pub fn threshold(&self) -> f64 {
        self.threshold
    }

    pub fn set_threshold(&mut self, confidence: f64) {
        self.threshold = confidence.clamp(0.0, 1.0);
    }

    /// The decisions whose proposal the model is at least `confidence`
    /// sure of.
    pub fn above(&self, confidence: f64) -> Vec<&str> {
        self.cards
            .iter()
            .filter(|c| c.status == Status::Proposed)
            .filter(|c| c.confidence().is_some_and(|k| k >= confidence))
            .map(|c| c.node.as_str())
            .collect()
    }

    /// Takes one option of a decision's proposal.
    pub fn accept(&mut self, node: &str, option: usize) {
        self.requests.push(Request::Accept {
            node: node.into(),
            option,
        });
    }

    /// Takes the best option of every proposal at least `confidence` sure.
    pub fn accept_above(&mut self, confidence: f64) {
        self.requests.push(Request::AcceptAbove { confidence });
    }

    /// Opens a decision's answer form, filled with the model's best
    /// option, else with the values the graph holds.
    pub fn correct(&mut self, node: &str) -> Result<(), String> {
        let card = self
            .cards
            .iter()
            .find(|c| c.node == node)
            .ok_or_else(|| format!("no decision `{node}` waiting"))?;
        let from = card
            .proposal
            .as_ref()
            .and_then(|p| p.options.first())
            .unwrap_or(&card.current);
        let values = card.answer.iter().fold(Params::new(), |values, spec| {
            let v = from
                .get(&spec.name)
                .or_else(|| card.current.get(&spec.name))
                .cloned()
                .unwrap_or_else(|| spec.kind.default_value());
            values.with(&spec.name, v)
        });
        self.forms.insert(node.to_string(), values);
        Ok(())
    }

    /// A decision's answer form as edited, when open.
    pub fn form(&self, node: &str) -> Option<&Params> {
        self.forms.get(node)
    }

    /// Changes one field of an open answer form.
    pub fn set_field(&mut self, node: &str, name: &str, value: Value) -> Result<(), String> {
        let form = self
            .forms
            .get_mut(node)
            .ok_or_else(|| format!("no answer form open for `{node}`"))?;
        if form.get(name).is_none() {
            return Err(format!("`{node}`'s answer has no `{name}`"));
        }
        form.set(name, value);
        Ok(())
    }

    /// Records an open form's values as the decision's answer.
    pub fn submit(&mut self, node: &str) -> Result<(), String> {
        let values = self
            .forms
            .remove(node)
            .ok_or_else(|| format!("no answer form open for `{node}`"))?;
        self.requests.push(Request::Decide {
            node: node.into(),
            values,
        });
        Ok(())
    }

    /// Closes an answer form, nothing recorded.
    pub fn close_form(&mut self, node: &str) {
        self.forms.remove(node);
    }

    /// The decisions waiting, as `decision.list` answers: plain values
    /// (numbers, booleans, text, lists).
    pub fn describe(&self) -> serde_json::Value {
        let values = |p: &Params| -> serde_json::Map<String, serde_json::Value> {
            p.iter()
                .map(|(k, v)| (k.to_string(), plain_json(v)))
                .collect()
        };
        let cards: Vec<serde_json::Value> = self
            .cards
            .iter()
            .map(|c| {
                serde_json::json!({
                    "node": c.node,
                    "module": c.module,
                    "description": c.description,
                    "question": values(&c.question),
                    "mode": c.mode,
                    "status": c.status,
                    "for_a_person": c.for_a_person(),
                    "answer": c.answer.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                    "proposal": c.proposal.as_ref().map(|p| serde_json::json!({
                        "model": p.model,
                        "options": p.options.iter().map(&values).collect::<Vec<_>>(),
                        "confidence": p.confidence,
                        "rationale": p.rationale,
                    })),
                })
            })
            .collect();
        serde_json::json!({ "decisions": cards, "threshold": self.threshold })
    }

    /// An answer given as plain JSON (`{"gain": 2}`), each value read as
    /// the answer's parameter of that name expects; a parameter left out
    /// keeps the value the form would start from.
    pub fn values_of(&self, node: &str, json: &serde_json::Value) -> Result<Params, String> {
        let card = self
            .cards
            .iter()
            .find(|c| c.node == node)
            .ok_or_else(|| format!("no decision `{node}` waiting"))?;
        let given = json
            .as_object()
            .ok_or("`values` is an object: parameter names and values")?;
        let mut values = Params::new();
        for (name, v) in given {
            let spec = card
                .answer
                .iter()
                .find(|s| s.name == *name)
                .ok_or_else(|| format!("`{node}`'s answer has no `{name}`"))?;
            values.set(
                name,
                typed(&spec.kind, v)
                    .ok_or_else(|| format!("`{name}`: {v} is not a value of this parameter"))?,
            );
        }
        Ok(values)
    }

    /// What the user asked since the last call.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// The panel: the cards, and "Accept all above" when several
    /// proposals wait.
    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let text = |m: Message| crate::i18n::text(&ctx, m);
        if self.cards.is_empty() {
            ui.weak(text(Message::new(
                "gui-decisions-none",
                "No decision waiting for an answer",
            )));
            return;
        }
        ui.strong(text(
            Message::new("gui-decisions-count", "{ $count } decisions waiting")
                .with("count", self.cards.len()),
        ));
        let proposed = self
            .cards
            .iter()
            .filter(|c| c.status == Status::Proposed)
            .count();
        if proposed >= 2 {
            ui.horizontal(|ui| {
                let above = self.above(self.threshold).len();
                let r = ui.add_enabled(
                    above > 0,
                    egui::Button::new(text(
                        Message::new(
                            "gui-decisions-accept-above",
                            "Accept the { $count } above { $percent } %",
                        )
                        .with("count", above)
                        .with("percent", percent(self.threshold)),
                    )),
                );
                crate::name_rect(&ctx, "helios.decisions.accept_above", r.rect);
                if r.clicked() {
                    self.accept_above(self.threshold);
                }
                let mut t = self.threshold;
                ui.add(
                    egui::Slider::new(&mut t, 0.0..=1.0)
                        .show_value(false)
                        .step_by(0.05),
                )
                .on_hover_text(text(Message::new(
                    "gui-decisions-threshold-hint",
                    "How sure the model must be for its proposal to be taken",
                )));
                if t != self.threshold {
                    self.set_threshold(t);
                }
            });
        }
        let cards = self.cards.clone();
        egui::ScrollArea::vertical()
            .id_salt("helios.decisions.scroll")
            .show(ui, |ui| {
                for card in &cards {
                    ui.separator();
                    self.card_ui(ui, card);
                }
            });
    }

    /// One card: what the decision asks, the proposal, Accept and
    /// Correct, the answer form when open. Drawn by the panel; the graph
    /// view draws it next to its node.
    pub fn card_ui(&mut self, ui: &mut Ui, card: &Card) {
        let ctx = ui.ctx().clone();
        let text = |m: Message| crate::i18n::text(&ctx, m);
        let node = card.node.as_str();
        let r = ui.strong(if card.description.is_empty() {
            card.module.clone()
        } else {
            card.description.clone()
        });
        crate::name_rect(&ctx, &format!("helios.decisions.{node}"), r.rect);
        let mode = match card.mode {
            Mode::Human => Message::new("gui-decisions-mode-human", "{ $node }: answered by you")
                .with("node", node),
            Mode::AiAssisted => Message::new(
                "gui-decisions-mode-assisted",
                "{ $node }: a model proposes, you settle",
            )
            .with("node", node),
            Mode::FullAi => Message::new(
                "gui-decisions-mode-full-ai",
                "{ $node }: decided by a model, you may answer instead",
            )
            .with("node", node),
        };
        ui.weak(text(mode));
        for (name, value) in card.question.iter() {
            ui.label(format!("{name}: {}", plain(value)));
        }
        if let Some(p) = &card.proposal {
            ui.label(text(
                Message::new("gui-decisions-proposal", "{ $model } proposes:")
                    .with("model", p.model.name.as_str()),
            ));
            if let Some(best) = p.options.first() {
                for spec in &card.answer {
                    if let Some(v) = best.get(&spec.name) {
                        let (label, shown) = field_text(&ctx, card, spec, v);
                        ui.label(format!("  {label}: {shown}"));
                    }
                }
            }
            if p.options.len() > 1 {
                ui.weak(text(
                    Message::new("gui-decisions-other-options", "{ $count } other options")
                        .with("count", p.options.len() - 1),
                ));
            }
            if let Some(c) = p.confidence {
                ui.add(
                    egui::ProgressBar::new(c as f32).text(text(
                        Message::new("gui-decisions-confidence", "Confidence: { $percent } %")
                            .with("percent", percent(c)),
                    )),
                );
            }
            if !p.rationale.is_empty() {
                ui.label(text(
                    Message::new("gui-decisions-why", "Why: { $why }")
                        .with("why", p.rationale.as_str()),
                ));
            }
        } else if card.mode != Mode::Human {
            ui.weak(text(Message::new(
                "gui-decisions-waiting-model",
                "Waiting for the model",
            )));
        }
        if self.forms.contains_key(node) {
            self.form_ui(ui, card);
            return;
        }
        ui.horizontal(|ui| {
            if card.proposal.is_some() {
                let r = ui.button(text(Message::new("gui-decisions-accept", "Accept")));
                crate::name_rect(&ctx, &format!("helios.decisions.{node}.accept"), r.rect);
                if r.clicked() {
                    self.accept(node, 0);
                }
                let r = ui
                    .button(text(Message::new("gui-decisions-correct", "Correct…")))
                    .on_hover_text(text(Message::new(
                        "gui-decisions-correct-hint",
                        "Change the proposal before it is recorded",
                    )));
                crate::name_rect(&ctx, &format!("helios.decisions.{node}.correct"), r.rect);
                if r.clicked() {
                    let _ = self.correct(node);
                }
            } else {
                let r = ui.button(text(Message::new("gui-decisions-answer", "Answer…")));
                crate::name_rect(&ctx, &format!("helios.decisions.{node}.correct"), r.rect);
                if r.clicked() {
                    let _ = self.correct(node);
                }
            }
        });
    }

    /// The answer form: a field per parameter of the answer, by its kind.
    fn form_ui(&mut self, ui: &mut Ui, card: &Card) {
        let ctx = ui.ctx().clone();
        let text = |m: Message| crate::i18n::text(&ctx, m);
        let node = card.node.as_str();
        let Some(mut values) = self.forms.get(node).cloned() else {
            return;
        };
        for spec in &card.answer {
            let Some(value) = values.get(&spec.name).cloned() else {
                continue;
            };
            let (label, shown) = field_text(&ctx, card, spec, &value);
            let edited = ui
                .horizontal(|ui| {
                    ui.label(&label);
                    field(ui, card, spec, value, &shown)
                })
                .inner;
            if let Some(v) = edited {
                values.set(&spec.name, v);
            }
        }
        self.forms.insert(node.to_string(), values);
        ui.horizontal(|ui| {
            let r = ui.button(text(Message::new("gui-decisions-record", "Record")));
            crate::name_rect(&ctx, &format!("helios.decisions.{node}.record"), r.rect);
            if r.clicked() {
                let _ = self.submit(node);
            }
            if ui
                .button(text(Message::new("gui-decisions-cancel", "Cancel")))
                .clicked()
            {
                self.close_form(node);
            }
        });
    }
}

/// A confidence as a whole percentage.
fn percent(confidence: f64) -> i64 {
    (confidence * 100.0).round() as i64
}

/// A value as plain JSON.
fn plain_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Float(x) => serde_json::json!(x),
        Value::Int(x) => serde_json::json!(x),
        Value::Bool(x) => serde_json::json!(x),
        Value::Choice(t) | Value::Text(t) => serde_json::json!(t),
        Value::List(l) => serde_json::json!(l),
        Value::Ints(l) => serde_json::json!(l),
    }
}

/// A plain JSON value as a parameter of this kind expects it.
fn typed(kind: &ParamKind, v: &serde_json::Value) -> Option<Value> {
    Some(match kind {
        ParamKind::Float { .. } => Value::Float(v.as_f64()?),
        ParamKind::Int { .. } => Value::Int(v.as_i64()?),
        ParamKind::Bool { .. } => Value::Bool(v.as_bool()?),
        ParamKind::Choice { .. } => Value::Choice(v.as_str()?.to_string()),
        ParamKind::Text { .. } => Value::Text(v.as_str()?.to_string()),
        ParamKind::Floats { .. } => Value::List(
            v.as_array()?
                .iter()
                .map(serde_json::Value::as_f64)
                .collect::<Option<_>>()?,
        ),
        ParamKind::Ints { .. } => Value::Ints(
            v.as_array()?
                .iter()
                .map(serde_json::Value::as_i64)
                .collect::<Option<_>>()?,
        ),
    })
}

/// A value as plain text.
fn plain(v: &Value) -> String {
    match v {
        Value::Float(x) => format!("{x}"),
        Value::Int(x) => x.to_string(),
        Value::Bool(x) => x.to_string(),
        Value::Choice(t) | Value::Text(t) => t.clone(),
        Value::List(l) => format!("{l:?}"),
        Value::Ints(l) => format!("{l:?}"),
    }
}

/// A field's label and value in the language of `ctx`, through the
/// parameter's descriptor (its module's translations, unit, precision).
fn field_text(ctx: &egui::Context, card: &Card, spec: &ParamSpec, v: &Value) -> (String, String) {
    let mut d = Descriptor::from_spec(spec, None);
    d.module = Some(card.module.clone());
    let path = helios_values::node_path(&card.node, &spec.name);
    let label = crate::params::label(ctx, &path, &d);
    let shown = match (&spec.kind, v) {
        (ParamKind::Choice { .. }, Value::Choice(c)) => crate::params::choice(ctx, &path, &d, c),
        // A mask, or any long text: not spelled out.
        (ParamKind::Text { .. }, Value::Text(t)) if t.len() > 64 => "…".into(),
        _ => crate::params::shown_in(&crate::i18n::locale(ctx), &d, v),
    };
    (label, shown)
}

/// One field of the answer form; the value when it was changed. A list
/// or a long text (a mask) is shown, not edited here.
fn field(ui: &mut Ui, card: &Card, spec: &ParamSpec, value: Value, shown: &str) -> Option<Value> {
    let id = format!("helios.decisions.{}.{}", card.node, spec.name);
    let (r, changed) = match (&spec.kind, value) {
        (ParamKind::Float { min, max, .. }, Value::Float(mut x)) => {
            let r = ui.add(egui::Slider::new(&mut x, *min..=*max));
            let changed = r.changed().then_some(Value::Float(x));
            (r, changed)
        }
        (ParamKind::Int { min, max, .. }, Value::Int(mut x)) => {
            let r = ui.add(egui::Slider::new(&mut x, *min..=*max));
            let changed = r.changed().then_some(Value::Int(x));
            (r, changed)
        }
        (ParamKind::Bool { .. }, Value::Bool(mut x)) => {
            let r = ui.checkbox(&mut x, "");
            let changed = r.changed().then_some(Value::Bool(x));
            (r, changed)
        }
        (ParamKind::Choice { options, .. }, Value::Choice(c)) => {
            let ctx = ui.ctx().clone();
            let mut d = Descriptor::from_spec(spec, None);
            d.module = Some(card.module.clone());
            let path = helios_values::node_path(&card.node, &spec.name);
            let mut picked = None;
            let r = egui::ComboBox::from_id_salt(&id)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for o in options {
                        let label = crate::params::choice(&ctx, &path, &d, o);
                        if ui.selectable_label(*o == c, label).clicked() && *o != c {
                            picked = Some(Value::Choice(o.clone()));
                        }
                    }
                })
                .response;
            (r, picked)
        }
        (ParamKind::Text { .. }, Value::Text(mut t)) if t.len() <= 64 => {
            let r = ui.text_edit_singleline(&mut t);
            let changed = r.changed().then_some(Value::Text(t));
            (r, changed)
        }
        _ => (ui.weak(shown), None),
    };
    crate::name_rect(ui.ctx(), &id, r.rect);
    changed
}
