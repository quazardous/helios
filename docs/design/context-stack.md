# The context stack on the Helios side: kernel, bus, vault, node, MCP

Status: **design note, not decided** (2026-10-13). Nothing here is built.
The generic part (what a frame is, the three architectures compared, the
contract between kernel and interface) is egui-pop's ADR 0012, "The
context stack: owned by the kernel, projected by the interface"
(proposed). This note covers what Helios has to do: who owns the stack,
which bus carries it, where it is stored, how a node and an agent reach
it, and the flows that matter here.

## Why Helios needs it

- **Sub-workspaces.** Delegating part of a network to a sub-workspace
  means entering it switches the whole projection (network scope, panels,
  keys, toolbox) and leaving it restores the parent's.
- **Work orientations.** Culling, retouch and review change keys, panels
  and tools for the whole window. Today the culling template does it by
  hand: its keymap goes into the `Workspace` layer with
  `when: "focusedPanel == 'helios.viewport'"`.
- **Coming back the same.** ROADMAP section 2.4 wants restorable work in
  progress (darktable restores neither the open image nor the culling
  set). The stack, with its view state, is that session state.

What exists today, none of it stacked or published:

- when-context keys set every frame by `Shell::set_context`
  (`focusedPanel`, `maskTool`, `heliosProfile`, …), flat;
- one window is one workspace and one process; `Shell::switch_workspace`
  saves the layout, sets the workspace key and loads the new layout;
- egui-pop arrangements, with Helios's image-first layout;
- quick open's `module:`, `image:` and `workspace:` targets, the nearest
  thing to places;
- mode-like toggles kept as view values or actions: compare and split
  (`view/viewport/compare`), the mask tool (`view/masks/tool`), the
  scopes mode, culling's auto-advance.

## Recommendation in one paragraph

The context stack is a **kernel service with one owner**, in
`helios-core`, reached through egui-pop's `Contexts` contract on the
desktop and through the control endpoint on a node. It is **not** a
fold of the bus: Helios's bus carries facts published by owners, never
commands, and its journal is a ring in memory. Its transitions are
published as a typed family of events, `context.*`, each carrying the
whole new state and a revision, so every reader (the interface, a
relay, an agent, a test) folds them trivially and a late reader takes a
snapshot then subscribes from its sequence number. The owner writes the
stack before publishing, so a `kill -9` keeps the last transition.

## Where each piece lives

| Crate | What |
|---|---|
| `helios-core` | `context`: the owner (`ContextService`, `Send`, behind a lock), the frame registry Helios fills, the kernel guards and holds, publication on the bus. Uses egui-pop-model's pure machine if the dependency is accepted (open question 1), or its own of the same shape. |
| `helios-vault` | the stored stack: a document per workspace (namespace `context`, key the workspace key), written in the transition's transaction. |
| `helios-values` | each undo `Step` gains the frame path it was made in. |
| `helios-gui` | `HeliosHost::contexts()` adapts the service to egui-pop's `Contexts`; the frames Helios declares (sub-workspace, orientations); the view-state document next to the layout; the relay follows `context.*` from nodes. |
| `helios-node` | the same service, headless; control requests and inspect reads; MCP tools. |
| `helios-inspect` | `describe context`; `subscribe` with the prefix `context.` (the `subscribe` request being added). |

## The kernel side

### The owner

```rust
// helios-core::context (pseudo-signatures)
pub struct ContextService { /* state, registry, holds, vault, bus */ }

impl ContextService {
    pub fn open(workspace: &WorkspaceKey, vault: &Vault, bus: Arc<Bus>,
                registry: FrameRegistry) -> (Self, RestoreNotes);
    pub fn state(&self) -> ContextState;                  // snapshot, with rev
    pub fn request(&mut self, r: Envelope, origin: &Origin) -> Answer;
    pub fn hold(&mut self, h: Hold, origin: &Origin) -> Result<HoldToken, String>;
    pub fn release(&mut self, t: HoldToken);
}
impl Describe for ContextService { /* state, holds, last transitions */ }
```

`request` runs, in order and under the service's lock:

1. `if_rev` check (a stale request is answered `Stale`, never applied);
2. the registry check (the frame exists, its payload is valid: the
   sub-workspace exists);
3. guards: the frame's `can_enter` and `can_leave`, and the holds;
4. the pure transition (egui-pop-model's `apply`), unless `dry_run`;
5. the vault write of the new stack, in a transaction;
6. `bus.publish("context", cause, ContextChanged { rev, state, entered, left })`;
7. the answer.

A refusal publishes `context.refused` and changes nothing. Events are
typed (`impl Event`, kinds `context.changed`, `context.refused`,
`context.restored`, `context.held`, `context.released`), with the
control action's sequence number as `cause` when the request came from
the control endpoint, as for every other control action.

### Guards Helios has

| Guard | Source | Effect |
|---|---|---|
| a parameter drag in progress | the value store's gesture (kernel side: `helios-values` locks the gesture's paths) | the orientation and the places are held until the gesture ends; the interface places no hold for it |
| a tab or panel drag | egui only knows it | the interface places a hold through the contract |
| a batch running "in review" | the batch places a hold on `orientation` at `batch_start` and releases it at `batch_end` (or when the orphaned batch is ended) | an orientation change is refused with the batch's job id |
| an agent holds the image (sandbox, decision in progress) | the agent's session | the "agent is driving" mode is set (transient) and places are held |
| a deleted sub-workspace | the registry check | `enter` is redirected to the parent; on restore the place is cut |

