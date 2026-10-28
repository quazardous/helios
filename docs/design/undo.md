# Undo and history on the Helios side: kernel, vault, networks, agents, node

Status: **decided** (2026-10-14): the owner accepted the plan and answered
its open questions (see "Decisions" at the end). Built so far: the first
migration step (the fixes below). The generic part (the contract between
kernel and interface, the focus rule, grouping, the history panel, a
survey of darktable, Lightroom, Photoshop, Blender, VS Code, Krita and
GIMP) is egui-pop's ADR 0014, "Undo: histories owned by the kernel,
routed by focus, projected by the interface" (accepted), and its
analysis. This note covers what Helios
does: which histories it keeps, where, how network edits become steps,
how they are saved with the image's edit, and how agents and nodes
take part.

## What exists today

Read from the code on `main` (2026-10-14), and on the workspace-edit
branch where said.

| History | Holds | Scope | Persisted | Limit | Ctrl+Z | Gaps |
|---|---|---|---|---|---|---|
| Value store (`helios-values::Store`) | `Step { label, origin, changes: [(path, old, new)] }`; gestures (a drag: one step, Escape cancels and leaves none); groups (held arrows and wheel, masks panel edits) | one store per window, every image and the workspace mixed | no | none | `edit.undo` → `HeliosHistory` (egui-pop's `History` contract, scope `edit`) → `Store::undo`; vetoed while a gesture is open (`plugins::History`); also `param.undo` | unbounded; not cleared when another image opens, so an undo may write the previous image's values into paths of the same name; one step per agent write |
| The image's edit (`helios-vault` versions, `EditSession`) | a content-addressed version per committed step (a write, a gesture's end, an undo, a redo, an agent's parameter, a decision answered), a drag every second as "in progress"; parameters by node, marks (labels and locks), the decision book | per image (a line of first parents from a head; two writers make two heads) | SQLite, one transaction per step; survives `kill -9` (survival scenarios gate a release) | none | an undo or redo of the store is saved as a new version; history only grows | the network's shape is not saved (only parameters of the nodes known when the image opened); undo stacks not rebuilt on reopen; the desktop saves every version as `user`, the node as `control`, whoever made the step; nothing lists or checks out versions |
| Network edits (masks panel, `mask.*` commands) | adding a shape, moving, taking out, rewiring: graph calls, no record | per image, in memory | no | n/a | not reachable | not undo steps, not saved; an undo across them can give a group's opacities and inversions to the members now on those ports; taking out a group with members asks for confirmation because it cannot be undone |
| Masks panel values | rename, invert, opacity, lock: store groups | per image | as versions (marks and parameters) | as the store | as the store | fine |
| Library fields (`Vault::apply`) | field commands (rating, reject, pick, labels, tags), one audit row per change (old, new, who, job) | the library | SQLite, audit kept | none | not reachable | no undo; the audit could give one but has no batch id |
| Stacks (`stacks.suggest` decisions) | accepted (whole or corrected) or declined, by whom | the library | vault documents | none | not reachable | no undo; dissolving is the only way back |
| Import and duplicates | a job: files referenced or copied, duplicates skipped, kept or asked | the library | files and catalogue rows | n/a | not reachable | nothing to undo but the catalogue rows; files are never deleted |
| Workspace (workspace-edit branch) | the whole state as one store value `workspace/state`; each edit, reset or parts import one group | the window's workspace | files written at once | as the store | as the store, mixed with image steps | save as template is not a step (it writes a file); panel moves are saved but not steps |
| Workspace, agents (workspace-edit branch, `Files::save_step`) | the state before each agent step, as files | one workspace | files, survives restarts | last 20, no redo | MCP `helios_workspace_undo` only | a second history the desktop neither writes nor reads |
| Decision book | every answer, who, model, inputs | per decision node | inside the version | none | not an undo | fine: a record |
| Bus journal, workflow journal, logs | events, claims | process, vault | ring of 1024; vault documents | | | logs, not histories |

One gap found while reading: a drag held longer than a second and then
given up with Escape has already saved an "in progress" version; the
cancel moves no commit, so nothing is saved after it, and a quit or a
kill at that moment leaves the given-up values at the head. The next
committed step repairs it. Saving a version on cancel when an "in
progress" one was written closes it.

Fixed (2026-10-14), with the two gaps of the table above: the value
store's undo and redo steps are forgotten when another image opens; a
drag saved in progress and then cancelled saves the values it put back
(a version labelled "<gesture> (cancelled)"); each version names who
made its step (`user`, `agent:<id>`, `script:<id>` on the desktop; on a
node, the request's `who` for a decision, else `control`).

On a node, each `set-param` and decision answer is saved as a version
before it is answered; there is no undo verb on control or MCP for an
image's edit, and `batch_start` / `batch_end` make no history.

## Recommendation in one paragraph

Helios's kernel owns three kinds of history behind egui-pop's `History`
contract: **the image's edit** (one scope per image, durable, built on
the version line it already writes), **the library** (one scope, marks
and decisions, from the audit trail), and **the workspace** (one scope
per workspace, durable, the files' undo records shared by the desktop
and agents). A network edit is a step of the image's scope carrying
the structure it changed and the values it moved; a version carries
the whole network, so saving and undoing are the same operation. Agent
requests are groups, attributed in the version. A node's steps are in
the vault; the desktop shows them, never undoes them by Ctrl+Z.

## Where each piece lives

| Crate | What |
|---|---|
| `helios-core` | `history`: the owner (`HistoryService`), the scopes, groups (one per request), the refusal rules; the graph's verbs needed for structural steps (remove a node, a patch type with its inverse) |
| `helios-values` | `Step` gains its scope, place (frame path), group and an optional network patch; a cap per scope; steps cleared per scope when an image closes |
| `helios-vault` | version metadata `step: {kind: do | undo | redo | jump, of}` beside `label` and `origin` (not part of the id); the network in the edit; an audit batch id; snapshots as named refs |
| `helios-template` | the workspace's undo records, written by the desktop too, with redo |
| `helios-gui` | `HeliosHost::history()`; the scopes' focus targets (viewport, parameters, masks → image; grid, filmstrip, stacks, duplicates → library; workspace commands → workspace); the history panel |
| `helios-node` | the same service, headless: undo, redo, jump and groups over control and MCP |

## The image's scope

**A step is a version.** Today each committed step already writes a
version; the step's kind and what it undoes become version metadata
(not hashed, like the label and origin): `do`, `undo of <v>`, `redo of
<v>`, `jump to <v>`. From the line of versions and these markers the
service rebuilds exactly the undo and redo lists on reopen, so Ctrl+Z
after a restart (or after `kill -9`) walks back the same steps. Without
the markers a rebuilt Ctrl+Z would undo an undo.

**Labels and origins kept.** The version's label is the step's label
(message id and arguments, rendered when shown); its origin the step's
(`user`, `agent:<session>`, `control:<client>`, `import:xmp`), no longer
the surface's.

**"In progress" versions** stay in the line (they are what `kill -9`
keeps) and are folded into the gesture's final step in the panel.

**Limits.** In memory, 500 steps per image; older ones are paged from
the vault when the panel scrolls, and Ctrl+Z past them reads them too.
Versions are small text; nothing is pruned automatically.

**Compress.** Versions are content-addressed and each id covers its
parents, so removing a middle version rewrites every later id. A
"compress history" therefore writes one new version with the current
edit, marked `compressed from <v>`, and retires the old line (its
versions kept, hidden, still reachable by id); snapshots are kept
pointing where they were. It is explicit, never automatic, and can be
undone like any jump. Not built for now (decided 2026-10-14).

**Jump and snapshots.** A jump to an older state writes a new version
with that state (`jump to <v>`), so it is a step like any other; a
snapshot is a named ref to a version, listed at the top of the panel,
and gives the viewport's comparison its "before" (the open question
"the unedited image, or a snapshot taken by hand" gets both).

