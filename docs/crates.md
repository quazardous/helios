# Crates and modules

How the Helios repository is split: the main crates of the Cargo workspace, the ported modules, and the tools. For building and testing, see [CONTRIBUTING.md](../CONTRIBUTING.md).

A Cargo workspace (Rust):

- `crates/helios-core`: the domain-agnostic core: module contract and
  manifest, graph, an engine that evaluates a network without computing the
  same content twice, the event bus, `describe()` snapshots.
- `crates/helios-testkit`: synthetic images and contract tests generated
  from a module's manifest.
- `crates/helios-inspect`: the read-only inspection endpoint of a Helios
  process (state snapshots, bus events, waiting on an event) as JSON lines
  over a local socket.
- `crates/helios-photo`: the photo domain: raw mosaics and their color
  filter arrays, camera metadata as context, color spaces (linear Rec.2020
  working space).
- `crates/helios-vault`: the vault: images identified by content (BLAKE3),
  locations as data, folder import and repair scan, field commands and
  cursors, over an abstract store and its contract (an in-memory store
  for tests).
- `crates/helios-vault-sqlite`: the SQLite driver of the vault store.
- `crates/helios-storage`: the volumes files live on (disks, cards,
  shares): identity, kind, roles, Linux detection, and the registry of
  those the vault knows.
- `crates/helios-connect`: import connectors: the trait every import
  source implements (paged listing with a cursor, items, access, fetch),
  their manifests and registry, the folder and card connectors, and the
  contract suite they pass.
- `crates/helios-load`: loaders, the framework reading files into sources:
  camera raws through rawler (sensor data and camera description), JPEG,
  PNG and TIFF pictures in the working space, embedded previews.
- `crates/helios-xmp`: darktable XMP sidecars read into Helios: history,
  params decoded with darktable's own layouts (`layouts.json`, from
  `tools/dt_param_layouts.sh`), mapped to Helios modules with a report of
  what is not carried over.
- `crates/helios-vkdt`: vkdt module files (`params`, `connectors`,
  `params.ui`) read into Helios manifests and written back unchanged.
- `crates/helios-gpu`: the GPU backend on wgpu: modules' WGSL variants run
  on the same plans as their CPU code and are checked against it.
- `crates/helios-compute`: compute units: the local machine's CPU and GPUs
  (and declared remote nodes), their machine-scope settings (enabled,
  role, priority, budgets), a calibration benchmark, and routing a job to
  a device with a CPU fallback; the `compute.*` commands.
- `packaging/linux`: the launcher and icons for GNOME and other freedesktop
  desktops (`install.sh` installs them for the current user).
- `crates/helios-shell`: the desktop shell's logic without a GUI toolkit:
  commands, `when` contexts, keymap layers, declared panels, workspace
  layouts.
- `crates/helios-export`: exports, the leaves that write files: a node
  rendered, converted from the working space to sRGB, encoded (JPEG, PNG 8
  or 16-bit, TIFF 16-bit) and written atomically, as jobs that hold their
  destination so two writes to one file never race.
- `crates/helios-decision`: decision nodes: resolution modes (full AI,
  AI-assisted, human), recorded and versioned answers (who decided, which
  model, on which inputs), replayed offline into a network.
- `crates/helios-journal`: the workflow level: a journal per item and node
  (claims, leases, joins, choices, retries), after grampy, with a contract
  every storage must pass.
- `crates/helios-config`: settings declared once, resolved through layers
  (defaults, global, project, user, run), each value reported with its
  origin.
- `crates/helios-node`: `helios-node`, a headless node that loads a network
  from a JSON description, renders it and serves inspection; and `helios`,
  the command line (`helios inspect state`, `events`, `wait-event`,
  `wait-idle`; in debug builds `helios control render`, `set-param`,
  `pause`, `fault`...; `helios sandbox new|start|stop|clean` for isolated
  nodes); and `helios-mcp`, an MCP server giving an agent the same verbs,
  sandboxes included.
- `modules/sigmoid`: darktable's sigmoid tone mapper (per channel, hue
  preservation), ported.
- `modules/blur`: a Gaussian blur whose radius is in full-resolution
  pixels, the same look at any render scale.
- `modules/exposure`: darktable's exposure module (manual mode, camera bias
  compensation), ported.
- `modules/rawprepare`: darktable's rawprepare (black level and scale from
  the camera's description), ported.
- `modules/whitebalance`: darktable's white balance (as shot or user
  coefficients, on the mosaic), ported.
- `modules/highlights`: darktable's highlights, clip mode, ported.
- `modules/demosaic`: mosaic to camera RGB, bilinear for Bayer and X-Trans.
- `modules/colorin`: darktable's input color (camera matrix to the linear
  Rec.2020 working space), ported.

Building and testing: see [CONTRIBUTING.md](../CONTRIBUTING.md).

## Tools

- `tools/iop_inventory.py`: regenerates the module inventory from a darktable
  checkout (`../darktable` by default, or `$DARKTABLE_SRC`).
- `tools/vkdt_inventory.py`: regenerates the vkdt module inventory from a vkdt
  checkout (`../vkdt` by default, or `$VKDT_SRC`).

Tooling that serves darktable itself (phase 0 SQL inventory, equivalence
bench) lives in the darktable fork under `tools/helios/`.