Holds belong to an origin and die with it: a gesture's when it ends or
is cancelled, a batch's when its job ends, a remote client's when its
link is lost.

### Which bus

The desktop has several buses today: the shell's (`gui.*`, `job.*`,
`param.changed`, read back by egui-pop as its journal), the one given to
`Persist` and the vault (`vault.*`, `edit.saved`), and the viewport
engine's. `context.*` goes on the **shell's bus**: it is the one egui-pop
reads, the one the relay republishes node events onto, and the one an
inspect endpoint on the desktop would serve. On a node there is one bus
per process; `context.*` goes there.

`HeliosHost::new` starts a waker thread that wakes the interface for any
event not starting with `gui.`; `context.*` therefore wakes it, which is
what a transition requested by an agent needs.

### The vault

- Document `context` / `<workspace key>`: `{ version, rev, places,
  modes, payloads }`, a versioned schema (egui-pop-model's `context`
  schema). Holds and transient frames are never written.
- Written in step 5 of `request`, in its own transaction; a transition
  that comes with a content change (create a sub-workspace and enter it)
  writes both in one transaction.
- The SQLite driver runs WAL with `synchronous=NORMAL`: a committed
  transaction survives a process kill (the operating system has it), not
  necessarily a power loss, like every other vault write.
- Today the workspace's own files (`workspace.json`, the template copy,
  the layout) live in the configuration directory, not in the vault.
  Putting the stack in the vault buys transactions and node access;
  putting it in `workspaces/<key>/context.json` keeps the workspace in
  one place. Open question 2.

### The view state

Focus, scroll, panel sizes, the viewport's zoom and pan, detached
windows: kept by egui-pop's projection per frame path, in a `view`
document next to the layout (`ui/workspaces/<key>/view`), written about
a second after the last change and on each transition. This needs the
debounced writer egui-pop's ADR proposes, which also closes an item
`persistence.md` leaves open: the layout is written only at exit, so a
kill loses the session's layout changes.

## Agents and MCP

- **On a node**, MCP already forwards to the node's sockets. Two tools:
  - `helios_context` (inspect): the state, the holds, the registry;
  - `helios_context_request` (control): `{request, if_rev, dry_run}`,
    answered with the typed `Answer`. Changing the orientation is not a
    debug-only action, so it belongs in the scope served in every build,
    like `batch_start` (open question 4).
  - `helios_wait_event` with `context.changed` or `context.released`
    covers "wait until I may switch"; `subscribe` with the prefix
    `context.` covers a long-lived mirror.
- **On the desktop**, MCP does not reach the process today: agents use
  the command registry (`Origin::Agent`) and egui-pop's debug bridge.
  The bridge gains a `context` collector (state, holds, last
  transitions; actions `request` and `dry_run`), which goes through the
  same `request` as a key press. A desktop inspect and control endpoint
  is a separate decision (open question 3); the flows below assume one
  only where they say so.
- **An agent driving** is a transient mode (`helios.agent-driving`), set
  when an agent takes control and shown as a chip with a way out; it is
  never restored.

## Flows

**K** is `ContextService`, **B** the shell's bus, **UI** egui-pop's
projection, **V** the vault.

### The user enters a sub-workspace

```
user : double-click the delegated group in the network, or `nav.enter`
UI   : step context.request {enter helios.sub-workspace {id: s1}, if_rev 7}
UI   :   before-hooks: no unsaved input → Go
UI   : HeliosHost::contexts().request(...)  (in process: Reply::Done)
K    : registry: s1 exists; guards: no gesture, no hold
K    : V: write context/<ws> rev 8
K    : B: context.changed {rev 8, places: [root, s1], entered: [s1]}
K    : the network scope is now s1's nodes (kernel state: what may be edited)
UI   : same frame: save root's view state; load s1's layout document
UI   :   (workspaces/<key>/sub/s1/layout) and its keymap layer;
UI   :   keys place=helios.sub-workspace, placeDepth=1; crumb "Main › s1"
```

Leaving is the same with `leave`; root's layout and view state come back
as they were.

### An agent switches the orientation while the user drags (desktop endpoint assumed)

