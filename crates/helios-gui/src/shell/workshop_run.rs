//! `workshop.run` (experimental, see `docs/design/workshops-on-the-fly.md`,
//! section 4): the workshop applied in the workspace run on a set of
//! photos. The shell names the run (the workshop node, its composite, the
//! workspace, the photos) and hands it to the host's runner
//! ([`crate::workshop_run::Runner`]), which holds the library: a dry run
//! answers at once, a run is a job and one library step.
//!
//! The runner asks the node, which may wait on a writer holding the
//! library: a dry run and a review the user asks (the Workshops panel,
//! the palette) run on a thread of their own, and the frame that picks
//! their answer up shows it. Others' asks wait for the answer.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use helios_shell::commands::Origin;
use helios_template::format::OnBreach;
use helios_vault::ImageId;
use serde_json::Value;

use super::Shell;
use crate::workshop_run::RunAsk;

/// The host's runner: a run asked, and whether it is a dry run.
pub type RunFn = Arc<dyn Fn(RunAsk, bool) -> Result<Value, String> + Send + Sync>;

/// The host's review of runs (`Runner::review`): the arguments, who asks,
/// and the workshop applied in the workspace (none when it is not).
pub type ReviewFn =
    Arc<dyn Fn(&Value, &str, Option<RunAsk>) -> Result<Value, String> + Send + Sync>;

/// A dry run or a review action running off the frame.
pub struct Waiting {
    /// A review's arguments; none: a dry run.
    review: Option<Value>,
    answer: Receiver<Result<Value, String>>,
}

/// Runs `work` on a thread of its own; its answer comes on the receiver.
fn spawn(
    work: impl FnOnce() -> Result<Value, String> + Send + 'static,
) -> Result<Receiver<Result<Value, String>>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("helios-workshop-ask".into())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .map_err(|e| e.to_string())?;
    Ok(rx)
}

impl Shell {
    /// Where `workshop.run` hands its runs (the host, once the library is
    /// open).
    pub fn set_workshop_runner(&mut self, runner: RunFn) {
        self.core.workshop_runner = Some(runner);
    }

    /// Where `workshop.review` reads and changes runs (the host's runner).
    pub fn set_workshop_reviewer(&mut self, reviewer: ReviewFn) {
        self.core.workshop_reviewer = Some(reviewer);
    }

    /// The run the Workshops panel reviews, if any.
    pub fn workshop_review(&self) -> Option<Value> {
        self.focus.workshops.borrow().review.clone()
    }

    /// The last dry run the user asked, once answered.
    pub fn workshop_plan(&self) -> Option<Value> {
        self.focus.workshops.borrow().plan.clone()
    }

    /// Whether a dry run or a review the user asked is still running.
    pub fn workshop_waiting(&self) -> bool {
        !self.core.workshop_waiting.is_empty()
    }

