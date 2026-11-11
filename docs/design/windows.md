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
workspace** (2026-10-11), with auxiliary windows (detached panels)
belonging to their workspace's window. A focus is that workspace, named
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
  the root may flash once. Not observed yet: X11 is blocked (below).

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

**Wayland, headless mutter (box job 20261111-123120-7256): not proven.**
Both windows open and focus events arrive, but neither window paints
past its first frames: no frame counts, and no quit within 60 s in
autopilot. The same happened without autopilot. Bare mutter lists no
windows and focuses none, so wbox's checks were not available. The
suspected cause is the headless compositor sending no frame callbacks
to a second surface, so a present blocks the one UI thread. If that
also happens on a real GNOME session, it is a blocker for option A,
since one stalled surface freezes every window. It must be checked on a
GNOME desktop session before slice 2's API is fixed.

**X11: blocked on the box.** The scenario runs the example under labwc's
Xwayland with `WAYLAND_DISPLAY` unset, but the image lacks
`libxkbcommon-x11`, so winit's X11 backend panics at start. The fix is
to add `libxkbcommon-x11` to the Benchfile's image (Helios
`tools/bench/Benchfile`) and run the same job again.

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
- Before the API is fixed, the mutter stall is investigated on a real
  GNOME session, and X11 runs on the box once the image has
  `libxkbcommon-x11`.

## 6. Open questions

### Product choices (for the owner)

| # | Question | Proposed default |
|---|----------|------------------|
| P1 | Can every focus be closed, the application staying alive? | Closing the last focus quits on Linux and Windows; on macOS the application stays with the welcome window, as macOS applications do. |
| P2 | After Import is pressed, does the import focus close (today) or stay? | It stays, follows its job, then offers "Cull these photos", which turns it into a cull focus scoped to the import; closing it leaves the job running. |
| P3 | A focus closed while its job runs | The job goes on, listed in the jobs window under the focus's name; the focus stays in the recent list with a running badge. |
| P4 | The same photo in two foci | One live edit shared by both windows, each step naming its focus; jobs that write take leases; the darkroom says "also open in 'Edit the wedding'". |
| P5 | Undo of library actions (ratings, tags, imports) | Per focus: Ctrl+Z undoes the focus's own newest library step; the history panel shows the others' steps greyed. |
| P6 | Is a focus a workspace? | Yes: a workspace with an intention and a scope; ephemeral foci (an import) archived once done and kept in the recent list for 30 days. |
| P7 | Word in the interface | "Focus" in menus (File > New Focus…), or "Project"; Helios uses "workspace" for the network today, so one of the two terms should yield. |
| P8 | Quick settings: what shows before typing, and where a change goes | Settings marked `quick`, then those changed lately; a change writes the layer the full settings would, its chip saying which. |
| P9 | Settings window and workspace-layer values | They belong to the focus the window was opened from, named in its header and switchable. |
| P10 | The "Library" focus | Always available (Window > Library) and reopened when nothing else is; it is the whole library's focus. |
| P11 | ROADMAP wording | "One window is one workspace" (2026-10-11) becomes "one window is one focus" once this is decided. |

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
