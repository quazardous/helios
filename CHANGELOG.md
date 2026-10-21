# Changelog

All notable changes to this project will be documented in this file.

> This is a curated, human-readable record -- **not a commit log**. Each
> entry says *what changed and why it matters to a user*, in plain
> language, not *how* it was implemented. Skip internal refactors.
>
> **House style** for editors:
> - One short bullet per change. Multi-paragraph entries are only for
>   the major changes a user really needs to read in full.
> - No internal tracker IDs (`#NNN`, `PROJ-123`) unless that tracker
>   has a public link -- they're noise otherwise. Mention the change,
>   not the ticket.
> - **Version bump = SemVer**: any `### Added` entry is at least
>   MINOR; `### Fixed` alone is PATCH; breaking change is MAJOR
>   (MINOR before 1.0).
>
> **This file is generated at release** from the fragments in
> `changelog.d/`, written with each change (see CONTRIBUTING.md). Do not
> edit it by hand between releases; `cargo xtask changelog` previews the
> next section.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.22.0] - 2026-10-21

### Added

- Part of a workspace's network can be delegated to a named sub-workspace, nested if wanted, and deleted again, each as one undoable step with a dry run; templates and workspace edits carry them, and importing a network copies its sub-workspaces. Entering a sub-workspace comes in a later release.

## [0.21.0] - 2026-10-21

### Added

- A third built-in workspace template, "Develop (preview)", sets exposure and
  the sigmoid tone curve on a picture, with the image large and its
  parameters and scopes beside it. A raw file is developed from its embedded
  preview for now, not from its sensor data.
- Workspace templates opened on the desktop may now use the sigmoid tone
  mapper, which maps scene light to the display's range.

## [0.20.1] - 2026-10-21

### Fixed

- An agent that asks for the running jobs right after seeing a job start on the event stream now always finds that job in the answer.

## [0.20.0] - 2026-10-20

### Added

- Settings > Appearance has an accent colour: pick any colour and the theme builds its accent shades from it, in light and dark.
- The theme editor can preview a draft in its own pane before applying it, generate a palette from one colour, and add font files in a new Fonts tab.

### Fixed

- After switching themes several times, docked panels no longer keep the colours of the other light or dark variant.

## [0.19.0] - 2026-10-20

### Changed

- User kits and themes read from disk are compiled once and kept in your cache folder, so the next start reads them faster. A theme file with a broken or unknown token now loads with a warning instead of failing.

## [0.18.0] - 2026-10-20

### Added

- Helios's light and dark themes are now a pair: Settings > Appearance can follow the system's light or dark mode, or keep one, and View > Toggle light and dark switches between them.

## [0.17.0] - 2026-10-20

### Added

- In a narrow window, the menus that no longer fit in the menu bar move into a "…" menu instead of being cut off, and Alt with a menu's letter opens it wherever it is.

## [0.16.0] - 2026-10-20

### Added

- Agents can read and move a workspace's places and modes through MCP (helios_context_read, _request, _go and _set_mode), each with a dry run and a revision check: a desktop that has the workspace open moves at once, otherwise a desktop opening the workspace later comes back where the agent left it.
- The window now knows where you are and how you work: the library, the darkroom on an image and the workspace's network are places with their own arrangement, and culling, mask editing and compare are modes. A breadcrumb and mode chips in the status bar show them; Alt+Left goes back, Escape leaves a compare or a path being drawn, and View offers the library, the network and culling, whose keys then work from anywhere.
- A workspace opens again where it was left, even after a crash: the same place, the image open in the darkroom again, culling and compare as they were. An image that was deleted or moved since, or a network that is another workspace's, is left out with a message, and the rest comes back.

## [0.15.0] - 2026-10-20

### Added

- The title bar shows the jobs going on, the stack suggestions waiting for
  an answer and the profile in use, in full screen too: a click opens the
  Jobs panel, the Stacks panel or the Profiles menu.
- `Ctrl+K Z` enters zen mode, the image alone on the screen; Escape
  brings the bars and panels back.

### Changed

- In full screen the title bar stays, with its menu, logo and buttons: a
  button leaves full screen in place of maximize. Zen mode (View > Zen
  mode) shows the image alone, Escape brings the workbench back.
- A tab dragged in the workbench shows a ghost of itself under the
  pointer, drawn by the theme's kit.

## [0.14.0] - 2026-10-20

### Added

- The keyboard shortcuts editor can change keys for the open workspace alone
  as well as for every workspace (the default), and says after each key
  whether it comes from the workspace's template, the workspace or you. A
  workspace's own keys are kept with it and undone with Edit > Undo.

### Fixed

- An agent editing a workspace that is open on the desktop now goes through
  the desktop: the change shows at once, is one step of Edit > Undo, and the
  desktop's next save no longer overwrites it. A workspace's files changed
  outside Helios are reloaded as one undo step, keeping what you changed
  meanwhile unless the same part changed on both sides, which a warning names.

## [0.13.0] - 2026-10-20

### Changed

- With a mask group selected, the Parameters panel names its operator by
  its effect (union, intersection, …) with a small preview, and lists each
  member's opacity and inversion on a line of its own, by name, instead of
  "8 values".

### Fixed

- A parameter offered as a list of choices in the Parameters panel now
  opens when clicked; it used to close at once, so its value could not be
  changed there.

## [0.12.0] - 2026-10-19

### Changed

- Helios runs on egui-pop's micro-indicators: status dots, badges, flags
  and swatches are drawn by the theme's kit, with shapes that keep each
  state apart without relying on colour alone.

## [0.11.0] - 2026-10-19

### Changed

- Helios runs on egui-pop's visual kits: every themable part of the
  interface (frames, menus, tables, progress and status indicators) is now
  drawn from the theme's data, ready for deeper themes.

## [0.10.2] - 2026-10-18

### Fixed

- With the interface in another language, Helios's own entries in the menus
  (such as opening a workspace in a new window) now show in that language
  too, instead of staying in English.

## [0.10.1] - 2026-10-18

### Fixed

- The workspace entries of the menu (edit, library settings, reset, save as
  template, add parts) now show a short name instead of a long sentence
  written for agents.

## [0.10.0] - 2026-10-18

### Added

- Every event an agent or a script reads from Helios (the inspect endpoint, the
  MCP tools) now says when it happened, as a wall-clock time and as a monotonic
  time for measuring delays.

## [0.9.1] - 2026-10-18

### Fixed

- Launching Helios from the desktop no longer keeps a busy cursor for
  several seconds after its window is up.
- The window manager's focus setting is read while the desktop starts, and
  a desktop slow to answer no longer holds the window up: past a quarter
  of a second the window opens with click to focus.
- The desktop opens its GPU device while it reads its settings and the
  vault, instead of after, so its window comes up sooner.
