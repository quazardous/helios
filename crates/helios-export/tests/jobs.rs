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
    assert!(err.contains("no format"), "{err}");
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
