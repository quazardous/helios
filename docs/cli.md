# The `helios` command line

`helios` talks to a running node (`helios-node`): it reads its state through
the inspect endpoint, drives it through the debug control endpoint, manages
sandboxes, runs batches and imports darktable edits. Answers are JSON. Exit codes: 0 answered, 1 the
node answered with an error, 2 wrong usage, 3 the node cannot be reached.
`helios` with no argument prints the full usage.

## Inspect and control

```bash
helios inspect --socket node.sock wait-idle
helios control --socket control.sock load-source src /photos/IMG_0001.CR2
helios control --socket control.sock export tone --format jpeg --quality 90 --to /out/IMG_0001.jpg
```

`load-source NODE PATH` gives a source node the image of a file (a raw or a
picture); the network stays and its results follow the new image.

`bypass NODE on|off` bypasses a module node or brings it back
(`node.bypass`, the MCP tool `helios_node_bypass`): bypassed, it keeps its
parameters and renders as if absent, its output the input it passes
through. Like `set-param`, it is saved with the image's edit as a step.

`export NODE --format jpeg|png|tiff [--quality Q] --to PATH` writes a node's
output to a file as a scheduler job holding the destination: two exports to
the same file take turns, and the file appears whole or not at all (an export
cancelled while rendering writes nothing). The export renders the network as
it is when asked, over the node's whole extent unless `--region X,Y,W,H` says
otherwise; `--set NODE.PARAM=VALUE` (repeatable) changes a parameter for this
export only. The answer carries the job id: wait for its
`scheduler.job_ended` event. Paths are made absolute before they are sent.

Each export is also a job people follow (`job.*` events on the node's
bus, see [jobs.md](jobs.md)), named by an id such as `export-3`.
`job-cancel ID` cancels a job by that id, as the desktop's jobs centre
does: an export queued or running (it writes nothing), or a whole batch
with its exports.

The control endpoint is served by debug builds only, when the node is
started with `--control` and `HELIOS_DEBUG_CONTROL` is set (a sandbox does
both). `load-source`, `export`, `cancel` and `job-cancel` are also
answered by the batch endpoint, which every build serves (see below).

`workflow-submit --format F --to DIR IMAGES...` submits the images as a
**run** of the workflow level (see [workflow-level.md](workflow-level.md)):
each image is exported through the network by the node itself, claimed
under a lease, retried on a failure that another attempt may change (three
attempts, 2 s then 4 s apart), and followed per queueing node in `describe
workflow` and in `workflow.rows` events. Each output path is decided at
submission, once. Submitting the same network and settings again extends
the same run: the images already in it are left as they are, and the
answer counts the new ones, those already there and those given twice;
`--again` starts a new run; `--jobs N` lets N exports of the run out at
once (the export node's concurrency by default). `workflow-runs` lists the runs with their
counts, `workflow-items RUN [NODE]` one page of a run's items at a node
(the export by default), `workflow-forget RUN` removes a run that is not
at work (its files stay).

The verbs on a run name its items by id (`path:…`, as `workflow-items`
lists them) or by file, and act on every item when given none:

- `workflow-cancel RUN [ITEMS...] [--node NODE]` cancels them (at that
  queueing node, else at every one): their exports in flight are stopped
  and write nothing, the rest are never claimed, done ones stay done.
  Cancelling a whole run is `job-cancel` of its job;
- `workflow-retry RUN [ITEMS...] [--node NODE]` gives their failed and
  cancelled rows the full allowance of attempts again, at once; a finished
  run is worked again;
- `workflow-prioritize RUN top|bottom ITEMS...` moves them before or after
  every other item of the run, in the order items are claimed (those
  running, or done, stay where they are);
- `workflow-pause RUN` stops claiming the run's items (those running
  finish), and the run stays paused across a restart; `workflow-resume
  RUN` works it again.

