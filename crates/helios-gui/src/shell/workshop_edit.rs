//! Workshops on the desktop (`workshop.*`, experimental, see
//! `docs/design/workshops.md`): a workshop applied after the node the
//! image is shown from, its targets aimed on a proxy of the open image,
//! as one undo step carrying the network and the aimed values
//! (`Shell::workspace_step_moving`, as the macro steps). The step itself
//! is `helios_template::workshop`'s; the commands exist only when the
//! experimental switch is on, and an agent's call is refused with the
//! switch's key when it is off.

use helios_core::{Message, Params};
use helios_shell::commands::Origin;
use helios_template::CompositeRef;
use helios_template::format::{At, Exposed, Measure, Target, Workshop};
use helios_template::workshop::Aimed;
use helios_template::workspace::State;
use serde_json::{Value, json};

use super::Shell;
use crate::workshops::SETTING;

/// The workshops' commands, with their titles (category
/// [`crate::workshops::CATEGORY`]).
pub(super) const COMMANDS: &[(&str, &str)] = &[
    (
        "workshop.apply",
        "Experimental: apply a workshop after a node, aimed on the open image (`workshop`: id@version; `after`: the node the image is shown from by default; `aim`: true by default; `dry_run`)",
    ),
    (
        "workshop.aim",
        "Experimental: aim an applied workshop's targets again on the open image, one value step (`node`, the selected one by default; `dry_run`)",
    ),
    (
        "workshop.save",
        "Experimental: save a macro as a workshop, its next version with an intent, keywords and targets (`node`, the selected one by default; `intent`, `keywords`, `targets`; `dry_run`); without `intent`, a form asks",
    ),
    (
        "workshop.suggest",
        "Experimental: find workshops by what you want, ranked by name, keywords, then intent (`intent`)",
    ),
    (
        "workshop.describe",
        "Experimental: start designing a workshop from your words, answered by your own agent over MCP (`intent`; `cancel`)",
    ),
    (
        "workshop.context",
        "Experimental: what an agent reads to design a workshop: the words, the image's measures, the modules, the bricks; a proxy only when allowed (`intent`, the session's by default; `proxy`; `modules`)",
    ),
    (
        "workshop.design",
        "Experimental: check a workshop draft on the open image and apply it as one undo step (`draft`; `intent`, the session's by default; `apply`: true by default; `replace`; `dry_run`)",
    ),
    (
        "workshop.session",
        "Experimental: the workshop session's record: each step piloting the workshop, its author, aim and zone measures (`action`: read, keep (yours alone: a session not kept is dropped when it ends) or close)",
    ),
    (
        "workshop.propose",
        "Experimental: propose values for the workshop being piloted, shown as ghost values the user accepts, overrides or rejects (`values`: by slider name; `aim`: the words it answers; `session`)",
    ),
    (
        "workshop.answer",
        "Experimental: answer a proposed value: accept it or set your own (one undo step each), or reject it; recorded in the session (`of`: the proposal's step; `param`; `action`: accept, override or reject; `value` for override); yours alone",
    ),
    (
        "workshop.generalise",
        "Experimental: make the workshop piloted automatic: one row per slider (a target a measure explains, else a fixed value, or kept as AI) and per AI zone (a fitted parametric mask, or kept as AI), with evidence, and J, A (the AI left, calls per photo) (`rows`: choices by name; `confirm`: write the next version, yours alone; `session`; `dry_run`)",
    ),
    (
        "workshop.run",
        "Experimental: run the applied workshop on a set of photos, a job: each photo aimed, its junctions answered, checked against the limits, written or held; one undo step (`node`, the selected one by default; `set`: selection or view; `images`: ids instead; `on_breach`: flag, hold or stop; `dry_run`: the photos, J, A and what the decision book already answers)",
    ),
    (
        "workshop.review",
        "Experimental: review a workshop run, flagged photos first, with their flags, provenance and values before and after; or act on one photo (`run`, the last one by default; `flag`: only photos raising it; `action`: accept or revert (yours alone), fix (`values`), decide a junction left to its fallback (`junction`, `value`, recorded in the decision book), rerun the held and decided photos (yours alone); `image`)",
    ),
    (
        "workshop.send_proxy",
        "Experimental: let an agent see a small proxy of the open image, for this session (`on`); the user's switch only",
    ),
];

