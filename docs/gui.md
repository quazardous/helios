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

Built: the areas, docked with `egui_dock` from the workspace's layout,
with auxiliary windows and tabs dragged between areas under each panel's
rules (`helios-gui/src/workbench.rs`), drawn inside egui-pop's
application (see "Windows, dialogs and menus") until egui-pop's workbench
takes them over.

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
(Helios's overlays of egui-pop's themes, in `helios-gui`, and their
commands) and profiles
(`helios-profile`, export, import with a diff, a workspace's profile).
Not yet: applying a theme's font families, the keyring API for plugins,
sync through the vault.

### Themes

- **The format.** A theme is a token file in the Design Tokens format
  (DTCG JSON), `*.helios-theme.json`, resolved by egui-pop (`egui_pop::theme`), the UI library
  Helios is built on; its token guide is the reference for the format,
  the tiers and the contract. The tokens are semantic:
  - surfaces: `surface.panel`, `surface.canvas`, `surface.overlay`;
  - text: `text.default`, `text.muted`;
  - `accent.default`, `danger`, `warning`, `success`, `info`;
  - sizes: `radius.widget`, `space.m`, `size.dialog.width`;
  - type: `font.size.body`, `font.family.ui`, `font.family.mono`;
  - component tokens: `button.*`, `dialog.*`, `toast.*`.
- **Helios's own tokens.** What egui-pop's contract does not have is a
  Helios component token, in each Helios theme: node states
  (`state.done`, `state.running`, `state.stale`, `state.failed`,
  `state.pending`), the destructive button (`button.danger.background`,
  `button.danger.text`) and the neutral surround behind a photo
  (`viewport.surround`). A theme without them falls back to the nearest
  egui-pop token. Still to come: module families (`family.tone`), graph
  wires.
- **Tokens only.** egui-pop maps tokens onto `egui::Visuals` and `Style`.
  A widget reads tokens (`Tokens::of(ctx)`), never a literal colour.
  egui's own fields are never saved, so an egui upgrade cannot break a
  theme. Unknown data goes under `$extensions` with the key
  `io.github.quazardous.helios`.
- **Inheritance.** A theme `$extends` a base. Helios Dark, Helios Light
  and Helios High Contrast are thin overlays of egui-pop's `pop-dark`,
  `pop-light` and `pop-high-contrast`. A plugin's theme may extend any
  installed theme. When a theme is missing, Helios falls back to Helios
  Dark and says so. Aliases (`{accent.default}`) let a theme change one
  colour and keep the rest consistent.
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
- **Variants per system or machine.** Any part may hold variants under
  `[on.<selector>]` (`[on.macos.theme]`, `[[on.machine.laptop.keymap]]`),
  overriding its common values. A selector is a system (`macos`, `linux`,
  `windows`), a screen class (`hidpi`) or a named machine
  (`machine.<name>`); a machine is named once, in `machine.toml` in the
  config directory (`name = "laptop"`), never in a profile, never synced.
  A profile is resolved for this machine when applied: common values,
  then the system's, the screen class's, the machine's, the most specific
  winning. `profile.resolved` says which values came from which variant.
  Variants for other machines, or selectors Helios does not know, stay in
  the file unapplied. They travel with an export and show per selector in
  an import's diff; `profile.export` with `this_machine` writes only this
  machine's view, flattened. Machine settings never enter a profile,
  variants included.
- **Sync.** Later, through the vault (the user's own server), never a
  third-party service. Conflicts show as a diff per part, and the last 20
  versions are kept.

## Windows, dialogs and menus

**On egui-pop's application framework (2026-10-12).** The desktop is an
egui-pop application (`egui_pop::api::App`, with its `DefaultPlugins`):
egui-pop draws, from declarations, what every application shares, and
Helios declares into it:

- **the menu bar** (File, Edit, View, Help) and every menu, the
  **command palette** (`Ctrl+Shift+P`, `F1`: a fuzzy match, the recent
  commands first, a command's arguments asked for), the **dialogs**, the
  **toasts**, the **settings window** (`Ctrl+,`) with its **shortcuts
  editor** (a command's keys changed by pressing them, conflicts said,
  each change taken back by its reset) and the **window's place** (size,
  position, maximized, full screen with `F11` and a way out at the top
  edge), kept between runs with the interface's other settings (focus
  mode, the user's keys) under `~/.config/helios/ui/`;
- **keys**: egui-pop's keymap resolves them (layers, `when` clauses, a
  text field keeping plain keys), with the traces its debug bridge shows.

Helios keeps what is its own: its title bar and status bar, its areas
(docked panels, auxiliary windows, tabs dragged between areas, see
"Panels"), drawn as a block of egui-pop's main zone until egui-pop's
workbench takes them over, and its panels.

**Commands run on Helios's kernel.** egui-pop wraps the kernel through a
host contract (`helios_gui::host`): every command of Helios's registry is
declared to egui-pop (its menus, palette, keymap and traces see it) and
runs on the registry, the control surface agents and scripts reach too.
A command whose work waits for the next frame shows as running until its
result arrives, and its failure is shown; egui-pop's events (a command
dispatched, done or failed, a dialog answered, a toast shown) go on
Helios's bus under `gui.*` kinds, with their sequence number, source and
cause, beside the engine's; events from elsewhere wake the window.

