//! Designing a workshop from the user's words (experimental, see
//! `docs/design/workshops-on-the-fly.md`, section 1): the near goal.
//!
//! - `workshop.describe`: the user's words start a design session (the
//!   Workshops panel's "Describe what you want"); their own agent answers
//!   it over the node's MCP server. Helios stores no key and calls no
//!   model.
//! - `workshop.context`: what the agent may read for the session's words:
//!   the measures of the open image (whole and in two candidate zones),
//!   the module catalogue with each parameter's range, the bricks'
//!   descriptions, the workshops the words already find, and a small proxy
//!   of the image only when the user switched [`SEND_PROXY`] on.
//! - `workshop.design`: the agent's draft, checked by Helios on the proxy
//!   (`helios_template::draft::check_draft`) and, unless refused, applied
//!   as one undo step ("design workshop: <words>"); a new draft in the same
//!   session replaces the former one, one more step.
//! - `workshop.send_proxy`: the user's switch; an agent is refused.

use helios_core::{Image, Params};
use helios_shell::commands::Origin;
use helios_template::draft::{Check, check_draft, parse_draft};
use helios_template::format::{Endpoint, Measure, Zone};
use helios_template::workshop::{Aimed, aim_rendered, measure_in, zone_masks, zone_share};
use serde_json::{Value, json};

use super::Shell;
use crate::workshops::{Design, SEND_PROXY};

/// Two zones offered to the agent: what a backlit photo splits into. An
/// agent may copy them into its draft.
fn candidate_zones() -> Vec<Zone> {
    let zone = |name: &str, lo: f64, hi: f64| Zone {
        name: name.into(),
        module: "mask_parametric".into(),
        params: serde_json::from_value(json!({
            "luminance": { "list": [lo, hi, 0.05, 0.0] }
        }))
        .expect("parameters"),
        port: "mask".into(),
    };
    vec![zone("bright", 0.25, 4.0), zone("dark", 0.0, 0.1)]
}

/// What an agent is told of the draft it posts.
const DRAFT_HELP: &str = "Post with helios_workshop_design: {\"intent\": the words, \"composite\": a composite file (as helios_macro_interface's: name, nodes, connections from \"@\" port \"input\", exits to \"output\", exposed sliders with min/max or semantic ones with drives [{node, param, curve: [[x, y], ...]}]; format, id, version, inputs and outputs may be left out) with a workshop block {zones: [{name, module: \"mask_parametric\", params}], targets: [{param, measure, value, zone, against}]}, \"rationale\": {workshop, nodes, exposed, zones, targets: a sentence per name}}. Helios renders each slider at its ends on the proxy: one that changes nothing is dropped, an end that clips is narrowed; the answer's check says so, and a refused draft can be posted again.";