- An image named at launch, or handed over by a later launch, is decoded
  in the background: the window opens at once and says "Opening…" in the
  status bar until the image shows. An image that cannot be read is now
  said in the window rather than stopping the launch.
- The library is read from the vault while the window opens, instead of
  before it: a large library no longer holds the window up, and the status
  bar says when it is being read and how many images it holds.

## [0.9.0] - 2026-10-18

### Changed

- A workspace created from "Import, cull, de-duplicate" shows the jobs
  centre beside the filmstrip, so an import's progress is in view while
  culling.
- The Vault panel keeps its search field, import button and marks bar in
  place while the library is empty, instead of showing them only once
  images are imported.

## [0.8.0] - 2026-10-14

### Added

- Hovering a mask handle on the image now says its gestures in the status
  line: what Shift snaps, Ctrl for fine moves, and when they apply a
  double click to reset and the wheel to adjust.
- In the Masks panel, a blur, feather or levels refinement is now a row of
  its own under what it refines: select it to edit its parameters. A
  member wired to two ports of a group shows once per port.
- A template can now ask for values when a workspace is created from it
  (the import source, a file it refers to), and carry an icon shown beside
  its name.
- A workspace can now be edited after it was created, and the result is
  kept: its name, description and icon, its library settings, its keys and
  tools. The File menu saves it as a template (after a review of what goes
  in and what is left out), resets it to its template, and adds parts of
  another workspace or template, each as one step that Undo takes back.
- Agents can now work on a workspace through MCP: list its parts, edit
  it, reset it to its template, add parts of another workspace or
  template, and save it as a template, each with a dry run first and an
  undo after.

### Changed

- A mask member's row now says its inversion and opacity in its group in
  words ("inverted", "opacity 30 %", "hidden"), in the interface's
  language, rather than a bare percentage.
- The Masks panel names a group's operator by its effect (union,
  intersection, subtract from the first, exclude, multiply...) rather
  than its formula, with a small preview of what it does to two shapes.

### Fixed

- A workspace's panel arrangement is saved as soon as it changes, not only
  when Helios closes: a crash or a forced quit no longer loses where the
  panels were.
- The Masks panel's buttons, hints and messages are now translated into
  French: some had been left in English.

## [0.7.0] - 2026-10-13

### Changed

- The Masks panel names the module whose mask it shows, and an empty mask
  now says that the module applies everywhere until a shape or a parametric
  member limits it, instead of showing a lone row.
- The Masks panel's buttons are translated and their hints give the keys
  and gestures that do the same (F2, Delete, drag and drop, C, E, G and P on
  the image); the delete button is now called Take out, as in the row's
  menu, and warns that it is not an undo step yet.
- Taking a group that holds members out of a mask now asks first, since its
  members go with it and taking out cannot be undone yet. Shapes and empty
  groups are still taken out at once.

### Fixed

- The Masks panel now shows a blur, feather or levels applied to a module's
  whole mask on the mask's own row: everything that shapes the result is
  listed.

## [0.6.0] - 2026-10-13

### Added

- A process's inspect endpoint pushes its events as they happen
  (`subscribe` with a kind prefix), and a client that reconnects picks up
  from the last event it saw, told when some were missed.
- Cancel in the jobs centre stops a node's job on that node: a batch run
  by `helios batch` can be stopped from the desktop.
- A node shows the jobs it runs now and the last ones that ended
  (`describe jobs`), so a client that lost touch catches up.
