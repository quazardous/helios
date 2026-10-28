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
- **Its node.** A job that works for a node of the graph names it
  (`Start::node`, `node` in `job.started`): an export names the node it
  renders. The desktop's Queue panel lists the jobs of the selected node
  by it (see [gui.md](gui.md), "Queue panel").

## Events

| Event | Data |
|---|---|
| `job.started` | `id`, `title`, `kind`, `total`, `steps` (`[{id, title, weight}]`), `parent`, `weight`, `cancellable`, `node` |
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
  written, its end; cancelled queued or running, writing nothing. An
  export names the node it renders when it has one (a node's exports
  do).
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
  on the node's bus (`export-<n>`). A run of the workflow level (what
  `helios batch` submits, see [workflow-level.md](workflow-level.md)) is
  a job of kind `batch` (`run-<n>`, its total the number of images), each
  export a child of it (weight 1), ending finished, failed when an image
  failed, or cancelled. A batch job asked for by a client
  (`batch_start`, `batch-<n>`) works the same way, its exports asked one
  by one and the job ended by `batch_end`.
  `job.cancel` with an id (`helios control job-cancel ID`, the
  `helios_job_cancel` tool) cancels an export, or a batch with its
  exports, queued or running; a run's images not exported yet are
  cancelled, a client batch's next exports are refused, and
  `helios batch` exits as interrupted.
  A run lasts until its images are done or it is cancelled, whether its
  client stays or not. A client batch whose client goes without ending
  it does not stay running: asked for on a connection held open
  (`hold`), the node fails it ("abandoned: its client went away") when
  that connection closes; any client batch also fails once no export was
  asked for it, and none of its exports ran, for its idle timeout
  (`idle_timeout_s`, ten minutes by default). Its exports already asked
  for go on.

## A node's jobs on the desktop

The desktop follows a running node's jobs when told about it:
`helios-gui --node NAME=INSPECT[,CONTROL]` (repeatable; a `tcp://`
address takes the node's token from `--token-file FILE` or
`HELIOS_TOKEN`), or `Shell::follow_node` with a
`helios_gui::relay::NodeLink`. A relay (`helios_gui::relay`) runs on a
thread of its own, off the UI thread:

- **Catching up, then following.** It reads the node's jobs whole
  (`describe jobs`: the running ones with their progress so far, the
  last ones ended), then subscribes to the node's `job.*` events after
  that point (`subscribe`, pushed by the node).
- **Republishing.** Each event goes on the shell's bus with the job's
  id, and its parent's, as `node:<name>/<id>`: the jobs centre shows a
  node's batch as a parent with its exports as children. The bus's
  waker asks the window for one frame per burst of events.
- **Cancel.** Cancel on a `node:` job goes to the node's control
  endpoint (the debug one or the batch one) as `job.cancel`; a node
  given no control endpoint refuses it, and the jobs centre says why.
- **A dropped link.** No line from the node (not even its heartbeat,
  every second) for five seconds, or the connection closed: the node's
  running jobs are marked lost (paused, "lost: node NAME is out of
  reach" in their log), and the relay tries again every second. Back,
  it catches up: the jobs still running go on where they are, the ones
  that ended meanwhile end as they did (failed when the node no longer
  remembers how), the new ones start. A node that restarted leaves its
  old jobs failed. Still lost after a minute, the jobs fail and leave
  the jobs centre (`node.jobs_forgotten`). Each change of the link is
  `node.link` on the bus (`node`, `up`, `error`).
