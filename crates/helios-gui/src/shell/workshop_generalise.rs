//! "Make it automatic" (`workshop.generalise`, experimental, see
//! `docs/design/workshops-on-the-fly.md`, section 3): the session's net
//! outcome on its test photo turned into one row per slider and per AI
//! zone (`helios_template::generalise`), each with its evidence and a
//! default, and `J` and `A`, the AI the workshop keeps. Nothing is
//! written until the user confirms: `confirm` writes the workshop's next
//! version (the targets, the fixed values as the sliders' defaults, the
//! fitted zones) as one undo step, and the session is kept, going on
//! with that version (the edit's version saved next carries its id). An agent reads and amends the rows; only the
//! user confirms.

use std::collections::BTreeMap;

use helios_shell::commands::Origin;
use helios_template::generalise::{self, AI_ZONE, Case, Choice, Minimum, Row, ZoneChoice, ZoneRow};
use helios_template::session::Kind;
use serde_json::{Value, json};

use super::Shell;
use super::workshop_session::NO_SESSION;

/// The rows the panel shows for the user to confirm.
#[derive(Clone, Debug, PartialEq)]
pub struct Generalised {
    pub session: String,
    pub rows: Vec<Row>,
    pub zones: Vec<ZoneRow>,
    pub minimum: Minimum,
}

impl Generalised {
    /// The rows' choices as they stand, as `workshop.generalise` takes
    /// them (`rows`).
    pub fn choices(&self) -> Value {
        let mut out = serde_json::Map::new();
        for r in &self.rows {
            out.insert(r.param.clone(), json!(choice_word(r.default)));
        }
        for z in &self.zones {
            let w = match z.default {
                ZoneChoice::Parametric => "parametric",
                ZoneChoice::Ai => "ai",
            };
            out.insert(z.zone.clone(), json!(w));
        }
        Value::Object(out)
    }
}

/// A slider row's choice as `rows` names it.
pub fn choice_word(c: Choice) -> &'static str {
    match c {
        Choice::Target => "target",
        Choice::Fixed => "fixed",
        Choice::Ai => "ai",
    }
}

impl Shell {
    /// `workshop.generalise`: the rows for the session going on (`rows`
    /// amends their choices, by slider or zone name); with `confirm`, the
    /// user's alone, the workshop's next version written as one undo step
    /// and the session kept and closed. `dry_run` with `confirm` says what
    /// would be written.
    pub(super) fn generalise_session(
        &mut self,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        self.follow_session(None);
        let record = {
            let panel = self.focus.workshops.borrow();
            panel.pilot.as_ref().ok_or(NO_SESSION)?.record.clone()
        };
        if let Some(asked) = args["session"].as_str().filter(|s| !s.is_empty())
            && asked != record.session
        {
            return Err(format!(
                "no session `{asked}`: the session going on is `{}`",
                record.session
            ));
        }
        let confirm = args["confirm"].as_bool().unwrap_or(false);
        let dry = args["dry_run"].as_bool().unwrap_or(false);
        if confirm && !dry && *origin != Origin::User {
            return Err(
                "only the user confirms a generalisation: the rows are shown in the Workshops panel, with Confirm"
                    .into(),
            );
        }
        let base = self.workspace_catalogue();
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let catalogue = state.bundle.catalogue(&base);
        let r = helios_template::network::composite_ref(&record.workshop)
            .ok_or("not a workshop reference")?;
        let file = catalogue
            .get_composite(&r)
            .cloned()
            .ok_or_else(|| format!("unknown composite `{r}`"))?;
        let block = file
            .workshop
            .clone()
            .ok_or_else(|| format!("`{r}` is not a workshop"))?;
        let input = self.session_input(&record)?;
        let values = self.session_values(&record, &file);
        let engine = self.focus.viewport.borrow().engine().clone();
        let masks =
            helios_template::workshop::zone_masks(&engine, &catalogue, &block.zones, &input)?;
        let built = catalogue.build_composite(&r, 0)?;
        // The sliders the session set, and those an AI's value holds.
        let net = record.net();
        let params: Vec<String> = net
            .keys()
            .filter(|p| file.exposed.iter().any(|x| x.name == **p))
            .cloned()
            .collect();
        let proposed: Vec<String> = net
            .iter()
            .filter(|(_, (_, seq))| record.step(*seq).is_some_and(|s| s.kind == Kind::Accept))
            .map(|(p, _)| p.clone())
            .collect();
        let zone_names: Vec<String> = block.zones.iter().map(|z| z.name.clone()).collect();
        let ai_zones: Vec<String> = block
            .zones
            .iter()
            .filter(|z| z.module == AI_ZONE)
            .map(|z| z.name.clone())
            .collect();
        let case = Case {
            input: &input,
            zones: &masks,
            values: &values,
        };
        let cases = [case];
        let mut rows =
            generalise::generalise_rendered(&engine, &built, &cases, &params, &zone_names)?;
        generalise::keep_ai_values(&mut rows, &proposed);
        let mut zones = if ai_zones.is_empty() {
            Vec::new()
        } else {
            generalise::generalise_zones(&cases, &ai_zones)?
        };
        match &args["rows"] {
            Value::Null => {}
            Value::Object(o) => {
                let choices: BTreeMap<String, String> = o
                    .iter()
                    .map(|(k, v)| {
                        v.as_str()
                            .map(|s| (k.clone(), s.to_string()))
                            .ok_or_else(|| {
                                format!("`rows.{k}`: a word (target, fixed, ai, parametric)")
                            })
                    })
                    .collect::<Result<_, String>>()?;
                generalise::choose(&mut rows, &mut zones, &choices)?;
            }
            _ => return Err("`rows`: an object, {slider or zone: choice}".into()),
        }
        let minimum = generalise::minimum(&rows, &zones);
        let mut answer = json!({
            "session": record.session,
            "workshop": record.workshop,
            "node": record.node,
            "photos": record.images.len(),
            "rows": rows,
            "zones": zones,
            "J": minimum.junctions,
            "A": minimum.calls_per_photo,
            "confirmed": false,
        });
        if !confirm {
            self.focus.workshops.borrow_mut().generalised = Some(Generalised {
                session: record.session.clone(),
                rows,
                zones,
                minimum,
            });
            return Ok(answer);
        }
        let (next, defaults) = generalise::written(&block, &rows, &zones)?;
        let node = record.node.clone();
        let label = format!("make {} automatic", file.name);
        let saved = self.macro_step(&label, args, origin, |now, values| {
            now.workshop_written(&base, &node, &next, &defaults, values)
        })?;
        answer["written"] = saved;
        if dry {
            answer["dry_run"] = json!(true);
            return Ok(answer);
        }
        // Kept, and going on with the version just written: the edit's
        // version saved next carries the session's id, linking them.
        {
            let mut panel = self.focus.workshops.borrow_mut();
            panel.generalised = None;
            if let Some(p) = panel.pilot.as_mut() {
                p.record.kept = true;
                let to = &answer["written"]["to"];
                if let (Some(id), Some(v)) = (to["id"].as_str(), to["version"].as_u64()) {
                    p.record.workshop = format!("{id}@{v}");
                }
            }
            panel.queue_write();
        }
        answer["kept"] = json!(true);
        answer["confirmed"] = json!(true);
        answer["dry_run"] = json!(false);
        Ok(answer)
    }
}
