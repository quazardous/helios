//! Workshops on the desktop (`workshop.*`, experimental, see
//! `docs/design/workshops.md`): a workshop applied after the node the
//! image is shown from, its targets aimed on a proxy of the open image,
//! as one undo step carrying the network and the aimed values
//! (`Shell::workspace_step_moving`, as the macro steps). The step itself
//! is `helios_template::workshop`'s; the commands exist only when the
//! experimental switch is on, and an agent's call is refused with the
//! switch's key when it is off.

use helios_core::Params;
use helios_shell::commands::Origin;
use helios_template::CompositeRef;
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
        "workshop.suggest",
        "Experimental: find workshops by what you want, ranked by name, keywords, then intent (`intent`)",
    ),
];

/// The aims of this session, by input content, workshop and targets: the
/// same input, workshop version and targets give the same values.
pub(super) type Aims = std::collections::BTreeMap<String, (Vec<Aimed>, Params)>;

impl Shell {
    /// Whether workshops are on for this session (the switch read at
    /// start declared their panel).
    pub fn workshops_on(&self) -> bool {
        self.panels
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
            let viewport = self.viewport.borrow();
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
        if let Some(hit) = self.workshop_aims.get(&key) {
            return Ok(hit.clone());
        }
        let built = state
            .bundle
            .catalogue(&self.workspace_catalogue())
            .build_composite(r, 0)?;
        let engine = self.viewport.borrow().engine().clone();
        let aimed =
            helios_template::workshop::aim_rendered(&engine, &built, input, current, targets)?;
        self.workshop_aims.insert(key, aimed.clone());
        Ok(aimed)
    }

    /// The applied workshop `node` (a node of the network whose composite
    /// has a workshop block): its pinned file.
    fn applied_workshop(
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
            None => crate::modules::selected(&super::lock(&self.values))
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
            let viewport = self.viewport.borrow();
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
            let mut store = super::lock(&self.values);
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

    /// `workshop.suggest`: the workshops of the workspace's catalogue the
    /// user's words find (`helios_template::workshop::suggest`), each with
    /// where it comes from and the node it is applied as, if it is.
    fn suggest_workshops(&self, args: &Value) -> Result<Value, String> {
        let intent = args["intent"].as_str().unwrap_or_default();
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let library = self
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
fn proxy(
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
