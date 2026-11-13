# Generated from Helios's declarations and code by `cargo xtask i18n extract`:
# do not edit. Translations live in i18n/<locale>/ under the same names.

category-calibration = Calibration

category-controls = Controls

category-decisions = Decisions

category-developer = Developer

category-experimental = Experimental

category-masks = Masks

category-node-graph = Node graph

category-preferences = Preferences

category-workspace = Workspace

clip-application-x-helios-images-json =
    .title = images

clip-application-x-helios-mask-json =
    .title = shapes

clip-application-x-helios-nodes-json =
    .title = nodes

clip-application-x-helios-param-json =
    .title = a parameter

command-admin-quick =
    .description = The settings changed often and those changed lately, edited without opening Settings.

command-graph-edit =
    .description = Asks a node editor for an edit, done at its next frame as the user's (a connection type-checked, then its graph.* step): `edit` is `connect` (`from`, `to`, and `replaces`: wires whose end it moves, one edit), `disconnect` (`edges`), `move` (`moves`), `delete` (`nodes`, `edges`) or `add` (`nodes`).

command-graph-export =
    .description = A node editor's graph as an image, drawn from the graph and its layout as the editor drew it last: `format` (svg by default; png or pdf when built with them), `selection` (true: only the selected nodes), `level` (overview, ports or detail; ports by default), `wires` (curves, orthogonal or straight; the editor's by default), `background` (false: transparent), `title`, `scale` (a PNG's pixels per point, 1 by default). Gives `svg` as text, or `png` or `pdf` base64-encoded.

command-graph-read =
    .description = The graph a node editor drew last: nodes (place, ports, group, parameters), wires, groups, the kinds it can add, the selection (`selection`: nodes, `selected_edges`: wires), the level and the part in view.

command-graph-select =
    .description = Selects nodes (`nodes`, their ids) and wires (`edges`: [{"{"}from, to{"}"}]) of a node editor at its next frame: what is named is the selection, nodes and wires; give one of them or both.

command-graph-view =
    .description = Fits a node editor's graph (`fit`: true), zooms to a level (`level`: overview, ports or detail) or focuses a node (`focus`), at its next frame.

command-window-close =
    .description = Closes a window: the one named, or the active one.

command-window-pick =
    .description = Lists the open windows by title; the one chosen comes to the front.

command-window-switch =
    .description = Brings a window to the front: the one named, or the next one.

frame-helios-compare =
    .title = Compare

frame-helios-comparison =
    .title = Compare

frame-helios-culling =
    .title = Culling

frame-helios-darkroom =
    .title = Darkroom

frame-helios-library =
    .title = Library

frame-helios-mask-editing =
    .title = Mask editing

frame-helios-sub-workspace =
    .title = Sub-workspace

frame-helios-workspace =
    .title = Network

menu-app-menu-window =
    .label = Window

menu-helios-file-importFrom =
    .label = Import from

menu-helios-masks-row-add =
    .label = Add

menu-helios-system-profiles =
    .label = Profiles

menu-helios-system-recent =
    .label = Recent workspaces

menu-helios-system-themes =
    .label = Themes

plugin-egui-pop-imaging =
    .title = Image tools

plugin-egui-pop-windows =
    .title = Windows

plugin-helios =
    .title = Helios

plugin-helios-agent-state =
    .title = Agent state

plugin-helios-clipboard =
    .title = Helios's clipboard types

plugin-helios-commands =
    .title = Helios

plugin-helios-config =
    .title = Helios settings

plugin-helios-contexts =
    .title = Helios places and modes

plugin-helios-display =
    .title = Display

plugin-helios-frame =
    .title = Helios frame

plugin-helios-history =
    .title = Helios history

plugin-helios-lists =
    .title = Helios lists

plugin-helios-platform =
    .title = Helios platform

plugin-helios-theme-editor =
    .title = Theme editor

plugin-helios-windowMenu =
    .title = Helios window menu

plugin-helios-workbench =
    .title = Helios areas

setting-catalogue-snapshotEvery =
    .description = Helios takes a verified snapshot of the catalogue this often, keeps it next to the catalogue and copies it to the backup disks plugged in.
    .label = Catalogue snapshot every (hours)

setting-catalogue-snapshotsKept =
    .description = How many catalogue snapshots are kept next to the catalogue and on each backup disk; the oldest go first.
    .label = Catalogue snapshots kept

setting-display-wide-gamut-preview =
    .description = Shows the image in Rec.2020 where the window can carry it (Wayland with colour management), so colours outside sRGB are not clipped. Elsewhere the image stays sRGB.
    .label = Wide-gamut preview

setting-health-destinationDays =
    .description = The health check warns when a disk the backup strategy copies to has not been plugged in for longer.
    .label = Warn when a backup disk is away for (days)

setting-import-automatic-ask-files =
    .description = An automatic import of more photos than this asks first, in a notice that opens the import window. 0: never asks.
    .label = Ask before importing more photos than

setting-import-automatic-ask-gb =
    .description = An automatic import larger than this asks first, in a notice that opens the import window. 0: never asks.
    .label = Ask before importing more than

setting-import-automatic-paused =
    .description = While on, no source imports on its own: cards plugged in and new files in watched folders wait until it is off again.
    .label = Pause automatic imports

setting-interface-experimental-workshops =
    .description = Experimental: the Workshops panel and commands, to find, apply and aim macros by what they are for. May change; what it saves stays a macro. Takes effect at the next start.
    .label = Workshops

setting-interface-graph-zoom =
    .description = Whether the graph view opens on the overview or the full detail.
    .label = Graph view opens on

setting-interface-graph-zoom-choice-detail = detail

setting-interface-graph-zoom-choice-overview = overview

setting-interface-panels-queue-when-empty =
    .description = What the queue panel does when the selected node has no queue.
    .label = Queue panel without a queue

setting-notifications-jobs =
    .description = Shows a desktop notification when a long job (an export, a batch, an import, a backup, a health check) finishes or fails while no Helios window has the focus, whoever started it.
    .label = Notify when a long job ends

setting-project-decisions-mode =
    .description = How decision nodes are resolved by default in this project.
    .label = Decision nodes

setting-project-decisions-mode-choice-ai-assisted = ai_assisted

setting-project-decisions-mode-choice-full-ai = full_ai

setting-project-decisions-mode-choice-human = human

setting-system-ai-calls =
    .description = How many calls to AI models (decision nodes) run at the same time.
    .label = Model calls at once

setting-system-cache-budget =
    .description = Memory the render cache may use before evicting what was used least recently; 0 means a share of the machine's memory (the setting below).
    .label = Render cache

setting-system-cache-share =
    .description = The share of the machine's memory the render cache may use when no size is set above; 0 means no limit.
    .label = Render cache share

setting-system-cache-textures =
    .description = Graphics memory the grids' thumbnails may hold; beyond it, the thumbnails out of view are freed, those seen longest ago first, and read back from the thumbnail cache when they come back.
    .label = Thumbnail textures

setting-system-cache-thumbnails =
    .description = Disk space the library's thumbnail cache (.helios/thumbs beside the catalogue) may use before evicting the thumbnails used least recently.
    .label = Thumbnail cache

setting-system-events-journal =
    .description = How many recent events the bus keeps for inspection and waits.
    .label = Event journal

setting-system-map-defaults =
    .description = Whether the image map leaves out the system's folders, Helios's own data, caches, build trees, hidden folders, trash and other photo applications' libraries; off keeps only the rules above.
    .label = Default blacklist

setting-system-map-exclude =
    .description = Rules added to the image map's blacklist, as a JSON list: each `{"{"}"glob": "**/Private/**"{"}"}` matches the absolute path, or with `"volume": "<id>"` the path inside that volume; a glob starting with `!` re-includes what a default excludes. The last rule that matches wins.
    .label = Places never recorded

setting-system-map-keepMissing =
    .description = How long a file seen but not in the Library stays in the image map once it is missing from its place.
    .label = Missing files kept

setting-system-map-limit =
    .description = How many files seen but not in the Library the image map keeps; past it, those seen longest ago are forgotten first. Library files are never counted.
    .label = Files seen kept

setting-system-map-record =
    .description = What the image map records: everywhere (every listing, and background scans of known volumes), listings (what Helios is shown, plus scans of volumes with a role) or import (only what is imported).
    .label = Image map records

setting-system-map-record-choice-everywhere = everywhere

setting-system-map-record-choice-import = import

setting-system-map-record-choice-listings = listings

setting-system-scheduler-workers =
    .description = How many jobs run at once on this machine; 0 means one per CPU.
    .label = Worker threads

setting-system-workflow-keep-days =
    .description = How long a finished batch (a run of the workflow level) stays in the vault, failures included, before it is removed whole; 0 removes it when the node next starts.
    .label = Finished batches kept