**Normalized (decided 2026-10-11).** Panels, widgets, modules and plugins
never draw a window, a popup, a menu or a dialog: they ask the shell
(`helios_gui::dialogs`), which hands them to egui-pop's components, the
same everywhere, drawn from the theme's tokens. A test fails if anything
else creates one (`crates/helios-gui/tests/normalized.rs`). Every open
dialog is also data: `dialog.current` describes it and `dialog.answer`
answers it, so an agent sees and answers what a human would, its origin
recorded.

- **Confirm**: a question, its consequences in one line, Cancel then the
  action on the right. The action is named by its verb ("Delete 3
  images", never "OK"). A destructive one uses the danger style and the
  focus starts on Cancel, so Enter does nothing harmful; otherwise Enter
  is the action and Escape cancels. `dialog.assumeYes` answers yes by
  policy, for scripts, and says so each time.

  ![A destructive confirmation](gui/confirm-helios-dark.png)

- **Form**: fields built from value descriptors (the value store's: type,
  range, steps, unit, choices), the same footer; never a free layout.
  Answered, it runs its command with the values.

  ![A form](gui/form-helios-light.png)

  A **path field** (a descriptor made with `Descriptor::path`: a
  directory, or a file with its accepted extensions) is a text field and
  a Browse… button opening egui-pop's in-app picker: the folder, Up, its
  subfolders (and the accepted files); Choose this folder, or a file,
  fills the field; Close the picker goes back to the form. "Open a
  workspace" is such a form. Not yet: the system's chooser (egui-pop's
  would block the window while it is open), the recent workspaces offered
  first, the picker walked by keys.

- **Picker**: a list to choose from, egui-pop's choice; its command runs
  with the value chosen. A filter field above the list narrows it as one
  types, the arrows move through it and Enter takes the highlighted
  entry.
- **Menu and context menu**: entries are commands, nothing else. Titles
  come from the commands, keys from the keymap, so a menu never shows a
  key that does not work; an entry whose `when` does not hold is greyed
  with the reason.

  ![A menu](gui/menu-helios-dark.png)

- **Notice**: shown in the status bar; warnings and errors also as a
  toast at the bottom right, with an optional action (a command). A
  notice never takes the focus; errors stay until dismissed.

  ![A toast](gui/toast-helios-dark.png)

- **Window**: auxiliary windows share the frame Helios draws (title bar
  and buttons), titled "`<panel>` — Helios".

