# GUI design notes

Status: **draft** (2026-10-11), from the review of the mockups. The toolkit
is egui on wgpu (decided 2026-10-11, ROADMAP section 2.1; the comparison
with iced is in [gui-prototype.md](gui-prototype.md)). Research behind
these notes: [ux-research.md](ux-research.md).

## Panels

Every panel can be moved, detached into its own window and docked anywhere,
as in VS Code: each user builds the view they want. Layouts are saved per
project workspace and follow the user (configuration, user scope). A
minimal image-first layout stays available for small screens.

## Settings pages

Settings pages are generated, never laid out by hand: a setting named
`page.group.name` appears on that page, in that group (see
[architecture.md](architecture.md), "Configuration"). Its widget comes from
its normalized input, the same description a module parameter has, so one
widget library draws both, and each widget shows where the effective value
comes from (default, global, project, user, run).

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

### Principal nodes, stacks and semantic zoom

A network quickly grows past what a screen shows. Some nodes are principal
because the network turns around them; the others are stacked under them.

- **Overview** (zoomed out): only principal nodes and the links between
  their groups. Each principal node carries a stack badge (`+3`) and a header
  that aggregates its stack: the worst state (failed > waiting > stale >
  running > queued > done) and the total queue.
- **Detail** (zoomed in): stacks unfold into the full network. Zooming moves
  smoothly from one to the other; a single stack can also be unfolded in
  place.
- **Principal nodes are proposed automatically**: sources, merges (two
  inputs or more), decision nodes, sinks and the outputs of the network.
  Every other node is stacked under the principal node it leads to (its
  first consumer's group). The framework computes this (`Graph::layout`,
  shown by `helios inspect describe graph` as `layout`), with each node's
  rank for a layered layout, so every surface draws the same grouping.
- **Always overridable by hand**: pin a node as principal, fold one into a
  stack. These choices are a view: saved in the project workspace (user
  scope of the configuration), never in the network itself.
- A stack can be **promoted to a macro**, a real module made of a network
  (AVS style, see [module-contract.md](module-contract.md)), when it is
  worth reusing.