## Network edits

**The step.** A structural edit (create or delete a shape, add, move or
take out a member, rewire) is one step of the image's scope, carrying a
`NetworkPatch`: nodes added (with their module and parameters), nodes
removed (with their whole content, so they can come back), edges
removed and added, and the values it moved (opacities and inversions
the rewiring shifted between ports, labels and locks). The patch has an
exact inverse. The service applies it under the graph's lock and the
store's together, so the viewport, the Masks panel and the store never
see one without the other, then saves one version.

**Consequences.**

- Taking out a group that holds members needs no confirmation: a toast
  says what went, with Undo. Agents and the confirmed command keep
  working as they do.
- The graph needs a remove verb (a deleted member today stays in the
  network, unconnected).
- An undo across a rewiring no longer moves values onto the wrong
  members: the patch moves them back.
- egui-pop's graph widget emits edits without an inverse (`Delete`
  lists ids only); Helios builds the patch from its own graph, not from
  the widget's event.

## Saving a network edited this way with the image's edit

Decided (2026-10-14): A, the whole network in each version, an image
keeping its own once edited. The analysis follows. Today a version carries the
parameters of the nodes known when the image opened, and the network's
shape belongs to the surface (the workspace's network, or the one given
with `--image` or `--graph`). Three ways:

| | A. The whole network in each version | B. A patch on the workspace's network | C. The network as an object referenced by hash |
|---|---|---|---|
| What a version holds | modules, parameters, edges, marks, decisions | parameters plus the structural edits since the workspace's network | parameters plus `network: <hash>`; the network stored once per distinct shape |
| Opening an image | self-sufficient | needs the workspace's network as it was | self-sufficient, one more read |
| The workspace's network changes later | the image keeps its own | the patch may no longer apply | the image keeps its own |
| Size | a few kilobytes per version | smallest | smallest after the first |
| Undo | restore the parent's edit | invert the patch | restore the parent's edit |
| darktable | close: each history item copies the forms list | | |

