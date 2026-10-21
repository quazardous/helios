# Keyboard shortcuts: research and proposal

Status: **decided** (2026-10-21): the credo and the map are in
[gui.md](gui.md), "Keyboard shortcuts", with the three open choices
settled (picks with `P`/`U` and `X` reject, `6`–`9` colour aliases,
`Ctrl+/` for bypass). Research and proposal of 2026-10-11 below.

The GUI follows VS Code's model (one window per workspace, every action
a named command; see [gui.md](gui.md)); this note gathers how darktable,
VS Code, Lightroom, Capture One and node editors handle keys, then
proposes Helios's credo and a default map. darktable paths are relative to its source tree.

## darktable

**The system (reworked in 3.8, kept in 4.x).**

- Each view, utility module and processing module registers **actions** in
  one tree (`dt_action_register`, `dt_action_define`,
  `src/gui/accelerators.h`). A component suggests its own default key with
  `dt_shortcut_register`: there is no central keymap, and users note that
  no list of defaults exists
  ([pixls](https://discuss.pixls.us/t/where-can-i-find-the-default-shortcuts-on-github/54043)).
- An action has **elements** (a slider's value, its button, its color
  picker) and each element **effects** (activate, toggle, hold, up/down,
  reset, next/previous, presets).
- A shortcut is a key with modifiers, long or double press, a mouse button
  and clicks, and a **move** (scroll, drag in a direction), from keyboard,
  MIDI or gamepad: "hold E and scroll" is a shortcut.
- **Fallbacks** give implicit meanings (`src/gui/accelerators.c`): Ctrl
  slows a value by ten, Shift speeds it by ten, double click resets.
- `data/shortcutsrc` holds key and move bindings for modules (for example
  `e;scroll` on exposure). Holding `h` shows the shortcuts window, which
  also binds a widget by clicking it.
- Conflicts are scoped per view, and the user is asked to keep or replace
  ([2011 design](https://www.darktable.org/2011/06/darktables-new-keyboard-shortcut-system/)).

**Defaults found in the sources.**

| Scope | Keys | Where |
|---|---|---|
| Global | `t` `l` `d` `m` `s` `p` views; `Ctrl+Q` quit; `F11` fullscreen; `Tab` panels; `Ctrl+Shift+L/R/T/B` panels; `Ctrl+H` header; `Ctrl+F` filmstrip; `Ctrl+,` preferences; `h` (hold) shortcuts | `src/gui/gtk.c`, `src/control/control.c` |
| Rating, labels | `0`–`5`, `r` reject; `F1`–`F5` red, yellow, green, blue, purple | `src/libs/tools/ratings.c`, `colorlabels.c` |
| Lighttable | arrows, `Home`/`End`, `PgUp`/`PgDn` (Shift extends); `Space` toggle; `x` culling, `f` full preview, `w` (hold) sticky preview, `Ctrl+W` focus detection; `Ctrl+Z`/`Ctrl+Y` | `src/views/lighttable.c` |
| Images | `Ctrl+A`, `Ctrl+Shift+A`, `Ctrl+I` selection; `Ctrl+D` duplicate; `Ctrl+G` group; `Ctrl+C`/`Ctrl+V` history (Shift: selective); `Ctrl+E` export; `Ctrl+T` tag; `Ctrl+K` collection history | `src/libs/*.c` |
| Darkroom | `Space`/`Backspace` next/previous; `Alt+1/2/3` zoom; `w` (hold) preview; `a` (hold) pan; `o` overexposure; `Ctrl+S` softproof; `Ctrl+G` gamut; `g` guides; `Ctrl+B` color assessment | `src/views/darkroom.c` |

**What users complain about.**

- Rating keys stopped working when defaults followed keycodes rather than
  characters, on some layouts
  ([pixls](https://discuss.pixls.us/t/dt-4-0-how-to-assign-star-rating-with-keyboard/31723)).
- A removed default comes back after a restart; the workaround binds the
  key to nothing
  ([pixls](https://discuss.pixls.us/t/how-to-delete-a-keyboard-shortcut/29650)).
- Single-letter view switches fire by accident (`s` starts a slideshow)
  ([pixls](https://discuss.pixls.us/t/short-cut-behaviour/36189)).
- The system is powerful but hard to discover.

## VS Code

- Every action is a command; extensions declare `contributes.commands` and
  suggest `contributes.keybindings` (`command`, `key`, `mac`, `when`,
  `args`) ([docs](https://code.visualstudio.com/api/references/contribution-points#contributes.keybindings)).
- `when` clauses are expressions over context keys (focus, view, resource)
  ([docs](https://code.visualstudio.com/api/references/when-clause-contexts)).
- Chords (`Ctrl+K Ctrl+S` opens the shortcuts editor).
- The user's `keybindings.json` is applied after the defaults, the last
  match wins, and `"-command"` removes a default for good.
- The shortcuts editor searches by command or by recorded keys, shows
  bindings sharing a key and where each comes from (default, user,
  extension).
- Whole keymaps (Sublime, IntelliJ, Vim) install as packages.
- Defaults: palette `Ctrl+Shift+P`, quick open `Ctrl+P`, sidebars `Ctrl+B`
  and `Ctrl+Alt+B`, bottom panel `Ctrl+J`, split `Ctrl+\`, editor groups
  `Ctrl+1…9`, next editor `Ctrl+Tab`, close `Ctrl+W`, settings `Ctrl+,`.

## Lightroom Classic and Capture One

- Lightroom ([Adobe](https://helpx.adobe.com/lightroom-classic/help/keyboard-shortcuts.html)):
  `0`–`5` stars; `P` pick, `X` reject, `U` unflag; `6`–`9` colors; Caps
  Lock or Shift with a key to apply and advance; `G` grid, `E` loupe, `C`
  compare, `D` develop; `Tab` panels; `\` before and after.
- Capture One: `0`–`5`, a few color keys, everything remappable with named
  sets; many users import a Lightroom-like set.
- What photographers carry between tools: digits for stars, arrows, undo,
  `Tab` for panels, `P`/`X` for picks.

## Node editors

Blender (`Shift+A` add, `F3` search, `Ctrl+G` group, `Tab` enter a group,
`F` link, `Home` view all, `M` mute), Nuke (`Tab` search, `F` frame
selected, `Ctrl+G` group, `D` disable) and Fusion (`Shift+Space` search,
`Ctrl+G` group, `Ctrl+P` bypass) share: a search popup to add a node, `F`
to frame the selection, `Ctrl+G` to group, `Del` to delete. Connecting by
keyboard has no common key.

## Proposed credo

1. **Everything is a command** with a stable id (`culling.rate.3`,
   `graph.frameSelected`); keys bind to commands, never to widgets, and
   every command runs from the palette.
2. **Components suggest, the framework decides.** Modules and plugins
   suggest `{command, key, when}`; the resolver orders user > workspace >
   framework defaults > suggestions, the most specific `when` winning
   within a layer.
3. **Contexts are explicit**: `when` over named focus and view keys
   (`viewportFocus`, `graphFocus`, `textInputFocus`); a single letter never
   fires while typing.
4. **Conflicts are visible**: the shortcuts editor shows the source and the
   other bindings of a key; losing suggestions are listed.
5. **Removing a default is first-class** and survives restarts.
6. **Keys are logical characters**, top-row and keypad digits alike.
7. **Shared conventions first**: platform and editor keys (`Ctrl+S`,
   `Ctrl+W`, `Ctrl+F`, `Ctrl+Shift+P`) and photographers' habits (digits,
   arrows) before darktable's; darktable's keys stay where they collide
   with neither.
8. **Keymaps are packages**: a "Lightroom" map and a "darktable classic"
   map install as contributions.
9. **Rich triggers stay**, opt-in: darktable's key with scroll or drag,
   holds, double presses and fallbacks are part of the grammar; defaults
   use plain keys and holds.
10. **Discoverable**: holding `H` shows the keys of the current context,
    `Ctrl+K Ctrl+S` opens the editor, the palette and tooltips show keys.

## Draft default map

| Command | Proposed | darktable | Lightroom | Note |
|---|---|---|---|---|
| Command palette | `Ctrl+Shift+P` | — | — | `F1` stays a color label |
| Quick open (image, node) | `Ctrl+P` | `p` print | — | print becomes a palette command |
| Shortcuts editor | `Ctrl+K Ctrl+S` | — | — | `Ctrl+K` becomes the chord prefix |
| Keys of this context | `H` (hold) | `h` (hold) | — | kept |
| Settings | `Ctrl+,` | `Ctrl+,` | `Ctrl+,` | all agree |
| Side bars | `Ctrl+B` / `Ctrl+Alt+B` | `Ctrl+Shift+L/R` | — | color assessment moves |
| Bottom panel, filmstrip | `Ctrl+J` | `Ctrl+F` | — | `Ctrl+F` stays find |
| All panels | `Tab` (not in text or graph) | `Tab` | `Shift+Tab` | kept |
| Fullscreen | `F11` | `F11` | `F` | kept |
| Zen mode (the image alone) | `Ctrl+K Z` | — | — | VS Code; Escape leaves it |
| Split, editor group n | `Ctrl+\`, `Ctrl+1…9` | — | — | VS Code |
| Next, previous editor | `Ctrl+Tab`, `Ctrl+Shift+Tab` | — | — | VS Code |
| Close editor | `Ctrl+W` | focus detection | close | platform wins |
| Darkroom, lighttable | `D`, `L` | `d`, `l` | `D`, `G` | kept, not while typing |
| Rate 0–5 | `0`–`5` (keypad too) | `0`–`5` | `0`–`5` | universal |
| Reject | `R` | `r` | `X` | Lightroom map: `X` |
| Pick, unflag | `P`, `U` (not decided) | — | `P`, `U` | if Helios adopts picks |
| Color labels | `F1`–`F5` | `F1`–`F5` | `6`–`9` | `6`–`9` as aliases (not decided) |
| Apply and advance | Shift with the key; `culling.autoAdvance` | — | Shift, Caps Lock | Caps Lock in the Lightroom map |
| Move, extend selection | arrows, `Home`, `End`, `PgUp`, `PgDn` (Shift) | same | same | kept |
| Next, previous image | `Space`, `Backspace` | same | arrows | in a grid `Space` toggles selection |
| Culling, full preview | `X`, `F` | `x`, `f` | `C`, `F` | kept |
| Sticky preview | `W` (hold) | `w` | — | kept |
| Selection all, none, invert | `Ctrl+A`, `Ctrl+Shift+A`, `Ctrl+I` | same | — | kept |
| Copy, paste edits | `Ctrl+C`, `Ctrl+V` (Shift: selective) | same | `Ctrl+Shift+C/V` | when the grid has focus |
| Undo, redo | `Ctrl+Z`, `Ctrl+Y` and `Ctrl+Shift+Z` | `Ctrl+Z`, `Ctrl+Y` | `Ctrl+Z`, `Ctrl+Shift+Z` | both redos |
| Zoom in, out | `Ctrl+=`, `Ctrl+-` | same | same | kept |
| Zoom 100%, fill, fit | `Alt+1`, `Alt+2`, `Alt+3` | same | `Z` | kept |
| Pan | `A` (hold), Space-drag | `a` (hold) | Space-drag | kept |
| Overexposure, guides | `O`, `G` | `o`, `g` | `J` | kept |
| Softproof | `Ctrl+Alt+S` | `Ctrl+S` | `S` | `Ctrl+S` is save |
| Gamut check | `Ctrl+Alt+G` | `Ctrl+G` | — | `Ctrl+G` is group |
| Before, after | `\` | — | `\` | Lightroom |
| Export | `Ctrl+E` | `Ctrl+E` | `Ctrl+Shift+E` | kept |
| Graph: add node | `Shift+A`, `Tab` in the graph | — | — | Blender, Nuke |
| Graph: frame selection, all | `F`, `Home` in the graph | — | — | `F` is full preview elsewhere |
| Graph: group, ungroup | `Ctrl+G`, `Ctrl+Shift+G` | image group | — | one meaning everywhere |
| Graph: bypass node | `Ctrl+/` (bound 2026-10-21) | — | — | no common key |
| Graph: delete, dissolve | `Del`, `Ctrl+Del` | remove image | — | dissolve reconnects |
| Graph: connect | palette only | — | — | no common key |

## darktable keys that do not fit

- `Ctrl+S` softproof, `Ctrl+W` focus detection, `Ctrl+F` filmstrip,
  `Ctrl+H` header, `Ctrl+K` collection history, `Ctrl+B` color assessment:
  they take platform or editor keys (save, close, find, chords, side bar).
- `s` slideshow, `p` print, `t` tethering, `m` map as global letters: they
  fire by accident; they become palette commands, `D` and `L` stay.
- `Ctrl+Shift+L/R/T/B`: panels by position, replaced by area commands.
- `Ctrl+G` gamut check and `Ctrl+X` with two meanings by view: they clash
  with group and cut.
- Module suggestions on `F1`–`F12` with scroll: they sit on the color label
  keys and fullscreen; as suggestions they rank last and the editor shows
  them.
- Keycode-based digits, silent fallbacks and defaults that come back after
  removal: the credo's points 5, 6 and 9 answer them.
