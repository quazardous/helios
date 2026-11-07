//! A batch export from its request: a parent job and an export per
//! image under it, the files written with the names, format, space and
//! size asked; an image that fails is listed with its reason and fails
//! the batch; cancelling the batch cancels its exports.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use helios_core::jobs::Jobs;
use helios_core::{Bus, Region, Scheduler};
use helios_export::batch::{self, BatchRequest, Clash, Item, Line, Settings, Summary};
use helios_export::{Format, Output, OutputSpace};
use helios_testkit::ramp;
use serde_json::json;

const LONG: Duration = Duration::from_secs(20);

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("helios-batch-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn item(name: &str, out: &Path) -> Item {
    Item {
        image: format!("{name:0>64}"),
        version: None,
        source: PathBuf::from(format!("/photos/{name}.png")),
        output: out.to_path_buf(),
        skip: None,
        error: None,
    }
}

fn request(folder: &Path, output: Output, items: Vec<Item>) -> BatchRequest {
    BatchRequest::new(
        Some("grade".into()),
        Settings {
            output,
            folder: folder.to_path_buf(),
            pattern: "{stem}".into(),
            clash: Clash::Number,
        },
        items,
    )
}

/// What the batch said, up to its end.
fn lines(said: &Receiver<Line>) -> (Vec<Line>, Summary) {
    let mut seen = Vec::new();
    loop {
        match said.recv_timeout(LONG).expect("the batch ends") {
            Line::Ended(summary) => return (seen, summary),
            line @ Line::Failed { .. } => seen.push(line),
        }
    }
}