    /// Each frame: the dry runs' and reviews' answers come in, shown in
    /// the Workshops panel; a frame comes again while one runs.
    pub(super) fn workshop_frame(&mut self, ctx: &egui::Context) {
        let mut left = Vec::new();
        for waiting in std::mem::take(&mut self.core.workshop_waiting) {
            match waiting.answer.try_recv() {
                Err(TryRecvError::Empty) => left.push(waiting),
                Err(TryRecvError::Disconnected) => {
                    self.warn("workshop: the call ended without an answer".to_string());
                }
                Ok(Err(e)) => self.warn(format!("workshop: {e}")),
                Ok(Ok(answer)) => match &waiting.review {
                    Some(args) => self.show_review(args, &answer),
                    None => self.focus.workshops.borrow_mut().plan = Some(answer),
                },
            }
        }
        self.core.workshop_waiting = left;
        if !self.core.workshop_waiting.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// A review's answer to `args`, shown in the Workshops panel.
    fn show_review(&self, args: &Value, answer: &Value) {
        let mut panel = self.focus.workshops.borrow_mut();
        panel.last_run = answer["run"].as_str().map(String::from);
        panel.review = if args["action"] == "rerun" {
            None
        } else {
            Some(answer.clone())
        };
    }

    /// The workshop applied as `node` (the selected one by default) as a
    /// run of it on no photo yet.
    fn workshop_ask(&self, node: Option<&str>, origin: &Origin) -> Result<RunAsk, String> {
        let node = match node.filter(|s| !s.is_empty()) {
            Some(n) => n.to_string(),
            None => crate::modules::selected(&super::lock(&self.core.values))
                .ok_or("no workshop is selected")?,
        };
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let (r, file) = self.applied_workshop(&state, &node)?;
        Ok(RunAsk {
            node,
            workshop: r.to_string(),
            name: file.name.clone(),
            catalogue: state.bundle.catalogue(&self.workspace_catalogue()),
            bundle: state.bundle.clone(),
            images: Vec::new(),
            on_breach: None,
            who: crate::persist::who(origin),
        })
    }

    /// `workshop.review`: the run (`run`, the last one started here by
    /// default) listed flagged photos first, or an action on one of its
    /// photos (`action`: accept, revert, fix, decide, rerun); the Workshops
    /// panel shows the answer.
    pub(super) fn review_workshop(
        &mut self,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        let reviewer = self
            .core
            .workshop_reviewer
            .clone()
            .ok_or("no library is open: a run's review reads it")?;
        let mut args = args.clone();
        if args["run"].as_str().is_none_or(str::is_empty) {
            let last = self.focus.workshops.borrow().last_run.clone();
            args["run"] = Value::from(last.ok_or("no run to review: name its `run`")?);
        }
        let node = args["node"].as_str().map(String::from);
        let ask = self.workshop_ask(node.as_deref(), origin).ok();
        let who = crate::persist::who(origin);
        if *origin == Origin::User {
            let asked = args.clone();
            let answer = spawn(move || reviewer(&asked, &who, ask))?;
            self.core.workshop_waiting.push(Waiting {
                review: Some(args.clone()),
                answer,
            });
            return Ok(serde_json::json!({ "run": args["run"], "pending": true }));
        }
        let answer = reviewer(&args, &who, ask)?;
        self.show_review(&args, &answer);
        Ok(answer)
    }

    /// `workshop.run`: the applied workshop `node` (the selected one by
    /// default) on `images`, else the Library's selection (`set`:
    /// `selection`) or the photos in view (`set`: `view`). Only the user
    /// starts a run; anyone may ask a dry run.
    pub(super) fn run_workshop(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        let dry = args["dry_run"].as_bool().unwrap_or(false);
        if !dry && *origin != Origin::User {
            return Err(
                "only the user starts a workshop run: ask a dry run, the user runs it from the Workshops panel or the palette"
                    .into(),
            );
        }
        let runner = self
            .core
            .workshop_runner
            .clone()
            .ok_or("no library is open: a run takes its photos from one")?;
        let mut ask = self.workshop_ask(args["node"].as_str(), origin)?;
        let ids: Vec<String> = match &args["images"] {
            Value::Array(a) => a
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .ok_or_else(|| "`images`: image ids".to_string())
                })
                .collect::<Result<_, _>>()?,
            Value::Null => match args["set"].as_str().unwrap_or("selection") {
                "selection" => self.focus.library.borrow().selection(),
                "view" => self.focus.library.borrow().ids(),
                other => {
                    return Err(format!("`set`: `{other}` is not a set (selection, view)"));
                }
            },
            _ => return Err("`images`: a list of image ids".into()),
        };
        if ids.is_empty() {
            return Err("no photo to run on: select photos in the Library".into());
        }
        let images = ids
            .iter()
            .map(|i| ImageId::from_hex(i).map_err(|e| format!("`{i}`: {e}")))
            .collect::<Result<Vec<_>, _>>()?;
        let on_breach = match args["on_breach"].as_str() {
            None => None,
            Some("flag") => Some(OnBreach::Flag),
            Some("hold") => Some(OnBreach::Hold),
            Some("stop") => Some(OnBreach::Stop),
            Some(other) => {
                return Err(format!("`on_breach`: `{other}` is not flag, hold or stop"));
            }
        };
        ask.images = images;
        ask.on_breach = on_breach;
        if dry && *origin == Origin::User {
            let answer = spawn(move || runner(ask, true))?;
            self.core.workshop_waiting.push(Waiting {
                review: None,
                answer,
            });
            return Ok(serde_json::json!({ "dry_run": true, "pending": true }));
        }
        let answer = runner(ask, dry)?;
        if !dry {
            self.focus.workshops.borrow_mut().last_run = answer["run"].as_str().map(String::from);
        }
        Ok(answer)
    }
}
