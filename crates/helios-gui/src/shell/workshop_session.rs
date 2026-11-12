//! The record of piloting a workshop (experimental, see
//! `docs/design/workshops-on-the-fly.md`, section 2, and
//! `helios_template::session`). Recording is always on: a session starts
//! when a workshop is designed or applied, and each value step on its
//! node is a step of the record, with its author, its kind, its aim and
//! the zone measures before and after, on the proxy. The record is kept
//! in the vault (`workshops/sessions`) as it grows, and dropped when the
//! session ends unless the user kept it (C6); the versions saved meanwhile
//! carry the session's id.
//!
//! - `workshop.session`: `read` the record, `keep` it (the user's), or
//!   `close` the session.
//! - `workshop.propose`: an AI's values for the workshop, shown as ghost
//!   values; nothing is set.
//! - `workshop.answer`: the user's answer to a ghost: `accept` (one undo
//!   step), `override` (their own value), `reject`; each one recorded.

use helios_core::Params;
use helios_shell::commands::Origin;
use helios_template::session::{Change, Kind, Measures, Record, Session, measured};
use serde_json::{Value, json};

use super::{Shell, lock};
use crate::persist::who;
use crate::workshops::Pilot;

impl Shell {
    /// Starts recording a session on the workshop node `node` (`r`, its
    /// `id@version`) just designed or applied by `origin`: the step that
    /// did it is the record's first, measured before (what enters the
    /// workshop) and after. The session going on ends first.
    pub(super) fn start_session(
        &mut self,
        node: &str,
        workshop: &str,
        kind: Kind,
        origin: &Origin,
        aim: Option<&str>,
    ) {
        self.end_session();
        let (top, rev) = {
            let store = lock(&self.core.values);
            (store.undo_steps().last().map(|s| s.id), store.history_rev())
        };
        let mut panel = self.focus.workshops.borrow_mut();
        panel.sessions += 1;
        let id = format!("s-{}-{}", session_stamp(), panel.sessions);
        let image = panel.image.clone().unwrap_or_else(|| "unsaved".into());
        drop(panel);
        let mut record = Session {
            session: id,
            workshop: workshop.into(),
            node: node.into(),
            intent: aim.map(String::from),
            images: vec![image],
            kept: false,
            steps: Vec::new(),
        };
        let measures = self
            .session_measures(&record, true)
            .ok()
            .zip(self.session_measures(&record, false).ok());
        record.push(Record {
            aim: aim.map(String::from),
            store_step: top,
            measures: measures.map(|(before, after)| Measures { before, after }),
            ..Record::new(&who(origin), kind)
        });
        let mut panel = self.focus.workshops.borrow_mut();
        panel.pilot = Some(Pilot { record, top, rev });
        panel.queue_write();
    }

    /// Ends the session going on: dropped from the vault unless kept.
    pub(super) fn end_session(&mut self) -> Option<Session> {
        let mut panel = self.focus.workshops.borrow_mut();
        let pilot = panel.pilot.take()?;
        if !pilot.record.kept {
            panel
                .session_writes
                .push((pilot.record.session.clone(), None));
        }
        Some(pilot.record)
    }

    /// The zone measures of the session's workshop on the open image's
    /// proxy: of what enters it (`input`), or of what it gives with the
    /// values its node holds now.
    fn session_measures(
        &self,
        record: &Session,
        input_only: bool,
    ) -> Result<std::collections::BTreeMap<String, f64>, String> {
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let catalogue = state.bundle.catalogue(&self.workspace_catalogue());
        let r = helios_template::network::composite_ref(&record.workshop)
            .ok_or("not a workshop reference")?;
        let file = catalogue
            .get_composite(&r)
            .ok_or_else(|| format!("unknown composite `{r}`"))?;
        let zones = file
            .workshop
            .as_ref()
            .map(|w| w.zones.clone())
            .unwrap_or_default();
        let input = self.session_input(record)?;
        let engine = self.focus.viewport.borrow().engine().clone();
        if input_only {
            let masks = helios_template::workshop::zone_masks(&engine, &catalogue, &zones, &input)?;
            return Ok(helios_template::session::measures(&input, &masks));
        }
        let built = catalogue.build_composite(&r, 0)?;
        let values = self.session_values(record, file);
        measured(&engine, &catalogue, &built, &zones, &input, &values)
    }