With `--vault`, the node keeps its runs in that
vault: started again after a crash, it resumes them (each file written
once); a node started on the same vault while another works it lists that
run read-only and takes it over if the other dies; a finished run is
removed after `system.workflow.keep_days` (30 by default); `--runs-vault`
keeps only the runs in that vault (no settings layer, no edit), as
`helios batch --vault` asks of its own node. The batch endpoint answers
every `workflow-*` verb as well.

A refusal answers `{"ok": false, "error": ...}` in English; when the node
says it by key (a file it cannot load, a node it does not have, a job
already ended), a `message` comes beside it (`{"key", "args", "english"}`),
as on a batch's lines.

## Batch

```bash
helios batch --network edit.json --out exported/ IMG_*.CR2
helios batch --network edit.json --out exported/ --format png --jobs 4 --name '{stem}-web.{ext}' --sidecar *.NEF
```

Applies a network to many images, each exported to a file in `--out`:

| option | default | |
|---|---|---|
| `--network FILE` | | the network (see `helios-node`); its own `renders` and `exports` are left out |
| `--out DIR` | | where files are written; created if needed |
| `--format jpeg\|png\|tiff` | `jpeg` | |
| `--quality Q` | `92` | JPEG quality, 1 to 100 |
| `--jobs N` | `2` | exports of the run out at once; the node's scheduler runs them side by side |
| `--name TEMPLATE` | `{stem}.{ext}` | the output file name; `{stem}` is the image's name without extension, `{ext}` the format's |
| `--sidecar` | off | applies each image's darktable sidecar (`IMG_0001.CR2.xmp`) on top of the network |
| `--vault FILE` | | its own node keeps the run in this vault, so the same batch run again finishes it; with `--sidecar`, keeps each sidecar applied there too (see below) |
| `--again` | off | a new run, even if the same batch has one: every image exported again, under new names |
| `--source NAME` | the only source | the source given each image |
| `--node NAME` | the network's first export, else the node nothing reads | the node exported |
| `--socket ADDR --control ADDR` | | attach to a running node instead of starting one (`--token-file FILE` for TCP) |

Without `--socket`, `helios batch` starts its own node in a temporary
sandbox and removes it when done. It submits the images as one **run** of
the workflow level (`workflow.submit`, see
[workflow-level.md](workflow-level.md)) and follows it: the node exports
each image itself, retrying a failure another attempt may change (three
attempts, 2 s then 4 s apart), and the batch prints an image's line when
its export ends. Each output name is decided when the run gets the image,
once: a name already taken, by an earlier image or by a file already there,
gets `-1`, `-2`... before its extension, and nothing is overwritten.

The run's id follows from the network and the export settings (`--out`,
`--format`, `--quality`, `--name`, the node exported and its source), so
the same batch run again extends the same run: an image it already
exported is not exported again (its line comes at once, with the file
written then), a new image is added. `--again` starts a new run. A run
lasts as long as its node: on a node attached to, while that node runs; on
the batch's own node, with `--vault FILE`, in that vault, so the same batch
run again after an interrupt or a crash finishes what was left; without
`--vault`, it ends with the batch.

On a terminal, stderr also shows the run's progress per queueing node
(done, failed, running, waiting, asking for a person).

Each image prints one JSON line on stdout, in the order they finish (an
image given twice is one item, with one line):

```json
{"path":"/photos/IMG_0001.CR2","output":"/out/IMG_0001.jpg","duration_ms":1840,"error":null,"sidecar":null}
```

An image that failed has its `error` in English and, when Helios said it
by key, a `message` too (`{"key", "args", "english"}`): a client shows it
in its user's language from Helios's catalogues
(`i18n/<locale>/messages.ftl`).