- The desktop follows a running node's jobs (`helios-gui --node
  NAME=INSPECT`): a node's batch shows in the jobs centre with its exports
  under it. When the node goes out of reach its jobs show as lost, then
  catch up once it is back, or leave the list after a minute.

### Fixed

- The batch endpoint's refusal now lists every request it answers, in
  English and French.
- A batch whose `helios batch` process dies no longer shows as running on
  its node for ever: the node ends it as abandoned when the process's
  connection closes, or after ten minutes with nothing asked for it.

## [0.5.1] - 2026-10-13

### Fixed

- A freshly opened library now fills in its thumbnails as they are read,
  instead of waiting for the mouse to move or a key to be pressed.

## [0.5.0] - 2026-10-13

### Added

- The Masks panel and the Scopes can float over the image as palettes:
  float them from their tab, drag them by their header, dock them back on
  a side, and hide them all with Tab. The Portrait retouch template opens
  with the Scopes floating over the image.
- A Navigator panel shows the whole image with a frame around the part in
  view: drag the frame or click elsewhere to move the view, zoom from its
  buttons or the wheel, double click to fit. It can dock in a side bar or
  float over the image.
- Ctrl+P opens quick open: type the name of a module of the image's
  network, an image of the library or a recent workspace, and Enter goes
  there. Every command stays in the command palette (Ctrl+Shift+P), with
  its shortcut.

### Changed

- The histogram now measures the edited image as the pipeline outputs it,
  shows when shadows or highlights clip, and moves the exposure when you
  drag it, as in darktable: one undo step per drag, Escape to give it up.

## [0.4.0] - 2026-10-13

### Added

- The library is a culling tool: a fast thumbnail grid and filmstrip,
  Lightroom's culling keys (0 to 5, P, X, U, 6 to 9) with optional
  auto-advance, picks, rejects, colour labels and tags saved in the vault, a
  search with filter chips, and a Compare panel for two to four images.
- A Duplicates panel lists the exact duplicates an import found and what
  its policy did with each. With the `ask` policy, each file now waits for
  your answer there: keep it as one more copy, or skip it.
- A second built-in workspace template, "Import, cull, de-duplicate": the
  library grid with Compare and the loupe, the stack review and the
  duplicates beside it, a card copied and renamed on import with its
  duplicates asked about, stacks proposed after each import, and the
  culling keys with auto-advance.
- Stacks show collapsed in the library, expanded with S or their chevron.
  A Stacks panel reviews the suggested stacks: each burst or bracket with
  its proposed top image, to accept all, accept only the ones you check, or
  decline.

### Fixed

- A value typed in a parameter's slider is now kept when you click
  elsewhere, instead of being dropped, and the keyboard goes where you
  clicked. In French, the space between a value and its unit is a narrow
  no-break space, as French typography writes it.

## [0.3.0] - 2026-10-13

### Added

- `helios batch` shows on the node it drives as one batch job with an
  export job per image under it, and cancelling the batch there (`helios
  control job-cancel`) stops it: the exports in flight are cancelled, the
  others are not started, and no file is left half-written.
- An import now shows in the jobs centre with its progress, the file at hand
  and a log line per file, and can be cancelled from there (or with Ctrl-C
  for `helios import`): it stops before the next file, keeps the files
  already imported and leaves no half-written copy.
- A node's exports are jobs people follow on its bus, and `helios control
  job-cancel ID` (the `job.cancel` command, also an agent tool) cancels one
  by its id, queued or running, without writing a file.

## [0.2.1] - 2026-10-13

### Fixed

- Closing Helios no longer keeps the window on screen under a busy cursor:
  the window goes at once, and the catalog is closed after it.
- Thumbnails are scaled down as they are read, so a large library no longer
  slows the window while they load.

## [0.2.0] - 2026-10-13

### Added

- The GPU calibration reports as a job once the compute manager is given
  the jobs (`Manager::report_to`): each device's reference modules as items,
  its speeds as log lines, a device that failed as a failed item, and a
  cancel that stops it between two modules.
- The desktop has egui-pop's jobs centre: the Jobs panel and the status
  bar's jobs item with its popover list every background job of the
  desktop's process (an export, a calibration) with its overall percentage,
  "n of m", the current item, the time left, a log console per job and
  Cancel. Helios's status line is now a block of egui-pop's status bar. A
  long export or batch that ends while no Helios window has the focus is
  said on the desktop from the job's end.
- `helios_core::jobs`: background work reports how it goes as `job.*`
  events on the bus (started, progress with done and total, the current
  item and step, log lines, an item failed, finished, failed, cancelled),
  the shape egui-pop's jobs centre folds; a job is cancelled by its id
  (`Jobs::cancel`). `Scheduler::canceller` cancels jobs without holding the
  scheduler.
- An export reports as a job (`job.*` events on its bus): started when
  queued, its render, encode and write steps, a log line with the file
  written, and its end; `helios_export::submit_reported` takes the jobs
  whose `cancel` stops it, queued or running, writing nothing.

### Changed

- The Parameters panel's sliders are egui-pop's slider family, built from
  each parameter's descriptor: Ctrl drags and steps ten times finer and
  Shift ten times coarser, Page Up and Page Down take coarse steps, Home and
  End go to the soft range's ends, Escape gives a drag up without a trace,
  and Enter types a value or an expression of the current one (`x * 2`).
  A logarithmic bar and a picker button follow the module's hints. Every
  finished change runs the `slider.change` step, so a plugin can veto or
  snap it; a drag is still one undo step and steps in a row one group.

## [0.1.1] - 2026-10-13

### Fixed

- The example plugins (exiftool, gphoto2) load again in 0.1.0: they still
  declared that they work only with Helios 0.0.x.

## [0.1.0] - 2026-10-13

### Added

- An agent sets up local AI models through MCP: `helios_ai_*` tools list and
  add providers, route capabilities, try prompts on an image, probe the
  machine, recommend models that fit, pull them as cancellable jobs, bench
  them and install Ollama for the user, step by step and only when each step
  is confirmed. `helios doctor ai` says what is available and why not, and a
  network can ask its decisions through the routing map (`"resolver": "routing"`).
- AI model providers with declared capabilities (Ollama, any OpenAI-compatible
  server, Anthropic, a local command), a routing map from capability or prompt
  to provider and model, and prompts as versioned files that a workflow can
  pin. Local-first: only Ollama on loopback is known until a provider is added;
  keys stay in the system keyring or the environment, images are downscaled
  before they leave, and every call is logged.
- A mask can hold an AI object member: the subject, the sky, a person or
  what a prompt names, found by a model and recorded with the edit, so the
  edit renders the same mask later without asking again.
- Decision nodes can be answered by a model: any program reading and
  answering JSON, or an OpenAI-compatible or Anthropic endpoint whose key
  comes from the environment, applied at once (full-AI) or kept as a
  proposal to accept (AI-assisted), never asked twice about the same pixels
  and capped by `system.ai.calls`. `helios control decisions` and the
  `decision-*` commands and MCP tools let you or an agent ask, review and
  answer; `decide_exposure` is a first decision module.
- `helios-node --batch-control PATH` serves, in every build, the few control
  commands a batch export needs (give a source a file, export, cancel), on a
  socket only its user can open; the other control commands stay in debug
  builds.
- `helios batch` applies a network to many images and exports each one to a JPEG, PNG or TIFF file, optionally on top of the image's darktable sidecar, with one JSON report line per image. A failed image does not stop the others, existing files are never overwritten, and Ctrl-C leaves no half-written file; it works with release builds.
- Path fields in dialogs have a Browse button that opens the system's file
  chooser, or Helios's own folder picker when there is none. Opening a
  workspace uses it and lists the recent workspaces.
- The render cache stays within a memory budget (a machine setting, 2 GiB by
  default): what was used least recently is evicted and computed again when
  asked for, and content still being computed is never evicted.
- An example plugin drives cameras through gphoto2: list the connected
  cameras, take a picture and download it into a chosen folder, with
  progress.
- Nothing is computed twice: a network shares any content it has already
  computed, or is computing, between nodes, branches and simultaneous
  renders, and a parameter change only recomputes what it affects.
- Compute units: Helios lists the CPU and every GPU of this machine
  (software ones off), with per-device settings kept for this machine only
  (enabled, role, priority, thread, memory and cache budgets), a short
  benchmark, and routing of each render to the device that fits, falling
  back to the CPU and saying where it ran. Remote helios-node units can be
  declared and probed; `compute.*` commands drive it all.
- A running node can take another image for its source and export a node to a JPEG, PNG or TIFF file on request (`helios control load-source` and `helios control export`, also as MCP tools). An export renders the network as it was when asked, and an export cancelled while rendering writes nothing.
- darktable's duplicates of a photo (`IMG_0001_01.CR2.xmp` and so on) now
  import as versions of that photo, each kept apart, next to the base edit,
  which stays the one a batch exports. The new `helios import-darktable`
  keeps a whole folder's edits in a vault, listing sidecars whose image is
  missing and importing photos that have no sidecar. A camera file name that
  merely ends in digits is not taken for a duplicate.
- Helios's exposure is now checked against darktable's own render: the same
  image goes through darktable-cli and through Helios, and the results must
  agree within darktable's colour difference thresholds (they agree to
  within a ten-thousandth of a Delta E). The check runs nightly.
- In debug builds, a running node can be driven from the command line with
  `helios control`: render a node, change a parameter, cancel a job, pause
  and resume work, and make a computation or a source read fail once to test
  what happens on errors. A failure injected this way is never cached.
- A network can hold decision nodes: points where a judgment is needed, made
  by an AI, proposed by an AI and confirmed by a human, or made by a human.
  Until a decision is answered, what depends on it waits, without computing
  anything. Every answer is recorded with who gave it, which model and which
  inputs it saw. A saved edit renders again from its answers without asking
  anyone, and asking again keeps the earlier answers.
- Raw files from Fujifilm X-Trans sensors are now demosaiced with
  Markesteijn's method, darktable's default for them, ported from
  darktable: Helios's render of an X-Trans file now matches darktable's to
  within a ten-thousandth of a Delta E. Blown highlights are clipped a
  touch lower, at darktable's level, so that clipped areas match too.
- Raw files from Bayer sensors are now demosaiced with RCD, darktable's
  default method, ported from darktable: fine detail and edges come out as
  darktable renders them, without the colour fringes of the previous
  bilinear method (still available as `method: bilinear`). X-Trans sensors
  keep the bilinear method until Markesteijn is ported.
- Helios's direction is written down: a roadmap with dated decisions, the
  module contract, the internal architecture, a testing strategy built for
  AI-driven development, and GUI notes backed by research on what users ask
  of darktable and its equivalents.
- The desktop speaks the language of your choice: a Language setting
  (System by default, which reads `HELIOS_LANG` then the system's
  language) switches it while Helios runs, and the menus, the command
  palette, panel titles, settings, dialogs and the shortcuts editor follow.
  Helios ships English and French; a missing translation shows English.
- A first Helios window: `helios-gui [workspace]` opens a workspace in a
  VS Code-like shell, with side bars, editors and a bottom panel docked from
  the workspace's saved layout, panels that can be moved to their own
  window, and a command palette (Ctrl+Shift+P or F1) running every action.
  The interface can picture any panel or named widget, and record a video
  of it, by itself (`gui.snapshot`, `gui.record.start`), or headless with
  `helios-gui --snapshot <target> <file.png>`. `packaging/linux/install.sh
  --dev` adds a "Helios Dev" launcher running a source checkout's build.
- `helios doctor display` says what the display offers: the session and,
  on Wayland, what the compositor's colour management supports, with the
  encoding the image is drawn in and why. The desktop's debug bridge says the
  same beside what its window resolved (`helios.display`), and shows
  egui-pop's `viewport` collector.
- Edits are kept as versions in the vault. Two edits made at the same time
  on the same image are both kept, side by side, instead of one
  overwriting the other, and a merge records where it came from. Writing
  the same edit twice changes nothing. darktable edits imported from XMP
  sidecars become versions too, marked as imported.
- Edits are saved as you make them: with a vault, each step (a slider
  released, an undo or redo, a parameter set by an agent, a decision
  answered) is kept at once as a new version of the image, and a long drag
  is saved every second while it goes. Opening the image again, even after
  a crash, continues from the last saved step, decisions included.
- An example plugin reads an image's camera, lens, exposure, GPS position
  and keywords with ExifTool, each value with the tag it came from.
- A network can write files: an export renders a node, converts it to sRGB
  and saves it as JPEG, PNG (8 or 16-bit) or TIFF (16-bit), with the format
  taken from the file name. A file is never seen half written, two exports to
  the same file wait for each other, and a node reports each export and says
  why one failed.
- darktable's exposure module, in manual mode (exposure and black level), is
  the first module ported, with darktable's own formula.
- External plugins (proof of concept): a plugin is any program speaking
  JSON-RPC over its standard input and output, described by a manifest that
  is checked before it runs. Helios starts it when first needed, restarts it
  after a crash without affecting anything else, and holds it to the
  capabilities the user grants (`plugin.list`, `plugin.grant`,
  `metadata.read`, `device.capture`).
- A network can start from files: camera raws (Bayer and X-Trans sensors,
  through rawler) and JPEG, PNG or TIFF pictures. A raw comes in as its sensor
  data, untouched, with its camera's description (maker, model, exposure,
  levels, white balance, color matrix) handed to the modules that read it; a
  picture comes in converted to the working color space. The preview a raw
  carries can be read without decoding the raw, for fast culling.
- Modules can run on the GPU (Vulkan, Metal, through wgpu): a module may
  carry a GPU version of its computation next to its CPU one, and the GPU
  result is checked against the CPU reference before it is trusted.
  Exposure is the first.
- The engine can compute on the GPU: given a GPU device, every step whose
  module has a GPU variant (exposure, blur, sigmoid so far) runs there, the
  rest stays on the CPU in the same network, and the result matches the CPU
  render. A GPU kernel that fails now reports an error instead of crashing.
- When a step cannot run on the GPU, or the GPU is lost, the desktop now says so in a toast, in the interface's language, instead of only recording it; a module that falls back is named once.
- On the GPU, images now stay in GPU memory from one step to the next: a
  chain of GPU modules uploads its source once and reads its result back
  once, and a render's result can be handed over in GPU memory, so a viewport
  on the same device shows it without a copy back to the CPU.
- A headless node renders a network described in JSON, and the `helios`
  command observes it: its state, its events, and waiting for an event, so
  scripts and agents never have to sleep or parse logs. A wait that times
  out says what happened instead.
- Raw files now come out in true colors: an input color module, ported from
  darktable, turns the camera's own colors into the working color space with
  the camera's color matrix. Exposure can take back the exposure bias the
  photo was shot with, as darktable's "compensate camera exposure" does.
- Image processing modules can be written and tested on their own: a module
  declares its parameters, ports and needs, and the framework checks every
  output against that declaration. Contract tests generated from the
  declaration catch non-finite values, renders that differ from one run to
  the next, and tiles that differ from a whole render.
- Jobs declare a priority and what they need (GPU memory, AI calls, a
  destination…): what someone is waiting for runs first, a limited resource
  holds jobs back instead of overloading it, and a job whose needs can never
  be met fails at once, saying why.
- A new "Notify when a long job ends" setting, on by default: when exports
  and batches run from the window, one that takes more than ten seconds
  and ends while you work in another application will say so with a
  desktop notification, finished or failed, in your language.
- Settings are declared once and resolved through layers (defaults, global,
  project, user, run), and every value says which layer it comes from. A
  value set where it does not belong, such as a machine setting in a project
  file, is refused with the reason.
- `helios import` brings a folder, a file or a mounted card into a vault (referenced, or copied with `--copy --into DIR` and renamed with `--rename`), with `--duplicates skip|keep|ask`; `helios stacks` lists, makes and dissolves stacks and asks for, accepts or declines stack suggestions. See docs/library.md.
- With a vault open, the desktop runs the library operations as commands: `library.import` (in the background, as a job, the library filled again when it ends) and `library.stacks.suggest`, `accept`, `decline`, `list` and `dissolve`.
- Hovering an image in the library or the filmstrip now shows its file's size and the day it was last modified, written the interface's way (`25,3 Mio · modifié le 12 oct. 2026` in French).
- Importing is now a library operation: a folder or a mounted card is referenced in place or copied into the library folder by capture date, renamed by a pattern, with its XMP sidecars, as a job with progress. Exact duplicates are found by content, within the import and against the library, and skipped, kept or listed for an answer; nothing is ever deleted. Each image gets near-duplicate signals (a perceptual hash, capture time, exposure, frame number, sharpness).
- Library: `helios-gui --vault FILE` shows the vault's images as a grid (the
  Vault panel) and a filmstrip, with their thumbnails (a raw file's embedded
  preview). Rate them 0 to 5 or reject them (keys 0–5 and R, as in
  darktable's lighttable), toggle color labels, and move with the arrows; the
  changes are written to the vault, audited like any other.
- Helios now keeps a log file and writes a crash report when it fails, both
  in your state directory (`~/.local/state/helios` on Linux). Nothing is
  sent anywhere: after a crash, the next start offers the report, and
  Help > About Helios says where both are.
- A brush stroke mask, `mask_brush`: darktable's brush, a curve painted with
  a round brush whose radius, hardness and density change from point to
  point. The drawn shape masks (circle, ellipse, gradient, path, brush) can
  now be used in a network description too.
- Drawn masks are edited on the image: the mask's shapes are outlined, the
  selected one has handles to move, size, rotate and feather it (Shift
  snaps the angle, Ctrl moves finely, Escape cancels, each drag one undo
  step), paths gain and lose points, and new circles, ellipses, gradients
  and paths are added with `C`, `E`, `G` and `P` or the `mask.*` commands.
- Mask modules build darktable's flexi mask groups as a network: `mask_combine` folds up to eight masks with one of seven operators (maximum, screen, minimum, product, sum, difference, exclusion), `mask_feather`, `mask_blur` and `mask_levels` refine a member or a group, and `mask_blend` applies a module through the finished mask.
- A parametric mask, `mask_parametric`, selects pixels by ranges of luminance, red, green, blue and perceptual lightness, chroma and hue, each with a feathered edge and its own inversion, as darktable's parametric masks do. Connect it to a module's input or to its output to mask on either.
- The drawn-mask parity job also builds darktable's flexi masks pull
  request at a pinned commit: it renders two flexi fixtures and every
  classic one again (which darktable migrates to flexi masks as it reads
  them), and keeps darktable's own decoding of the fixtures' masks, which
  a test compares with what Helios reads.
- Drawn masks imported from darktable are checked against darktable itself:
  a bench box job renders hand-made sidecars (one per shape kind, two
  groups) on raws of the corpus with darktable-cli, built from a pinned
  upstream commit, and with Helios, and compares the masks in Delta E.
- A raster mask, `mask_raster`, reuses the mask of one module in another, optionally inverted and scaled, as darktable's raster masks do. The reused mask is not computed again, and stays the same in tiles and previews.
- A model can answer an AI object mask: your own program reading JSON and
  writing a PNG, or a local server speaking the same JSON, picked by name
  per mask member. Nothing is configured by default, and a model that finds
  nothing or fails leaves the mask as it was and says why.
- Each row of the Masks panel shows its member's icon and ends with an eye
  and a lock: the eye turns the member's opacity off or back on, the lock
  keeps it from moving or leaving the mask and its shape from being
  edited on the image (a locked group keeps its members). A row's context
  menu acts on the row right-clicked, and a rename starts from the plain
  name.
- Masks can be drawn: circles, ellipses, linear gradients and paths (polygons
  or Bézier curves), each with a feather, an opacity and an inversion,
  combined with the other mask members by every group operator. Shapes are
  placed in image pixels, so they stay put at every zoom and crop, their
  edges are anti-aliased, and parts of the image away from a shape cost
  almost nothing to render.
- The Masks panel shows an AI object member with its own icon, and the
  Parameters panel what it looks for (the kind of object or a prompt, and
  which model to ask); changing it asks again.
- A Masks panel shows the selected module's mask as a tree of groups and members, with their icons, operators, inversion and opacity, kept up to date with the image and the Parameters panel. Selecting a member there selects it on the image and back; members are renamed with F2, inverted, faded, reordered or moved between groups by drag and drop, deleted, and new shapes or parametric members added.
- `helios-mcp` lets an AI agent drive a running node with the same verbs as
  the `helios` command: read its state and events, wait for it, render,
  change parameters, inject faults, and read every module's parameters and
  ports from its manifest.
- New workspace from template: pick a template by its scenario and a
  directory, and the workspace opens with the template's network, panels
  and keys; an image opened there goes through the template's network.
- Every node reports the same normalized status (idle, stale, running, done
  or failed, with its load), ready for the node headers of every surface; the
  graph also proposes principal nodes and stacks, so a large network can be
  shown as an overview.
- Normalized dialogs: confirmations, forms, pickers, menus and notices are
  drawn by the shell alone, the same way everywhere from the theme's tokens
  (a test fails if a panel draws its own). A destructive confirmation is
  named by its verb, in the danger style, with the focus on Cancel; warnings
  and errors show as toasts. `dialog.current` and `dialog.answer` let an
  agent read and answer any dialog, `dialog.assumeYes` confirms by policy
  for scripts. Detached panel windows get Helios's frame.
- A module may declare optional inputs: left unconnected, the module gets an empty image in their place and the render goes on, so one module can take a variable number of members up to a fixed maximum.
- Parameter widgets: the Parameters panel shows each node's values as
  widgets generated from their descriptors: sliders over the soft range
  (drag, wheel, arrows, double click to reset, Enter to type a value with
  its unit), toggles, lists and fields. A drag is one undo step and keeps
  agents off the value meanwhile; a value changed from its default, moved
  by someone else (and who) or held by another's gesture is marked; the
  context menu resets or copies the path or the value.
- Modules can now speak photography: raw mosaics with their Bayer or X-Trans
  pattern, what the camera recorded (levels, white balance, color matrix,
  exposure) offered as context a module declares, and the color spaces of
  the pipe, which works in linear Rec.2020 like darktable.
- A plugin's manifest now gives a title and a description, and the plugin can translate them, with its commands' titles, in its own catalogues; a catalogue that tries to change another plugin's or Helios's text is refused. The ExifTool example plugin ships English and French.
- A first workspace template, "Portrait retouch": an exposure brightened
  through a feathered mask, the Masks panel and the parameters docked, the
  mask tools at hand, in English and French.
- A profile can differ per system or per machine: a bigger font on the
  laptop, another shortcut on macOS. Its parts may hold variants for a
  system (`macos`, `linux`, `windows`), a high-density screen (`hidpi`) or
  a machine named once in `machine.toml` in the config directory; Helios
  applies those that match where it runs, the most specific last, and
  `profile.resolved` says which values came from which variant. Variants
  for other machines stay in the file, travel with an export and show in an
  import's diff; `profile.export` with `this_machine` writes only this
  machine's view.
- Profiles. Your theme, your own shortcuts and your panel layout can be
  saved as a profile (`profile.export`), read back on another machine
  (`profile.import` shows what would change, `profile.import.apply`
  applies it), and chosen by name (`profile.use`); each workspace remembers
  its profile. Profile files are readable text and never carry secrets: an
  export that would is refused. A panel you closed now stays closed when a
  layout is restored.
- A camera raw can now be developed from start to finish: two new modules,
  ported from darktable, prepare the sensor data (black level and scale) and
  demosaic it (bilinear, Bayer and X-Trans), reading the camera's levels and
  filter pattern on their own. A raw goes through to an exported picture.
- A loaded raw says where its image sits on the sensor: the full photosite
  grid and the corner of the crop kept. darktable's drawn masks are placed
  on that grid, so a mask read from a sidecar lands where it was drawn.
- Previews: any part of a network can be rendered at a reduced scale, half
  or a quarter of full resolution, and look like the full render reduced.
  Sizes in a module (a blur radius) are given in full-resolution pixels and
  follow the scale on their own; previews and exact renders are cached
  apart. A Gaussian blur is the first module to use it.
- `helios sandbox new|start|stop|clean` gives a node its own directory,
  settings and sockets in one command each, and an agent can do the same from
  MCP. A sandbox is never created over a directory holding something else, and
  only a directory marked as a sandbox is ever removed.
- Background work runs as named, cancellable jobs, announced when they are
  queued, start and end; `helios inspect wait-idle` returns as soon as nothing
  is left to do, and the number of jobs run at once is a machine setting.
- Scopes: the Scopes panel shows a histogram, a waveform or an RGB parade of
  the image, in stops (where exposure and clipping read best) or linear,
  switched from the panel or the palette ("Scopes: …").
- The desktop's foundations, ahead of its window: every action is a named
  command that a menu, a key, the palette, an agent or a test runs the same
  way, each run remembering who asked; keys are resolved by the framework in
  layers (yours over the workspace's over the defaults over what modules
  suggest), can be removed for good, and conflicts are listed; panels are
  declared and a workspace's arrangement is saved and repaired when panels
  or screens change.
- `helios batch --sidecar --vault FILE` keeps each darktable sidecar it
  applies: the image's darktable edit becomes a version marked as imported,
  and the sidecar's rating (or reject) is kept, but only when the image has
  none yet, so it never overwrites a rating chosen in Helios.
- A raw now develops into a finished picture: the sigmoid tone mapper,
  ported from darktable (its recommended one), rolls the scene's highlights
  off into the display's range instead of clipping them, keeps middle grey
  where it is, and keeps hues. Contrast, skew and the display's white and
  black targets can be set.
- Helios now runs once per user: opening an image or a workspace from the
  file manager, or a `helios://open?path=...` link, while Helios runs hands
  it to the open window, which comes to the front and shows it.
  `helios-gui --new-instance` still opens a window of its own.