**Recommended: A now, C later if size calls for it.** A version
becomes a full description of the image's edit, as a version's id
already promises; saving, undo and jump are all "write that edit".
C is an optimisation of A with the same meaning, adoptable without
breaking a version (`network` inline or by hash). B makes every image
depend on the workspace staying as it was, which a parts import or a
reset breaks.

What follows from A (decided with it): an image
first opened takes the workspace's network; once saved, it keeps its
own, and a later change of the workspace's network does not reach
edited images unless the user asks ("Use the workspace's network",
itself one step).

Versions written before the change hold parameters only: they open as
today (the workspace's network, the parameters by node name), and the
next step saves the whole network.

## The library's scope

- Field commands become undoable: each `Vault::apply` batch gets a
  batch id in its audit rows; an undo applies the inverse commands with
  a compare-and-set (each field must still hold the batch's new value),
  otherwise it is refused with "changed since by <who>".
- A stack accepted is undone by dissolving it and reopening the
  proposal; a declined one by reopening the proposal; each one step.
- An import is a job; the library step it leaves (when it finishes) is
  "Import 120 images", undone by removing the catalogue entries it
  added; copied files stay on disk, and the dialog says so. A Keep in
  the Duplicates panel is the same for one file.
- Session or durable: for the session only (decided 2026-10-14), as
  Lightroom; rebuilding it from the audit trail after a restart comes
  later.

## The workspace's scope

One history per workspace, written to its undo records (the files the
agents' tools already use), by the desktop and by agents alike, with a
redo list; the cap stays 20 states or grows (they are small). The
`workspace/state` value leaves the image's mixed stack. Save as
template is not a step (it writes a file elsewhere). Panel moves stay
view state, restored by the context stack, not undone.

## Agents

- **Groups.** Each control or MCP request is one group; an agent may
  open a longer one (`history_group` with a label, closed by
  `history_close` or when its session ends). The group is one step and
  one version, labelled "Agent ‘<session>’: <label> (n changes)".
- **Attribution.** The step's origin is the agent's session, saved in
  the version and shown in the panel and on the parameter (the
  Parameters panel already names who moved a value).
- **Undo only the agent's batch.** From the panel or `history_undo
  {group}`: allowed when no later step touched the group's targets;
  otherwise refused with what touched them, and the panel offers to
  undo both. This is VS Code's rule for a workspace edit.
- **Tools.** `helios_history {scope, from, limit}`,
  `helios_history_undo {scope, group?, if_rev, dry_run}`,
  `helios_history_redo`, `helios_history_jump {scope, version, if_rev,
  dry_run}`, `helios_history_snapshot {scope, name}`; the workspace's
  `helios_workspace_undo` becomes the workspace scope's undo. On the
  desktop they arrive with its inspect and control endpoint (decided
  with the context stack); until then through egui-pop's commands.

## The node and other processes

- A node's steps are versions in the vault with the node's origin; its
  own service undoes them over control and MCP, as above.
- The desktop's Ctrl+Z covers the desktop's steps only. When a version
  from another writer appears on the image shown (another head), the
  history panel shows it as another process's ("Edited by node ‘box’,
  12 changes"), with "Open that edit" and "Keep mine"; the status line
  says "Edits by node ‘box’ are not in this window's undo".
- A batch run on a node is a group per image on the node; reverting it
  from the desktop is an explicit "Revert to before the batch" (a jump,
  one step on the desktop), not Ctrl+Z.
- The desktop does not see another process's events today (the catalog
  version counter is per process); following versions written by a node
  needs the relay to carry `edit.saved`, or a poll of the image's
  heads: part of building this, not a new decision.

## Flows

**K** is `HistoryService`, **S** the value store, **V** the vault,
**UI** egui-pop's projection.

### A slider drag

```
user : presses on Exposure      → S: gesture "Exposure" opens, paths held
user : drags                    → V: "Exposure (in progress)" each second
user : Escape                   → S: old value back, no step; V: a version
                                  restoring the values before the drag
                                  (fixed 2026-10-14, see above)
user : drags, releases          → S: step; K: image scope step, place root
K    : V: version {label "Exposure +0.30 EV", origin user, step do}
UI   : "Undo Exposure +0.30 EV"; the panel folds the in-progress rows
```

### Taking a member out of a mask group

```
user : selects group "Face", Delete
K    : NetworkPatch {remove edges Face→group port 2; members' ports
       shifted: opacity and inversion of port 3 moved to port 2}
K    : graph and store changed under one lock; V: version with the
       whole network, label "Take out ‘Face’"
UI   : toast "‘Face’ taken out" with Undo; no dialog
user : Ctrl+Z → inverse patch; V: version {step undo of <v>}
```

### An agent's batch undone as one

```
agent: helios_history_group {label "Retouch skin"}  → g7
agent: six helios_set_param under g7, then close    → K: one step
V    : version {label "Agent ‘retouch’: Retouch skin (6 changes)",
       origin agent:retouch, group g7}
user : moves Exposure meanwhile (not in g7)
user : panel, g7's row, "Undo this"  → K: g7's targets untouched since
       → undone, one version {step undo of g7's version}
       (had Exposure been in g7: refused, "Exposure changed after it",
        the panel offering to undo both)
```

### A workspace import

```
user : Add from ‘Wedding’…, parts and sides chosen, confirmed
K    : workspace scope step "Add parts from ‘Wedding’"; undo record
       written with the state before; the image reopens through the
       new network
user : Ctrl+Z in the viewport → the newest step among the image's,
       the library's and the workspace's is the import → undone; the
       record's state written back; the image reopens
agent: helios_workspace_undo would have done the same, on the same records
```

### Undo after reopening the app

```
user : three steps on an image, then kill -9 in the middle of a drag
start: V: the head is the drag's last "in progress" version
K    : rebuilds undo [step1, step2, step3, drag (partial)], redo []
       from the versions' step markers
user : Ctrl+Z → the partial drag is undone (one version), then step3…
```

### Ctrl+Z in a text field versus the viewport

```
user : F2 on a member, types "Fac", Ctrl+Z → the field's own undo
user : Enter → step "Rename to ‘Fa’" (a label, in the version's marks)
user : focus on the viewport, Ctrl+Z → the rename is undone
user : in the theme editor, Ctrl+Z → the theme draft (egui-pop's local scope)
```

### Undo while in another place of the context stack

```
user : in sub-workspace s1, sets a parameter → step, place root›s1
user : leaves to root; Ctrl+Z
K    : the image's scope is active in root too → undone, a version
UI   : toast "Undone in s1" with "Go there" (read from the step)
```

## Migration

1. Two fixes on their own, before anything else: clear the value
   store's stacks when another image opens; save a version when a
   gesture that wrote "in progress" is cancelled. Done (2026-10-14),
   with each version's real origin.
2. The step markers, labels and origins on versions; rebuild on
   reopen; survival scenario "undo after `kill -9`".
3. Network edits as steps: the graph's remove verb, `NetworkPatch`, the
   whole network in the version (option A), the confirmation dialog
   replaced by a toast.
4. egui-pop's contract (ADR 0014): `HeliosHost::history()` with the
   image, library and workspace scopes; focus targets.
5. Agent groups and attribution; the `helios_history*` tools.
6. The library's undo (audit batch id); stacks; import.
7. The history panel, jump and snapshots, compare.

Undoable edits added after this note was written are covered by step 3:
the workspace network's `network.*` commands, `node.bypass`, the
workspace's imports (one store step each today), and the clipboard's
pastes (decided in docs/design/clipboard.md, one undo step each). Their
library side (an import's catalogue entries, pasted library images)
belongs to step 6.

## Risks

- **Two truths for the image.** The store's stack and the version line
  must not diverge: the service writes the version inside the step,
  and the rebuild on reopen is checked against the live stack in
  tests.
- **Patches that no longer apply.** A network changed by another writer
  (a second head) is never patched: the session continues its own
  head, as versions already do.
- **Groups left open.** A group dies with its origin (the agent's
  session, the request); `describe history` lists open ones.

## Decisions

The owner's answers to the open questions (2026-10-14), the same as in
egui-pop's ADR 0014:

1. Saving the network: each version keeps the whole network (A); an
   image keeps its own once edited.
2. Library undo: for the session only; restoring it from the audit
   trail after a restart comes later.
3. Ctrl+Z takes the newest step among the active scopes (the image
   shown, the library, the workspace), after a focused text input and a
   focused local scope.
4. Compress history: not for now.
5. The dropped redo states: hidden in the history panel, with a toggle
   that shows them as "replaced" rows.
6. A copy-import undone removes the catalogue entries only, never the
   files.
7. The cap: 500 steps per scope in memory, and a host can change it.
8. The code editor keeps its own local scope.
