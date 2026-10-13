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

## Doctor

```sh
helios doctor display
```

Says what the display offers the desktop, as JSON: the `session`
(`wayland`, `x11` or `none`); on Wayland, what the compositor's colour
management (`wp_color_management_v1`) announces (`color_management`:
its version, features, named primaries and transfer functions, rendering
intents; `null` when it has none); `error` when the compositor could not
be asked; and `viewport`, the encoding the image is drawn in there with
the Wide-gamut preview setting on, and why. It asks the compositor only:
the running desktop's debug bridge (`helios.display` collector) says what
its window actually resolved. See [Wide gamut](wide-gamut.md).