- A `stacks.suggest` decision proposes stacks from the import signals: frames close in time (or of one burst) that look alike, brackets included, the sharpest proposed on top. Nothing is applied until a person accepts all the groups or some of them, and the answer is recorded with the proposal; the resolver is picked by name, rule-based by default.
- System menu: the Helios logo at the top left of the window (or Alt+Space)
  opens a menu to open a workspace (in a new window) or a recent one,
  switch profile or theme (the one in use ticked), list the keyboard
  shortcuts, act on the window, read about Helios or quit. Menus are now
  declared: panels and plugins add commands to a menu by its id, with keys
  shown from the keymap.
- Tabs move between areas: drag a panel's tab onto another side bar, the
  editors or the bottom panel (or the edge where a hidden one would be) and
  it moves there; the area is highlighted while dragging, or says why the
  panel may not go there (the image and the graph stay in the editors).
  `<panel>.moveTo` does the same from keys, the palette or an agent, under
  the same rules; a saved layout breaking them is repaired.
- A node can also be reached over loopback TCP (`tcp://127.0.0.1:PORT`), for
  systems without Unix sockets: only local connections are served, and each
  must present a session token the node writes to a file only its user can
  read.
- The GUI now takes its theme from the egui-pop library: Helios's dark, light and high-contrast themes are small overlays of egui-pop's, keeping only the viewport surround, node state colours and the danger button. Headless runs settle faster, with no cursor blink or animation.
- Themes. Helios ships Helios Dark (the default), Helios Light and Helios
  High Contrast, chosen from the palette ("Theme: …"). A theme is a Design
  Tokens file (`*.tokens.json`) of named colours and sizes; it can build on
  another and change only a few of its values, and `theme.export` writes
  the theme in use as one file to share. A theme with a mistake still
  works: the faulty values fall back to the dark theme's, and Helios says
  which.