impl Shell {
    /// `workshop.describe`: starts a design session with the user's words
    /// (`intent`), or ends the one waiting (`cancel`).
    pub(super) fn describe_workshop(&mut self, args: &Value) -> Result<Value, String> {
        let mut panel = self.focus.workshops.borrow_mut();
        if args["cancel"].as_bool() == Some(true) {
            let was = panel.design.take();
            return Ok(json!({ "cancelled": was.map(|d| d.session) }));
        }
        let intent = args["intent"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("`intent`: say what the workshop is for")?
            .to_string();
        panel.sessions += 1;
        let session = panel.sessions;
        panel.design = Some(Design {
            session,
            intent: intent.clone(),
            waiting: true,
            ..Design::default()
        });
        Ok(json!({ "session": session, "intent": intent, "waiting": true }))
    }

    /// `workshop.send_proxy`: the user's switch for this session.
    pub(super) fn switch_send_proxy(
        &mut self,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        if *origin != Origin::User {
            return Err(format!(
                "only the user switches {SEND_PROXY}: ask them to tick it in the Workshops panel"
            ));
        }
        let on = args["on"].as_bool().ok_or("`on`: true or false")?;
        self.focus.workshops.borrow_mut().send_proxy = on;
        Ok(json!({ SEND_PROXY: on }))
    }

    /// The session's number and the words: `intent` when given, else the
    /// session's. Refused with neither.
    fn design_words(&self, args: &Value) -> Result<(Option<u32>, String), String> {
        let panel = self.focus.workshops.borrow();
        let session = panel.design.as_ref().map(|d| d.session);
        match args["intent"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            Some(words) => Ok((session, words.to_string())),
            None => panel
                .design
                .as_ref()
                .map(|d| (session, d.intent.clone()))
                .ok_or_else(|| {
                    "no design session: give `intent`, or ask the user to describe what they want in the Workshops panel".into()
                }),
        }
    }

    /// Where a design goes: after `after`, else the node the image is
    /// shown from; and what that gives the open image, on a proxy (`None`
    /// without an image open).
    fn design_input(&self, after: Option<&str>) -> Result<(Endpoint, Option<Image>), String> {
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let net = &state.bundle.template.network;
        let at = match after {
            None => net.view.clone(),
            Some(node) if node == net.view.node => net.view.clone(),
            Some(node) => {
                let cat = state.bundle.catalogue(&self.workspace_catalogue());
                let (_, outputs) = helios_template::network::ports_of(net, &cat, node)
                    .ok_or_else(|| format!("no node `{node}`"))?;
                let port = outputs
                    .iter()
                    .find(|p| p.ty.starts_with("image"))
                    .ok_or_else(|| format!("`{node}` has no image output"))?;
                Endpoint {
                    node: node.into(),
                    port: port.name.clone(),
                }
            }
        };
        let viewport = self.focus.viewport.borrow();
        let Some(graph) = viewport.graph() else {
            return Ok((at, None));
        };
        let id = graph
            .find(&at.node)
            .ok_or_else(|| format!("the image's network has no `{}`", at.node))?;
        let input = super::workshop_edit::proxy(&viewport, graph, id, &at.port)?;
        Ok((at, Some(input)))
    }

    /// `workshop.context`: what an agent may read to design a workshop for
    /// the words: measures, the catalogue, the bricks, the workshops they
    /// find; the proxy only when the user allowed it.
    pub(super) fn workshop_context(&mut self, args: &Value) -> Result<Value, String> {
        let (session, intent) = self.design_words(args)?;
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let cat = state.bundle.catalogue(&self.workspace_catalogue());
        let after = args["after"].as_str().filter(|s| !s.is_empty());
        let (at, input) = self.design_input(after)?;
        let measures = match &input {
            None => Value::Null,
            Some(image) => {
                let engine = self.focus.viewport.borrow().engine().clone();
                let zones = candidate_zones();
                let masks = zone_masks(&engine, &cat, &zones, image)?;
                let measure = |zone: Option<&Image>| {
                    let mut m = serde_json::Map::new();
                    for k in [
                        Measure::LumaMedian,
                        Measure::LumaHigh,
                        Measure::ClipHigh,
                        Measure::ZoneContrast,
                    ] {
                        if let Some(v) = measure_in(image, k, zone, None) {
                            m.insert(k.name().into(), json!((v * 1000.0).round() / 1000.0));
                        }
                    }
                    Value::Object(m)
                };
                let candidates: Vec<Value> = zones
                    .iter()
                    .map(|z| {
                        let mask = &masks[&z.name];
                        json!({
                            "zone": z,
                            "share": (zone_share(mask) * 10.0).round() / 10.0,
                            "measures": measure(Some(mask)),
                        })
                    })
                    .collect();
                let r = image.region();
                json!({
                    "proxy_size": [r.width, r.height],
                    "image": measure(None),
                    "candidate_zones": candidates,
                    "units": "stops around 1.0 for luma.*, stops for zone.contrast, % for clip.high",
                })
            }
        };
        let wanted: Option<Vec<String>> = args["modules"].as_array().map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        });
        let modules: Vec<Value> = cat.describe()["modules"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|m| {
                wanted
                    .as_ref()
                    .is_none_or(|w| m["name"].as_str().is_some_and(|n| w.iter().any(|x| x == n)))
            })
            .collect();
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
        let found = helios_template::workshop::suggest(entries.iter().map(|e| &e.file), &intent);
        let allowed = self.focus.workshops.borrow().send_proxy;
        let mut answer = json!({
            "session": session,
            "intent": intent,
            "after": { "node": at.node, "port": at.port },
            "measures": measures,
            "modules": modules,
            "bricks": helios_bricks::BrickRegistry::core().catalogue(),
            "workshops": found,
            "privacy": { SEND_PROXY: allowed },
            "draft": DRAFT_HELP,
        });
        if input.is_none() {
            answer["note"] =
                json!("no image is open: no measures; a draft is checked on an open image");
        }
        if args["proxy"].as_bool() == Some(true) {
            match (&input, allowed) {
                (Some(image), true) => answer["proxy"] = proxy_png(image)?,
                (None, _) => answer["proxy"] = Value::Null,
                (Some(_), false) => {
                    answer["proxy"] = Value::Null;
                    answer["proxy_refused"] = json!(format!(
                        "{SEND_PROXY} is off for this session: only the user switches it on, in the Workshops panel"
                    ));
                }
            }
        }
        Ok(answer)
    }

    /// `workshop.design`: the agent's `draft` for the words, checked on
    /// the proxy of what enters it, and applied (unless `apply` is false,
    /// `dry_run`, or the check refuses it) as one undo step, its targets
    /// aimed. A draft refused is an answer: the check says why.
    pub(super) fn design_workshop(
        &mut self,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        let (session, intent) = self.design_words(args)?;
        let dry = args["dry_run"].as_bool().unwrap_or(false);
        let apply = args["apply"].as_bool().unwrap_or(true);
        let after = args["after"].as_str().filter(|s| !s.is_empty());
        let mut posted = args["draft"].clone();
        if !posted.is_object() {
            return Err("`draft`: an object, {intent, composite, rationale}".into());
        }
        if posted["intent"]
            .as_str()
            .is_none_or(|s| s.trim().is_empty())
        {
            posted["intent"] = json!(intent);
        }
        let check = match parse_draft(&posted) {
            Ok(draft) => {
                let (_, input) = self.design_input(after)?;
                let input =
                    input.ok_or("no image is open: open one, the draft is checked on it")?;
                let state = self.workspace_state().ok_or("no workspace is open")?;
                let cat = state.bundle.catalogue(&self.workspace_catalogue());
                let engine = self.focus.viewport.borrow().engine().clone();
                let check = check_draft(&draft, &cat, &engine, &input);
                (check, Some((input, cat, engine)))
            }
            Err(problems) => (
                Check {
                    notes: problems.iter().map(|p| format!("refused: {p}")).collect(),
                    refused: problems,
                    ..Check::default()
                },
                None,
            ),
        };
        let (check, rendered) = check;
        self.design_notes(session, &check.notes);
        let answer = |applied: bool| {
            json!({
                "session": session,
                "intent": intent,
                "check": check,
                "applied": applied,
                "dry_run": dry,
            })
        };
        let (Some((input, cat, engine)), Some(cleaned), true) =
            (rendered, check.draft.clone(), check.ok && apply)
        else {
            return Ok(answer(false));
        };
        // Its targets aimed on the same proxy.
        let built = helios_template::catalogue::build(&cleaned.composite, &cat, 0)?;
        let block = cleaned.composite.workshop.clone().unwrap_or_default();
        let (aimed, values): (Vec<Aimed>, Params) = if block.targets.is_empty() {
            (Vec::new(), Params::new())
        } else {
            let masks = zone_masks(&engine, &cat, &block.zones, &input)?;
            aim_rendered(
                &engine,
                &built,
                &input,
                &masks,
                &Params::new(),
                &block.targets,
            )?
        };
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let catalogue = self.workspace_catalogue();
        // A new draft of the session replaces the former one.
        let replace: Option<String> = match args["replace"].as_str().filter(|s| !s.is_empty()) {
            Some(node) => Some(node.into()),
            None => self
                .focus
                .workshops
                .borrow()
                .design
                .as_ref()
                .filter(|d| Some(d.session) == session)
                .and_then(|d| d.node.clone())
                .filter(|n| {
                    state
                        .bundle
                        .template
                        .network
                        .nodes
                        .iter()
                        .any(|x| x.name == *n)
                }),
        };
        let mut step_answer = if dry {
            state
                .draft_applied(&catalogue, &cleaned, after, replace.as_deref(), &values)?
                .answer
        } else {
            let mut got = Value::Null;
            let label = format!("design workshop: {intent}");
            self.workspace_step_moving(&label, origin, |now| {
                let step =
                    now.draft_applied(&catalogue, &cleaned, after, replace.as_deref(), &values)?;
                got = step.answer;
                Ok((step.state, step.values))
            })?;
            got
        };
        if !dry {
            if let Some(node) = step_answer["node"].as_str() {
                self.select_node(node);
                let mut panel = self.focus.workshops.borrow_mut();
                if let Some(d) = panel.design.as_mut().filter(|d| Some(d.session) == session) {
                    d.waiting = false;
                    d.node = Some(node.into());
                    d.applied = step_answer["designed"].as_str().map(String::from);
                }
            }
        }
        let mut out = answer(!dry);
        if let Some(o) = step_answer.as_object_mut() {
            o.insert("aimed".into(), json!(aimed));
            for (k, v) in std::mem::take(o) {
                out[k] = v;
            }
        }
        Ok(out)
    }

    /// The check's notes shown under the session, when the words are the
    /// session's.
    fn design_notes(&self, session: Option<u32>, notes: &[String]) {
        let mut panel = self.focus.workshops.borrow_mut();
        if let Some(d) = panel.design.as_mut().filter(|d| Some(d.session) == session) {
            d.notes = notes.to_vec();
        }
    }
}

/// The proxy as an 8-bit sRGB PNG, in base64, without metadata: what an
/// agent sees when the user allows it.
fn proxy_png(image: &Image) -> Result<Value, String> {
    use base64::Engine as _;
    let rgba = helios_export::srgb8_rgba(image).map_err(|m| m.english)?;
    let r = image.region();
    let img =
        image::RgbaImage::from_raw(r.width, r.height, rgba).ok_or("the proxy has a wrong size")?;
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "mime": "image/png",
        "width": r.width,
        "height": r.height,
        "base64": base64::engine::general_purpose::STANDARD.encode(&png),
    }))
}