```
user : drags the exposure slider → helios-values begins a gesture
K    : the gesture is a kernel-known guard (no interface hold needed)
agent: helios_context_request {set_mode orientation=review, if_rev 8}
K    : guard: gesture in progress → Refused {by: "user gesture", reason}
K    : B: context.refused {request, by, rev 8}
agent: helios_wait_event context.released, or param.changed with the
       gesture's end, then retries
user : releases → gesture ends → K: context.released
agent: retries {if_rev 8} → Applied {rev 9} → context.changed → UI
```

### Two windows

Helios is one window per workspace per process. Two cases:

- **A detached panel** (an auxiliary viewport of the same process)
  shares the stack and the when-context; one projection pass updates
  both, and a panel the new arrangement lacks closes, remembered in the
  left frame's view state.
- **Two workspaces in two processes** have two stacks, two owners, two
  vault documents: independent by design.
- **The same workspace opened twice** would give two owners of one
  document. The vault write is guarded by `rev` (compare-and-set: a
  write whose base `rev` is not the stored one fails), so the second
  process gets `Stale` and reloads; better, opening a workspace already
  open focuses the other process (open question 5).

### A node running a batch in "review"

```
agent: helios_context_request {set_mode orientation=review}  (node)
K    : applied, V written, B: context.changed rev 3
agent: helios batch …  → batch_start → the batch holds `orientation`
K    : B: context.held {token: batch-12, groups: [orientation]}
desktop (following the node through the relay):
       the relay republishes context.* as node:<name>/context.*;
       the desktop shows a chip "node: review, batch running"; it does
       not adopt the node's stack (another owner)
agent: set_mode orientation=culling → Refused {by: batch-12}
K    : batch_end (or the orphaned batch ended) → context.released
```

What "review" changes on a headless node (which decisions it asks, what
it exports) is for the owner to define (open question 6); the stack
itself works the same without a window.

### `kill -9` and reopen

```
user : enters s1 (rev 8), sets culling (rev 9), pushes compare (rev 10)
K    : each transition wrote V before publishing
UI   : view document written ~1 s after each change
kill -9
start: ContextService::open reads context/<ws> rev 10
K    : registry: s1 exists, culling and compare are declared → all kept
K    : B: context.restored {rev 10, dropped: []}
UI   : projection of rev 10, then s1's view state: same crumb, chips,
       panels, zoom
```

A survival scenario covers it in `crates/helios-gui/tests/survival.rs`,
gating the release like the others; a node scenario does the same with
a mode set through the control endpoint. If s1 was deleted meanwhile,
the stack is cut at root, culling and compare stay, and a toast says
what was dropped.

### Undo across a transition

```
user : in s1, changes a parameter → Step 41 {path root›s1, origin user}
user : leaves to root (rev 11)
user : Ctrl+Z → HeliosHost::undo
host : Step 41's path is root›s1, the current path is root
       → undo it (a new version in the vault, as today) and toast
         "Undone in s1" with "Go there"
```

Transitions are not undo steps (egui-pop's ADR, open question 1 there).
Versions are unaffected: an undo is still a new version.

## Migration

1. egui-pop delivers the model, the contract, `LocalContexts`, the
   projection and the debounced writer (its ADR).
2. `helios-core::context` with the registry, guards, holds, events and
   the vault document; unit tests without a window.
3. `HeliosHost::contexts()`; the orientation modes; the culling
   template's keys move to `when: "mode.orientation == 'culling' &&
   focusedPanel == 'helios.viewport'"`, and a template may declare the
   orientation a new workspace starts in.
4. The survival scenario; `describe context`; the node's control
   request and MCP tools; the relay follows `context.*`.
5. Sub-workspaces as places, with their own layout and keymap layer.
6. Decide, case by case, which current toggles become frames: compare
   is a natural pushed mode; the mask tool may stay view state.

## Risks

- **Two truths**: Helios keeps setting mode-like keys every frame in
  `Shell::set_context`. Every key that becomes a frame is removed from
  there in the same change.
- **The wrong bus**: publishing `context.*` on the vault's bus would
  hide it from egui-pop and the relay. One place publishes, the shell's
  bus.
- **A held group nobody releases**: holds die with their origin; the
  `context` collector and `describe context` list them.
- **Restore depending on plugins**: a mode from a disabled plugin is
  dropped on restore and comes back only if the user sets it again.

## Open questions for the owner

1. May `helios-core` depend on `egui-pop-model` (no egui, MIT or Apache)
   to share the pure machine, or does Helios keep its own, as
   `helios-shell` keeps its own `when`?
2. The stack's home: a vault document (transactions, node access) or a
   file in the workspace's configuration directory, next to its layout?
3. Should the desktop serve an inspect and control endpoint, so MCP
   reaches it as it reaches a node?
4. In which control scope does `context.request` belong: every build
   (like batch) or debug only?
5. Opening a workspace already open in another process: focus that
   process, or allow two owners guarded by `rev`?
6. What does an orientation mean on a headless node: does "review" on a
   node change what it does, or is it only a label read by clients?
7. Should a desktop following a node be able to adopt the node's stack
   (drive its orientation), or only show it?
