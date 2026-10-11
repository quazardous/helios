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

**One window is one workspace (decided 2026-10-11).** A window holds one
project; another project opens another window. A detached panel opens an
auxiliary window of the same workspace: closing it docks the panel back,
closing the workspace closes them all. Tabs live inside editor groups
(several graphs or images side by side), never at the project level.

The shell is a framework from the start:

- **areas**: editor groups (tabbed, splittable) in the centre, side and
  bottom panels, an activity bar, a status bar;
- **panels are declared** (id, title, default area, commands), so the shell
  places, saves and restores them and exposes them to shortcuts and the
  palette;
- **every action is a named command** (`view.graph.focus`,
  `panel.queue.toggle`), the same for menus, shortcuts, the command
  palette, agents (control API) and tests;
- **navigation by keyboard**: editor groups, tabs, the palette, quick open
  (an image, a node), a key per panel, cycling between areas; the map
  follows darktable's keys where they fit a clear credo, and is being
  researched;
- the layout, auxiliary windows and their screens included, is saved per
  workspace in the user scope.

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

## Themes and profiles

Status: **decided** (2026-10-11). Research:
[profiles-research.md](profiles-research.md). Built: themes
(`helios-theme`, the GUI's mapping and commands) and profiles
(`helios-profile`, export, import with a diff, a workspace's profile).
Not yet: applying a theme's font families, the keyring API for plugins,
sync through the vault.

### Themes

- **The format.** A theme is a token file in the Design Tokens format
  (DTCG JSON), `*.helios-theme.json`. Its tokens are semantic:
  - surfaces: `bg.panel`, `bg.canvas`;
  - text: `fg.text`, `fg.weak`;
  - `accent`;
  - node states: `state.running`, `state.failed`, `state.pending`;
  - module families: `family.tone`;
  - graph wires;
  - sizes: `radius.widget`, `space.item`;
  - type: `font.ui`, `font.mono`, `font.size`.
- **Tokens only.** Code maps tokens onto `egui::Visuals` and `Style`. A
  widget reads tokens, never a literal colour. egui's own fields are never
  saved, so an egui upgrade cannot break a theme. Settings without a
  semantic token (`striped`, handle shape) go under `$extensions` with the
  key `io.github.quazardous.helios`.
- **Inheritance.** A theme `$extends` a base: Helios Dark, Helios Light,
  high contrast. A plugin's theme may extend any installed theme. When a
  base is missing, the theme falls back to Helios Dark and says so.
  Aliases (`{accent}`) let a theme change one colour and keep the rest
  consistent.
- **Overrides.** User overrides are a token file per theme, or for every
  theme, in the user's settings: the counterpart of VS Code's `[Theme]`
  keys and darktable's `user.css`. They are applied last and validated;
  unknown tokens are reported, not fatal.
- **Settings outside the theme.** Font size, interface scale and the
  dark/light/system preference are ordinary settings, so changing the
  theme does not undo them.
- **Live preview** in the theme picker. `theme.export` writes the current
  theme with its overrides merged into one file.

### Profiles

- **A profile is a named set of parts, each optional:**
  - settings of the `user` concern;
  - keymap layers (User);
  - theme and overrides;
  - panel layouts;
  - plugins;
  - export targets;
  - AI and workshop preferences.

  A part a profile leaves out comes from the **Default** profile. Several
  profiles per user (work, client, teaching).
- **Workspaces choose.** A workspace names its profile, as a VS Code
  folder does; the window title and status bar show it.
- **What stays out of profiles:**
  - presets and styles: a library of looks shared by every profile, so
    switching profiles never hides one;
  - the vault and its images;
  - settings of the `machine` concern;
  - secrets, which live in the OS keyring (Secret Service, Keychain,
    Windows credentials) behind a keyring API for plugins, and never in a
    file.
- **Export.** One readable file, `*.helios-profile` (TOML with a part per
  section), or only the chosen parts. Plugins are listed by id, pinned
  version, registry and enabled state, with their settings minus those
  their manifest marks `secret` or `machine`. A secret scanner runs before
  writing and refuses tokens that look like keys.
- **Import.** It shows what changes, part by part, as a diff, and lists
  the plugins to install with their capabilities, reviewed once for the
  set. It installs them in bulk from their registries; an optional signed
  bundle carries them for offline use. Any part can be left out.
- **Keymaps across systems.** Bindings use `Mod` (Ctrl on Linux and
  Windows, Cmd on macOS); a binding may still name a system. MIDI and
  controller bindings are a separate part, since devices differ between
  machines.
- **Sync.** Later, through the vault (the user's own server), never a
  third-party service. Conflicts show as a diff per part, and the last 20
  versions are kept.
