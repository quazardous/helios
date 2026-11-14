# Windows: one window per focus

Status: **proposed, not decided** (2026-11-11). A study the owner asked
for: what egui and egui-pop offer for several OS windows, what windows
that behave almost as separate applications need, what a "focus" is in
Helios's model, the options with their costs, a recommendation and the
order of work. Product choices for the owner and technical choices are
listed apart at the end, each with a proposed default.

## The direction

Helios becomes **one OS window per focus**. A focus is a work
intention, a project: "sort today's shoot", "edit the wedding", "import
the card". This is VS Code's model, where each window is almost an
application of its own, with its own state, layout, history and
selection; it is not a window per tool.

- Foci run side by side, each with its own jobs. An import runs in one
  while another runs an automated task on photos, and neither blocks the
  other.
- The import window, today an `egui::Window` drawn inside the main
  window, is the first to become a focus window of its own.
- The settings, given their size, become one global window of their own,
  tied to no focus.
- An idea to validate: two levels of settings, sharing one source.
  - **Quick settings**: a light launcher-style panel inside the focus
    window, as in Quicksilver. Type a few letters, change a common
    setting in place.
  - **Full settings**: the separate window, with the families and pages
    in the left pane and the page in the right pane.

This extends a decision already in the ROADMAP: **one window is one
workspace** (2026-10-11), reworded **one window is one focus**
(2026-11-13, P11), with auxiliary windows (detached panels) belonging to
their focus's window. A focus is that workspace, named
by its intention (see "What a focus is").

## 1. What egui 0.37 and egui-pop offer today

### egui's viewports

egui calls a native window a *viewport*. eframe's native backend runs
them all on **one winit event loop, on the main thread**, sharing **one
`egui::Context`** and, with wgpu, one device: one surface per window.

| Kind | How | Repaint | State |
|------|-----|---------|-------|
| Root | `App::ui` | its own | anything |
| Immediate | `Context::show_viewport_immediate(id, builder, FnMut)`, called in the parent's pass | **with its parent**: a repaint of either repaints both, so N windows can cost N times the work | borrows freely (`FnMut`, no `Send`) |
| Deferred | `Context::show_viewport_deferred(id, builder, Fn + Send + Sync + 'static)` | **on its own**: an idle window sleeps while another animates | must be reachable from a `Send + Sync` closure (egui's docs: channels or `Arc<Mutex>`) |
| Embedded | the fallback when the backend has no windows (`embed_viewports`, the web, kittest) | inside the parent, as an `egui::Window` | the callback gets `ViewportClass::EmbeddedWindow` |

Rules that shape the design (read in `egui/src/context.rs` and
`eframe/src/native/wgpu_integration.rs`, 0.37.0):

- **A child lives while its parent declares it.** Each pass of the
  parent must call `show_viewport_*` again; at the end of the parent's
  pass, a child it did not declare is removed, and a child whose parent
  is gone is removed too. Viewports form a tree: a focus window's
  detached panels can be its children and go with it.
- **The root cannot close alone.** A close request on the root ends the
  application (`EventResult::CloseRequested`). Any other window only
  gets `ViewportInfo::close_requested()` (and `ViewportCommand::CancelClose`
  exists), so the application decides. "Closing one focus keeps the
  others" therefore needs a root that is not a focus: a hidden
  coordinator window (`ViewportCommand::Visible(false)`). While the root
  is hidden, eframe runs no pass for it, only `App::logic`, unless it is
  sent `ViewportCommand::RequestPaintWhileHidden`. Its pass is where the
  focus windows are declared, so that command is needed. This is the
  first thing a spike must prove on each platform.
- **eframe saves only the root's geometry** (`persist_window`). The
  other windows' size and position are the application's to save and
  restore.
- **Input, focus and close are per viewport.** Each window's pass gets
  that window's input (`InputState::viewport()`: focused, minimised,
  maximised, its rects, its scale, `close_requested`). egui's keyboard
  focus is kept per viewport. egui-pop tests the Escape order in a
  second window for both immediate and deferred windows
  (`egui-pop-core/tests/focus_windows.rs`), with real viewports and no
  GPU. Styles, fonts and textures are shared, since there is one
  context.
- **Deferred callbacks run on the main thread** in eframe native,
  despite the `Send + Sync` bound. A deferred window can therefore reach
  `Rc` state through a thread-local registry keyed by `ViewportId`: the
  closure holds only the id, and asserts that it runs on the main
  thread. This shim is what lets egui-pop's `Rc`-based application (see
  below) use deferred windows without becoming `Arc<Mutex>` throughout.

**Platform limits:**

- **Wayland:**
  - A client cannot read or set a top-level window's global position.
    `with_position` is ignored and `outer_rect` is unknown, so only the
    size and the maximised state can be restored, and the compositor
    places windows.
  - Focus-stealing prevention: `ViewportCommand::Focus` needs an
    activation token (xdg-activation, which egui-pop's single instance
    already forwards on a second launch), or it may only flash.
  - A drag between two windows: while a button is held, the pointer
    stays grabbed by the source surface, and the target sees no pointer
    until release. A real drag between windows needs the OS drag and
    drop (`wl_data_device`), which winit 0.30 does not offer (it only
    receives file drops).
  - Windows sharing the application id are grouped by the shell, which
    is what is wanted.
- **X11:** positions work; a drag between windows can be followed by
  pointer coordinates (egui-pop's workbench does this for detached
  panels).
- **Windows:** positions and per-monitor scale work. The taskbar groups
  windows by application id, processes included. egui-pop's own
  hit-testing for its title bar (`frame.hit_test`) is installed on the
  root only today.
- **macOS:** one menu bar for the whole application. By convention an
  application stays alive with no window, and Cmd with the backtick key cycles its
  windows. Windows of **separate processes** show as separate
  applications in the Dock, which counts against one process per
  window.
- **Web:** no native windows; deferred viewports are embedded as
  `egui::Window`s inside the page. One browser tab per focus (one wasm
  instance each) is the web's own form of "one process per window".

Upstream's open work on viewports is tracked in egui issue 3556.

### egui-pop: per window and global today

egui-pop's application (`AppHandle(Rc<Shared>)`, `egui-pop-api/src/app.rs`)
is **one per process**. Its windows today are the main window plus
auxiliary windows: detached panels (`workbench::windows`) and the
Settings app popped out (`admin/shell.rs`), all **immediate** viewports.