    /// The values the session's workshop node holds now, by exposed name.
    pub(super) fn session_values(
        &self,
        record: &Session,
        file: &helios_template::format::CompositeFile,
    ) -> Params {
        let mut values = Params::new();
        let store = lock(&self.core.values);
        let prefix = format!("node/{}/", record.node);
        for (path, entry) in store.list(&prefix) {
            if let Some(name) = path.strip_prefix(&prefix)
                && file.exposed.iter().any(|x| x.name == name)
            {
                values.set(name, entry.value.clone());
            }
        }
        values
    }

    /// What enters the session's workshop on the open image, at the
    /// preview's proxy size.
    pub(super) fn session_input(&self, record: &Session) -> Result<helios_core::Image, String> {
        let viewport = self.focus.viewport.borrow();
        let graph = viewport.graph().ok_or("no image is open")?;
        let id = graph
            .find(&record.node)
            .ok_or_else(|| format!("the image's network has no `{}`", record.node))?;
        let (from, port) = graph
            .input_ports(id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .next()
            .flatten()
            .ok_or("nothing feeds the workshop")?;
        super::workshop_edit::proxy(&viewport, graph, from, &port)
    }

    /// Measures the session's first step once the image's network holds
    /// the workshop's node (a design or an apply reaches the viewport a
    /// frame or two after its step).
    fn measure_first_step(&mut self) {
        let record = {
            let panel = self.focus.workshops.borrow();
            match &panel.pilot {
                Some(p) if p.record.steps.first().is_some_and(|r| r.measures.is_none()) => {
                    p.record.clone()
                }
                _ => return,
            }
        };
        let (Ok(before), Ok(after)) = (
            self.session_measures(&record, true),
            self.session_measures(&record, false),
        ) else {
            return;
        };
        let mut panel = self.focus.workshops.borrow_mut();
        if let Some(first) = panel
            .pilot
            .as_mut()
            .and_then(|p| p.record.steps.first_mut())
        {
            first.measures = Some(Measures { before, after });
        }
        panel.queue_write();
    }

    /// Records the value steps made on the session's node since the last
    /// call: a new step (a set, or `hint`'s kind answering a proposal), an
    /// undo or a redo. Called each frame and right after a command that
    /// set values. Nothing without a session.
    pub(super) fn follow_session(&mut self, hint: Option<(Kind, u32)>) {
        let (node, top, rev) = {
            let panel = self.focus.workshops.borrow();
            let Some(p) = &panel.pilot else {
                return;
            };
            (p.record.node.clone(), p.top, p.rev)
        };
        self.measure_first_step();
        let prefix = format!("node/{node}/");
        let (step, kind, now_top, now_rev, origin) = {
            let store = lock(&self.core.values);
            let now_rev = store.history_rev();
            if now_rev == rev {
                return;
            }
            let now_top = store.undo_steps().last().map(|s| s.id);
            let found = if store.last_replayed() {
                match store.redo_steps().last() {
                    // The step on top went to the redo stack: undone.
                    Some(s) if Some(s.id) == top => {
                        let mut s = s.clone();
                        for c in &mut s.changes {
                            std::mem::swap(&mut c.1, &mut c.2);
                        }
                        Some((s, Kind::Undo))
                    }
                    _ => store
                        .undo_steps()
                        .last()
                        .filter(|s| Some(s.id) != top)
                        .map(|s| (s.clone(), Kind::Redo)),
                }
            } else {
                store
                    .undo_steps()
                    .last()
                    .filter(|s| Some(s.id) != top)
                    .map(|s| (s.clone(), Kind::Set))
            };
            let origin = store.last_origin().cloned().unwrap_or(Origin::User);
            match found {
                Some((s, k)) => (Some(s), k, now_top, now_rev, origin),
                None => (None, Kind::Set, now_top, now_rev, origin),
            }
        };
        {
            let mut panel = self.focus.workshops.borrow_mut();
            if let Some(p) = panel.pilot.as_mut() {
                p.top = now_top;
                p.rev = now_rev;
            }
        }
        let Some(step) = step else {
            return;
        };
        let changes: Vec<Change> = step
            .changes
            .iter()
            .filter_map(|(path, old, new)| {
                let param = path.strip_prefix(&prefix)?;
                Some(Change {
                    param: param.into(),
                    old: plain(old),
                    new: plain(new),
                })
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        let record = self
            .focus
            .workshops
            .borrow()
            .pilot
            .as_ref()
            .map(|p| p.record.clone());
        let Some(record) = record else {
            return;
        };
        // A user's move of a value an open proposal holds overrides it.
        let (kind, of) = match (kind, hint) {
            (Kind::Set, Some((k, of))) => (k, Some(of)),
            (Kind::Set, None) if origin == Origin::User => record
                .open_proposals()
                .into_iter()
                .find(|(_, p, _)| changes.iter().any(|c| c.param == *p))
                .map_or((Kind::Set, None), |(seq, _, _)| (Kind::Override, Some(seq))),
            (k, _) => (k, None),
        };
        let author = match kind {
            Kind::Undo | Kind::Redo => who(&origin),
            _ => who(&step.origin),
        };
        let after = self.session_measures(&record, false).ok();
        let before = record.last_measures().cloned();
        let mut panel = self.focus.workshops.borrow_mut();
        let Some(p) = panel.pilot.as_mut() else {
            return;
        };
        p.record.push(Record {
            aim: step.aim.clone(),
            changes,
            of,
            store_step: Some(step.id),
            measures: after.map(|after| Measures {
                before: before.unwrap_or_default(),
                after,
            }),
            ..Record::new(&author, kind)
        });
        panel.queue_write();
    }

    /// `workshop.session`: the session's record (`read`, the default),
    /// kept past its end (`keep`, the user's alone), or ended (`close`:
    /// dropped unless kept).
    pub(super) fn workshop_session(
        &mut self,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        self.follow_session(None);
        match args["action"].as_str().unwrap_or("read") {
            "read" => {
                let panel = self.focus.workshops.borrow();
                let p = panel.pilot.as_ref().ok_or(NO_SESSION)?;
                Ok(json!({ "record": p.record, "net": net_of(&p.record) }))
            }
            "keep" => {
                if *origin != Origin::User {
                    return Err(
                        "only the user keeps a session: ask them to press Keep in the Workshops panel"
                            .into(),
                    );
                }
                let mut panel = self.focus.workshops.borrow_mut();
                let p = panel.pilot.as_mut().ok_or(NO_SESSION)?;
                p.record.kept = true;
                let id = p.record.session.clone();
                panel.queue_write();
                Ok(json!({ "session": id, "kept": true }))
            }
            "close" => {
                let record = self.end_session().ok_or(NO_SESSION)?;
                Ok(json!({
                    "session": record.session,
                    "kept": record.kept,
                    "dropped": !record.kept,
                }))
            }
            other => Err(format!("`action`: read, keep or close, not `{other}`")),
        }
    }

    /// The vault writes of the session records since the last call, by
    /// session id (`None`: drop it): the host keeps them in
    /// `workshops/sessions`.
    pub fn take_session_writes(&mut self) -> Vec<(String, Option<Value>)> {
        std::mem::take(&mut self.focus.workshops.borrow_mut().session_writes)
    }

    /// What the versions saved now carry: the session's id, while one is
    /// recorded.
    pub fn session_meta(&self) -> Option<Value> {
        let panel = self.focus.workshops.borrow();
        let p = panel.pilot.as_ref()?;
        Some(json!({ "workshop_session": p.record.session }))
    }

    /// The id of the session recorded, if one is.
    pub fn session_id(&self) -> Option<String> {
        let panel = self.focus.workshops.borrow();
        panel.pilot.as_ref().map(|p| p.record.session.clone())
    }

    /// The vault image the edit is saved to, as the host knows it: a
    /// session names its test images by it.
    pub fn set_edited_image(&mut self, image: Option<String>) {
        self.focus.workshops.borrow_mut().image = image;
    }
}

impl Shell {
    /// `workshop.propose`: an AI's `values` for the session's workshop
    /// (by exposed name) with the words it answers (`aim`), recorded as a
    /// `propose` step and shown as ghost values: nothing is set until the
    /// user accepts one.
    pub(super) fn propose_values(
        &mut self,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        self.follow_session(None);
        let (node, session) = {
            let panel = self.focus.workshops.borrow();
            let p = panel.pilot.as_ref().ok_or(NO_SESSION)?;
            (p.record.node.clone(), p.record.session.clone())
        };
        if let Some(asked) = args["session"].as_str().filter(|s| !s.is_empty())
            && asked != session
        {
            return Err(format!(
                "no session `{asked}`: the session going on is `{session}`"
            ));
        }
        let values = args["values"]
            .as_object()
            .filter(|v| !v.is_empty())
            .ok_or("`values`: an object, {exposed name: value}")?;
        let mut proposal = std::collections::BTreeMap::new();
        {
            let store = lock(&self.core.values);
            for (param, value) in values {
                let path = format!("node/{node}/{param}");
                let entry = store
                    .get(&path)
                    .ok_or_else(|| format!("`{param}`: the workshop has no such slider"))?;
                let v = value
                    .as_f64()
                    .ok_or_else(|| format!("`{param}`: a number"))?;
                let d = &entry.descriptor;
                if let Some((lo, hi)) = d.soft.or(d.hard)
                    && !(lo..=hi).contains(&v)
                {
                    return Err(format!("`{param}`: {v} is outside {lo} to {hi}"));
                }
                proposal.insert(param.clone(), json!(v));
            }
        }
        let aim = args["aim"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let mut panel = self.focus.workshops.borrow_mut();
        let p = panel.pilot.as_mut().ok_or(NO_SESSION)?;
        let seq = p.record.push(Record {
            aim,
            proposal: proposal.clone(),
            ..Record::new(&who(origin), Kind::Propose)
        });
        panel.queue_write();
        Ok(json!({ "session": session, "proposal": seq, "ghosts": proposal }))
    }

    /// `workshop.answer`: the user's answer to a proposed value (`of`, the
    /// proposal's step; `param`): `accept` sets it, `override` sets the
    /// user's `value` instead, each one undo step; `reject` sets nothing.
    /// Each is a step of the record. The user's alone.
    pub(super) fn answer_proposal(
        &mut self,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        if *origin != Origin::User {
            return Err(
                "only the user answers a proposal: it is shown to them as ghost values".into(),
            );
        }
        self.follow_session(None);
        let of = args["of"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("`of`: the proposal's step")?;
        let param = args["param"]
            .as_str()
            .ok_or("`param`: the proposed slider")?
            .to_string();
        let action = args["action"].as_str().unwrap_or("accept");
        let (node, proposed, aim) = {
            let panel = self.focus.workshops.borrow();
            let p = panel.pilot.as_ref().ok_or(NO_SESSION)?;
            let (_, _, value) = p
                .record
                .open_proposals()
                .into_iter()
                .find(|(seq, name, _)| *seq == of && *name == param)
                .ok_or_else(|| format!("no open proposal of `{param}` at step {of}"))?;
            let aim = p.record.step(of).and_then(|r| r.aim.clone());
            (p.record.node.clone(), value, aim)
        };
        let (kind, value) = match action {
            "accept" => (Kind::Accept, proposed.as_f64().ok_or("not a number")?),
            "override" => (
                Kind::Override,
                args["value"]
                    .as_f64()
                    .ok_or("`value`: the user's own number")?,
            ),
            "reject" => {
                let mut panel = self.focus.workshops.borrow_mut();
                let p = panel.pilot.as_mut().ok_or(NO_SESSION)?;
                let seq = p.record.push(Record {
                    aim,
                    of: Some(of),
                    proposal: std::collections::BTreeMap::from([(param.clone(), proposed)]),
                    ..Record::new(&who(origin), Kind::Reject)
                });
                panel.queue_write();
                return Ok(json!({ "step": seq, "kind": "reject", "param": param }));
            }
            other => {
                return Err(format!(
                    "`action`: accept, override or reject, not `{other}`"
                ));
            }
        };
        let path = format!("node/{node}/{param}");
        {
            let mut store = lock(&self.core.values);
            let label = format!("{action} {param}");
            let group = store.begin_group(origin.clone(), &label);
            if let Some(aim) = &aim {
                store.aim(group, aim);
            }
            let set = store
                .set(
                    helios_values::Set::new(
                        &path,
                        helios_values::Change::To(helios_core::Value::Float(value)),
                        origin.clone(),
                    )
                    .gesture(group),
                )
                .map_err(|e| format!("{path}: {e}"));
            store.end(group);
            set?;
        }
        self.follow_session(Some((kind, of)));
        let seq = self
            .focus
            .workshops
            .borrow()
            .pilot
            .as_ref()
            .and_then(|p| p.record.steps.last().map(|r| r.seq));
        Ok(json!({ "step": seq, "kind": action, "param": param, "value": value }))
    }
}

pub(super) const NO_SESSION: &str = "no workshop session: design or apply a workshop first";

/// A value as plain JSON: a number, a boolean, a string or a list.
pub(super) fn plain(v: &helios_core::Value) -> Value {
    use helios_core::Value as V;
    match v {
        V::Float(f) => json!(f),
        V::Int(i) => json!(i),
        V::Bool(b) => json!(b),
        V::Choice(s) | V::Text(s) => json!(s),
        V::List(l) => json!(l),
        V::Ints(l) => json!(l),
    }
}

/// The net outcome per parameter, as an object.
fn net_of(record: &Session) -> Value {
    let net: serde_json::Map<String, Value> = record
        .net()
        .into_iter()
        .map(|(p, (v, seq))| (p, json!({ "value": v, "step": seq })))
        .collect();
    Value::Object(net)
}

/// A stamp telling this run's sessions from former runs': seconds since
/// the epoch, in hexadecimal.
fn session_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("{secs:x}")
}
