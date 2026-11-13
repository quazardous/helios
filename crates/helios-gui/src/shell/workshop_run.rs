//! `workshop.run` (experimental, see `docs/design/workshops-on-the-fly.md`,
//! section 4): the workshop applied in the workspace run on a set of
//! photos. The shell names the run (the workshop node, its composite, the
//! workspace, the photos) and hands it to the host's runner
//! ([`crate::workshop_run::Runner`]), which holds the library: a dry run
//! answers at once, a run is a job and one library step.

use std::sync::Arc;

use helios_shell::commands::Origin;
use helios_template::format::OnBreach;
use helios_vault::ImageId;
use serde_json::Value;

use super::Shell;
use crate::workshop_run::RunAsk;

/// The host's runner: a run asked, and whether it is a dry run.
pub type RunFn = Arc<dyn Fn(RunAsk, bool) -> Result<Value, String> + Send + Sync>;

impl Shell {
    /// Where `workshop.run` hands its runs (the host, once the library is
    /// open).
    pub fn set_workshop_runner(&mut self, runner: RunFn) {
        self.core.workshop_runner = Some(runner);
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
        let node = match args["node"].as_str().filter(|s| !s.is_empty()) {
            Some(n) => n.to_string(),
            None => crate::modules::selected(&super::lock(&self.core.values))
                .ok_or("no workshop is selected")?,
        };
        let state = self.workspace_state().ok_or("no workspace is open")?;
        let (r, file) = self.applied_workshop(&state, &node)?;
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
        let ask = RunAsk {
            node,
            workshop: r.to_string(),
            name: file.name.clone(),
            catalogue: state.bundle.catalogue(&self.workspace_catalogue()),
            bundle: state.bundle.clone(),
            images,
            on_breach,
            who: crate::persist::who(origin),
        };
        runner(ask, dry)
    }
}