/// The aims of this session, by input content, workshop and targets: the
/// same input, workshop version and targets give the same values.
pub(super) type Aims = std::collections::BTreeMap<String, (Vec<Aimed>, Params)>;

impl Shell {
    /// Whether workshops are on for this session (the switch read at
    /// start declared their panel).
    pub fn workshops_on(&self) -> bool {
        self.core
            .panels
            .iter()
            .any(|p| p.id.as_str() == crate::workshops::PANEL)
    }

    /// Runs one of [`COMMANDS`]; refused with the switch's key when off.
    pub(super) fn workshop_command(
        &mut self,
        id: &str,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        if !self.workshops_on() {
            return Err(format!(
                "workshops are experimental and off: set {SETTING} to true, then start Helios again"
            ));
        }
        match id {
            "workshop.apply" => self.apply_workshop(args, origin),
            "workshop.aim" => self.aim_workshop(args, origin),
            "workshop.suggest" => self.suggest_workshops(args),
            "workshop.save" => self.save_workshop(args, origin),
            "workshop.describe" => self.describe_workshop(args),
            "workshop.context" => self.workshop_context(args),
            "workshop.design" => self.design_workshop(args, origin),
            "workshop.send_proxy" => self.switch_send_proxy(args, origin),
            "workshop.session" => self.workshop_session(args, origin),
            "workshop.propose" => self.propose_values(args, origin),
            "workshop.answer" => self.answer_proposal(args, origin),
            "workshop.generalise" => self.generalise_session(args, origin),
            "workshop.run" => self.run_workshop(args, origin),
            "workshop.review" => self.review_workshop(args, origin),
            other => Err(format!("no workshop command `{other}`")),
        }
    }

    /// `workshop.apply`: the workshop's node inserted after `after`, its
    /// targets aimed on the open image (unless `aim` is false, or no
    /// image is open: the answer says so), as one undo step.
    fn apply_workshop(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        let text = args["workshop"].as_str().unwrap_or_default();
        let r = helios_template::network::composite_ref(text)
            .ok_or_else(|| format!("`workshop`: `{text}` is not id@version"))?;
        let after = args["after"].as_str().filter(|s| !s.is_empty());
        let aim = args["aim"].as_bool().unwrap_or(true);
        let dry = args["dry_run"].as_bool().unwrap_or(false);
        let catalogue = self.workspace_catalogue();
        let now = self.workspace_state().ok_or("no workspace is open")?;
        let (file, at) = now.workshop_insertion(&catalogue, &r, after)?;
        let targets = file
            .workshop
            .as_ref()
            .map(|w| w.targets.clone())
            .unwrap_or_default();
        let (aimed, values, note) = if !aim || targets.is_empty() {
            (Vec::new(), Params::new(), None)
        } else {
            match self.aim_after(&now, &r, &at, &targets)? {
                Some((aimed, values)) => (aimed, values, None),
                None => (
                    Vec::new(),
                    Params::new(),
                    Some("no image is open: not aimed"),
                ),
            }
        };
        let mut answer = if dry {
            now.workshop_applied(&catalogue, &r, after, &values)?.answer
        } else {
            let mut answer = Value::Null;
            let label = format!("apply workshop {}", file.name);
            self.workspace_step_moving(&label, origin, |now| {
                let step = now.workshop_applied(&catalogue, &r, after, &values)?;
                answer = step.answer;
                Ok((step.state, step.values))
            })?;
            if let Some(node) = answer["node"].as_str() {
                self.select_node(node);
                let node = node.to_string();
                let workshop = answer["workshop"]
                    .as_str()
                    .map_or_else(|| r.to_string(), String::from);
                self.start_session(
                    &node,
                    &workshop,
                    helios_template::session::Kind::Apply,
                    origin,
                    None,
                );
                answer["session"] = json!(self.session_id());
            }
            answer
        };
        answer["aimed"] = json!(aimed);
        if let Some(note) = note {
            answer["note"] = json!(note);
        }
        answer["dry_run"] = json!(dry);
        Ok(answer)
    }

    /// The workshop `r`'s targets aimed on what `at` gives the open
    /// image, rendered at the preview's proxy size (256 px on the long
    /// side); `None` without an image open. Cached for the session.
    fn aim_after(
        &mut self,
        state: &State,
        r: &CompositeRef,
        at: &helios_template::format::Endpoint,
        targets: &[helios_template::format::Target],
    ) -> Result<Option<(Vec<Aimed>, Params)>, String> {
        let input = {
            let viewport = self.focus.viewport.borrow();
            let Some(graph) = viewport.graph() else {
                return Ok(None);
            };
            let id = graph
                .find(&at.node)
                .ok_or_else(|| format!("the image's network has no `{}`", at.node))?;
            proxy(&viewport, graph, id, &at.port)?
        };
        self.aim_cached(state, r, &input, &Params::new(), targets)
            .map(Some)
    }

