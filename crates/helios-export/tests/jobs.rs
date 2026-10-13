//! Exports as scheduler jobs: writes to one file take turns, the file is
//! announced once written, and a failed export says why.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use helios_core::graph::SOURCE_PORT;
use helios_core::{Bus, Engine, Graph, JobState, Region, Scheduler};
use helios_export::{Format, Request, submit};
use helios_testkit::ramp;

const LONG: Duration = Duration::from_secs(10);

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("helios-jobs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Setup {
    bus: Arc<Bus>,
    scheduler: Scheduler,
    engine: Arc<Engine>,
    graph: Arc<Graph>,
    request: Request,
}

fn setup(path: PathBuf) -> Setup {
    let bus = Arc::new(Bus::new());
    let mut graph = Graph::new();
    let region = Region::sized(8, 4);
    let node = graph.add_source("src", ramp(region, 4));
    Setup {
        scheduler: Scheduler::new(bus.clone(), 4),
        engine: Arc::new(Engine::new(bus.clone())),
        graph: Arc::new(graph),
        request: Request {
            node,
            port: SOURCE_PORT.into(),
            region,
            path,
            format: None,
        },
        bus,
    }
}

#[test]
fn exports_to_one_file_take_turns_and_announce_the_file() {
    let d = dir("turns");
    let s = setup(d.join("a.png"));
    let other = Request {
        path: d.join("b.jpg"),
        ..s.request.clone()
    };
    let jobs: Vec<_> = [
        s.request.clone(),
        s.request.clone(),
        s.request.clone(),
        other,
    ]
    .into_iter()
    .map(|r| {
        submit(
            &s.scheduler,
            s.bus.clone(),
            s.engine.clone(),
            s.graph.clone(),
            r,
        )
        .unwrap()
    })
    .collect();
    assert!(s.scheduler.wait_until_idle(LONG));
    for job in &jobs {
        assert_eq!(s.scheduler.job(*job), Some(JobState::Done));
    }
    // Replay the journal: never two writers on a.png at once.
    let a: Vec<_> = jobs[..3].iter().map(|j| j.0).collect();
    let (mut running, mut written) = (0, Vec::new());
    for e in s.bus.since(0) {
        let job = e.payload["job"].as_u64();
        match e.kind {
            "scheduler.job_started" if a.contains(&job.unwrap()) => {
                running += 1;
                assert_eq!(running, 1, "two exports to a.png overlap");
            }
            "scheduler.job_ended" if a.contains(&job.unwrap()) => running -= 1,
            "export.written" => written.push(e.payload["path"].as_str().unwrap().to_string()),
            _ => {}
        }
    }
    assert_eq!(written.len(), 4);
    let ext = std::path::Path::new(&written[0])
        .extension()
        .and_then(|e| e.to_str());
    assert!(matches!(ext, Some("png" | "jpg")));
    let decoded = image::open(d.join("a.png")).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (8, 4));
    assert!(image::open(d.join("b.jpg")).is_ok());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_failed_export_says_why_and_an_unknown_format_is_refused_at_once() {
    let d = dir("fail");
    let s = setup(d.join("missing/a.tif"));
    let job = submit(
        &s.scheduler,
        s.bus.clone(),
        s.engine.clone(),
        s.graph.clone(),
        s.request.clone(),
    )
    .unwrap();
    assert!(s.scheduler.wait_until_idle(LONG));
    assert_eq!(s.scheduler.job(job), Some(JobState::Failed));
    let ended = s
        .bus
        .since(0)
        .into_iter()
        .find(|e| e.kind == "scheduler.job_ended")
        .unwrap();
    assert!(
        ended.payload["error"]
            .as_str()
            .unwrap()
            .contains("missing/a.tif")
    );
    // Said by key too, for an interface to translate.
    assert_eq!(ended.payload["message"]["key"], "export-write-failed");

    let webp = Request {
        path: d.join("a.webp"),
        ..s.request.clone()
    };
    let err = submit(
        &s.scheduler,
        s.bus.clone(),
        s.engine.clone(),
        s.graph.clone(),
        webp,
    )
    .unwrap_err();
    assert_eq!(err.key, "export-no-format");
    assert!(err.english.contains("no format"), "{err}");
    let forced = Request {
        path: d.join("a.webp"),
        format: Some(Format::Png { bits: 16 }),
        ..s.request.clone()
    };
    let job = submit(
        &s.scheduler,
        s.bus.clone(),
        s.engine.clone(),
        s.graph.clone(),
        forced,
    )
    .unwrap();
    assert!(s.scheduler.wait_until_idle(LONG));
    assert_eq!(
        s.scheduler.job(job),
        Some(JobState::Done),
        "an explicit format wins over the extension"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_export_cancelled_while_rendering_writes_nothing() {
    let d = dir("cancel");
    let path = d.join("a.png");
    let s = setup(path.clone());
    let (started_tx, started) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let image = ramp(Region::sized(8, 4), 4);
    let job = helios_export::submit_with(
        &s.scheduler,
        s.bus.clone(),
        &path,
        Format::Png { bits: 8 },
        move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(image)
        },
    )
    .unwrap();
    started.recv().unwrap();
    assert!(s.scheduler.cancel(job));
    release.send(()).unwrap();
    assert!(s.scheduler.wait_until_idle(LONG));
    assert_eq!(s.scheduler.job(job), Some(JobState::Cancelled));
    let left: Vec<_> = std::fs::read_dir(&d).unwrap().collect();
    assert!(
        left.is_empty(),
        "no file, not even a temporary one: {left:?}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// An export is a job people follow: started when queued, its steps, a
/// log line, finished; its id cancels it through the jobs.
#[test]
fn an_export_reports_its_job_and_is_cancelled_by_its_id() {
    use helios_core::jobs::Jobs;

    let d = dir("reported");
    let s = setup(d.join("a.png"));
    let jobs = Jobs::new(s.bus.clone());
    let image = ramp(Region::sized(8, 4), 4);
    helios_export::submit_reported(
        &s.scheduler,
        &jobs,
        &d.join("a.png"),
        Format::Png { bits: 8 },
        move || Ok(image),
    )
    .unwrap();
    assert!(s.scheduler.wait_until_idle(LONG));
    let events: Vec<_> = s
        .bus
        .since(0)
        .into_iter()
        .filter(|e| e.kind.starts_with("job."))
        .collect();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind).collect();
    assert_eq!(kinds.first(), Some(&"job.started"));
    assert_eq!(kinds.last(), Some(&"job.finished"));
    assert!(kinds.contains(&"job.log"), "{kinds:?}");
    let started = &events[0].payload;
    assert_eq!(started["kind"], "export");
    assert_eq!(started["title"], "Export a.png");
    assert_eq!(started["steps"].as_array().unwrap().len(), 3);
    let steps: Vec<&str> = events
        .iter()
        .filter_map(|e| e.payload["step"].as_str())
        .collect();
    assert_eq!(steps, ["render", "encode", "write"]);

    // Cancelled by its id while it renders: nothing is written.
    let path = d.join("b.png");
    let (started_tx, started) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let image = ramp(Region::sized(8, 4), 4);
    let seen = s.bus.last_seq();
    helios_export::submit_reported(
        &s.scheduler,
        &jobs,
        &path,
        Format::Png { bits: 8 },
        move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(image)
        },
    )
    .unwrap();
    started.recv().unwrap();
    let id = jobs
        .cancellable()
        .pop()
        .expect("the export can be cancelled");
    jobs.cancel(&id).unwrap();
    release.send(()).unwrap();
    assert!(s.scheduler.wait_until_idle(LONG));
    assert!(!path.exists());
    let kinds: Vec<&str> = s
        .bus
        .since(seen)
        .iter()
        .filter(|e| e.kind.starts_with("job."))
        .map(|e| e.kind)
        .collect();
    assert!(kinds.contains(&"job.cancelling"), "{kinds:?}");
    assert_eq!(kinds.last(), Some(&"job.cancelled"));
    let _ = std::fs::remove_dir_all(&d);
}

/// An export asked for as part of a batch says its parent, so the jobs
/// model folds it under the batch.
#[test]
fn an_export_in_a_batch_names_its_parent() {
    use helios_core::jobs::Jobs;

    let d = dir("child");
    let s = setup(d.join("a.png"));
    let jobs = Jobs::new(s.bus.clone());
    let image = ramp(Region::sized(8, 4), 4);
    helios_export::submit_child(
        &s.scheduler,
        &jobs,
        "batch-1",
        &d.join("a.png"),
        Format::Png { bits: 8 },
        move || Ok(image),
    )
    .unwrap();
    assert!(s.scheduler.wait_until_idle(LONG));
    let started = s
        .bus
        .since(0)
        .into_iter()
        .find(|e| e.kind == "job.started")
        .unwrap()
        .payload;
    assert_eq!(started["parent"], "batch-1");
    assert_eq!(started["weight"], 1.0);
    assert!(d.join("a.png").exists());
    let _ = std::fs::remove_dir_all(&d);
}