- The Helios logo and name are back at the start of the window's title bar,
  next to the button that opens the system menu.
- Translation catalogs: Helios's commands, panels and module parameters get
  Fluent catalogs in `i18n/`, generated in English from their declarations
  and falling back from a regional locale to its language, then to English;
  `cargo xtask i18n check` reports each language's coverage and stale
  translations. A first French catalog and an accented pseudo-locale
  (`x-pseudo`) are included; the desktop shows them in a next step.
- Value store: every edited value lives at a stable path (`node/…`,
  `settings/…`, `view/…`) with a descriptor from the module's manifest and
  hints (ranges, steps, unit). `param.set` and its siblings set a value or
  a step, say whether a range held it back, refuse a stale write or one
  during someone else's drag, undo by gesture, and announce each change
  with who made it.
- Ratings, color labels, titles and descriptions are changed by commands that
  skip values already stored, keep only the last change of a batch, and record
  who changed what and when; collections are read through cursors, so opening
  one or showing a screen of thumbnails costs one query each, and refreshing
  says exactly which images came in and went out.
- The vault identifies an image by its content, not by where it is: a folder
  import records every copy of a file as a location, a file already known is
  never read again, and a repair scan finds moved or renamed files by content
  while reading as little as it can.
- A vault that was not closed properly (a crash, a kill, a power cut) is
  checked before it is used. If the check fails, the vault opens read-only
  and the error names the latest verified snapshot to restore from; the
  vault keeps its last five snapshots.