#[test]
fn a_batch_writes_its_files_and_lists_the_image_that_failed() {
    let d = dir("write");
    let bus = Arc::new(Bus::new());
    let (scheduler, jobs) = (
        Scheduler::new(Arc::new(Bus::new()), 2),
        Jobs::new(bus.clone()),
    );
    let mut skipped = item("c", &d.join("c.jpg"));
    skipped.skip = Some("its file exists".into());
    let items = vec![
        item("a", &d.join("a.jpg")),
        item("broken", &d.join("broken.jpg")),
        skipped,
        item("b", &d.join("b-2.jpg")),
    ];
    let output = Output {
        format: Format::Jpeg { quality: 85 },
        space: OutputSpace::DisplayP3,
        long_edge: Some(16),
    };
    let request = request(&d, output, items);
    // The request goes through JSON, as it would to a node.
    let request = BatchRequest::from_json(&request.to_json()).unwrap();
    let render: batch::Render = Arc::new(|item: &Item| {
        if item.name() == "broken.png" {
            return Err("cannot load the image: truncated".into());
        }
        Ok(ramp(Region::sized(64, 32), 4))
    });
    let (tx, said) = channel();
    let tx = std::sync::Mutex::new(tx);
    let actions =
        json!([{ "title": "Open folder", "command": "library.openFolder", "args": { "path": d } }]);
    let parent = batch::run(
        &scheduler,
        &jobs,
        &request,
        render,
        move |l| {
            let _ = tx.lock().unwrap().send(l);
        },
        actions,
    )
    .unwrap();
    let (seen, summary) = lines(&said);
    assert!(scheduler.wait_until_idle(LONG));

    // The files, with the names decided when asked, scaled down to 16
    // pixels on their longer side.
    for name in ["a.jpg", "b-2.jpg"] {
        let decoded = image::open(d.join(name)).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (16, 8), "{name}");
        assert_eq!(
            image::ImageFormat::from_path(d.join(name)).unwrap(),
            image::ImageFormat::Jpeg
        );
    }
    assert!(!d.join("broken.jpg").exists());
    assert!(!d.join("c.jpg").exists());
    assert_eq!(summary.job, parent);
    assert_eq!(summary.written.len(), 2);
    assert_eq!(
        (summary.failed, summary.skipped, summary.cancelled),
        (1, 1, 0)
    );
    assert!(!summary.interrupted);
    match &seen[..] {
        [Line::Failed { item, error, .. }] => {
            assert_eq!(item, "broken.png");
            assert!(error.contains("truncated"), "{error}");
        }
        other => panic!("one failure said: {other:?}"),
    }

    // The jobs: the batch, then an export per image under it, naming the
    // node; the failure listed under the batch, which fails.
    let events: Vec<_> = bus
        .since(0)
        .into_iter()
        .filter(|e| e.kind.starts_with("job."))
        .collect();
    let started: Vec<_> = events.iter().filter(|e| e.kind == "job.started").collect();
    assert_eq!(started[0].payload["id"], parent.as_str());
    assert_eq!(started[0].payload["kind"], "batch");
    assert_eq!(started[0].payload["title"], "Export 4 photos");
    assert_eq!(started[0].payload["total"], 4);
    assert_eq!(started.len(), 4, "the batch and its three exports");
    for child in &started[1..] {
        assert_eq!(child.payload["parent"], parent.as_str());
        assert_eq!(child.payload["node"], "grade");
    }
    let failed_item = events
        .iter()
        .find(|e| e.kind == "job.item_failed" && e.payload["id"] == parent.as_str())
        .expect("the failure is listed under the batch");
    assert_eq!(failed_item.payload["item"], "broken.png");
    let end = events
        .iter()
        .rev()
        .find(|e| e.payload["id"] == parent.as_str())
        .unwrap();
    assert_eq!(end.kind, "job.failed");
    assert_eq!(end.payload["error"], "1 of 4 photos failed");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_finished_batch_offers_its_actions_and_writes_each_format() {
    let d = dir("formats");
    let bus = Arc::new(Bus::new());
    let (scheduler, jobs) = (
        Scheduler::new(Arc::new(Bus::new()), 2),
        Jobs::new(bus.clone()),
    );
    for (format, name, expected) in [
        (Format::Png { bits: 16 }, "a.png", image::ImageFormat::Png),
        (Format::Tiff, "a.tif", image::ImageFormat::Tiff),
        (
            Format::Jpeg { quality: 90 },
            "a.jpg",
            image::ImageFormat::Jpeg,
        ),
    ] {
        let request = request(&d, format.into(), vec![item("a", &d.join(name))]);
        let render: batch::Render = Arc::new(|_: &Item| Ok(ramp(Region::sized(8, 4), 4)));
        let (tx, said) = channel();
        let tx = std::sync::Mutex::new(tx);
        let parent = batch::run(
            &scheduler,
            &jobs,
            &request,
            render,
            move |l| {
                let _ = tx.lock().unwrap().send(l);
            },
            json!([{ "title": "Open folder", "command": "library.openFolder", "args": {} }]),
        )
        .unwrap();
        let (_, summary) = lines(&said);
        assert_eq!(summary.written, [d.join(name)]);
        assert_eq!(
            image::ImageFormat::from_path(d.join(name)).unwrap(),
            expected
        );
        let decoded = image::open(d.join(name)).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (8, 4),
            "never enlarged"
        );
        let end = bus
            .since(0)
            .into_iter()
            .rev()
            .find(|e| e.payload["id"] == parent.as_str())
            .unwrap();
        assert_eq!(end.kind, "job.finished");
        assert_eq!(end.payload["actions"][0]["command"], "library.openFolder");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn cancelling_a_batch_cancels_its_exports_and_writes_nothing() {
    let d = dir("cancel");
    let bus = Arc::new(Bus::new());
    // One worker: one export renders, the others wait in the queue.
    let (scheduler, jobs) = (
        Scheduler::new(Arc::new(Bus::new()), 1),
        Jobs::new(bus.clone()),
    );
    let items: Vec<Item> = ["a", "b", "c"]
        .iter()
        .map(|n| item(n, &d.join(format!("{n}.png"))))
        .collect();
    let request = request(&d, Format::Png { bits: 8 }.into(), items);
    let (started_tx, started) = channel();
    let (release, release_rx) = channel::<()>();
    let (started_tx, release_rx) = (
        std::sync::Mutex::new(started_tx),
        Arc::new(std::sync::Mutex::new(release_rx)),
    );
    let render: batch::Render = Arc::new(move |_: &Item| {
        let _ = started_tx.lock().unwrap().send(());
        let _ = release_rx.lock().unwrap().recv_timeout(LONG);
        Ok(ramp(Region::sized(8, 4), 4))
    });
    let (tx, said) = channel();
    let tx = std::sync::Mutex::new(tx);
    let parent = batch::run(
        &scheduler,
        &jobs,
        &request,
        render,
        move |l| {
            let _ = tx.lock().unwrap().send(l);
        },
        json!([]),
    )
    .unwrap();
    started
        .recv_timeout(LONG)
        .expect("the first export renders");
    jobs.cancel(&parent).unwrap();
    release.send(()).unwrap();
    let (_, summary) = lines(&said);
    assert!(scheduler.wait_until_idle(LONG));
    assert!(summary.interrupted);
    assert_eq!(summary.cancelled, 3);
    assert!(summary.written.is_empty());
    let left: Vec<_> = std::fs::read_dir(&d).unwrap().collect();
    assert!(left.is_empty(), "nothing written: {left:?}");
    let ends: Vec<(String, &str)> = bus
        .since(0)
        .into_iter()
        .filter(|e| e.kind == "job.cancelled")
        .map(|e| (e.payload["id"].as_str().unwrap().to_string(), e.kind))
        .collect();
    assert_eq!(ends.len(), 4, "the three exports and the batch: {ends:?}");
    assert_eq!(ends.last().unwrap().0, parent);
    let _ = std::fs::remove_dir_all(&d);
}
