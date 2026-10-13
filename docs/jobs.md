# Jobs people follow

Background work in Helios (an export, a batch, the GPU calibration, an
import) runs on the scheduler (`helios_core::scheduler`) and says how it
goes through `helios_core::jobs`: `job.*` events on a bus. On the
desktop the bus is the shell's, which egui-pop reads as its journal, so
every job shows in the jobs centre (see [gui.md](gui.md), "Jobs") with
its progress, its log console and Cancel. In a node the events go on the
node's bus like its other events.

## Reporting a job

```rust
use helios_core::jobs::{Jobs, Progress, Start};

fn import(jobs: &Jobs, files: &[std::path::PathBuf]) {
    let job = jobs.start(
        Start::new("import-1", &format!("Import {} photos", files.len()), "import")
            .total(files.len() as u64),
    );
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    job.on_cancel(move || flag.store(true, std::sync::atomic::Ordering::SeqCst));
    for (i, f) in files.iter().enumerate() {
        if stop.load(std::sync::atomic::Ordering::SeqCst) {
            job.cancelled();
            return;
        }
        let name = f.display().to_string();
        job.progress(Progress::new().counts(i as u64, files.len() as u64).item(&name));
        // ... the work; a file that fails does not stop the others:
        // job.item_failed(&name, "unreadable");
        job.log(format!("{name}: imported"));
    }
    job.finished();
}
```

- **Where the jobs come from.** On the desktop, `Shell::jobs()`; in a
  test, `Jobs::new(bus)` over the bus to watch. Clones share the jobs
  (and what cancels them).
- **Its id** names the job while it runs (`import-1`): unique among
  running jobs, and what `job.cancel` takes. Its kind (`export`, `batch`,
  `calibration`, `import`) groups jobs; the desktop notification says
  the end of a long `export` or `batch`.
- **Progress** sends only what is set: `counts(done, total)` (or
  `done(n)` when the total is not known yet), `item` (a file name),
  `item_progress` (0 to 1), `step` and `step_counts` for a job declared
  with steps (`Start::step(id, title, weight)`: an export is render 3,
  encode 1, write 1), `failed`, `skipped`, and `estimate` when the job
  knows its own fraction better than the counts. egui-pop works out the
  overall percentage and the time left from them.
- **Log lines** (`log`) go to the job's console; a failed item
  (`item_failed`) is listed under the job, which goes on.
- **Its end**, once: `finished` (or `finished_with` actions such as
  "Show in folder", `[{title, command, args}]`), `failed(why)` or
  `cancelled`. Later ends are ignored; a report dropped without an end
  says `failed`, so no job stays running in people's eyes. Each end
  carries the job's kind and how many seconds it took.
- **Cancel.** `on_cancel` gives the way to stop the job: a flag the work
  checks between items, or the scheduler's cancel of its job
  (`Scheduler::canceller`). `Jobs::cancel(id)` publishes
  `job.cancelling`, then runs it; the job then says `cancelled` itself.
  A job that cannot be stopped says so when it starts
  (`Start::not_cancellable`): no Cancel button.

## Events

| Event | Data |
|---|---|
| `job.started` | `id`, `title`, `kind`, `total`, `steps` (`[{id, title, weight}]`), `parent`, `weight`, `cancellable` |
| `job.progress` | `id`, `done`, `total`, `failed`, `skipped`, `item`, `item_progress`, `step`, `step_done`, `step_total`, `estimate` |
| `job.item_failed` | `id`, `item`, `error` |
| `job.log` | `id`, `text` |
| `job.cancelling` | `id` |
| `job.finished`, `job.failed`, `job.cancelled` | `id`, `kind`, `seconds`; `error` when failed; `actions` when finished with some |

They are egui-pop's jobs events (its `docs/progress.md`), so its jobs
model folds them as they are. A child job (`Start::parent(id, weight)`)
folds under its parent, its weight in the parent's estimate: a batch
whose images are jobs of their own.

## What reports today

- **Exports** (`helios_export::submit_reported`, and `submit_with` on its
  bus): started when queued, then its steps, a log line with the file
  written, its end; cancelled queued or running, writing nothing.
- **The GPU calibration** (`helios_compute::Manager::report_to`): each
  device's reference modules as items, its speeds as log lines, a device
  that failed as a failed item; cancelled between two modules.

- **The library import** (`helios_library::import::import`, the
  desktop's `library.import`, `helios import`): each file as an item, a
  log line per file brought in, a file that failed as a failed item;
  cancelled before its next file, a copy under way removed (see
  [library.md](library.md)).

`helios_core::jobs` is the only way a job reports: the older
`helios_core::job`, whose events named a job by a number, is gone.

- **A node's exports and batches** (`helios-node`): each export as a job
  on the node's bus (`export-<n>`); `helios batch` starts a batch job on
  the node it drives (`batch_start`, kind `batch`, its total the number of
  images), each export a child of it (weight 1), and ends it
  (`batch_end`): finished, failed when an image failed, or cancelled.
  `job.cancel` with an id (`helios control job-cancel ID`, the
  `helios_job_cancel` tool) cancels an export, or a batch with its
  exports, queued or running; the batch's next exports are refused and
  `helios batch` exits as interrupted.

Not yet: the desktop does not follow a node's bus, so work run in a node
(a batch, its exports) shows in the jobs centre only once the desktop
relays the node's `job.*` events and sends its Cancel there as
`job.cancel`. A batch whose `helios batch` dies without ending it stays
running on its node until the node stops.