- A workflow's journal (which images went through which step, what failed
  and is due again) can be kept in the vault, so it survives a restart and
  lives in the same database as the catalog.
- The vault now records its schema version and upgrades older vaults step by
  step, after taking a verified copy of the database next to it. A vault
  written by a newer Helios is refused with a clear message instead of being
  silently changed, and an upgrade gets past leftovers from old versions
  (stale views, rows pointing to deleted images), reporting what it found.
- Your own settings and each project's can live in the vault
  (`helios-node --vault FILE`): they follow you to any machine that opens
  it. A settings file given the first time is imported once; after that
  the vault's copy is the one used.
- Image viewport: the Image panel shows a node's output on the GPU, a proxy
  first, then the exact visible part in tiles reused when panning, never
  blocking the window. Zoom and position are values moved by mouse,
  `viewport.zoom.*` or an agent, a changed parameter renders again, and
  `gui.snapshot` of `viewport.scene` saves the data before the display
  transform. `helios-gui --image FILE` shows a picture.
- vkdt's local contrast (`vkdt_contrast`): a guided filter on the
  luminance, its blurs computed at lower resolutions, then the detail
  boosted against it. It is the first module made of several kernels, each
  computed, cached and tiled on its own, those reading one image on the
  GPU; the nightly check compares it with vkdt's own render.
