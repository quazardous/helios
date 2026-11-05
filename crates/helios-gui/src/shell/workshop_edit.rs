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
pub(super) const COMMANDS: &[(&str, &str)] = &[(
    "workshop.apply",
    "Experimental: apply a workshop after a node, aimed on the open image (`workshop`: id@version; `after`: the node the image is shown from by default; `aim`: true by default; `dry_run`)",
)];

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
            let extent = graph
                .extent(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("`{}` reads no image", at.node))?;
            let (scale, region) = crate::preview::plan(extent);
            viewport
                .engine()
                .render_at(graph, id, &at.port, region, scale)
                .map_err(|e| format!("{}: {e}", at.node))?
        };
        let key = format!(
            "{}|{r}|{}",
            input.content_hash(),
            serde_json::to_string(targets).map_err(|e| e.to_string())?
        );
        if let Some(hit) = self.workshop_aims.get(&key) {
            return Ok(Some(hit.clone()));
        }
        let built = state
            .bundle
            .catalogue(&self.workspace_catalogue())
            .build_composite(r, 0)?;
        let engine = self.viewport.borrow().engine().clone();
        let aimed = helios_template::workshop::aim_rendered(
            &engine,
            &built,
            &input,
            &Params::new(),
            targets,
        )?;
        self.workshop_aims.insert(key, aimed.clone());
        Ok(Some(aimed))
    }
}