    /// [`helios_template::workshop::aim_rendered`] on `input`, cached for
    /// the session by the input's content, the workshop, the values it
    /// starts from and its targets.
    fn aim_cached(
        &mut self,
        state: &State,
        r: &CompositeRef,
        input: &helios_core::Image,
        current: &Params,
        targets: &[helios_template::format::Target],
    ) -> Result<(Vec<Aimed>, Params), String> {
        let key = format!(
            "{}|{r}|{}|{}",
            input.content_hash(),
            serde_json::to_string(current).map_err(|e| e.to_string())?,
            serde_json::to_string(targets).map_err(|e| e.to_string())?
        );
        if let Some(hit) = self.focus.workshop_aims.get(&key) {
            return Ok(hit.clone());
        }
        let catalogue = state.bundle.catalogue(&self.workspace_catalogue());
        let built = catalogue.build_composite(r, 0)?;
        let engine = self.focus.viewport.borrow().engine().clone();
        // The workshop's zones, drawn once on the input.
        let zones = catalogue
            .get_composite(r)
            .and_then(|f| f.workshop.as_ref())
            .map(|w| w.zones.clone())
            .unwrap_or_default();
        let masks = helios_template::workshop::zone_masks(&engine, &catalogue, &zones, input)?;
        let aimed = helios_template::workshop::aim_rendered(
            &engine, &built, input, &masks, current, targets,
        )?;
        self.focus.workshop_aims.insert(key, aimed.clone());
        Ok(aimed)
    }

    /// The applied workshop `node` (a node of the network whose composite
    /// has a workshop block): its pinned file.
    pub(super) fn applied_workshop(
        &self,
        state: &State,
        node: &str,
    ) -> Result<(CompositeRef, helios_template::CompositeFile), String> {
        let r = state
            .bundle
            .template
            .network
            .nodes
            .iter()
            .find(|n| n.name == node)
            .ok_or_else(|| format!("no node `{node}`"))?
            .composite
            .clone()
            .ok_or_else(|| format!("`{node}` is a module, not a workshop"))?;
        let file = state
            .bundle
            .catalogue(&self.workspace_catalogue())
            .get_composite(&r)
            .cloned()
            .ok_or_else(|| format!("unknown composite `{r}`"))?;
        if file.workshop.is_none() {
            return Err(format!("`{node}` is a macro without a workshop block"));
        }
        Ok((r, file))
    }

