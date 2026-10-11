# GUI design notes

Status: **draft, not decided** (2026-10-11), from the review of the
mockups. The toolkit is not chosen yet (ROADMAP section 2.1: portable, GPU
accelerated on Linux, Windows and macOS, close to Vulkan). Research behind
these notes: [ux-research.md](ux-research.md).

## Panels

Every panel can be moved, detached into its own window and docked anywhere,
as in VS Code: each user builds the view they want. Layouts are saved per
project workspace and follow the user (configuration, user scope). A
minimal image-first layout stays available for small screens.

## Node header

Every node of a network shows the same header, whatever surface draws it.
The data comes from the framework (the module manifest and the engine's
node status), never from the GUI:

- **family** (a colour mark) and **type**: source, process, macro,
  decision, sink;
- **state**: idle, queued, running, waiting, done, stale, failed;
- **load**: queue depth, running, oldest waiting, retries due;
- **where it runs**: CPU, GPU, a remote node, an AI model.

`helios inspect describe node` already returns these headers for the
renders a node was asked for.

## Queue panel

When a node has a queue (workflow level, see
[architecture.md](architecture.md), "Two levels of execution"), a queue
panel shows it. The panel stays in the same place and follows the selected
node:

- a summary: mode, waiting and done counts, oldest wait, concurrency limit,
  retries due;
- one row per item (an image or anything else): state, since when,
  attempts, who or where (the user, a remote model, a GPU node, the job
  holding a lease).

For a node without a queue the panel stays in place with an empty state by
default; hiding it is a user setting.

## Graph view

- Layout is automatic and layered: nodes never overlap and edges do not
  cross; a node moved by hand pushes its neighbours instead of covering
  them.
- The graph and the darktable-like stack are two views of the same network
  and stay in sync.
- Edits use explicit actions (insert before, insert after, group as macro)
  rather than precise gestures.
