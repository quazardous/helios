# The `helios` command line

`helios` talks to a running node (`helios-node`): it reads its state through
the inspect endpoint, drives it through the debug control endpoint, manages
sandboxes, and runs batches. Answers are JSON. Exit codes: 0 answered, 1 the
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

`export NODE --format jpeg|png|tiff [--quality Q] --to PATH` writes a node's
output to a file as a scheduler job holding the destination: two exports to
the same file take turns, and the file appears whole or not at all (an export
cancelled while rendering writes nothing). The export renders the network as
it is when asked, over the node's whole extent unless `--region X,Y,W,H` says
otherwise; `--set NODE.PARAM=VALUE` (repeatable) changes a parameter for this
export only. The answer carries the job id: wait for its
`scheduler.job_ended` event. Paths are made absolute before they are sent.

The control endpoint is served by debug builds only, when the node is
started with `--control` and `HELIOS_DEBUG_CONTROL` is set (a sandbox does
both). `load-source`, `export` and `cancel` are also answered by the batch
endpoint, which every build serves (see below).

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
| `--jobs N` | `2` | exports in flight at once; the node's scheduler runs them side by side |
| `--name TEMPLATE` | `{stem}.{ext}` | the output file name; `{stem}` is the image's name without extension, `{ext}` the format's |
| `--sidecar` | off | applies each image's darktable sidecar (`IMG_0001.CR2.xmp`) on top of the network |
| `--vault FILE` | | with `--sidecar`: keeps each sidecar applied in this vault (see below) |
| `--source NAME` | the only source | the source given each image |
| `--node NAME` | the network's first export, else the node nothing reads | the node exported |
| `--socket ADDR --control ADDR` | | attach to a running node instead of starting one (`--token-file FILE` for TCP) |

Without `--socket`, `helios batch` starts its own node in a temporary
sandbox and removes it when done. For each image it gives the source the
file (`load-source`), asks for the export (`export`) and waits for the job.
A name already taken, by an earlier image or by a file already there, gets
`-1`, `-2`... before its extension: nothing is overwritten.

Each image prints one JSON line on stdout, in the order they finish:

```json
{"path":"/photos/IMG_0001.CR2","output":"/out/IMG_0001.jpg","duration_ms":1840,"error":null,"sidecar":null}
```

With `--sidecar`, `sidecar` reports what the sidecar changed: each module of
the edit goes to the one node of the network running that module, and what
could not be carried over is listed with the reason (a module the network
does not run, or the XMP import's own report). An image without a sidecar
uses the network as it is.

With `--vault FILE` as well, each sidecar applied is kept in that vault:
the image is imported, its darktable edit becomes a version with origin
`import:xmp` (the modules in darktable's pipe order, and the import report
under `import`), and its rating is written only when the vault has none
(darktable's `-1` is a reject), so a sidecar never overwrites a rating
chosen in Helios. The version is a first edit: importing the same sidecar
again adds nothing, and an image already edited keeps its edit beside the
import, as a second head. `sidecar.vault` says what was kept (`image`,
`version`, `added`, `rating`, `rating_set`).

A failed image (a file that does not load, an export that fails) says why
in `error`, and the others go on; the exit code is then 1. Ctrl-C cancels the
exports in flight (an interrupted export leaves no file, not even a part of
one), prints a `cancelled` line for each image not written, and exits with
130.

The batch works with release builds. The node it starts serves the
*batch endpoint* (`helios-node --batch-control PATH`), not the debug
control endpoint: it answers `load-source`, `export` and `cancel` only, on
a Unix socket in a directory made for that run and open to its user alone.
The other control verbs (render, set-param, pause, fault injection,
decisions) stay in debug builds. With `--socket` and `--control`, the batch
drives a running node through either endpoint.