| Per window already | Global (one per application) |
|--------------------|------------------------------|
| Title bar and frame: decorations, native hover, maximise and full screen (`frame.rs`, slots keyed by `viewport_id`) | The workbench layout: one layout, its auxiliary windows inside it |
| Scale, and a `scale.changed` event per window (`egui-pop-core/src/scale.rs`) | Commands, the keymap and its dispatcher, the palette's entries |
| The keyboard-focus order and Escape (egui's per-viewport focus, `focus.rs`) | The `when` context keys (one `WhenContext`) |
| `overlay::block_window`, one pass at a time per window | The host, its journal and events, the jobs centre |
| Image display surfaces (`egui-pop-viewport/src/display.rs`, keyed by viewport) | The histories (ADR 0014), the settings store and its history |
| The "active window" (`AppHandle::active_window`): the window worked in last | The theme (one context) and the strings |
| | Toasts: one queue, shown in the active window |
| | Dialogs and the palette: one queue, opened over the active window, **blocking every other window** while open |
| | The Settings app (`admin`): an overlay of the main window, or one window with a fixed id |
| | The agent socket (one per application id), the single instance, the clipboard |
| | The frame commands (`frame.close`, `frame.hit_test`) mostly address `"root"` |

In short, egui-pop already knows that several windows exist (the active
window, per-window scale, focus and frame), but **everything a user
works with is global**: one layout, one context, one modal queue. A
dialog in one focus would freeze every other focus, which is the
opposite of "neither blocks".

### Helios today

- **One shell per process** (`helios-gui/src/shell.rs`, `Shell`):
  - one library (`Rc<RefCell<Library>>`), one open workspace (a key);
  - `Shell::switch_workspace` saves the layout left and loads the next
    in the same window;
  - one viewport engine, one scheduler, one jobs list (`Shell::jobs`);
  - one GPU device, decided in the ROADMAP: one wgpu instance and one
    device per process.
- **Opening another workspace starts another process**
  (`Action::OpenWorkspace` spawns `helios-gui --new-instance <dir>`;
  `platform.rs` calls this "one process per workspace"). Without that
  flag, a later launch hands its arguments to the running instance
  (egui-pop's `instance::single`). So option B below already exists in
  embryo, with its costs: a second engine, cache and GPU device.
- **The import window** (`shell/import_window.rs`) is an `egui::Window`
  inside the main window.
- **The image display is wired to the root** (`display.rs` passes
  `ViewportId::ROOT`); detached panels get their own display through
  egui-pop.
- **The settings** are egui-pop's Settings app, with Helios pages
  (storage, health check), opened as an overlay or popped out.
- **The vault** is SQLite in WAL mode with a busy timeout and no lock
  file, so several processes can open it; their in-memory states would
  drift apart, though, and only the vault bus of each process would hear
  of its own writes.

## 2. What near-separate applications need

Each need is checked against one process (several windows) and several
processes.

| Need | One process | Several processes |
|------|-------------|-------------------|
| Own state: selection, layout, places and modes, panels | an egui-pop window scope (to build) | free |
| Own undo | already shaped: "Ctrl+Z goes to … the newest step among the scopes active in the window" (ROADMAP, undo) | free, but library steps from another process are invisible |
| Own shell: title, activity bar, status bar, toasts, dialogs that block only their own window | to build in egui-pop (see section 1) | free |
| Saved size and position | the application saves them per focus (eframe saves the root only); size only on Wayland | eframe per process, same Wayland limit |
| Closing one keeps the others | a hidden coordinator root (spike) | free |
| Window switcher and Window menu | in-process list, instant | needs a registry across processes |
| Drag and drop between windows | in-process payload; works on X11, Windows and macOS by pointer; on Wayland needs OS drag and drop (not in winit) | OS drag and drop only (not in winit for outgoing drags) |
| Copy and paste of typed items (edits, nodes, photos) | the kernel's clipboard, shared as is | must go through a shared node or the system clipboard's text fallback |
| One GPU device, one cache, "never load the same photo twice in RAM or VRAM" (ROADMAP 2.3) | kept | **broken**: one device and one cache per process, unless the engine moves out to a shared node and the display gives up zero-copy GPU frames (wgpu has no cross-process texture sharing) |
| Crash isolation | a panic takes every window (mitigated by reopening the foci open at the crash, as the `kill -9` scenarios already test for the workspace) | one window dies alone |
| A slow UI pass | freezes every window (one UI thread): work stays off the interface thread, the rule already | only its own window |
| Opening a window | instant | a process start (`start-timing` bench) |

**How VS Code does it.** VS Code has one main process (window
management, lifecycle, the single instance), one renderer process per
window, an extension host per window, and shared helper processes (the
shared process, the file watcher, the terminal host). It pays for
isolation with memory and with IPC everywhere. It can afford this
because a text editor's state is small and its "engine" is the file
system. Helios's engine state is GPU memory and a cache of decoded
photos, which is exactly what must not be duplicated.

**Sharing the node, scheduler and vault.** Whatever the window model,
the ROADMAP already places these below the interface: commands go to
the owner of the state, events go on a bus, and data access is the
framework's. In one process, every focus talks to the same in-process
core, the way the shell does today. The node can still move out of
process later for compute (a GPU PC, a NAS) without changing the window
model, because foci talk to it through the same contract that relays
use today.

**Jobs: owned by a focus, run by the shared scheduler.**

- **Ownership.** A job carries the focus that started it (a `focus` field
  in its `Start`, an `owner` tag on the scheduler's `JobSpec`). The
  shared scheduler runs every job, keeping the declared resource needs
  that already prevent deadlocks.
- **Progress per window.** A focus's status bar and Queue panel show its
  own jobs. The global jobs centre shows all of them, grouped by focus.
  `request_repaint_of(viewport)` wakes only the window concerned.
- **Priorities.** The scheduler has `Interactive > Prefetch > Batch`.
  Proposed: interactive work from the **front window** stays
  `Interactive`, interactive work from background windows drops to
  `Prefetch`, and batches stay `Batch`. A new named resource,
  `io:<volume>`, keeps an import reading a card from competing with a
  batch reading the same disk without bound. An import (IO-bound) and an
  automated task (CPU or GPU) then run side by side, as the owner wants.
- **Two foci touching the same photos.** The ROADMAP's data-access
  decision applies: data is immutable and addressed by content,
  concurrent edits coexist as versions, and explicit locks are expiring
  leases the scheduler takes, never modules.
  - An interactive edit in a focus takes no lock. In one process both
    windows show **the same live edit document** (one per image), and
    each step names the focus that made it.
  - A job that writes (a batch applying a style, an export to files, a
    move) takes a lease per photo or per destination. A photo leased by
    a batch shows "being changed by 'Apply style'" in the other focus,
    whose own write to that photo waits for the lease or is refused with
    that reason.
- **A focus closed while its job runs.** Proposed: the job goes on,
  since it is the scheduler's, not the window's. The global jobs centre
  lists it under the closed focus's name, and the focus is kept in the
  recent foci with a running badge; reopening it shows the job again.
  Closing asks a question only when the job cannot outlive the window,
  which no job should.

## 3. What a focus is in Helios's model

A focus is a **workspace named by its intention**. It reuses what a
workspace has (its network, layout, context stack, profile, template)
and adds an intention and a scope.

| Part | What it holds | Exists today as |
|------|---------------|-----------------|
| Identity | id, the user's words ("Edit the wedding"), kind (intake, cull, edit, export, free), created, last opened | a workspace's key and template |
| Scope | the photos it works on: a collection query, an explicit set, an import's results, or the whole library | the library's collection, global today |
| Layout | its panels, areas, auxiliary windows and their screens, the window's size and position | `workspaces/<key>/layout` |
| Where the user is | places and modes (a sub-workspace, darkroom on an image, compare, culling) | the context stack, `workspaces/<key>/context` |
| Selection | its own selection and focused photo | global today |
| Undo | its workspace scope; the library steps it made; the image edits are per image and shared | ROADMAP undo scopes |
| Open tools | panels shown, darkroom image, compare set, filmstrip | layout plus context stack |
| Jobs | the jobs it started | global today |

- **Create:** File > New Focus… (or the welcome page) asks for the
  intention in the user's words and offers templates. The workspace
  templates already exist, including the intake template "Import,
  cull, …". Some entry points create a focus directly: inserting a card
  creates an import focus; "Edit in a new window" on a selection creates
  an edit focus scoped to it.
- **Save:** continuous, as workspaces are written today (layout on exit,
  context at each transition).
- **Reopen:** File > Open Recent and the welcome page list the recent
  foci, each with its intention, its scope's count and its running jobs.
  The foci open at quit (or at a crash) reopen at start.
- **Ephemeral foci:** an import focus is done when its job ends and the
  user leaves it. It is then archived (kept in the recent list for a
  while), never deleted with its photos.

### Today's surfaces mapped

| Surface | Becomes |
|---------|---------|
| Library (grid, collections, filters) | a view inside every focus, filtered to its scope; a default "Library" focus (whole library) when no other is open |
| Darkroom | a place inside a focus, not a window; the image detached to a second screen is an auxiliary window of the focus |
| Compare, culling | modes inside a focus (the context stack) |
| Import | **a focus window of its own** (kind intake), the first one built |
| Export (one) and batch export | jobs owned by the focus that asked; the dialog opens in that window |
| Export queue, jobs centre | global: a jobs window or the Queue panel in "all foci" mode; each focus's Queue panel shows its own by default |
| Settings (the Settings app: storage, health check, keys, gestures, fonts) | **one global window** |
| Quick settings | a panel in every focus window, from the same source |
| Log, health check, storage page, plugins' pages | global (Settings app pages or the jobs window) |
| Theme editor | global window (it edits one theme for all windows) |
| Command palette, quick open | per window (opens in the window it was asked in, blocks only it) |

**Quick settings and full settings, one source.** Both read the same
declarations: the settings store and the Settings app's manifests
(`egui_pop_model::admin`), with their value scopes (user, library,
workspace, command line) and their history.

- **Quick settings** is a launcher over those declarations, close to the
  command palette (which already lists settings entries through
  `Hook::settings`):
  - type a few letters and the matching settings show as rows with
    their editor inline (a toggle, a choice, a number);
  - Enter or a click changes the value in place;
  - each row has a chip for the layer it writes ("you" or "this focus")
    and a link to its page in the full window.
  - Which settings show without typing: those marked `quick` in their
    manifest, then the ones changed lately.
- **The full settings** are the Settings app as it is today (scopes as
  tabs, families in the left pane, the page on the right), in its own
  window by default. A workspace-layer value it edits belongs to the
  focus it was opened from, which its header names and can switch.

## 4. Options

### A. Deferred viewports in one process, through an egui-pop "focus app" API

egui-pop gains a multi-window API. Helios's shell splits into a process
core and a focus per window.

**egui-pop:**

- `App::open_window(WindowSpec { id, title, kind, geometry, parent })`,
  plus close, raise and list. Each window is a deferred viewport, run
  through the thread-local shim, as the child of a hidden coordinator
  root.
- **A window scope**, keyed by window:
  - its workbench layout, its `when` keys, its selection hooks, its
    active history scopes;
  - its overlays: dialogs and the palette become **window-modal**, with
    app-modal kept for the quit and crash questions;
  - its toasts and status bar.
- **Commands carry their window**: the origin gains the window, and
  agents pass `window`, as `frame.*` already pass `viewport`. A
  `windows` collector lists them.
- Each window's geometry is saved under its id.
- The close policy: the last window quits on Linux and Windows; on
  macOS the application stays alive with the welcome window.
- A Window menu, and `window.switch` (a quick pick).

**Helios:**

- `Shell` splits into a **core**, one per process (library, vault,
  scheduler, jobs, engine and cache, GPU device, kernel clipboard,
  settings), and a **focus**, one per window (workspace, layout, context
  stack, selection, views, the display surfaces keyed by the window
  rather than `ViewportId::ROOT`).
- `workspace.open` opens a focus window in the process, instead of
  starting another process.

| | |
|---|---|
| Costs | The largest refactor: egui-pop's global state becomes per window (the layout, the context, modals, toasts, the palette); Helios's `Shell` is split. A spike for the hidden root on Wayland (labwc, mutter), X11, Windows and macOS. A headless harness for deferred windows (egui-pop's `focus_windows.rs` shows the way; Helios's `windows.rs` covers immediate ones). |
| Gains | One device, one cache, no photo loaded twice; live shared state (an edit seen in two windows, a rating at once everywhere); a new window opens instantly; the typed clipboard and drags between windows (where the OS allows); one agent socket that sees everything; one scheduler that can rank by front window. |
| Risks | One crash or one stalled pass takes every window (mitigated by reopening the foci and by the off-thread rule); upstream maturity of deferred viewports; the `Rc` shim depends on eframe running callbacks on the main thread (it does in 0.37; a test pins it). |

### B. One process per window, talking to the node

Each focus is a `helios-gui` process, as `--new-instance` does today.
The engine, scheduler and vault move into a shared local node process
(`helios-node`), and each window relays its events.

| | |
|---|---|
| Costs | Either each window keeps its own engine, cache and device (several times the VRAM and RAM, against ROADMAP 2.3), or the engine lives in the node and the darkroom loses zero-copy GPU display (no cross-process texture sharing in wgpu: frames copied back and uploaded again, latency in the darkroom). Every shared state goes through IPC: settings changes, the clipboard, the selection sent to another focus, undo of library steps. No drag between windows without OS drag and drop. Separate Dock icons on macOS; a process start per window. A registry of windows across processes for the switcher. |
| Gains | Crash and stall isolation; egui stays at one window per process; mostly exists today. |
| Risks | Two caches deciding independently (budgets, eviction); the vault's in-memory states drifting between processes; the node becomes mandatory on the desktop. |

### C. Mixed

One process holds every focus of **one library** (option A). A
**separate process** is kept where the boundary is real: another library
or profile (a different catalogue), plugins (out-of-process already),
and remote or local nodes for compute. The web, when it comes, uses one
tab per focus.

| | |
|---|---|
| Costs | Those of A, plus keeping `--new-instance` for another library. |
| Gains | Those of A, with isolation where data is separate anyway, and the node's process boundary where it pays (compute, not the interface). |

### Recommendation

**C, which is A within a library.** The decisive constraints are already
decided in the ROADMAP: one GPU device per process, no photo loaded
twice in RAM or VRAM, and zero-copy GPU display. Option B can keep them
only by moving the engine out of the window, which costs the darkroom
its latency. VS Code's isolation fits text; Helios's state is GPU
memory. One process also makes the owner's other wishes cheap: windows
that do not block each other (window-modal overlays, a shared
scheduler), live shared state, and an import window that opens
instantly. Crash isolation is the price, paid back by reopening the
foci.

Deferred viewports rather than immediate ones: with several photo-heavy
windows (thumbnail grids, the darkroom), immediate viewports would
repaint every window on any change. Immediate viewports stay for the
embedded fallback and kittest.

## 5. Slices

Each slice builds, passes its tests, and leaves the application usable.

1. **Spike (egui-pop, a gallery example).** A hidden coordinator root
   with `RequestPaintWhileHidden` declaring two deferred windows, whose
   state is `Rc` behind the thread-local shim. Close one, keep the
   other; quit when the last closes. Measure idle CPU with three windows
   (one animating). Run in wbox (labwc and mutter), on X11, on the
   Windows box, and on macOS when available. This decides "hidden root"
   versus "root is the first focus".
2. **egui-pop's multi-window API.** `open_window` with its spec, the
   window scope (layout, `when` keys, overlays, toasts, palette, status
   bar), window-modal dialogs, commands carrying their window, the agent
   socket's `window` argument and `windows` collector, geometry saved
   per window, the close policy, a Window menu and `window.switch`. A
   headless harness for deferred windows in `egui-pop-test`. Documented
   as an ADR (window scope).
3. **Helios split, behaviour unchanged.** `Shell` becomes core and
   focus, with exactly one focus window. The display surfaces are keyed
   by window. Nothing visible changes; the scenarios and snapshots
   pass.
4. **Import as the first focus window.** The import window's content
   moves into a focus window (kind intake), opened by File > Import…, a
   card toast, a drop or a paste. Its job is owned by the focus, with
   progress in its status bar. The behaviour after pressing Import is
   the owner's choice P2. The main window is no longer blocked while
   choosing what to import.
5. **The settings window.** The Settings app in a global window through
   the new API: one instance, `Ctrl+,` from any focus raises it, page
   deep links (`admin.open`) unchanged, overlay mode kept for small
   screens.
6. **Quick settings.** A launcher panel in each focus window over the
   same declarations: a `quick` flag in manifests, inline editors, the
   layer chip, a link to the full page.
7. **Foci as first-class.** New Focus with an intention and template,
   recent foci, reopen with geometry, `workspace.open` in process. The
   scheduler gets focus tags and the front-window boost, and leases on
   photos for writing jobs. The jobs window gets "all foci".
8. **Between windows.** "Send to focus…" (selection, photos, an edit),
   the typed clipboard across windows, and drags where the OS allows.

### Spike results (slice 1, 2026-11-11)

The spike is egui-pop's `multi_window_spike` example
(`crates/egui-pop-gallery/examples/multi_window_spike.rs`, plain eframe
0.37) and its wbox scenario (`tools/wbox/multi_window.py`), run on the
bench box with `cargo xtask bench-remote egui-pop-wbox
egui_pop=<branch> script=multi_window.py [compositor=mutter]`. A root
sends `ViewportCommand::Visible(false)` and declares two deferred
windows: "A" repaints every frame, "B" only on input. Their state is
`Rc<RefCell<…>>`, reached from the `Send + Sync` callbacks through a
thread-local registry keyed by `ViewportId`. Each callback holds only
its id and checks that it runs on the main thread. The example prints
JSON lines (frames per window, focus, close, geometry, quit).
`SPIKE_AUTOPILOT=1` makes it close B, then A, by itself, for runners
with no window driver.

**Read in eframe 0.37 and winit 0.30.13 first:**

- **Wayland cannot hide a window.** winit's Wayland `set_visible` does
  nothing, and `is_visible` returns `None`, which eframe reads as
  visible. A hidden coordinator root is therefore impossible on Wayland
  with winit 0.30.
- **A hidden root still runs its pass while a child is visible.**
  eframe 0.37 runs a hidden viewport's ui when it or any descendant is
  visible (`is_viewport_or_descendant_visible`). Only with no window
  visible does it fall back to `App::logic` alone, and then
  `RequestPaintWhileHidden` buys one pass.
- **The first painted frame shows the root.** eframe creates the root
  hidden and calls `set_visible(true)` after its first paint. A hide sent
  in that pass is applied after that call, so on X11, Windows and macOS
  the root may flash once. On X11 (below) the root ran 26 passes before
  the hide took effect; a flash was not captured, as the first
  screenshot comes 1.5 s later.

**Wayland, labwc (box job 20261111-122717-1c38): proven, except
hiding the root and raising a window.**

| Check | Result |
|-------|--------|
| Hidden root | **not possible**: "Spike root" stays listed and shown after `Visible(false)` |
| Independent repaint | **proven**: over 2.9 s, A painted 180 frames, B 0 and the root 0. CPU was 16.5 % with software rendering |
| Closing one window | **proven**: closed through the compositor (`wlrctl toplevel close`), B reports `close_requested`, the application stops declaring it, and it goes. A keeps painting and the process stays |
| Closing the last window | **proven**: closing A quits the process. The root ran 7 passes in the whole run |
| The `Rc` shim | **proven**: 0 callbacks off the main thread |
| Focus | **proven**: each window's `focused` follows the compositor (B, then A once focused) |
| Raising another window (`ViewportCommand::Focus` on B from A) | **not honoured**: A stays activated. This is focus-stealing prevention, so it needs an xdg-activation token |
| Geometry | **not readable as tried**: `ViewportInfo::inner_rect` and `outer_rect` stay `None` for deferred windows on Wayland, so nothing was saved. The restored builder size was passed but not measured. The size has to be read from the pass (`Context::content_rect`), and the position is never available on Wayland |

**Wayland, headless mutter: the stall is the FIFO present (box jobs
20261111-130143-0109 and 20261111-131011-30b9).** The spike now takes
variants from the environment: `SPIKE_PRESENT_MODE` (the wgpu present
mode), `SPIKE_A_SIZE` and `SPIKE_ROOT_SIZE` (window sizes, which change
what covers what, as Wayland lets no client place its windows). The
scenario runs each variant in autopilot on mutter. With software
rendering (lavapipe), the surfaces offer `[Mailbox, Fifo]`.

| Variant | Result |
|---------|--------|
| default (`AutoVsync`, so `Fifo`) | **stalls**: A stops after 6 frames, no quit in 25 s |
| A larger than B (B covers part of A), `Fifo` | **stalls** after 15 frames |
| root larger (A, B and the root each partly visible), `Fifo` | **stalls** after 13 frames |
| `AutoNoVsync` (here `Mailbox`) | **works**: A paints 300 frames in 5.1 s (~60 per second), B stays at 5, the root ran 7 passes; B's close and A's close then the quit as on labwc; 0 off-thread callbacks |
| `Mailbox` | **works**, the same figures |
| A larger, `AutoNoVsync` | **works**, the same figures |
| `Immediate` | **refused**: wgpu panics in `Surface::configure`, the mode is not offered |

So the stall is not occlusion: with the root larger, every window shows
part of itself and the FIFO run still stalls; under the same
compositor, the same windows and the same placement, `Mailbox` never
stalls. A FIFO present waits on the compositor (a frame callback, or
the `fifo-v1` barrier with recent Mesa and mutter) inside the one UI
thread, and on headless mutter that wait does not end, which freezes
every window. Why mutter does not release it was not found: Mesa's
Wayland FIFO was rebuilt on `fifo-v1` and `commit-timing` (Mesa 24.3),
and mutter's own FIFO work, with its occluded-surface rules, is still
moving. A single-window FIFO control run was not made, so whether one
window alone stalls on headless mutter is open. A real GNOME session
was not tried.

The fix is on the application side, and eframe already takes it for
its other renderer: its glow backend never blocks in the swap on
Wayland ("a compositor may never present a hidden window's buffer",
egui issue 5136), because winit already paces `RedrawRequested` by the
compositor's frame callbacks (`pre_present_notify`, which egui-wgpu
calls too). A non-blocking present mode gives wgpu the same pacing:
the measured rate stayed at ~60 frames per second, not unbounded. A
glow variant was not run: eframe's `glow` feature changes
`App::on_exit`'s signature, and egui-pop's `App` implementation does
not build with it.

**X11, labwc's Xwayland (box job 20261111-130143-f811): proven, hiding
the root included.** The image now has winit's X11 libraries
(`libxkbcommon-x11` and the X libraries it loads, Helios
`tools/bench/Benchfile`). The same job's Wayland run gave the labwc
results above again.

| Check | Result |
|-------|--------|
| Hidden root | **works**: "Spike root" is not listed after `Visible(false)`. eframe's `ViewportInfo::visible` for the root still reads `true` on every frame, so the application cannot trust it |
| Independent repaint | **proven**: over 3.25 s, A painted 330 frames, B 0 and the root 0. Not paced: about 400 frames per second at 263 % CPU even with the default `AutoVsync`, as lavapipe on Xwayland does not throttle `Fifo` |
| Closing one window | **proven**: B goes, A keeps painting, the process stays |
| Closing the last window | **proven**: closing A quits the process, the root ran 29 passes in the whole run |
| The `Rc` shim | **proven**: 0 callbacks off the main thread |
| Raising another window | **honoured**: `F` in A activates B |
| Geometry | **readable and restored**: `inner_rect` and `outer_rect` are filled; a second start with B at 500 x 330, position 300, 200 opens B there. The sizes saved at close were 1280 x 800 at 0, 0: labwc made the Xwayland windows fill the output after they opened (the screenshots show B undecorated and full screen), a harness behaviour, not the application's |

At the end of the second start, winit's X11 backend panicked in
`inner_size` once the X server had gone (the compositor stopped first).
This is teardown only.

**Windows and macOS: not run.** A `workflow_dispatch` on a branch is
not allowed, and egui-pop's CI runs on Ubuntu only. Proposed: a
non-blocking job in egui-pop's `ci.yml`, run on pushes to `next`, with
`continue-on-error: true`. A matrix `windows-2022`, `macos-14` and
`ubuntu-24.04` would build `cargo build --release -p egui-pop-gallery
--example multi_window_spike` and run it with `SPIKE_AUTOPILOT=1`, under
`xvfb-run` with a window manager on Ubuntu (X11). It would upload the
JSON lines. The job passes when the `quit` line shows 0 off-thread
callbacks, the `counts` lines show A advancing while B stays still,
`root_visible` is `false` after the first frames, and the process exits
0. The OS close path (the user's close button) and focus stealing need a
window driver: wbox's `win32` backend on the Windows box, and a Mac when
one is available.

**What this decides:**

- **T2, the root window: "the root is the first focus", not a hidden
  coordinator.** The hidden root cannot work on Wayland, Helios's main
  Linux target. Proposed instead: the root window is always a focus
  window. When the user closes it while other foci are open, the
  application cancels the close (`CancelClose`) and moves the most
  recent other focus into the root, and that focus's deferred window
  closes. This needs no hiding on any platform. On X11, Windows and
  macOS a hidden root remains possible, but one code path for every
  platform is simpler.
- **T1 and T3 hold:** deferred windows repaint independently, and the
  thread-local `Rc` shim works as designed.
- **T6:** geometry is read from each window's pass, not from
  `ViewportInfo`; position is restored only where the platform gives it.
- **Raising a window** on Wayland goes through an activation token
  (xdg-activation), which winit 0.30 does not expose for an existing
  window. Until then, "raise the settings window" may only flash it.

**For slice 2, the egui-pop API:**

- `open_window` creates a deferred viewport declared from the root's
  pass, and the root's own content is a focus like any other.
- The registry and shim are egui-pop's (`WindowId` to the window
  scope), with a debug assertion on the main thread.
- Each window's close goes to the application, which stops declaring the
  window, and the root's close moves a focus into it.
- Geometry comes from each window's content rect.
- **Present mode: egui-pop's multi-window launcher sets wgpu's present
  mode to `AutoNoVsync` on Wayland** (`Mailbox` where `Immediate` is not
  offered), and keeps `AutoVsync` on X11, Windows and macOS, where the
  present is not what stalls and an unpaced mode would spin. On Wayland
  the pacing comes from winit's frame callbacks, as for eframe's glow.
  The choice is made at start from the display winit will use
  (`WAYLAND_DISPLAY` set), as eframe's surface configuration is one for
  every window; the application can override it.
- The root stays "the first focus" (T2) even though X11 can hide it:
  Wayland cannot, and `ViewportInfo::visible` is not reliable on X11.
- Still open: a single-window FIFO run on headless mutter, a real GNOME
  session, and the Windows and macOS runs proposed above.

### Built: slice 3 (2026-11-11)

Helios's `Shell` (`crates/helios-gui/src/shell.rs`) is now a process
core and one focus, with nothing visible changed: the process still has
one window, the main one, and the scenarios and snapshots are the same.

```text
Shell
├── core: Core    (shell/core.rs)   one per process, shared by every window
├── focus: Focus  (shell/focus.rs)  the main window: the root viewport, the first focus
└── notes         what the shell has to say to its caller
```

**The core** holds what every window shares:

- egui-pop's application (one per process with egui-pop 0.8), the
  command registry, its declarations and the queue of the commands'
  actions, the panels declared;
- the theme and the settings (profiles, the machine, the resolved view);
- the value store and its undo, the open image's history snapshots;
- the viewport engine and its render cache (`Core::engine`), the GPU
  render state and what installing it found (one device per process);
- the clipboard (its endpoints, the system clipboard, a copy being
  rendered);
- the jobs: their reports, the library's undo steps, the status line, the
  nodes followed, the jobs ended and forgotten;
- the imports and exports handed to the host and their lines, the
  duplicates and stack proposals they bring, the library's files on disk;
- the notices and the log, the dialogs, the clock, the catalogues
  (`i18n`), the launches and links to open (`opens`), the Settings app's
  Storage and Health check pages.

The catalogue itself (the vault, the cache service, the node connection)
is the host's (`main.rs`), outside the shell, and one per process too.

**A focus** holds what belongs to its window:

- the window it is drawn in (`Focus::window`, `ViewportId::ROOT` for the
  main window), which keys its display surface (the Wide-gamut image
  surface is taken down for that window), its title bar's system menu and
  its pictures (named rectangles, scale, screenshots, recording);
- its workspace: the layout and its last save, the workspace's state as
  edited, the sub-workspaces, the places and modes followed, the import
  source, the title;
- its views and tools: the library as the window shows it (grid, filter,
  selection), the viewport on the open image and the image being opened,
  the preview, parameters, masks, network, scopes and decisions, the Queue
  and Workshops panels, what each panel draws (`contents`), the status
  bar items, a menu open at a point, a tab's drag refusal;
- the import window and the photos the imports brought, to select.
- the comparison (2026-11-11): the photos the Compare place names, the
  active one, the best, the zoom and pan and whether they are
  synchronised (`crate::comparison::Comparison`), which the Compare panel
  draws; a Compare window of its own takes it over with slice 4.

**The seams slice 4 uses.** A focus is made by `Focus::new` from a
`Seed` (its window, workspace, library view, viewport and the panels'
contents), so the Import window becomes a second `Focus` on the same
core. Its viewport comes from `focus::viewport_on(&core.engine)`
(`Viewport::on_engine`): every window renders on the core's one engine,
so no photo is decoded or cached twice. Slice 4 turns `focus` into the
list of foci, the root's first, and keys the frame's work by the window
egui-pop's multi-window API passes; nothing here depends on that API
yet (Helios was on egui-pop 0.8 until slice 4).

**Left for later slices.** The value store, the snapshots and the jobs
are the core's: a focus's own undo scope and its jobs as tags (T7) come
with foci as first-class (slice 7). The Queue panel and the status line
show every job. The host's per-image work in `main.rs` (the image shown,
the images being made ready) still serves the one focus.

### Built: slice 4 (2026-11-11)

Helios is on egui-pop 0.9, and the Import window is the first focus
window: an OS window of its own, opened through egui-pop's application
windows (`AppHandle::open_window`, an egui deferred viewport).

```text
Shell
├── core: Core                  one per process
├── focus: Focus                the main window: the root viewport, the first focus
└── import: ImportFocus         the import focus (kind intake), OS window `import`
    └── state: Rc<RefCell<ImportWindow>>   shared with the window's content
```

- **One state, two passes.** The import window's state
  (`shell/import_window.rs`, `ImportWindow`) is shared, behind an `Rc`, by
  the shell and the window's content, the closure given to
  `open_window`. egui-pop reaches it from the deferred callback through
  its thread-local registry (T3). The content runs in the window's own
  pass: it follows the cards and the scans, draws the three columns, the
  footer and the status bar, and does there what needs only the window's
  state: ticks, favourites, presets, the folder choosers (opened from the
  window's pass, so the dialog is the window's). What needs the shell
  (Import, "Forget this folder", Storage…, sources dropped) waits in the
  state's events for the main window's next pass, which the content asks
  for. The shell gives the content its translator and the two lines it
  needs of the core (the backup strategy, no library open) at each of the
  main window's passes while the focus is open.
- **Opening and closing.** `library.importWindow` (File > Import…,
  Ctrl+Shift+I, the empty Library's button, a card's toast, a drop, a
  paste) opens the focus; the main window's pass then opens its OS
  window, sized four fifths of the main window at first (egui-pop keeps
  its size under `import` after that), or brings it to the front when it
  is open. Cancel, Import and the window's close button close the focus,
  and the window with it; a window closed by other means (`window.close`)
  closes the focus too. The main window stays. Closing the main window
  while the import window is open moves the import focus into it (T2,
  egui-pop's rule).
- **Its own shell.** egui-pop gives the window its own toasts, dialogs
  and palette (ADR 0025): the folder chooser opened from it shows there
  and blocks it alone, never the main window. Its keys run with
  `appWindow == 'import'`, and agents name it (`window: "import"` in
  `call` and `snapshot`; the `windows` collector lists it).
- **Its status bar** says what the scans read, "Reading <path>", the
  path shortened in its middle to the bar's width (`Scan::reading`),
  with a spinner; the line above the cells is gone.
- **Repaint.** A scan's progress repaints the window it is drawn in, not
  the main window: the main window sleeps while a card is read.
- **One core.** The thumbnails come from the library's one thumbnail
  cache and the glance cache, shared with the main window: no photo is
  read twice.
- **Present mode.** `AppBuilder::multi_window(true)`: `AutoNoVsync` on
  Wayland (`Mailbox`), as the spike found; `AutoVsync` elsewhere. The
  root window stays the first focus (T2).
- **Embedded.** Where the backend has no windows of its own (kittest,
  the web), egui draws the same content in an `egui::Window` inside the
  main window, so the import tests and snapshots run as before, the
  window placed by egui rather than centred.
- **Tests.** `tests/import.rs`,
  `import_opens_in_its_own_window_reads_in_its_status_bar_and_closes_alone`,
  runs the shell with real viewports (no embedding): the import window is
  declared as a deferred viewport and drawn in a pass of its own, its
  status bar names what is read, agents reach it as `import`, its close
  button closes the focus and leaves the main window as it was.

**Not done here.** The import focus is not a full `Focus` (a workspace,
a library view, a viewport): it needs none yet. "Cull these photos"
after an import (P2) is still the owner's choice; the window closes once
Import is pressed, as before. The jobs are the core's, shown in the main
window's status bar and Queue panel (focus tags come with slice 7).

### Built: slice 5 (2026-11-12)

The Settings are the second window of their own: egui-pop's Settings app
in its window mode, which egui-pop now opens as an application window
(`AppHandle::open_window`, id `settings`, `admin::APP_WINDOW`) instead of
an immediate viewport.

- **Window mode by default.** Helios sets the app's mode (`admin.window`)
  to its own window while the person has never chosen one
  (`shell.rs`, `settings_in_their_own_window`); "Pop in" still takes the
  Settings back over the main window, and that choice is kept.
- **Opening.** Their usual commands open the window (`admin.open`:
  `Ctrl+,`, File > Settings, the deep links from Storage, the health
  check and the import window), and so does Window > Settings. Open
  already, it comes to the front, on the page asked for. One instance.
- **Chrome and geometry.** As the import window: Helios's title bar under
  its own frame, its size and place kept under `settings`, agents naming
  it (`window: "settings"`), its keys run with `appWindow == 'settings'`.
- **Closing.** Its close button, the page's Close, Escape, `Ctrl+W` and
  `admin.close` close it alone; the main window stays. Closing the main
  window while it is open moves the Settings into the main window with
  their size, place and title bar (egui-pop's rule).
- **The forms** are egui-pop's pages, unchanged: drawn in the window's own
  pass, writing the same store and layers, so a change reaches every
  window at once.
- **Embedded.** Where the backend has no windows of its own (kittest, the
  web), the overlay as before: the settings tests and snapshots run
  unchanged.
- **Tests.** `tests/settings_window.rs` runs the shell with real
  viewports: the window and its title bar, a section chosen and the
  search typed in it, closing it alone, Window > Settings, and the move
  into the main window. `tests/scenarios/settings-window.ron` runs the
  same live, through egui-pop-test's window targeting.

**Not done here.** Workspace-level values edited in the Settings window
still belonged to the main window's workspace (P9): done in slice 7's
focus header below. Quick settings are slice 6.

### Built: slice 6, quick settings (2026-11-13)

P8-A, as built. `admin.quick` (`Ctrl+Shift+Q`, File > Quick settings)
opens a small panel over the window it is asked in, one per window as the
palette. Before typing it lists the settings manifests mark quick
(`Manifest::quick`; Helios marks a dozen in `config::QUICK`), then those
the person changed in this session within the last hour, which age out;
typing searches every setting. Each row is the Settings app's own row, so
a change goes through the host's store (the node for Helios's settings),
and "Set for" shows only on a setting with a library level. Each row
links to its page in Settings. Deviations from the plan: "changed lately"
lasts the session (not kept across runs), and the quick flag lives in
manifests rather than in each setting's declaration, so Helios can mark
egui-pop's own settings (theme, size, language).

### Built: slice 7, the focus the Settings edit (2026-11-13)

P9-A, as built. egui-pop's Settings app takes the host's foci
(`Admin::set_foci`, `SettingsFoci`): each an id, a label and the
application windows that belong to it. Opened from a window (`admin.open`,
`Ctrl+,`, a deep link), the Settings edit that window's focus; asked in
their own window, they keep the one edited. Once there are several foci,
a header above the page names the focus edited ("Settings for") and its
switch picks another (`admin.focus {id}` for agents). A focus that closes
gives way to the first, the main window's.

Helios's foci (`config/focus.rs`): the main window's, named by its
workspace and library ("Main window: workspace Wedding, library photos"),
and the import window's while it is open, named by the library it imports
into. Editing one attaches the store holding its values and offers its
scopes: the library's in both (one library per process), the workspace's
only in the main window's focus. A setting with no such level is the
app's, the same whatever the focus.

Deviations from the plan, newer decisions winning: since each setting
declares its own levels, no Helios setting has a workspace level yet, so in Helios the
switch changes which scopes "Set for" offers, not values; egui-pop's
tests show the values following a switch between foci that hold
different ones. egui-pop resolves one value per setting in a process, so
the layers of the focus edited are the ones applied: per-window values
wait for foci as first-class (the remaining slice 7 of section 5).

## 6. Open questions

### Product choices (for the owner)

| # | Question | Proposed default |
|---|----------|------------------|
| P1 | Can every focus be closed, the application staying alive? | Closing the last focus quits on Linux and Windows; on macOS the application stays with the welcome window, as macOS applications do. See section 7, D2 and D3. |
| P2 | After Import is pressed, does the import focus close (today) or stay? | It stays, follows its job, then offers "Cull these photos", which turns it into a cull focus scoped to the import; closing it leaves the job running. |
| P3 | A focus closed while its job runs | The job goes on, listed in the jobs window under the focus's name; the focus stays in the recent list with a running badge. |
| P4 | The same photo in two foci | One live edit shared by both windows, each step naming its focus; jobs that write take leases; the darkroom says "also open in 'Edit the wedding'". |
| P5 | Undo of library actions (ratings, tags, imports) | Per focus: Ctrl+Z undoes the focus's own newest library step; the history panel shows the others' steps greyed. |
| P6 | Is a focus a workspace? | Yes: a workspace with an intention and a scope; ephemeral foci (an import) archived once done and kept in the recent list for 30 days. |
| P7 | Word in the interface | "Focus" in menus (File > New Focus…), or "Project"; Helios uses "workspace" for the network today, so one of the two terms should yield. |
| P8 | Quick settings: what shows before typing, and where a change goes | Settings marked `quick`, then those changed lately; a change writes the layer the full settings would, its chip saying which. |
| P9 | Settings window and workspace-layer values | Decided 2026-11-13 (P9-A): they belong to the focus the window was opened from, named in its header and switchable. Built in slice 7. |
| P10 | The "Library" focus | Always available (Window > Library) and reopened when nothing else is; it is the whole library's focus. |
| P11 | ROADMAP wording | Done 2026-11-13: "One window is one workspace" (2026-10-11) now reads "one window is one focus". |

### Technical choices

| # | Question | Proposed default |
|---|----------|------------------|
| T1 | Deferred or immediate viewports | Deferred; immediate only embedded and in kittest. |
| T2 | The root window | The root is the first focus (the spike, 2026-11-11: Wayland cannot hide a window). Closing it while others are open moves the most recent other focus into it. |
| T3 | `Rc` application state and `Send + Sync` callbacks | A thread-local registry keyed by window and a shim closure holding the id, asserting the main thread; no move to `Arc<Mutex>`. |
| T4 | Modal scope | Window-modal by default; app-modal only for the quit, crash and "another instance" questions. |
| T5 | Agent socket | One per process; commands and collectors take `window` (a focus id), defaulting to the front window. |
| T6 | Geometry | Saved per focus id: size, maximised, screen; position restored where the platform allows (not Wayland). |
| T7 | Scheduler | A focus tag on jobs; interactive work from background windows ranked as prefetch; `io:<volume>` resources; leases per photo for writing jobs. |
| T8 | GPU | One device, one surface per window (eframe does this); the display's surfaces keyed by window. |
| T9 | Crash recovery | The foci open at a crash reopen at the next start, with their jobs resumed where the journal allows. |
| T10 | Web | Embedded windows first; one tab per focus later. |
| T11 | Another library | Still a separate process (`--new-instance`), its windows grouped by the platform. |

## 7. Closing and menus

Status: **D1, D3 and D5 decided** (2026-11-12, the owner: Ctrl+Q quits
the whole application, Ctrl+W closes one window as its close button
does, Helios stays open while any of its windows exists, every window
has a menu bar with at least a Window menu) and **built**, except
reopening a closed main window (see "Built" below). D2, D4, D6 and D7
keep their proposed defaults, not decided; D2's and D7's macOS part is
built (2026-11-13, "Built on macOS" below), its window-following menus
apart. Now that a focus can be an
OS window of its own (the Import window, slice 4), this section settles
what closes one window and what quits Helios, what happens to running
work, which menus each window shows and where its keys act.

**The boundary.** The process's core (the library, the vault, the jobs,
the kernel) is Helios's local node; every OS window, the main one
included, is a client view of it. No window owns the application:
quitting is the core's policy (`shell/lifetime.rs`), which the desktop's
host asks when the main window's close is requested. Ctrl+Q asks the
core to quit; it does not close a window. The last window closed follows
the same rule as Ctrl+Q. A node on a server keeps running when its
desktop client quits, and so do its jobs.

### Before this section was built

Read in Helios and in egui-pop 0.9 (`egui-pop-api`), the version Helios
uses.

**Keys and commands**

| What | Command | Key | What it does today |
|------|---------|-----|--------------------|
| Quit | `app.quit` (egui-pop, `app.rs`, File > Quit) | Ctrl+Q (Cmd+Q on macOS: egui-pop counts Command as Ctrl, `egui-pop-core/src/keys.rs`) | Sends `ViewportCommand::Close` to the viewport of the pass it runs in: the main window from the main window. |
| Close window | `window.close` (Helios, `shell.rs`, a View command) | none | Asks the main window's frame to close (`frame::request(.., Close)`), whatever window it is run from. |
| Close window | `window.close` (egui-pop's `WindowsPlugin`, `windows.rs`) | none | Closes the window named, or the active one. **Not installed by Helios**, so no Window menu either; it would clash with Helios's command of the same id. |
| Close window | `app.window.close` (egui-pop's frame, `frame.rs`) | none | The title bar's system menu, Close: closes the window drawn by egui-pop's frame. |
| Close the Import window | the window's close button, Cancel (`shell/import_window.rs`) | none (Escape is not bound there) | Cancels what the window reads (its scans) and closes the focus. Import starts the job and closes the window. |

Ctrl+W is bound to nothing; the darktable keymap leaves darktable's
Ctrl+W out on the ground that "Ctrl+W closes", which is not true yet.

**The main window's close** (its ×, Alt+F4, Ctrl+Q, the window
manager). Helios's `App::ui` (`main.rs`) sees the close request first:
it writes what the user changed to the vault and returns without
drawing. The root viewport closes and eframe ends, taking every other
window with it; then `on_exit` saves the settings, keys and layout, and
the vault closes within its deadline (`quit.rs`). Running exports are
cancelled without a question (`export.rs`, `Drop for Exporter`). Nothing
asks before quitting.

**Found along the way** (read in the code, then checked headless):

1. egui-pop's rule for a main window closed while other windows are
   open (the most recent one moves into the main window, T2) runs in
   egui-pop's `App::show`. Helios returns before calling it in the
   closing pass, so in Helios the rule never runs: closing the main
   window with the Import window open quits, the Import window with it.
   Slice 4's text says otherwise; one of the two must change (see D3).
   Fixed: the host asks the shell first (D3 decided, "Built" below).
2. Ctrl+Q pressed in the Import window runs `app.quit`, whose `Close`
   goes to the window it runs in. A window's keys are dispatched in its
   own pass, but egui-pop runs their commands at the main window's next
   pass, so that `Close` reaches the main window, not the Import window,
   and quits only through finding 1; once the main window's close keeps
   the other windows (D3), Ctrl+Q would close the main window alone.
   Fixed either way: Quit is the core's (D1, "Built" below).
3. The system's end of session (SIGTERM, a logout, `WM_ENDSESSION`,
   `ending.rs`) closes the main window as Ctrl+Q does, so it quits, as
   wanted, but asks nothing and waits for nothing (8 s deadline).
4. The whole keymap is dispatched in the Import window too
   (`dispatch_window_keys` in egui-pop's `windows.rs`, one shared
   keymap, `appWindow == 'import'` in the context). A key whose command
   acts on the shell (Ctrl+B folds a side bar, Ctrl+P, Ctrl+Shift+E)
   fires from the Import window and acts on the main window, unless its
   `when` clause rules the window out.

**Menus.** egui-pop draws one menu bar, `MENU_MAIN`, inside the window,
in the main window's pass only (`App::show`, `app.rs`; switched off by
`AppBuilder::menu_bar(false)`). The Import window has no menu bar: its
commands are its buttons. There is no native menu bar on macOS: the menu
bar is drawn inside the window there as elsewhere (egui-pop's only native
menu is the tray's, `egui-pop-platform/src/tray.rs`).

**Per window already** (ADR 0025): each application window has its own
toasts, dialogs and palette; a modal blocks its own window only; its keys
run with `appWindow` set; agents name it (`window: "import"`).

### How established applications do it

| Application | Close a window / quit | The last window | The main window closed, others open | Running work | Menus per window |
|-------------|-----------------------|-----------------|-------------------------------------|--------------|------------------|
| Lightroom Classic | Ctrl+Q (Cmd+Q) quits; the secondary window has its own toggle | quits (one main window) | the secondary display window goes with it | users report a question before quitting during an export | menus on the main window only; Import is a modal dialog |
| darktable | Ctrl+Q quits; Ctrl+W is focus detection | quits | the darkroom's second window goes with it | not checked | one window with no menu bar; import is a modal dialog |
| Capture One | Cmd+W / Cmd+Q; Alt+F4 / Ctrl+Q on Windows | quits on Windows | viewer windows go with it | not checked | viewer windows have no menus of their own |
| Photoshop | Ctrl+W closes a document; Ctrl+Q quits | on macOS the app stays with its Home screen | (documents, not windows) | asks for each unsaved document | one menu bar (global on macOS) |
| VS Code | Ctrl+W closes an editor; Ctrl+Shift+W the window; Ctrl+Q quits (Alt+F4 on Windows) | quits on Linux and Windows; stays on macOS | windows are peers: the others stay | asks before ending running terminal processes | the full menu bar in every window; global on macOS |
| Firefox | Ctrl+W tab, Ctrl+Shift+W window, Ctrl+Q quits (no Ctrl+Q on Windows, "following the OS convention") | quits on Linux and Windows; stays on macOS | windows are peers | warns before cancelling downloads | browser windows have the full menus; the Library window has a small menu of its own |
| GIMP | Ctrl+W closes an image view, Ctrl+Q quits | an empty image window stays; closing it quits | (multi-window: image windows are peers) | lists the unsaved images before quitting | image windows have menus; docks have none |

| Guideline | What it says |
|-----------|--------------|
| macOS HIG | Cmd+W closes the window, Cmd+Q quits. One menu bar, for the window in front. Document applications stay open with no window; single-window applications quit with it (Photos, Calendar). |
| GNOME HIG | Ctrl+W closes the window or tab; Ctrl+Q "closes the app, including all its windows". Alt+F4 closes the focused window. Primary windows carry their menu; secondary windows do not. |
| Windows | Alt+F4 closes the active window; File > Exit ends the application. There is no standard quit key, though Ctrl+Q is common. |

The photo applications agree: one main window owns the others, closing
it quits, and secondary windows (a second display, an import, a viewer)
have no menus of their own. The editors and browsers whose windows are
peers (VS Code, Firefox) keep the others when one closes and give every
window the full menus.

### Decisions

**D1. Keys.**

- Ctrl+W (Cmd+W) closes the window in front, whichever it is: the
  Import window as its close button does, the main window as its own
  close does (D3). Alt+F4 and the title bar's × do the same.
- Ctrl+Q (Cmd+Q) and File > Quit end Helios from any window: the
  command closes the main window, not the window it runs in (fixes
  finding 2). On Windows the menu says "Exit", the key stays.
- Escape does not close a focus window: it is not a dialog. It still
  closes dialogs, menus and the palette.
- One command, `window.close`, egui-pop's: Helios's own command of that
  id goes, and Helios installs `WindowsPlugin` (the Window menu; its
  Switch to Window… is not offered, see D5). The darktable keymap's note
  then holds.

Default: as above.

**D2. The last window.** Closing it quits on Linux and Windows. On macOS
Helios stays running with no window, as document applications do: the
Dock icon reopens the main window, Cmd+Q quits. Until a macOS build is
tested, macOS quits as the others do.

**D3. The main window closed while others are open.** Two ways:

- **It quits** (the photo applications' way): the main window is the
  application (the library, the menus, the jobs); closing it asks D4's
  question when work is at stake, then closes every window.
- **Another window takes its place** (VS Code's way, egui-pop's rule
  T2): the most recent other window moves into the main window and the
  application keeps going.

Decided (2026-11-12): **another window takes its place**. Helios stays
open while any of its windows exists: closing the main window with the
Import window open moves the Import window into it, its scan and choices
kept, and its Window menu lists the main window. (The default proposed
here was "it quits", for the Import window is not a full focus; the
owner chose the peers' way.)

**D4. Running work.**

- Closing a window never cancels a job it started. An import or an
  export runs in the core and shows in the main window's Queue panel and
  status bar; the window that started it may close (P3).
- Closing the Import window before Import is pressed cancels its scans:
  they are the window's own reading, not jobs.
- Quitting asks one question, app-modal (T4), only when work would be
  lost: jobs running or queued (imports, exports), a workshop session not
  kept. It lists them; "Quit" cancels them, "Cancel" keeps Helios open.
  Edits need no question: they are written as they are made. Jobs that
  run on a node are not the desktop's: they go on and are not asked
  about.
- The system's end of session asks nothing: the quit goes on within its
  deadline, jobs are cancelled and listed at the next start as
  interrupted (resumed later where the journal allows, T9).
- Later, not now: "Quit when done", which waits for the jobs, then quits.

Default: as above.

**D5. Menus per window.**

- The main window keeps the full menu bar.
- The Import window has **no menu bar**: its commands are its buttons
  and its own keys, as the photo applications' import dialogs and GNOME's
  secondary windows have. Its palette lists its own commands and the
  app-wide ones (D6).
- The alternative: a reduced bar in the Import window (File with Close
  and Quit, Edit, Window), as Firefox's Library window has.

Decided (2026-11-12): **the reduced bar**, then (2026-11-13, the owner)
**the full bar in every window**, as VS Code's windows have: see "Built:
the same bars in every window" below. Every window has a menu bar
with at least a Window menu, so any window can be opened or brought
forward from any other. The Import window's bar has File (Close window,
Quit) and Window: Close window, Helios's windows (Main window, Import
photos…) and the windows open, the one in front ticked. egui-pop's
switcher (Switch to window…, `window.pick`, and `window.switch` in the
palette) is not offered (2026-11-12): the Window menu's list does its
work. A later focus window that is a peer of the main one (slice 7)
gets the full menus, as VS Code's windows do.

**D6. Where keys act.** Three scopes:

| Scope | Examples | Rule |
|-------|----------|------|
| App-wide | Quit, Settings, the palette, Import… (Ctrl+Shift+I) | run in any window |
| The window in front | Close window, Escape, undo in a text field, the Import window's own keys | act on the window they are pressed in |
| The main window | the layout toggles (Ctrl+B, Ctrl+J), views, library and darkroom commands, Export, Ctrl+P | run only when the main window is in front |

Today every command runs in every window (finding 4). The fix belongs
to egui-pop: a command declares whether it is app-wide, and a command
that does not say runs only in the window whose shell declared it (the
main window for Helios's shell). `when: "appWindow == 'main'"` on each
command would do it in Helios alone, but every new command would have
to remember it.

Default: the egui-pop scope, with `appWindow` clauses only for the
exceptions.

**D7. macOS's menu bar.** One native menu bar for the application,
built from the same declared menus and following the window in front:
the main window's menus when it is in front; with the Import window in
front, the application menu (About, Settings…, Quit), Edit and Window,
the main window's commands greyed by D6's scopes. egui-pop builds it
(a native menu crate, as its tray already uses), and the in-window bar
is not drawn on macOS. Until then the in-window bar stays.

Default: as above, after a macOS build exists; nothing to do before.

### Built (2026-11-12)

- **Quit from any window.** egui-pop's `app.quit` (Ctrl+Q, File > Quit)
  closes the window it runs in; the shell follows it in egui-pop's
  journal and quits the application: it closes the main window, and
  marks the core as quitting, so that close ends the process (finding 2).
  Helios's `workbench.quit` (the system menu's Quit, the Import window's
  File > Quit) does the same. With jobs running, one question first
  (`helios.quit.confirmed` runs on its Quit button): D4's question,
  about the jobs only for now.
- **The main window's close** goes through the shell
  (`Shell::main_window_closing`): it quits when the core is quitting or
  no other window is open, and otherwise lets egui-pop move the window
  worked in last into the main window (finding 1). The system's end of
  session quits at once, asking nothing.
- **Ctrl+W** is bound to egui-pop's `window.close`; `WindowsPlugin` is
  installed (the Window menu, without its switcher); Helios's own
  `window.close` is gone. Alt+F4 and the close button were already the
  system's close, which now follows the same rule.
- **Tests** (`tests/import.rs`, real viewports): Ctrl+Q in the Import
  window quits; Ctrl+W there closes it alone; closing the main window
  keeps the Import window, moved into the main one with its Window menu
  listing the main window, and closing it then quits; the last window
  closed quits.

### Built on macOS (2026-11-13)

- **The global menu bar** (D7). `native_menu` lays out egui-pop's
  declared menus (`app.main` and its submenus, their `when` and
  `checked` clauses, their lists) as a native bar does: the application
  menu first (About Helios, Settings… Cmd+,, Services, Hide, Hide
  Others, Show All, Quit Helios Cmd+Q), About, Settings and Quit taken
  out of the other menus, File > New Window first in File, Minimize,
  Zoom and Bring All to Front in Window, which the system also lists the
  windows in. The layout is plain data, unit-tested on every platform.
  `macos` turns it into an `NSMenu` with `muda` (Apache-2.0 OR MIT,
  adopted: it is the menu crate egui-pop's tray already depends on
  through `tray-icon`), lays it out again every quarter second and
  rebuilds the `NSMenu` only when it changed, and runs the command an
  entry names. The in-window bar is not drawn on macOS. Only the
  application menu's keys (Cmd+, and Cmd+Q) are the native menu's; the
  others stay egui-pop's keymap's, so a text field keeps Cmd+C, and the
  native menus do not show them yet.
- **The application kept alive** (D2). On macOS closing the last window
  hides it instead (`LastWindow::StayOpen`, `Closing::Hidden`): Helios
  goes on with no window, its jobs running, until Cmd+Q. The Dock icon
  (an `applicationShouldHandleReopen:` answer added to winit's
  application delegate, which has none), File > New Window and Window >
  Main window bring it back. Linux and Windows still quit with the last
  window; tests set macOS's policy on Linux.
- **The bar follows the window in front** (2026-11-13). Every window's
  pass notes when it is the focused one and wakes the main window's
  pass, which lays the bar out again in that window's context
  (`native_menu::window_context`: its `appWindow`, and `modalOpen` for
  its group) and runs what is chosen there (`Shell::run_in`): Settings…
  chosen with the import window in front edit the import window's focus
  (slice 7). Newer than D7's reduced menus, the bar is the full one in
  every window ("the same bars in every window" below): the main
  window's commands (`appWindow == 'main'`) are greyed, not removed. The
  focus chain's keys stay the main window's, as egui-pop's own context
  does for a group other than the one drawn.
- **Keys in the native menus** (`native_menu::native_key`). An entry
  shows its key only where the native menu may answer it itself (AppKit
  draws a key beside an item only as its key equivalent, which also
  takes the key before the window sees it): one chord with `Ctrl`
  (Command on macOS, as egui-pop's keymap reads it), global, with no
  clause of its own, firing while typing anyway, not a key text fields
  edit with (Cmd+A, C, V, X, Y, Z, the arrows, Home, End, Delete,
  Enter, Tab), and, with a modal open in the window in front, only a
  binding the modal lets through. A key answers one entry, the first in
  the bar; Cmd+H, Cmd+Alt+H and Cmd+M stay the system's. Every other key
  (sequences, keys without Cmd, the editing keys, a surface's keys)
  stays egui-pop's keymap's and is not shown in the native menu.
- **Not yet:** a run on a Mac beyond CI's build: the release's macOS
  job builds and smoke-tests the bundle headless, it does not click
  menus or press keys.

### Built: the same bars in every window (2026-11-13)

The owner, 2026-11-13: the import window's bar showed only "File
Window", with no logo, no menu button and none of the other menus.
Every Helios window now draws the main window's bars.

- **egui-pop** (`WindowSpec::shell_bars`): an application window opened
  with it draws the start of the main title bar (the `CAPTION_START`
  zone: Helios's menu button, logo and name) and the main menu
  (`app.main`) under it, evaluated and run in that window's context
  (`appWindow`). A menu opened as a popup from the window (the menu
  button's system menu) is drawn in it. The bars follow the window into
  the main window when the main window closes. The title bar's end
  (Helios's jobs and decisions items, the layout controls) and the top
  bar's zone (the workspace tabs) stay the main window's.
- **Greyed, not removed.** A command whose `when` holds only in another
  open window is greyed, its tooltip naming that window ("Only in the
  Helios window"). Helios marks its main-window commands with
  `appWindow == 'main'` (`shell::IN_MAIN_WINDOW`): Export…, Copy files
  and Copy the image, which act on the library's selection or the image
  shown. Their keys (Ctrl+Shift+E) do nothing in the import window
  either, as D6 wants for the main window's commands.
- **Routed to the main window.** The layout commands of View (the side
  bars, the panel, arrangements, Zen mode) act on the main window's
  layout, the only one there is, from any window. The app-wide commands
  (Settings, Import…, Storage…, Health check, the palette, About, Quit,
  the Window menu, themes, zoom) run from any window, what they open
  opening in the window they ran in.
- **The window's own entries.** File > Close window (`window.close`,
  Ctrl+W), the import window's File entry before, is in every window's
  File menu, closing the window it is chosen in. Import photos… (File)
  brings the import window forward; the import window's own commands
  stay its buttons.
- **macOS:** the in-window bar is not drawn, as for the main window; the
  native bar follows the window in front (D7, "Built on macOS" above).
- **Test** (`tests/import.rs`, real viewports): the import window's bar
  has the menu button, the logo and the name, and the same top-level
  menus as the main window; Export… is greyed there with "Only in the …
  window"; File > Close window chosen there closes the import window and
  leaves the main one.
- **The Settings window** (2026-11-13): egui-pop's Settings app opens
  its window with the same bars. Test (`tests/settings_window.rs`, real
  viewports): its bar has the menu button, the logo and the name, the
  main window's menus, and Export… greyed there.

**Not built yet: reopening the main window.** egui-pop 0.9 moves a
window into the main one but has no way back: once the main window is
closed, Window > Main window says it comes back when the window in its
place closes. Reopening it needs egui-pop to move the content back to a
window of its own and draw the shell again in the main window.

### Work this leads to

- Reopening a closed main window from any window's Window menu, with
  egui-pop's way back (above).
- Window scope for commands (D6), in egui-pop, then Helios's commands
  marked.
- The quit question (D4), with the jobs and workshop sessions it lists.
- macOS: a click-through on a real Mac (the menus following the window
  in front, their keys); the rest of D2 and D7 is built.

## 8. Multi-window checklist

Every new window passes these checks, opened from that window (not the
main one). The `window-audit` bench job (`tools/bench/window-audit.py`)
runs the probes marked *wbox* on the import window in a headless
compositor; the others are covered by egui-pop's tests on real viewports
(`egui-pop-api/tests/app_windows.rs`, `windows.rs`) or read in the code.

### Checks for a new window

Each opened from the new window, never from the main one:

- Native dialogs (file choosers, the portal) are parented to the window.
- Modals, toasts, confirmations, tooltips, menus and context menus are
  drawn in the window.
- Keyboard focus and Escape act in the focused window alone.
- Closing the window with a modal, a dialog or a background task open
  leaves nothing behind: nothing moves over the main window, nothing
  keeps running for it.
- Size and position are saved per window.
- The agent socket can target the window (`window` in its requests).

Audit of the import window, 2026-11-13:

| Check | Where it lands | How checked | Status |
|---|---|---|---|
| Folder picker (egui-pop's in-app picker, "Choose a folder…") | Was over the main window, hidden behind the import window. Now over the import window, blocking it alone | wbox `picker`; egui-pop tests | Fixed |
| Escape with the picker open | Was lost (the picker was in the other window). Now closes the picker, the import window stays | wbox `escape` | Fixed |
| Closing the window with the picker open | Was: the picker came up over the main window. Now the choice is cancelled | wbox `close-with-picker`; egui-pop test | Fixed |
| System file chooser (rfd, desktop portal) | Not used: Helios builds egui-pop without `native-dialogs`. egui-pop's native path sets no parent window at all | code | Open (egui-pop) |
| Command palette | The import window | wbox `palette` | OK |
| Quick settings (`Ctrl+Shift+Q`) | The window it is asked in, as the palette | Helios test on real viewports (`settings_window.rs`); egui-pop test | OK |
| Settings (`Ctrl+,`) | The Settings window (one instance), editing the focus it was asked in: the import window's, its header saying so; the import window stays open | Helios test on real viewports (`settings_window.rs`); egui-pop test (`admin_focus.rs`) | OK |
| egui-pop dialogs (confirmations) | The window they are opened from; each window has its own queue | egui-pop tests | OK |
| Helios's own modals (consent, connection, picking) | Drawn in the import window's pass | code | OK |
| Closing the window with an egui-pop dialog open | Was: the dialog moved to the main window's queue. Now it is cancelled with the window (a window moved into the main window still takes its dialogs there) | wbox `close-with-dialog`; egui-pop tests (`window_scope.rs`) | Fixed |
| Toasts | The window they are raised from. Was: what the window asked of the shell (Import without a library, a forget or Storage… failing) is answered in the main window's pass, so its toast went to whichever window was active by then, often the main one. Now the shell raises those in the import window's group (the main window's once it closes) | egui-pop tests; Helios test (`import.rs`) | Fixed |
| Tooltips | The window of the widget hovered (egui draws a tooltip in that viewport) | wbox `tooltip` (the folder's row hovered: its path); egui-pop-agent test | Probed |
| Menus, context menus | The window they open in (egui popups per viewport; egui-pop's menu popup follows its window) | wbox `context-menu` (the folder's row right-clicked: "Forget this folder"); egui-pop tests | Probed |
| Shortcuts and keyboard focus | The focused window, with its context (`appWindow`) | egui-pop tests; wbox `palette` | OK |
| A main-window page asked from the window (Storage…, the health check) | In their own window (the default), the Settings window opens on the page. Popped in over the main window: was opened behind the import window; now the main window is asked to the front (the compositor may refuse an activation without a seat) | wbox `storage`, `storage-popped-in`; Helios test (`import.rs`) | Fixed |
| Background task when the window closes (folder read) | Closes at once, the main window draws on | `import-close` job | OK |
| Size and position | Kept per window across runs | egui-pop test | OK |
| Agent socket and test steps | Address each window (`window`) | egui-pop-agent tests | OK |
| Menu bar | The window's own: the main window's title bar start and menus, evaluated in it, the main window's commands greyed (the import and Settings windows) | Helios tests on real viewports (`import.rs`, `settings_window.rs`); egui-pop tests | OK |

For a new window: add it to `window-audit.py`'s probes (or a probe of its
own) so the job checks the same things for it.