- vkdt modules can be described in Helios: their parameter, connector and
  interface files read into Helios manifests and write back unchanged, for
  every module of vkdt. Manifests gained what that took: array and text
  parameters, pixel formats beyond 32-bit float, named channel layouts (a raw
  mosaic, RGBA), ports shaped like another port, and storage buffers.
- The first modules ported from vkdt: its exposure and its unsharp mask
  (`vkdt_exposure`, `vkdt_usm`), each with a GPU variant translated from
  vkdt's shader. A nightly check builds vkdt and compares Helios's results
  with vkdt's own.
- A study of vkdt's module format, with an inventory of its 99 modules, maps
  what vkdt modules declare onto the Helios module contract and lists what
  the contract still lacks to host them.
- Raw files come out with their colors: a white balance module, ported from
  darktable, applies the camera's as-shot balance (or the user's), and a
  highlights module keeps blown areas neutral instead of letting them turn
  magenta.
- `helios doctor display` and the debug bridge say where the image goes with the Wide-gamut preview setting on: `tagged`, `wsi` or `srgb`, with the encoding and the reason.
- A Wide-gamut preview setting (Display, off by default): the image asks to
  be drawn in Rec.2020 with gamma 2.2, so colours outside sRGB are not
  clipped where the window can carry it; elsewhere it stays sRGB, and the
  interface always does. In English and French.
- The Wide-gamut preview setting now shows the image in a wide gamut on Wayland compositors with colour management: tagged BT.2020 with a 2.2 power on KWin 6.3 and later, sway and Weston, Display P3 on GNOME and Hyprland. Colours outside sRGB are no longer clipped there; the interface is unchanged, and elsewhere the image stays sRGB.
- Helios draws its own window frame: a title bar with the workspace's name,
  minimize, maximize and close buttons in the interface's style, a double
  click to maximize, and resizing from any edge. F11 toggles full screen,
  which also hides the title bar.
- A workflow can be applied to many items over time: each item's progress
  through each step is journaled, workers claim items without ever taking one
  twice, a choice skips the branches it did not take, failures are retried
  after a growing delay, and work held by a worker that died is given back.