With `--sidecar`, `sidecar` reports what the sidecar changed: each module of
the edit goes to the one node of the network running that module, and what
could not be carried over is listed with the reason (a module the network
does not run, or the XMP import's own report). An image without a sidecar
uses the network as it is.

A history item Helios cannot apply (a module it does not have, a params
version it cannot read, a blend or mask, a second instance of a module
darktable runs once, an entry it cannot read) never stops the import and
never moves the items around it. It is listed in `sidecar.import.not_applied`
with its operation, instance, version and reason, and one line sums them up,
in `sidecar.warning` and once on stderr:

```text
helios batch: /photos/IMG_0001.CR2.xmp: 4 of 9 history items not applied, kept for later: flip v2, colorout v5, gamma v1 (no Helios module); exposure v6 (blending not supported)
```

`sidecar.warning_message` holds the same line by key (`xmp-not-applied`,
each reason with its own key), for a client to show it translated.

With `--vault FILE` as well, each sidecar applied is kept in that vault:
the image is imported, its darktable edit becomes a version with origin
`import:xmp` (the modules in darktable's pipe order, the import report
under `import`, and under `darktable` the sidecar's text and each of its
history items with its raw params and what became of it: `applied` to a
node, `not_applied` with the reason, `disabled`, `replaced` by a later
item, or `undone` past the history end), and its rating is written only when the vault has none
(darktable's `-1` is a reject), so a sidecar never overwrites a rating
chosen in Helios. The version is a first edit: importing the same sidecar
again adds nothing, and an image already edited keeps its edit beside the
import, as a second head. `sidecar.vault` says what was kept (`image`,
`version`, `label`, `added`, `rating`, `rating_set`).

**Duplicates.** darktable keeps its duplicates of a photo (its versions)
as `IMG_0001_01.CR2.xmp`, `IMG_0001_02.CR2.xmp`... next to the base
sidecar `IMG_0001.CR2.xmp`. The batch lists each folder once and finds
them by name (see [Darktable import](#darktable-import) for the rules).
The base sidecar is the edit exported; `sidecar.duplicates` lists the
others (`path`, `version`, `exact`), and with `--vault` each of them is
kept too, as its own version (`vault`, `warning`, or `error`).

## Darktable import

```bash
helios import-darktable --vault library.db ~/Pictures/2024-06-trip/
helios import-darktable --vault library.db IMG_0001.CR2 IMG_0002.CR2
```

Keeps darktable edits in a vault without exporting anything. Each path is a
folder (its files, not its subfolders) or an image file. Every image file
is imported, and each of its sidecars becomes a version of it with origin
`import:xmp`, as with `batch --sidecar --vault`:

- the base sidecar (`IMG_0001.CR2.xmp`) is labelled `import:xmp` and gives
  the image its rating when the vault has none;
- each duplicate (`IMG_0001_01.CR2.xmp`) is labelled
  `import:xmp duplicate 01`, with darktable's name for it in parentheses
  when it has one. A duplicate is a version of its own, even when its edit
  is the same as another's: duplicates are never merged;
- a case variant (`IMG_0001.CR2.XMP`, `IMG_0001.cr2.xmp`) is kept too, as
  another version of the same number, after the one spelled as darktable
  spells it, which stays the base; `conflicts` lists the numbers that have
  more than one sidecar;
- a name that is both an image's own sidecar and a duplicate of another
  image (`DSC_01.NEF.xmp` when `DSC_01.NEF` and `DSC.NEF` both exist) goes
  where its `xmpMM:DerivedFrom` says, else to the image of that exact
  name. darktable's suffix is the number as it prints it (`_01`, `_100`):
  a camera's `IMG_0001` is never taken for duplicate 1 of `IMG`.

One JSON line per image, with its sidecars (`path`, `version`, `exact`, and
`vault` or `error`, and the import's `warning`), then one line per sidecar
whose image is not in the folder:

```json
{"image":"/trip/IMG_0001.CR2","id":"…","sidecars":[{"path":"/trip/IMG_0001.CR2.xmp","version":0,"exact":true,"vault":{…}}],"conflicts":[]}
{"image":"/trip/IMG_0002.CR2","id":"…","sidecars":[],"conflicts":[]}
{"orphan":"/trip/IMG_0003.CR2.xmp","image":"IMG_0003.CR2","version":0}
```

An image without a sidecar is imported with no version. An orphan is
listed, not imported (the vault keeps images, and its image is not
there); its `image` comes from its `xmpMM:DerivedFrom`, else from its
name. Importing again adds nothing new. A sidecar that cannot be read
does not stop the others: it gets an `error`, and the exit code is 1.

On the node, the run is a job people follow (kind `batch`, its exports
as its children, see [jobs.md](jobs.md)): `helios control job-cancel
run-N` on that node cancels the run, its exports in flight and the images
not exported yet, which are reported `cancelled`, and the batch exits with
130.

A failed image (a file that does not load, an export that failed its
attempts) says why in `error`, and the others go on; the exit code is then
1. A run that can only go on with a person's answer (a decision of the
network nobody answered) or that is paused does not keep the batch
waiting: the images left get an `error` (`batch-asking`,
`batch-run-paused`), and the exit code is 1.

Ctrl-C cancels the batch's images in the run (`workflow-cancel`: an
interrupted export leaves no file, not even a part of one; images another
client added to the run go on), prints a `cancelled` line for each image
not written, and exits with 130. With `--vault` and its own node, it stops that node
instead: each image not written gets a `batch-stopped` line, the run stays
in the vault, and the same batch run again finishes it. A batch that dies
without Ctrl-C leaves a run on a node it attached to going.

The batch works with release builds. The node it starts serves the
*batch endpoint* (`helios-node --batch-control PATH`), not the debug
control endpoint: it answers `load-source`, `export`, `cancel`,
`job.cancel` and the runs' `workflow.*` only, on a Unix socket in a
directory made for that run and open to its user alone.
The other control verbs (render, set-param, pause, fault injection,
decisions) stay in debug builds. With `--socket` and `--control`, the batch
drives a running node through either endpoint.

## Import and stacks

```bash
helios import --vault library.db [--reference | --copy [--into DIR]] [--rename PATTERN]
              [--duplicates skip|keep|ask] [--no-signals] FOLDERS-CARDS-OR-FILES...
helios stacks --vault library.db list | create IMAGE... [--top IMAGE] | dissolve STACK
              | expand STACK | collapse STACK | top STACK IMAGE
              | suggest [IMAGE...] [--resolver NAME] [--window S] [--distance N]
              | accept DECISION [--groups N,N...] | decline DECISION | decisions
```

`import` brings a folder, a file or a mounted card into the vault,
referenced in place or copied into the library folder (renamed by a
pattern, sidecars along), finds exact duplicates by content and handles
them by policy, and measures each image's near-duplicate signals. One JSON
line per file, then `{"report": ...}`; exit 1 when a file failed, 130
when interrupted (Ctrl-C stops it before its next file and leaves no
half-written copy).

`stacks` lists, makes and dissolves stacks, and asks for stack
suggestions, which are recorded as decisions and applied only once
accepted. Its answers are JSON. See [library.md](library.md) for both.

## Doctor

```sh
helios doctor display
```

Says what the display offers the desktop, as JSON: the `session`
(`wayland`, `x11` or `none`); on Wayland, what the compositor's colour
management (`wp_color_management_v1`) announces (`color_management`:
its version, features, named primaries and transfer functions, rendering
intents; `null` when it has none); `error` when the compositor could not
be asked; and `viewport`, where the image goes there with the Wide-gamut
preview setting on: its `route` (`tagged`, a surface of its own that
Helios tags through the compositor; `wsi`, one in a colour space the GPU
driver declares; `srgb`, the window's surface), its `encoding` and the
`reason`. It asks the compositor only, so the route is a prediction: the
running desktop's debug bridge (`helios.display` collector, its `route`)
says what its window actually negotiated. See
[Wide gamut](wide-gamut.md).

```sh
helios doctor ai
```

Reports the AI models of this machine, as JSON: each provider (its kind,
URL, capabilities, whether it is remote, whether it answers, where its
key is, never the key), the routing map with, for each capability and
each prompt, whether it is available and why not (nothing routed, a
provider that does not answer, a model not pulled, a model without that
capability, a key not found), the local runtime (installed, running, its
version and models), the privacy settings and where the AI files are. An
agent sets all of this up through the `helios_ai_*` MCP tools. See
[AI models](ai-models.md).
