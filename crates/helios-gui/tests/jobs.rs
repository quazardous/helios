//! Jobs on the desktop: an export reports on the shell's jobs, shows in
//! egui-pop's jobs centre with its progress, and is cancelled from it;
//! a long export's end is said on the desktop.

use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::Duration;

use egui::vec2;
use egui_pop::api::jobs::{self, JOBS};
use egui_pop::model::jobs::{JobState, Request};
use helios_core::{Image, Region, Scheduler};
use helios_export::{Format, submit_reported};
use helios_gui::platform::Job;
use helios_gui::{Shell, headless, helios_panels};

const LONG: Duration = Duration::from_secs(10);

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("helios-gui-jobs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn image() -> Image {
    Image::from_fn(Region::sized(8, 4), 4, |x, _, _| x as f32 / 8.0)
}

#[test]
fn an_export_shows_in_the_jobs_centre_and_is_cancelled_from_it() {
    let d = dir("centre");
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    let shell_jobs = h.state().jobs();
    let scheduler = Scheduler::new(shell_jobs.bus().clone(), 2);
    let app = h.state().app().handle().clone();

    // An export that ends: its steps, then finished, whole.
    let picture = image();
    submit_reported(
        &scheduler,
        &shell_jobs,
        None,
        &d.join("a.png"),
        Format::Png { bits: 8 },
        move || Ok(picture),
    )
    .unwrap();
    assert!(scheduler.wait_until_idle(LONG));
    h.run();
    let centre = app.service(&JOBS).expect("the jobs plugin");
    let done = centre
        .all()
        .into_iter()
        .find(|j| j.title == "Export a.png")
        .expect("the export is in the jobs centre");
    assert_eq!(done.kind, "export");
    assert_eq!(done.state, JobState::Done);
    assert_eq!(done.fraction, Some(1.0));
    // Its progress went through the journal: each step, the file.
    let progress: Vec<String> = app
        .events_since(0)
        .into_iter()
        .filter(|e| e.name == "job.progress" && e.data["id"] == done.id.as_str())
        .filter_map(|e| e.data["step"].as_str().map(str::to_string))
        .collect();
    assert_eq!(progress, ["render", "encode", "write"]);
    assert!(
        centre
            .log_from(&done.id, 0)
            .iter()
            .any(|l| l.contains("bytes written")),
        "its log console has the file"
    );

    // An export held in its render, cancelled from the jobs centre.
    let path = d.join("b.png");
    let (started_tx, started) = channel();
    let (release, release_rx) = channel::<()>();
    let picture = image();
    submit_reported(
        &scheduler,
        &shell_jobs,
        None,
        &path,
        Format::Png { bits: 8 },
        move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(picture)
        },
    )
    .unwrap();
    started.recv().unwrap();
    h.run();
    let running = centre
        .all()
        .into_iter()
        .find(|j| j.title == "Export b.png")
        .expect("the running export is in the jobs centre");
    assert!(running.state.active(), "{:?}", running.state);
    assert!(running.cancellable);
    assert_eq!(centre.summary().active, 1);
    // The jobs centre's Cancel: `job.cancel`, run in the desktop.
    jobs::ask(&app, &running.id, Request::Cancel);
    h.run();
    release.send(()).unwrap();
    assert!(scheduler.wait_until_idle(LONG));
    h.run();
    let cancelled = centre.job(&running.id).unwrap();
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert!(!path.exists(), "a cancelled export writes nothing");
    assert!(centre.idle());
    let _ = std::fs::remove_dir_all(&d);
}

/// A job's end is what the desktop notification says: its kind, how long
/// it took, its title. Every kind is said, the node's own jobs too; not
/// a job inside another, nor the node's background upkeep.
#[test]
fn a_job_s_end_feeds_the_desktop_notification() {
    use helios_core::jobs::Start;
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    let shell_jobs = h.state().jobs();
    let app = h.state().app().handle().clone();
    let mut ended = helios_gui::jobs::Ended::after(app.last_event());
    shell_jobs
        .start(Start::new("e9", "Export 12 images", "export"))
        .finished();
    shell_jobs
        .start(Start::new("c1", "Calibrate", "calibration"))
        .failed("no GPU");
    shell_jobs
        .start(Start::new("backup-1", "Back up to Disk A", "backup"))
        .finished();
    shell_jobs
        .start(Start::new("health-1", "Health check", "health"))
        .finished();
    // Not news: the image map's upkeep, an export inside a batch.
    shell_jobs
        .start(Start::new("map-1", "Image map", "map"))
        .finished();
    let batch = shell_jobs.start(Start::new("b1", "Export 2 photos", "batch").total(2));
    shell_jobs
        .start(Start::new("e10", "Export a", "export").parent("b1", 1.0))
        .finished();
    batch.finished();
    h.run();
    let ends = ended.take(&app);
    let jobs: Vec<(Job, Result<String, String>)> =
        ends.iter().map(|e| (e.job, e.outcome.clone())).collect();
    assert_eq!(
        jobs,
        vec![
            (Job::Export, Ok("Export 12 images".to_string())),
            (Job::Other, Err("Calibrate: no GPU".to_string())),
            (Job::Backup, Ok("Back up to Disk A".to_string())),
            (Job::Health, Ok("Health check".to_string())),
            (Job::Batch, Ok("Export 2 photos".to_string())),
        ]
    );
    assert!(ends.iter().all(|e| e.seconds >= 0.0));
    assert!(ended.take_backups(), "the backup's end is seen");
    assert!(ended.take(&app).is_empty(), "each end is taken once");
}