- darktable's drawn masks come over with a sidecar: a module blending
  through circles, ellipses, gradients, paths, brushes and their groups
  (with each member's inversion and opacity, the mask's inversion, blur,
  contrast and brightness, and the module's opacity) gets the same mask as
  Helios nodes. A mask Helios cannot reproduce exactly (a clone or AI
  shape, a curved gradient, a parametric or raster mask, another blend
  mode...) is never approximated: the module is kept aside, and the
  import's warning says why.
- Sidecars written by darktable's flexi masks (the pull request
  darktable-org/darktable#22553, masks version 7) are read too: groups with
  their operator (maximum, screen, minimum, product, sum, difference,
  exclusion), inversion, bypass and opacity, hidden and disabled members,
  and their blur, contrast and brightness refinement, as a tree of Helios
  mask groups. A later masks version is kept aside with a warning.
- darktable edits can be read: Helios opens a darktable XMP sidecar, decodes
  each module's settings by name with darktable's own description of them,
  and carries the modules Helios has over to its own, with their values. A
  report lists everything else (modules Helios does not have yet, settings
  without a counterpart, older module versions still to migrate), so
  nothing is lost without a word.

### Changed

- Decision nodes can be answered through the AI routing map: a decision
  naming a capability (`segmentation`), or naming no resolver, goes to the
  provider and model routed for it. The OpenAI-compatible and Anthropic
  endpoints of the `http` resolver now share the AI providers' code.
- The render cache now uses at most a quarter of the machine's memory by default
  instead of a fixed 2 GiB; the share, or a fixed size, is a machine setting.
  A render never evicts what it still needs, so a small budget no longer makes
  it compute the same step twice.
- The desktop's own notices (a profile applied, a theme exported, an edit not saved), the Open a workspace dialog, and its refusals (undo while a drag is in progress, a mask member dropped where it cannot go) now follow the interface's language; French is included.
- The viewport's display transform encodes for the target the window draws
  for, instead of always for sRGB: on a target wider than sRGB (Rec.2020,
  extended sRGB) colours outside the sRGB gamut are no longer clipped.
- Helios now follows the desktop's focus mode (focus on click or on hover) from the start, and a vault whose images cannot be listed or imported when Helios opens is reported in the window instead of closing it.
- The window's title bar and edges are now egui-pop's: a menu button at the top left opens the Helios menu (a right click on the title bar and Alt+Space too), the edges are easier to grab on high-density screens, and a "Window frame" setting gives the desktop's own frame back.
- Panels are now placed by egui-pop's workbench: a tab's menu moves, detaches or closes its panel, named arrangements can be saved, switched and reset, and Helios offers an "Image" arrangement (the image and the filmstrip alone) for small screens. Layouts saved by earlier versions open unchanged; the areas are now called the centre, the start and end side bars and the bottom panel.
- Export and batch errors (a file that cannot be written, a format not known, a network with several sources, an image cancelled) now carry a translation key with their English, so an interface or an agent can show them in its user's language; French is included.
- The desktop is now built on egui-pop's application framework: a menu bar, a command palette that ranks commands by a fuzzy match with the recent ones first, a settings window (Ctrl+,) with a shortcuts editor where a key is changed by pressing it, and the window's size and place kept between runs. Dialogs, toasts and menus are egui-pop's; for now a path field browses with the in-app picker only, and a picker is a list of buttons without a search.
- The warning a darktable import gives when it cannot apply some history items now comes with a translation key, each reason translated too, beside its English line; French is included.
- A form's Browse… button opens the new in-app file picker: a click on a file highlights it and Open (or a double click) chooses it, a filter field narrows the list or takes a typed path, and Cancel goes back to the form.
- A brush stroke in the Masks panel now has its own icon (a pencil),
  instead of the generic one.
- A locked path in the Masks panel also keeps its points: none can be
  added, removed or smoothed until it is unlocked.
- A row's menu in the Masks panel now says each command in a word or two
  (Rename…, Invert, Lock or unlock…), in the interface's language, instead
  of the command palette's long titles.
- When the node refuses a request (a file it cannot load, a node it does not have, a job already ended), its answer and the batch's line for that image now carry a translation key beside the English, so they can be shown in the user's language; French is included.
- The Parameters panel shows each parameter's label, description and choices in the interface's language (`exposition`, `noir` in French), from the modules' translations.
- The Parameters panel writes values in the interface's language (`0,50` and `12,5 %` in French), shows exposures signed (`+0.70 EV`), and its tooltips and notices (a value held to its range, a typed value that is not a number) are translated.
- Pickers (move a panel, choose an area...) have a filter field again: typing narrows the list, the arrows move through it and Enter takes the highlighted entry.
- A node read by several others over different areas is now computed once over
  the area they all need, each reading its part of it: a network of 58 mask
  nodes renders about nine times faster with a sixth of the memory, and an area
  inside one already rendered is shown without computing anything.
- The image viewport runs on egui-pop's viewport. 100 % now means one image
  pixel on one screen pixel at any window scale, and the zoom value shows
  100 % there. Zoom in and zoom out walk a ladder of whole ratios (50 %,
  100 %, 200 %...), and the wheel stops at 100 % when it crosses it. Edits
  re-render in tiles, the old image staying on screen until the new tiles
  arrive. New: `viewport.compare` shows before and after side by side
  (before is the edit as it was opened; hold `\` to see it alone), and the
  working-space values of the pixel under the pointer show in a corner.
- A darktable edit that uses modules Helios does not have yet (or a version,
  a blend or a mask it cannot apply) still imports: those steps are kept
  untouched, listed as not applied with their name and version, and one
  warning per sidecar sums them up, instead of being left out. A masked or
  blended step is no longer applied to the whole image, and a damaged history
  entry no longer stops the import. The import report now lists these steps
  under `not_applied`, which replaces `unknown_modules`,
  `unsupported_versions` and `undecodable`.

### Fixed

- Raw files from a camera the decoder does not list (a DNG from a Ricoh GR, for
  one) no longer carry the padding of their metadata in the camera name, so
  the same camera reads and compares under one name.
- Connecting nodes in a network where many nodes read the same inputs no longer slows down exponentially: the check that refuses cycles now looks at each node once.
- The outlines of drawn mask shapes stay crisp on screens scaled to 125 % or 150 %, in the main window and in a detached image window alike. Picturing a part of a detached panel now says it is not supported yet instead of saving a wrong crop of the main window.
- A parameter slider no longer lets its panel scroll on after a wheel step,
  the window's own title bar is hidden in full screen, and the mask outlines
  on the viewport stay sharp at 125 % and 150 % scaling.
- A profile's key bindings now keep where they apply (their `when` clause) and the default keys they take away, and applying a profile says how many bindings changed and which keys conflict.
- A profile's high-density screen variant now follows the screen itself, also after the window moves to another screen, and no longer the interface zoom (Ctrl+Alt+= and Ctrl+Alt+-); focus rings stay sharp at fractional scales.
- GPUs are now opened with everything they allow instead of wgpu's small default limits, so images over 128 MB, such as a 24 MP photo in full precision, no longer fail on a GPU that was chosen to run them. When a driver refuses its own limits, the device opens with less, says why, and work that no longer fits goes to the CPU.
- GPU steps over about 4.2 megapixels, such as a full 24 MP photo, no longer fail: kernels now run over a two-dimensional grid of workgroups instead of a single row capped by the GPU.
- Helios now opens one GPU device per process and shares it between the window and the engine, instead of letting the window open a second one. On Linux with a Vulkan loader older than 1.4.350, two devices could crash the application when one was being opened while the other was in use.
- A failing GPU no longer fails a render. A step whose GPU code fails or runs out of GPU memory is computed on the CPU instead, with the same result, and the event log says which step and why. GPU memory now has a budget per device: when it is spent, older results kept on the GPU are dropped first. A GPU that stops answering or is lost (a driver reset) is restarted once and the render replayed; if that fails, the rest of the session renders on the CPU, and a message says so. No wait for the GPU can hang the application any more.
- Toggling a color label in the library no longer erases a label another
  program put on the image meanwhile, and taking a reject back restores the
  stars the image had. When a rating or label cannot be saved, the library
  shows the saved value again and says why, instead of showing a change that
  was lost.
- The day the library shows for a file's last change is the one of the system's time zone, no longer UTC's: a file saved late in the evening no longer reads as the next day.
- The names and locks given to mask members in the Masks panel are now
  saved with the image's edit, so they are still there after closing and
  reopening it.
- In the Parameters panel, a path's points no longer spill across the
  panel and push its feather and opacity out of view: they show as a
  summary ("12 points") that opens to list them, a point a line.
- The wheel over the Parameters panel scrolls it instead of changing the slider under the pointer; a slider takes the wheel once clicked, or while Alt is held. A held arrow (or a turned wheel) on a slider is now one undo step, and a value being typed lets go of the keyboard when you click elsewhere, so the image's keys work again.
- Rendering a network where several modules read the same inputs, as chained masked modules do, no longer slows down exponentially with the chain's length: each shared step is planned once per render. Six chained masked modules render about three times faster and with a seventh of the memory.
- Raw files whose sensor has masked photosites (many Canon cameras) now take
  their black level from the median of those photosites, as darktable does,
  instead of their mean. Shadows were slightly lifted and tinted before; the
  raw chain now matches darktable's within a Delta E 2000 of 0.14 on average.
- Raw photos previewed at reduced size keep their colours: the sensor data is
  no longer averaged before demosaicing, which mixed red, green and blue
  photosites into grey. Demosaicing now runs at full resolution and the result
  is reduced after it, as darktable does.
- A node started in a sandbox no longer stops when Ctrl-C is pressed in the terminal that started it: it runs until `helios sandbox stop`.
- Selecting a node is no longer an undo step: Ctrl+Z takes back the last
  edit, not the last selection.
- The system menu no longer repeats the window's own buttons: its Window
  submenu is gone.
- A long notice no longer runs under its close button: its text wraps before it.
- Edit > Undo (`Ctrl+Z`) and Redo undo parameter changes again, each one saved in the image's history, and the menu names what they take back ("Undo drag exposure"); they wait while a drag is in progress. The interface settings keep their own undo, in the settings window.
- A batch of rating or label changes is now saved whole or not at all: a
  crash or a failed write no longer leaves a change without its audit entry
  or half a batch applied, and several Helios processes can share one vault
  without failing as busy or recording a wrong previous value.
- Panning or zooming the image is no longer an undo step: undo goes back
  to the last edit, and moving the view no longer saves a new version of
  the edit.
- Raw edits saved by older darktable releases that never touched highlight
  reconstruction now import with the clipping darktable itself gives them
  when it opens them, instead of without it.
- Edits imported from darktable sidecars now run their modules in darktable's pipe order, custom orders and multiple instances included, instead of the order they were last edited in, so they render as in darktable. Modules the sidecar's order does not place are named in the import report.
