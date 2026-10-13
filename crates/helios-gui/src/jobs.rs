//! Jobs on the desktop: what runs in the background, followed in
//! egui-pop's jobs centre.
//!
//! - Helios's background work (an export, a batch, the GPU calibration,
//!   an import) reports through [`helios_core::jobs`]: `job.*` events on
//!   the shell's bus, which egui-pop reads as its journal. The shell's
//!   [`Jobs`] is [`Shell::jobs`](crate::Shell::jobs); work started in the
//!   desktop's process reports there.
//! - egui-pop's [`JobsPlugin`] folds the events into its jobs model: the
//!   jobs centre (the Jobs panel and the status bar's popover, a bar,
//!   "n of m", the item, the time left, Cancel), the status bar's jobs
//!   item, and each job's log console (`JobConsolesPlugin`, a terminal
//!   over the job's `job.log` lines).
//! - Cancel runs `job.cancel` in the desktop's process ([`plugin`]):
//!   [`Jobs::cancel`], which stops the job the way it gave when it
//!   started. Pause, resume and retry are refused: no Helios job offers
//!   them yet.
//! - A long export or batch that ends while no Helios window has the
//!   focus says so on the desktop ([`Ended`], then
//!   `platform::job_finished`).
//! - A connected node's jobs come through a relay ([`crate::relay`]),
//!   ids `node:<name>/<id>`; the ones of a node lost for too long are
//!   taken out of the jobs centre ([`Forgotten`]).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use egui_pop::api::AppHandle;
use egui_pop::api::host::Seq;
use egui_pop::api::jobs::JobsPlugin;
use egui_pop::model::jobs::Request;
use helios_core::jobs::Jobs;
use serde_json::json;

use crate::platform::{Job, JobEnd};

/// egui-pop's jobs plugin over Helios's jobs: the job commands run in
/// this process, a cancel through `jobs`.
pub fn plugin(jobs: Jobs) -> JobsPlugin {
    JobsPlugin::default().run_locally(move |_, request, id| match request {
        Request::Cancel => jobs.cancel(id).map(|()| json!({ "cancelling": id })),
        Request::Pause | Request::Resume | Request::Retry => {
            Err(format!("job `{id}` cannot be paused, resumed or retried"))
        }
    })
}

/// The jobs' ends seen in the journal, for the desktop notification.
#[derive(Debug, Default)]
pub struct Ended {
    seen: Seq,
}

impl Ended {
    /// Starts after what the journal holds now.
    pub fn after(seq: Seq) -> Self {
        Self { seen: seq }
    }

    /// The exports and batches that ended since the last call: how, and
    /// how long they took. The title comes from the jobs model, else the
    /// job's id.
    pub fn take(&mut self, app: &AppHandle) -> Vec<JobEnd> {
        let mut ends = Vec::new();
        for e in app.events_since(self.seen) {
            self.seen = e.seq;
            let ok = match e.name.as_str() {
                helios_core::jobs::FINISHED => true,
                helios_core::jobs::FAILED => false,
                _ => continue,
            };
            let job = match e.data["kind"].as_str() {
                Some("export") => Job::Export,
                Some("batch") => Job::Batch,
                _ => continue,
            };
            let id = e.data["id"].as_str().unwrap_or_default();
            let title = app
                .service(&egui_pop::api::jobs::JOBS)
                .and_then(|j| j.job(id))
                .map_or_else(|| id.to_string(), |j| j.title);
            let outcome = if ok {
                Ok(title)
            } else {
                Err(e.data["error"]
                    .as_str()
                    .map_or_else(|| title.clone(), |why| format!("{title}: {why}")))
            };
            ends.push(JobEnd {
                job,
                outcome,
                seconds: e.data["seconds"].as_f64().unwrap_or(0.0),
            });
        }
        ends
    }
}

/// The nodes the desktop follows, by name.
pub type Relays = Rc<RefCell<BTreeMap<String, crate::relay::Relay>>>;

/// The node jobs a relay forgot (`node.jobs_forgotten` in the journal),
/// taken out of the jobs centre.
#[derive(Debug, Default)]
pub struct Forgotten {
    seen: Seq,
    /// Not yet ended in the jobs centre's model when asked: tried again
    /// next frame.
    pending: Vec<String>,
}

impl Forgotten {
    /// Starts after what the journal holds now.
    pub fn after(seq: Seq) -> Self {
        Self {
            seen: seq,
            pending: Vec::new(),
        }
    }

    /// Takes the jobs forgotten since the last call out of the jobs
    /// centre (with their children); how many went.
    pub fn take(&mut self, app: &AppHandle) -> usize {
        for e in app.events_since(self.seen) {
            self.seen = e.seq;
            if e.name == crate::relay::FORGOTTEN {
                self.pending.extend(
                    e.data["ids"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|id| id.as_str().map(str::to_string)),
                );
            }
        }
        let Some(centre) = app.service(&egui_pop::api::jobs::JOBS) else {
            self.pending.clear();
            return 0;
        };
        let mut gone = 0;
        self.pending.retain(|id| match centre.job(id) {
            None => false,
            Some(_) if centre.dismiss(id) => {
                gone += 1;
                false
            }
            // Its end is not folded yet.
            Some(_) => true,
        });
        gone
    }
}

/// What Helios says in the status bar, beside egui-pop's items (the jobs
/// item): keys pending, a recording, the last note. The shell sets it each
/// frame; the status bar block draws it.
#[derive(Clone, Debug, Default)]
pub struct StatusLine {
    pub recording: bool,
    pub note: Option<String>,
}

/// The status line, shared between the shell and its block.
pub type Status = Rc<RefCell<StatusLine>>;

/// Draws Helios's status line in egui-pop's status bar.
pub fn status_block(status: Status) -> impl FnMut(&mut egui::Ui, &AppHandle) + 'static {
    move |ui, app| {
        let line = status.borrow();
        // One line's height, whatever it says: the bar never changes
        // height (the viewport under it would move).
        ui.set_min_height(ui.spacing().interact_size.y);
        ui.horizontal_centered(|ui| {
            if let Some(keys) = app.pending_keys() {
                ui.label(format!("{keys} …"));
                ui.separator();
            }
            if line.recording {
                ui.colored_label(ui.visuals().error_fg_color, "● REC");
                ui.separator();
            }
            if let Some(note) = &line.note {
                ui.label(note);
            }
        });
    }
}