    /// `workshop.aim`: the applied workshop `node`'s targets solved again
    /// on the open image, from what feeds the node and the values it holds
    /// now, as one value step ("aim Lift midtones"); nothing in the
    /// workspace changes. A dry run answers the values without setting
    /// them.
    fn aim_workshop(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        let node = match args["node"].as_str().filter(|s| !s.is_empty()) {
            Some(n) => n.to_string(),
            None => crate::modules::selected(&super::lock(&self.core.values))
                .ok_or("no workshop is selected")?,
        };
        let dry = args["dry_run"].as_bool().unwrap_or(false);
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let (r, file) = self.applied_workshop(&state, &node)?;
        let targets = file
            .workshop
            .as_ref()
            .map(|w| w.targets.clone())
            .unwrap_or_default();
        if targets.is_empty() {
            return Err(format!("`{node}` has no target to aim"));
        }
        let (input, current) = {
            let viewport = self.focus.viewport.borrow();
            let graph = viewport
                .graph()
                .ok_or("no image is open: open one to aim on it")?;
            let id = graph
                .find(&node)
                .ok_or_else(|| format!("the image's network has no `{node}`"))?;
            let (from, port) = graph
                .input_ports(id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .next()
                .flatten()
                .ok_or_else(|| format!("nothing feeds `{node}`"))?;
            let current = graph
                .params(id)
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            (proxy(&viewport, graph, from, &port)?, current)
        };
        let (aimed, values) = self.aim_cached(&state, &r, &input, &current, &targets)?;
        if !dry {
            let label = format!("aim {}", file.name);
            let mut store = super::lock(&self.core.values);
            let group = store.begin_group(origin.clone(), &label);
            let mut set = Ok(());
            for (param, value) in values.iter() {
                let path = format!("node/{node}/{param}");
                set = store
                    .set(
                        helios_values::Set::new(
                            &path,
                            helios_values::Change::To(value.clone()),
                            origin.clone(),
                        )
                        .gesture(group),
                    )
                    .map(|_| ())
                    .map_err(|e| format!("{path}: {e}"));
                if set.is_err() {
                    break;
                }
            }
            store.end(group);
            set?;
        }
        let values: serde_json::Map<String, Value> = values
            .iter()
            .map(|(p, v)| {
                (
                    p.to_string(),
                    v.as_f64().map_or_else(|| json!(v), |f| json!(f)),
                )
            })
            .collect();
        Ok(json!({
            "node": node,
            "workshop": r.to_string(),
            "aimed": aimed,
            "values": values,
            "dry_run": dry,
        }))
    }

    /// `workshop.save`: the macro `node` saved as a workshop, its next
    /// version carrying the block (`intent`, `keywords`, `targets`), the
    /// node pinned to it, as one workspace step that undo takes back
    /// (`State::workshop_saved`). Without `intent`, a form asks for the
    /// block, each target prefilled from what the open image measures
    /// now. Publishing stays `macro.publish`, asked first.
    fn save_workshop(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        let node = match args["node"].as_str().filter(|s| !s.is_empty()) {
            Some(n) => n.to_string(),
            None => crate::modules::selected(&super::lock(&self.core.values))
                .ok_or("no macro is selected")?,
        };
        // A node of an opened macro names it.
        let node = node.split('.').next().unwrap_or(&node).to_string();
        let Some(intent) = args["intent"].as_str() else {
            self.workshop_form(&node)?;
            return Ok(json!({ "node": node, "asked": true }));
        };
        let catalogue = self.workspace_catalogue();
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let interface = state.macro_interface(&catalogue, &node)?;
        let keywords: Vec<String> = match &args["keywords"] {
            Value::Null => Vec::new(),
            Value::String(s) => s
                .split(',')
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(String::from)
                .collect(),
            v => serde_json::from_value(v.clone()).map_err(|e| format!("keywords: {e}"))?,
        };
        let targets: Vec<Target> = match &args["targets"] {
            Value::Null => form_targets(args, &interface.exposed)?,
            v => serde_json::from_value(v.clone()).map_err(|e| format!("targets: {e}"))?,
        };
        // Zones: those given, else the ones the macro's file has.
        let zones: Vec<helios_template::format::Zone> = match &args["zones"] {
            Value::Null => state
                .bundle
                .catalogue(&catalogue)
                .get_composite(&interface.composite)
                .and_then(|f| f.workshop.as_ref())
                .map(|w| w.zones.clone())
                .unwrap_or_default(),
            v => serde_json::from_value(v.clone()).map_err(|e| format!("zones: {e}"))?,
        };
        // Junctions: the ones the macro's file has, while their zones and
        // sliders stay.
        let junctions: Vec<helios_template::format::Junction> = state
            .bundle
            .catalogue(&catalogue)
            .get_composite(&interface.composite)
            .and_then(|f| f.workshop.as_ref())
            .map(|w| {
                w.junctions
                    .iter()
                    .filter(|j| match j.kind {
                        helios_template::format::JunctionKind::Zone => {
                            zones.iter().any(|z| z.name == j.name)
                        }
                        helios_template::format::JunctionKind::Value => {
                            !targets.iter().any(|t| t.param == j.name)
                        }
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let mut workshop = Workshop {
            intent: intent.trim().to_string(),
            keywords,
            targets,
            zones,
            junctions,
            limits: None,
        };
        // Limits: the file's, while their sliders, targets and junctions
        // stay.
        let exposed: Vec<&str> = interface.exposed.iter().map(|e| e.name.as_str()).collect();
        workshop.limits = state
            .bundle
            .catalogue(&catalogue)
            .get_composite(&interface.composite)
            .and_then(|f| f.workshop.as_ref())
            .and_then(|w| w.limits.clone())
            .map(|l| helios_template::limits::prune(l, &workshop, &exposed));
        let label = format!("save {node} as a workshop");
        self.macro_step(&label, args, origin, |now, values| {
            now.workshop_saved(&catalogue, &node, &workshop, values)
        })
    }

    /// What the macro `node`'s output measures on the open image now, by
    /// measure; `None` without an image open.
    fn measures_now(&self, node: &str, port: &str) -> Option<Vec<(Measure, f64)>> {
        let viewport = self.focus.viewport.borrow();
        let graph = viewport.graph()?;
        let id = graph.find(node)?;
        let image = proxy(&viewport, graph, id, port).ok()?;
        Some(
            Measure::ALL
                .into_iter()
                .filter_map(|m| Some((m, helios_template::workshop::measure(&image, m)?)))
                .collect(),
        )
    }

    /// The form of `workshop.save` on the macro `node`: its intent, its
    /// keywords, and per exposed number parameter a target (none by
    /// default, the value prefilled with what the open image measures
    /// now at the macro's output). The answer runs `workshop.save`.
    fn workshop_form(&mut self, node: &str) -> Result<(), String> {
        use crate::dialogs::Field;
        use helios_values::Descriptor;
        let catalogue = self.workspace_catalogue();
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let interface = state.macro_interface(&catalogue, node)?;
        let file = state
            .bundle
            .catalogue(&catalogue)
            .get_composite(&interface.composite)
            .cloned()
            .ok_or_else(|| format!("unknown composite `{}`", interface.composite))?;
        let block = file.workshop.clone().unwrap_or_else(|| Workshop {
            intent: file.description.clone(),
            keywords: Vec::new(),
            targets: Vec::new(),
            zones: Vec::new(),
            junctions: Vec::new(),
            limits: None,
        });
        let port = file
            .outputs
            .first()
            .map(|o| o.name.clone())
            .unwrap_or_default();
        let now = self.measures_now(node, &port);
        let text_field = |key: &str, label: String, value: String| Field {
            key: key.into(),
            descriptor: Descriptor::text(&label, ""),
            value: helios_core::Value::Text(value),
        };
        let mut fields = vec![
            text_field(
                "intent",
                self.text(Message::new("gui-workshops-save-intent", "what it is for")),
                block.intent.clone(),
            ),
            text_field(
                "keywords",
                self.text(Message::new(
                    "gui-workshops-save-keywords",
                    "keywords, separated by commas",
                )),
                block.keywords.join(", "),
            ),
        ];
        let now_text = now.as_ref().map(|now| {
            now.iter()
                .map(|(m, v)| {
                    let (what, unit) = crate::workshops::measure_words(*m);
                    format!("{} {v:.2} {}", self.text(what), self.text(unit))
                })
                .collect::<Vec<_>>()
                .join(", ")
        });
        let mut seen: Vec<&str> = Vec::new();
        for x in &interface.exposed {
            if seen.contains(&x.name.as_str()) {
                continue;
            }
            seen.push(&x.name);
            // A semantic slider is a number of its own.
            let number = x.is_driven()
                || interface.inner.iter().any(|p| {
                    p.node == x.node
                        && p.spec.name == x.param
                        && matches!(
                            p.spec.kind,
                            helios_core::ParamKind::Float { .. }
                                | helios_core::ParamKind::Int { .. }
                        )
                });
            if !number {
                continue;
            }
            let shown = x.label.clone().unwrap_or_else(|| x.name.clone());
            let target = block.targets.iter().find(|t| t.param == x.name);
            let measure = target.map_or(NONE, |t| t.measure.name());
            let value = target.map(|t| t.value).or_else(|| {
                now.as_ref()?
                    .iter()
                    .find(|(m, _)| *m == Measure::LumaMedian)
                    .map(|(_, v)| (v * 100.0).round() / 100.0)
            });
            let label = match &now_text {
                Some(now) => self.text(
                    Message::new(
                        "gui-workshops-save-target-now",
                        "{ $param }: target (the image now: { $now })",
                    )
                    .with("param", &shown)
                    .with("now", now),
                ),
                None => self.text(
                    Message::new("gui-workshops-save-target", "{ $param }: target")
                        .with("param", &shown),
                ),
            };
            let mut choices = vec![NONE];
            choices.extend(Measure::ALL.iter().map(|m| m.name()));
            fields.push(Field {
                key: format!("{MEASURE_KEY}{}", x.name),
                descriptor: Descriptor::choice(&label, &choices, NONE),
                value: helios_core::Value::Choice(measure.into()),
            });
            fields.push(Field {
                key: format!("{VALUE_KEY}{}", x.name),
                descriptor: Descriptor::float(
                    &self.text(
                        Message::new(
                            "gui-workshops-save-value",
                            "{ $param }: value (stops, or % for clip.high)",
                        )
                        .with("param", &shown),
                    ),
                    -100.0,
                    100.0,
                    0.0,
                ),
                value: helios_core::Value::Float(value.unwrap_or_default()),
            });
        }
        let f = crate::dialogs::Form {
            title: self.text(
                Message::new("gui-workshops-save-title", "Save { $node } as a workshop")
                    .with("node", node),
            ),
            fields,
            verb: self.text(
                Message::new("gui-workshops-save-verb", "Save as { $file }")
                    .with("file", interface.next.to_string()),
            ),
            command: "workshop.save".into(),
        };
        self.form_with(f, json!({ "node": node }))
    }

    /// `workshop.suggest`: the workshops of the workspace's catalogue the
    /// user's words find (`helios_template::workshop::suggest`), each with
    /// where it comes from and the node it is applied as, if it is.
    fn suggest_workshops(&self, args: &Value) -> Result<Value, String> {
        let intent = args["intent"].as_str().unwrap_or_default();
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let library = self
            .focus
            .edit
            .as_ref()
            .map(|e| e.library.clone())
            .unwrap_or_default();
        let entries = crate::workshops::entries(
            &helios_template::shipped::composites(),
            &library,
            &state.bundle.composites,
        );
        let found: Vec<Value> =
            helios_template::workshop::suggest(entries.iter().map(|e| &e.file), intent)
                .into_iter()
                .map(|s| {
                    let origin = entries
                        .iter()
                        .find(|e| e.file.reference() == s.workshop)
                        .map(|e| e.origin);
                    let applied: Vec<&str> = state
                        .bundle
                        .template
                        .network
                        .nodes
                        .iter()
                        .filter(|n| n.composite.as_ref().is_some_and(|r| r.id == s.workshop.id))
                        .map(|n| n.name.as_str())
                        .collect();
                    json!({
                        "workshop": s.workshop.to_string(),
                        "name": s.name,
                        "intent": s.intent,
                        "matched": s.matched,
                        "words": s.words,
                        "from": match origin {
                            Some(crate::workshops::Origin::Shipped) => "shipped",
                            Some(crate::workshops::Origin::Library) => "library",
                            _ => "workspace",
                        },
                        "applied": applied,
                    })
                })
                .collect();
        Ok(json!({ "intent": intent, "workshops": found }))
    }
}

/// What `node`'s `port` gives the open image, rendered at the preview's
/// proxy size (256 px on the long side).
pub(super) fn proxy(
    viewport: &crate::viewport::Viewport,
    graph: &helios_core::Graph,
    node: helios_core::NodeId,
    port: &str,
) -> Result<helios_core::Image, String> {
    let extent = graph
        .extent(node)
        .map_err(|e| e.to_string())?
        .ok_or("the node reads no image")?;
    let (scale, region) = crate::preview::plan(extent);
    viewport
        .engine()
        .render_at(graph, node, port, region, scale)
        .map_err(|e| e.to_string())
}

/// The form's choice of no target.
const NONE: &str = "none";
/// The form's keys: per exposed parameter, its target's measure and value.
const MEASURE_KEY: &str = "measure_";
const VALUE_KEY: &str = "value_";

/// The targets the form of `workshop.save` answered, in the order the
/// parameters are exposed: a measure other than [`NONE`] and its value.
fn form_targets(args: &Value, exposed: &[Exposed]) -> Result<Vec<Target>, String> {
    let mut targets: Vec<Target> = Vec::new();
    for x in exposed {
        if targets.iter().any(|t| t.param == x.name) {
            continue;
        }
        let Some(m) = args[format!("{MEASURE_KEY}{}", x.name)]
            .as_str()
            .filter(|m| *m != NONE)
        else {
            continue;
        };
        let measure = Measure::ALL
            .into_iter()
            .find(|k| k.name() == m)
            .ok_or_else(|| format!("{}: `{m}` is not a measure", x.name))?;
        let value = args[format!("{VALUE_KEY}{}", x.name)]
            .as_f64()
            .ok_or_else(|| format!("{}: the target's value is not a number", x.name))?;
        targets.push(Target {
            param: x.name.clone(),
            measure,
            value,
            at: At::Output,
            zone: None,
            against: None,
        });
    }
    Ok(targets)
}
