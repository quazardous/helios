# Log and alerts: one record, one sink, several views

Status: analysis (2026-11-15). Nothing here is built; the slices at the
end say what would be, in which order.

The owner's ask: are error toasts logged too? A central log and alert
component is missing: a toast is an alert, or a log line, like any other,
which Helios decides to show. Then, on the same thread: a notification
area in the status bar, on the left, as Windows has, and a central
system that modules use to manage the notifications they show.

This note builds on [hooks.md](hooks.md), which already places it:
posting, updating and withdrawing a notification are **commands** to the
log's owner, a new record is the **fact** `log.recorded`, and how a
record is shown is the **hook point** `log.present`. The bus is
[architecture.md](../architecture.md)'s ("Events: the central bus").

## In short

- Today the desktop reports through **eighteen paths**, and no two
  keep the same things (section 1). A toast's text is kept nowhere the
  user can find once it fades, unless the code that showed it also
  wrote the Log panel by hand; the Log panel keeps no time and no
  source; repeats are never grouped; the node's `log::` lines are
  dropped, because the `helios-node` binary installs no logger.
- **One record** (section 2): level, `Message`, source, node, cause (the
  bus's `cause`), time, details, actions, a de-duplication key and a
  count. **One sink**: a bounded journal in memory, the log file, and
  the node's records relayed. **Presentation is a policy**, decided
  per record: a toast, an entry in the notification area, a badge, a
  row in the window only, or the file only. Rule: **no toast without its
  record**, and the record is written first.
- **The notification area** (section 3) sits at the left end of the
  status bar: an icon per source with something pending, an unread
  count, a popover with each notification's actions, "Clear all", and a
  way to the window. A quiet bell stays when nothing is pending, so the
  window can always be opened (the lesson of the Activity indicator
  that vanished when idle). Modules post, update and withdraw
  notifications through commands; they never draw a toast or an icon.
- **The Log & Alerts window** (section 4) replaces the Log panel: a
  focus window, filters, search, repeats grouped, the cause chain, the
  actions, copy and export for a bug report.
- **Agents** (section 5) read the same journal: `log.list {since,
  level}` on the node's MCP and inspect endpoint, and on the desktop's
  debug bridge.
- **Survey** (section 6): stay on the `log` facade (no Helios crate uses
  `tracing`); keep egui-pop's own `Logs` file sink; **own** the journal,
  which no crate offers. The record and the journal go in
  **egui-pop-model** (UI-free, the node already depends on it), the
  views (toasts as a view, the area, the window's list) in
  **egui-pop-api**; Helios brings its sources, its policies, the node
  relay and the window's composition.
- **First slice:** every toast Helios shows goes through one function
  that writes its record first (section 7).

## 1. Inventory

Every way the desktop and the node report something to a person or a
reader, on `next` as of 2026-11-15. "Goes to" lists every place it
lands; "Lost" is what a person who looked away cannot get back.

| # | Path | Where (files) | Goes to | Lost |
|---|---|---|---|---|
| 1 | `Shell::notify`, `warn` (some 80 call sites) | `helios-gui/src/shell.rs` (`notify`, `notify_unlogged`, `notify_with`) | the log file in English (`log_panel::write`); the status line's last note (`notes`); `core.notices`; a toast for a warning or an error, none for information | **not in the Log panel**; information is only the status line's note, replaced by the next one; `notices` and `notes` grow without bound and are read by tests only; no source, no cause; four identical warnings make four toasts |
| 2 | `Shell::notify_failure` (error with a bug-report context) | `shell.rs`, `report.rs` | as row 1, plus a "Details" action on the toast that opens an error dialog with Copy | the details are reachable from the toast while it shows, then only for the **last** failure (`notice_details` with no index); older failures' reports are gone |
| 3 | Direct toasts, around `notify` | `shell/import_window.rs` (`card_toast`), `shell/library_imports.rs` (`offer_import_session`) | a toast and the status line's note | **not in the log file, not in the Log panel, not in `notices`**: once faded, "EOS_DIGITAL inserted" and "The import ended" leave no trace |
| 4 | Direct toasts that write the Log panel by hand | `shell/export.rs` (export ended), `shell/import.rs` (import ended), `shell/import_auto.rs` (`toast_with`), `shell/storage.rs` (card safe to format) | Log panel (so the file), a toast, mostly `notices` | each site repeats the same five steps by hand; the export's "Open folder" and the import's actions stay on the toast only, not on the Log panel's line |
| 5 | Snapshot restored | `shell.rs` (`follow_snapshots`) | a success toast with Undo, `notices`, the note | not in the log file nor the Log panel |
| 6 | Engine notices (GPU fallback, GPU lost) | `shell.rs` (`engine_notices`, viewport engine bus) | row 1 | de-duplicated by hand (`core.fell_back`, once per module per run) |
| 7 | egui-pop's own toasts | egui-pop `api/app.rs` (`workbench_refused`), `api/clipboard.rs`, `api/links.rs`, `platform/crash.rs` (crash report offered) | a toast, and egui-pop's journal (`helios.gui.toast.shown` on the shell bus, kept with the bus's recent events; `Toasts::history`, 32 entries) | not in Helios's log file or Log panel; plain text, so it never changes language |
| 8 | The Log panel | `helios-gui/src/log_panel.rs` (`Log`, `KEPT` = 1000, panel `helios.log`) | memory (1000 entries) and the log file | an entry keeps **no time, no source, no action, no details**; the panel holds only rows 4's reports (import outcomes, darktable edits not kept whole, exports, automatic imports, cards safe); emptied at quit |
| 9 | `log::warn!` / `log::error!` in desktop crates (some 180 lines across the workspace; most in `shell/import_accounts.rs`, `main.rs`, `import_scan.rs`, `shell/storage.rs`) | `helios-gui/src/platform.rs` (`file_logs`: egui-pop's `Logs` installed as the `log` logger) | the rotated log file (`~/.local/state/helios/logs/helios.log` on Linux) and its 200-line tail (quoted by a crash report) | never shown: "cannot open the browser", "picking from Google: the source could not be reached" are silent unless an inline error says it too |
| 10 | The node's `log::` lines | `helios-node-core/src/node.rs`, `health.rs`; `helios-storage`, `helios-library`, `helios-plugin` when they run in the node | **nowhere** until 2026-11-15: the `helios-node` binary installed no logger, so the `log` facade dropped them. Now its standard error, so the node's log file (row 11), as `helios_node::logger` writes it | as row 11 |
| 11 | The node's `eprintln!` | `helios-node/src/main.rs`; `connect.rs` sends a spawned node's stderr to `.helios/NAME.node.log` beside the vault (`sandbox.rs`: `node.log`) | a file per node | not rotated by Helios, not shown, not relayed |
| 12 | Node facts (`helios.job.failed`, `state.failed`, `command.failed`, storage and volume facts) | `helios-core/src/jobs.rs` and the node's bus; journal read with `events --since` | the node's bus journal (recent, in memory); the desktop sees `helios.job.*` through the jobs relay and promises through Activity | a node failure that is not a job reaches no person; the relay of other facts (`NodeEvents::listen`) is in progress |
| 13 | Activity | `helios-gui/src/activity.rs`, `shell/activity_focus.rs` | the status bar indicator (a warning badge until seen), the Activity focus (each job's log lines, `LogLine`, its Retry and result links) | a job's log is kept with the job only; the failure is not in the log file unless the job wrote it |
| 14 | Desktop notifications for long jobs | `helios-gui/src/platform.rs` (`job_finished`, `notifications.jobs`), egui-pop `platform/notify.rs` | the system's notification centre while no Helios window has the focus (else a toast) | no record; the job's end is in Activity only |
| 15 | Health findings | `helios-health`, `helios-gui/src/shell/health.rs` | Settings › Health and the status bar's badge (the worst finding and the count) | a finding that comes and clears while the user looks away is never seen; findings are not records |
| 16 | Error dialogs | `shell.rs` (`notice_details`, `open_dialog`), `report.rs` | a modal-less error dialog with Copy (details and system) | the dialog's text is the toast's; once closed, see row 2 |
| 17 | Inline errors | forms and windows: the import window, accounts admin, settings pages, the init wizard | a label in the window | gone with the window; some also write row 9's file, most do not |
| 18 | Status line notes | `shell.rs` (`notes: Vec<String>`, `status_bar`) | the status line shows the last one | strings, not messages: they keep the language they were made in |

What this shows:

- **Five ways of keeping**, each partial: `notes`, `notices`, the Log
  panel, the log file, egui-pop's toast history. None has everything a
  person needs later (time, source, action, details).
- **De-duplication is re-invented per site**: `core.fell_back` (row 6),
  `auto.paused_said` and `auto.asked` (`shell/import_auto.rs`), the
  Activity's "seen" failures. Where nobody did it, repeats stack: the
  four "rules must be a group" toasts the owner saw.
- **The node is mute**: its `log::` lines are dropped (row 10) and its
  failures reach the desktop only where a feature relays them.

## 2. The record and the sink

### The record

```rust
/// One thing worth reporting: a log line, an alert or a notification.
pub struct Record {
    /// Unique in its journal, increasing.
    pub id: RecordId,
    pub level: Level,             // Debug, Info, Success, Warning, Error
    /// What it says, as a catalogue key with its arguments and its
    /// English: it reads in the interface's language, even after the
    /// language changes (Helios's `Message`).
    pub message: Text,
    /// Who says it: a module, a plugin, a job, a window, the shell, a
    /// `log` target. Notifications group by it; mutes name it.
    pub source: Source,           // { kind, id }, "storage", "import.auto"
    /// The node it comes from; none for the desktop itself.
    pub node: Option<String>,
    /// What led to it: the bus event or command it answers, as the
    /// envelope's `cause` (`bus` + `seq`), so the window walks back.
    pub cause: Option<Cause>,
    /// Wall clock, milliseconds (the bus's `wall_ms`).
    pub at_ms: u64,
    /// Longer text and the bug-report context (Helios's
    /// `report::Context`: command, causes, plugin, job).
    pub details: Option<Details>,
    /// Retry, Open, Show in Library: a label and a command with its
    /// arguments, as toasts' actions are today.
    pub actions: Vec<Action>,
    /// De-duplication: records with the same key, not yet read, fold
    /// into one; `count` and `last_ms` grow.
    pub key: Option<String>,
    pub count: u32,
    pub last_ms: u64,
    /// Set when the record is also a notification (section 3).
    pub notice: Option<Notice>,
}

pub struct Notice {
    /// Stays in the area until withdrawn or cleared; else it goes once
    /// read, or after the transient lifetime.
    pub sticky: bool,
    /// Progress, 0 to 1, for a notification updated in place.
    pub progress: Option<f32>,
    pub state: NoticeState,       // Pending, Read, Withdrawn, Cleared
}
```

- **The key.** A poster may give one (`"import.rules.not-a-group"`,
  `"storage.card-safe:EOS_DIGITAL"`); without one the journal uses
  source + message key + level. A new record whose key matches an unread
  one updates it (count, `last_ms`, the newest arguments and actions)
  instead of adding a line; its toast, if one shows, says ×4 instead of
  stacking. A read record no longer folds: the next one is news. This
  replaces rows 6 and 13's hand-made sets.
- **The cause.** The bus envelope already has `cause` (the seq of the
  event that caused it). A record posted while handling an event or a
  command carries it, so the window shows "because: import #12 started
  by an agent → job failed → this". A relayed node record keeps the
  node's cause, prefixed by the node.
- **Levels.** `Success` is kept apart from `Info` because toasts already
  draw it apart. `Debug` exists so that a `log::debug!` line can become
  a record when the developer level asks for it.

### The one sink

A **journal** per process, UI-free and `Send + Sync`:

- **Memory, bounded.** A ring of records (proposed 5,000), with a floor
  per level so that a flood of information never pushes the last errors
  out (the last 500 warnings and errors are kept apart). The Log panel's
  `KEPT` = 1000 becomes this.
- **The log file.** Every record is written once, in English, through
  egui-pop's `Logs` (`platform/logs.rs`): level, target = source,
  message, and `node`, `key`, `cause` as `k=v` after it. A folded repeat
  writes one line per occurrence, so the file stays a faithful history.
- **The `log` facade feeds it.** The installed logger (egui-pop's
  router) hands each line at `warn` and above (or the developer level,
  if lower) to the journal as a record: source = the target, message =
  plain text (no catalogue key), no presentation beyond the window. So
  row 9's silent warnings show in the window, and stay out of toasts.
- **Node records relayed.** The node runs the same journal (and now a
  logger: row 10). It publishes `log.recorded` on its bus; the desktop's
  node-event relay (`NodeEvents::listen("log", ["log.*"])`) brings each
  record over with its `node` set and its cause prefixed, and the
  desktop's journal keeps it beside its own. Resume and resync come
  with the relay: after a reconnection the desktop asks
  `log.list {since}` for what it missed.
- **Facts.** The journal publishes `log.recorded` (id, level, source,
  node), `log.updated` (a fold, a notification's progress),
  `log.withdrawn` and `log.read` on the bus of the part it reports to:
  the shell bus on the desktop. The status bar's counts and the window
  follow them; tests wait on them.

### Presentation is a policy

How a record shows is decided once per record, at the hook point
`log.present` (hooks.md, section 5): its input is the record and the
context (which window has the focus, whether a focus window asked for
its toasts, zen mode, whether any Helios window has the system's
focus); a listener returns a decision or passes. The first decision
wins; the user's mutes run first, modules' rules after them, the
defaults last.

| Decision | What it draws |
|---|---|
| `Toast { lifetime }` | an egui-pop toast in the window that should hear it, carrying the record's id: its actions are the record's, ×N when folded, closing it marks the record read |
| `Area` | an entry in the notification area, under the record's source |
| `Badge(indicator)` | a count on an existing status item (Activity, Health) |
| `Window` | a row in the Log & Alerts window only |
| `File` | the log file only (not kept in memory) |
| `System` | the system's notification centre (egui-pop's `notify`), when no Helios window has the focus |

A decision may combine `Toast` or `System` with `Area`. Every decision
except `File` includes `Window`: the window shows every record kept.

Defaults (the last listener):

| Record | Default |
|---|---|
| `Debug`; `log` lines below `warn` | `File` |
| `log` facade lines at `warn` and above (no catalogue key) | `Window`, plus the window's unread count |
| `Info` | `Window`; `Area` when posted as a notification |
| `Success` of something the user started in this window | short `Toast` + `Window` |
| `Warning` | `Toast` + `Window`; `Area` when posted as a notification |
| `Error` | sticky `Toast` with Details + `Area` under its source |
| Any of the above while no Helios window has the focus | `System` for warnings, errors and long jobs' ends (the `notifications.jobs` setting), else as above |
| A folded repeat | no new toast: the shown one counts |
| A muted source | `Window` |

**No toast without its record.** egui-pop's toasts keep their API, but
Helios never calls them directly: the presentation step does, with the
record's id. A test walks every toast shown in a frame test and asserts
its record is in the journal (slice a).

## 3. The notification area and the module API

### The area

At the **left end** of the status bar, before egui-pop's breadcrumb, as
Windows puts its tray at the end the eye reaches first:

```text
┌──────────────────────────────────────────────────────────────────────────┐
│ 🔔 💾² 📥 ⚠1 │ Library › Darkroom [crop]          ⟳ Importing 120/412  1:1 │
└──────────────────────────────────────────────────────────────────────────┘
  │   │   │   └ an error count when any source has errors
  │   │   └ the import source: one notification, read
  │   └ storage: 2 unread (a card safe to format, a backup due)
  └ the bell: always there; its popover lists everything pending
```

- **One icon per source** with something pending, its icon declared by
  the source (`phosphor:hard-drives` for storage); an unread count as a
  badge; the error tone when one of them is an error. Icons appear in
  the order they first spoke, and at most five show; the others go
  under a `^` as Windows hides its overflow icons.
- **A click** opens the source's popover: its notifications, newest
  first, each with its time, text, progress, its actions as buttons and
  a close mark; "Clear all" for the source; "Mute this source" in its
  context menu (to `Window` only, undone in Settings › Notifications).
- **The bell** is drawn always, quiet when nothing is pending (the
  Activity indicator that disappeared when idle taught that a thing to
  open must stay). Its popover lists every source's pending
  notifications, "Clear all", and "Open Log & Alerts…".
- **Reading.** Opening a popover marks its notifications read; a
  transient one leaves the area once read (or after its lifetime, ten
  minutes by default), a sticky one stays until withdrawn or cleared.
  Cleared or withdrawn, the record stays in the journal and the window.
- **Built on egui-pop's strips.** The status bar's indicators
  (`egui_pop::api::strips`: count, errors, warnings, icon, popover,
  `Alignment::Start`) cover one icon with a popover. The area needs two
  additions in egui-pop: a place **before** the breadcrumb
  (`Alignment::Start` is after it today), and a **group** that draws a
  dynamic set of icons as one item (sources come and go). Both are
  generic: any egui-pop application with notifications wants them.
- The Activity indicator and the Health badge stay where they are: they
  show state, not news. A job that fails and a finding that needs
  attention also post a notification (from `activity` and `health`),
  withdrawn when the job is retried or the finding clears.

### The module API

hooks.md's vocabulary, applied:

| What | Channel | Name | Notes |
|---|---|---|---|
| Post a record (log line or notification) | **command** to the journal's owner | `log.post {level, message, source, key?, actions?, details?, notify?: {sticky, progress?}}` → `{id}` | one owner, an answer (the id). In process, the same call is a method on the journal's service handle (`Alerts::post(Record) -> RecordId`), so a module does not build JSON |
| Update in place | **command** | `log.update {id, message?, progress?, actions?, level?}` | progress of a long check, "3 cards waiting" becoming "2" |
| Withdraw | **command** | `log.withdraw {id}` or `{source, key}` | the card was removed, the finding cleared; the record stays, marked withdrawn |
| Mark read, clear | **command** | `log.read {id \| source}`, `log.clear {source?}` | what the popover and the window run |
| Mute | **command** + setting | `log.mute {source, on}`; `notifications.muted` | read by the mutes listener of `log.present` |
| A record arrived, changed, went | **fact** | `log.recorded`, `log.updated`, `log.withdrawn`, `log.read` | the area, the window, tests and agents follow them |
| How it shows | **hook point** | `log.present` | section 2; modules add rules (`after: ["user.mutes"]`), plugins through `[[hooks]]` |
| A hook listener failed | **fact** | `hook.failed` | the journal records it (source = the listener's owner), so a failing module is seen once |

- **Sources are declared** with their icon and label (a typed contract,
  next to the module's commands), so the area and the Settings page can
  name a source before it ever spoke; a plugin declares its own in its
  manifest.
- **Plugins** post through the same commands over their RPC; their
  source is their plugin id, and they cannot post as another source.
- **The node** offers the same commands (`log.post`, `log.withdraw`):
  a job running there posts on the node's journal, and the relay
  brings it to every desktop following that node.
- Modules never hold a toast, an icon or a popover: they post, update
  and withdraw; the policy draws.

## 4. The Log & Alerts window

A focus window of its own (as Activity is: `shell/activity_focus.rs`,
[windows.md](windows.md)), opened by the bell, by View › Log & Alerts
and its key, and by "Details" on any toast (selecting that record).
It **replaces the Log panel**: the panel's id `helios.log` opens the
window, and a saved layout that had the panel drops it with a note.

```text
┌ Log & Alerts ──────────────────────────────────────────────────────────────┐
│ [Errors ✓] [Warnings ✓] [Info ✓] [Debug ☐]  Source: [All ▾]  Node: [All ▾]  │
│ Since: [This session ▾]   🔍 [rules group                ]   Unread only ☐  │
├────────────────────────────────────────┬───────────────────────────────────┤
│ ● 10:42  ⚠ import.rules   ×4           │ ⚠ Rules must be a group           │
│      Rules must be a group             │ import.rules · desktop · ×4       │
│   10:40  ✓ storage                     │ first 10:41:02, last 10:42:17     │
│      EOS_DIGITAL is safe to format     │                                   │
│ ● 10:38  ✖ node:studio  backup         │ Because:                          │
│      Backup to "Archive" failed        │  ← command import.rules.apply     │
│   10:31  ℹ log  helios_gui::accounts   │    (agent "mcp", 10:41:01)        │
│      cannot open the browser: …        │                                   │
│   10:30  ✓ export                      │ Details:                          │
│      12 photos exported                │  rule 3 is not inside a group     │
│                                        │                                   │
│                                        │ [Open rules]  [Copy]  [Mark read] │
├────────────────────────────────────────┴───────────────────────────────────┤
│ 214 records · 2 unread · [Mark all read]  [Copy filtered]  [Export…]       │
└────────────────────────────────────────────────────────────────────────────┘
```

- **Filters**: level toggles, source and node lists (sources that spoke
  this session, with counts), time ("This session", "Last hour",
  "Today", from the file for older), unread only. **Search** over the
  text in the interface's language and in English, and over the source.
- **Repeats grouped**: a folded record is one row with ×N and its first
  and last time; the detail lists each occurrence's time and arguments
  when they differ.
- **The detail**: level, source, node, time, the cause chain (each
  step a link: a command opens its record in the journal, an event its
  envelope, a job its Activity row), the details, the actions as
  buttons, Copy, Mark read.
- **Copy and export** for a bug report: Copy gives the record with its
  cause chain and `report.rs`'s system details (version, commit,
  system, display server), secrets blanked and the home folder written
  `~` as today; Export writes the filtered records as JSON lines (one
  record per line, the journal's own format) and a text version, ready
  to attach. The gallery's bug-report goal reuses the same export.
- **Unread**: the bell's count is the unread notifications; the
  window's title and the "Unread only" filter use the unread records.
  Selecting a row marks it read.
- **Older history**: the memory journal holds the session; "Today" and
  older read the log file (parsed back from its `k=v` lines), shown
  without actions (they belonged to a past session).
- Accessibility: rows are a list with each row's level said first; new
  errors are announced through the live region, as toasts are now.

## 5. Agents

The same journal, readable the same way everywhere:

- **Node**: `log.list {since?, level?, source?, limit?}` →
  `{records, last}` on the inspect endpoint and as an MCP tool, beside
  `events` and `wait_event`; `since` is a record id, so an agent pages
  as it does with events. Waiting is `wait_event` on `log.recorded`,
  nothing new.
- **Desktop**: the same `log.list` as a command (read-only) and as the
  debug bridge's `log` collector (egui-pop's `toasts` collector becomes
  a view of it), with the node records it relayed. An agent driving the
  desktop sees what the owner saw: the toast, its record, its folded
  count and its cause.
- `log.post` is open to agents too, with source `agent:<name>`: an agent
  can tell the owner something in the area, and the owner can mute it.

## 6. Survey, and where it lives

Licences as published on crates.io (2026-11-15); `deny.toml` allows MIT,
Apache-2.0, BSL-1.0, ISC, Zlib and the other permissive ones, and
Helios is GPL-3.0-or-later.

| Crate | Licence | What it gives | Fit |
|---|---|---|---|
| `log` 0.4 | MIT OR Apache-2.0 | the facade every Helios crate, egui and wgpu log through | **Keep**: the source of the facade's records |
| egui-pop's `Logs` (`platform/logs.rs`) | (ours) | the facade's logger: rotated file, level setting, tail for crash reports; its doc already weighed `tracing-appender` and owned a hundred lines instead | **Keep** as the file half of the sink; it gains one hook: each line handed to the journal |
| `tracing` 0.1, `tracing-subscriber` 0.3 | MIT | spans, structured fields, `Layer`s composed over one subscriber | No Helios crate uses `tracing` today; adopting it for the journal means moving 180 log lines and the logger. Its `Layer` idea (several consumers of one stream) is what the journal plus the file already are. **Not now**; if Helios later wants spans (timings per job), a `tracing-log` bridge and a layer feeding the journal can be added without changing the record |
| `tracing-appender` 0.2 | MIT | non-blocking, time-rotated file writer for `tracing` | Same as above; egui-pop's `Logs` covers the file |
| `tracing-log` 0.2 | MIT | bridges `log` to `tracing` and back | Only if `tracing` comes |
| `env_logger`, `simplelog`, `fern`, `flexi_logger`, `log4rs` | MIT / MIT OR Apache-2.0 | loggers with filters, outputs, rotation | Each is a logger; none keeps structured records in memory, folds repeats or knows actions. egui-pop's `Logs` is already the logger |
| `egui-notify`, `egui-toast` | MIT | toasts | Already weighed by egui-pop's toasts (owned); no record behind them |
| `notify-rust` | MIT OR Apache-2.0 | the system's notification centre | Already used by egui-pop's `notify`; stays the `System` decision's backend |

**Own** the record and the journal (a ring, a fold by key, an unread
set, facts out): a few hundred lines, no crate does it, and it must be
UI-free for the node. **Wrap** `log` and egui-pop's `Logs` to feed it.
**Adopt** nothing new.

Where each part lives:

| Part | Home | Why |
|---|---|---|
| `Record`, `Level`, `Source`, `Cause`, `Notice`, the journal (ring, fold, unread, mutes, `log.list` query) | **egui-pop-model** (UI-free; `helios-node` already depends on it) | generic, and the node needs it without egui |
| The `log` facade feed (a hook in `Logs` handing lines to the journal) | **egui-pop-platform** | it owns the logger |
| Toasts as a view of the journal (a toast carries a record id, folds show ×N, closing marks read); the notification area (strips: a place before the breadcrumb, a group of icons); the window's list widget (filters, search, folded rows) | **egui-pop-api** / **egui-pop-views** | any egui-pop application wants them; the gallery shows them |
| The `log.present` hook point | declared by **egui-pop** with the kernel (hooks.md places the kernel there) | the decision is generic; the rules are not |
| `Message` → the record's text, the sources' declarations, the presentation rules per module, mutes in Settings, the Log & Alerts focus and its cause links (Activity, Library), the node's journal and logger, the relay, `log.list` on MCP | **Helios** | Helios's vocabulary, modules and processes |

The record's text: egui-pop-model takes a text as key + arguments +
English (the shape of Helios's `Message`, `helios-core/src/message.rs`);
Helios converts its `Message` one to one, egui-pop's own toasts move to
keyed strings. If the record is better kept in Helios first (to move
faster), it moves to egui-pop unchanged later: nothing in it knows
either side.

## 7. Slices

Each slice builds and passes its tests alone.

**Before them, a bug fix (done 2026-11-15):** the `helios-node` binary
installed no logger (row 10), so the node's `log::` lines were dropped.
Its binaries now install `helios_node::logger`, which writes each line,
in the desktop's format, to standard error: the node's log file, beside
its `eprintln!` lines and in their order. Not egui-pop's `Logs`: the node
does not know where its host sends its log, and `Logs` would bring egui
into a headless binary. Rotating that file stays to do.

1. **(a) Every toast with its record first.** One function in the shell,
   `Shell::say(record)`, writes the Log panel's entry (so the file),
   then shows the toast, the note and the notice. Rows 1 to 5 move onto
   it: `card_toast`, `offer_import_session` and `follow_snapshots`
   gain their log line; `notify` also writes the Log panel; the panel's
   entry gains its time, source and actions. A frame test: every toast
   shown has a Log panel entry. No new crate. Small.
2. **(b) The record, the journal, de-duplication.** The types and the
   journal (in egui-pop-model, or Helios first); the Log panel reads
   the journal; `notes` and `notices` become views of it (bounded); the
   key folds repeats and a shown toast counts ×N; the hand-made sets of
   rows 6 and 13 go. The `log` facade feeds `warn` and above. Facts
   `log.recorded` and the rest on the shell bus. `log.list` on the
   desktop's debug bridge.
3. **(c) The Log & Alerts window.** The focus, its filters, search,
   folded rows, detail with the cause chain and actions, copy and
   export; the Log panel retired; "Details" on a toast opens it.
4. **(d) The notification area.** egui-pop: a status item before the
   breadcrumb and a group of icons. Helios: the bell, the per-source
   icons and popovers, `log.post`/`update`/`withdraw`/`read`/`clear`
   as commands; cards safe to format, automatic imports, failed jobs
   and health findings post notifications and withdraw them.
5. **(e) Node records relayed.** The node's journal and `log.list` on
   its MCP and inspect endpoint; `log.*` facts relayed through the
   node-event relay; resume through `log.list {since}`.
6. **(f) Policies per module.** `log.present` on the hook kernel (after
   hooks.md's slice 0): the defaults as a listener, user mutes
   (Settings › Notifications), modules' own rules, plugins' `[[hooks]]`.
   Until then, the defaults table is one function in the shell.

## Open questions (not decided)

- The journal's first home: egui-pop-model (recommended) or Helios
  first, moved later.
- Keep a docked compact view of the journal (a one-line strip at the
  bottom) beside the window, or the window alone (recommended).
- The memory bound (5,000 records, 500 warnings and errors kept apart)
  and the transient lifetime in the area (ten minutes).
- Whether "older than this session" reads back the log file, or the
  journal is persisted on its own (a small file of records next to the
  log).