**Menus are declared** (egui-pop's menus): a menu is an id to which the
shell, a panel or a plugin contributes items, each a command (with
arguments and a label of its own), or a submenu. Items sit in ordered
groups, separated when shown; a `when` clause hides or greys an item, a
`checked` clause ticks it. `Shell::menu_items(id)` gives a menu as data,
as it would open now. The logo at the top left opens the `helios.system`
menu (Alt+Space too): open a workspace, recent workspaces, profiles,
themes (the one in use ticked), keyboard shortcuts, about, quit (the
window has its buttons). egui-pop's menus have no list filled when a menu
opens: the profiles and recent workspaces are declared again before the
system menu shows.

![The system menu](gui/system-helios-dark.png)

The pictures are written by `HELIOS_DOC_PICTURES=$PWD/docs/gui cargo test
-p helios-gui --test dialogs pictures` (and `--test params pictures`), from
the repository's root: headless, never on the desktop.

## Parameters

Parameter widgets are generated from value descriptors and bound to the
value store (see [widgets.md](widgets.md)); a widget never holds its
value. A number is a slider over its soft range: a drag is one gesture
(one undo step; an agent's write meanwhile is refused), the arrows step
(Shift coarse), a double click resets. The wheel
scrolls the panel: a slider takes the wheel only when it has focus or
while a modifier is held, so scrolling past sliders never changes a value
(decided 2026-10-12, after darktable's most complained-about default).
Enter types a value
(units and either decimal mark accepted; past the soft range it is forced
up to the hard one, and says so). A dot on the left marks a value changed
from its default (click: reset); a diamond on the right, a value just
moved by someone else (the tooltip says who); an amber dot, a value held
by another origin's gesture. Each widget is found by its path
(`param:<path>`), is a slider to assistive technology, and its context
menu resets, copies the path or copies the value.

![The Parameters panel, the exposure moved by an agent](gui/params-helios-dark.png)

## Image viewport

The image panel is egui-pop's viewport (`egui_pop::viewport`, decided
2026-10-11, built 2026-10-12): egui-pop draws, Helios supplies the pixels
and the colour (`helios-gui/src/viewport.rs`).

- **Pixels.** The engine is the viewport's tile source
  (`engine_source.rs`): a proxy of the whole image at a power of two
  first, then the exact tiles of what is visible at the view's scale,
  rendered on a thread of their own a union of neighbouring tiles at a
  time; a tile the view no longer needs is skipped. An edit renders
  again; the old tiles stay on screen until the new ones arrive.
- **Colour.** Tiles are linear Rec.2020 floats (the working space); the
  display transform, a matrix to sRGB primaries then egui-pop's clamp and
  encoding, runs in egui-pop's shader on the GPU.
- **The view** lives in the value store (`view/viewport/zoom`,
  `center_x`, `center_y`, `fit`), so commands, keys and agents move it.
  The zoom counts physical screen pixels per image pixel: 100 % is one
  image pixel on one screen pixel at any window scale. A drag is one
  gesture (an agent's write meanwhile is refused); none of it is an undo
  step nor saves the edit.
- **Gestures** are egui-pop's: a drag (the middle button anywhere, or
  Space and drag) pans and Escape puts it back, the wheel zooms around the
  pointer and stops at 100 % when it crosses it (Shift and the wheel
  pans), a double click fits; with the viewport focused, `+` and `-` walk
  the zoom ladder, `0` fits, `1` is 100 %, `2` is 200 %, the arrows pan. The commands
  `viewport.zoom.fit`, `viewport.zoom.100`, `viewport.zoom.in` and
  `viewport.zoom.out` do the same from the palette and for agents.
- **Before and after.** `viewport.compare` shows them side by side
  ("before" left of a split dragged by its handle, `view/viewport/split`);
  holding `\` shows "before" alone. "Before" is the edit as it was
  opened (not decided: the unedited image, or a snapshot taken by hand).
- **Readout.** The working-space R, G, B of the pixel under the pointer
  show in the bottom left corner.
- **Pictures.** `viewport.display` is what is on screen,
  `viewport.scene` the float data of the view before the display
  transform (`gui.snapshot`).

![The image viewport, the readout under the pointer](gui/viewport-wbox.png)

The picture is a run of `helios-gui --image` in wbox (a headless nested
compositor). Beside the same run of the viewport it replaced, the image's
pixels differ by at most one code value (egui-pop's tiles are half
floats), 96 % not at all; the surround is now the theme's
`viewport.surround`, aliased to egui-pop's `viewport.background`.
