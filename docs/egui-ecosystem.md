# The egui ecosystem: what Helios adopts, wraps or builds

Status: **survey** (2026-10-11), with a decision per need. It answers:
before writing a widget, does a crate already do it? Versions are the
latest on crates.io on that date; "0.36" means the crate depends on egui
0.36, the version Helios uses. Every crate named is MIT and/or Apache-2.0
unless said otherwise, so compatible with GPLv3.

**What decides.** Helios keeps one source of truth (the graph, the value
store, the command registry and keymap) and draws views over it; anything
an agent or a test must drive is data, not widget state. Crates are judged
on that, on egui 0.36 support and on maintenance. Most egui crates hold
their own state (a queue of strings, their own graph), so they become
**drawing helpers at most**; egui's own primitives (Modal, Popup, menus)
are the preferred foundation, since they follow egui releases, AccessKit
and keyboard work.

## Decisions

| Need | Crate(s) looked at | Decision |
|---|---|---|
| Node graph editor | egui-snarl 0.12 (0.36, owns the graph in `Snarl<T>`, weak selection, no grouping, semantic zoom or AccessKit); egui_graph (nannou, model-agnostic but egui 0.34); egui_graphs (petgraph viewer); egui_node_graph (gone), forks stale | **Own widget** (`graph_view`), taking snarl's wire and pin interaction and egui_graph's "the view holds only positions and camera" |
| Scopes | egui_plot 0.37 (lines, bars) | **Own**: scopes are density images fed by the engine; egui_plot for debug panels only |
| Sliders | egui `Slider` / `DragValue` (no soft and hard range, fine and coarse, label in the bar); egui_double_slider; egui_knob | **Own** darktable-style slider (done); egui_double_slider and egui 0.37's `RangeSlider` as references for ranges |
| Curves, colour wheels | no maintained curve editor or wheel; egui `color_picker` | **Own** curves and wheels; egui's picker for simple swatches; egui_colors (12-step palettes) for theme work |
| Toasts | egui-notify 0.23, egui-toast 0.22 (string queues, no command actions, no inspectable state) | **Own** (done), ideas for animation and stacking from egui-notify |
| Modals, confirmations | egui `Modal` (built in); egui-modal (dead, egui 0.30) | **Own layer on `egui::Modal`** (already so): the semantics (verbs, danger, agent answers) are ours, backdrop and focus are egui's |
| Forms | egui-probe (binds Rust structs, not descriptors) | **Own**, from value descriptors |
| Menus | egui `MenuBar` / `MenuButton` / `SubMenuButton`; muda (native, GTK on Linux) | **Own declaration layer, drawn with egui's menu primitives**; muda only for a macOS global menu later |
| Command palette, fuzzy search | no egui crate; re_ui's palette (tied to Rerun); nucleo-matcher (MPL-2.0) | **Own**; nucleo-matcher if ranking is needed |
| File and folder dialogs | rfd 0.17 (native, XDG portal); egui-file-dialog 0.15 (drawn in egui, testable) | **Adopt rfd** as the default, **wrap egui-file-dialog** as the in-app fallback that tests can drive, both behind one Browse path |
| Docking | egui_dock 0.21 (in use); egui_tiles 0.17 (rerun) | **Keep egui_dock**; neither vetoes a drop by destination, so area rules stay a check of our own; an upstream `allowed_destination` hook is worth proposing |
| Tables, grids | egui_table 0.10 (rerun, virtualized); egui_extras `TableBuilder`; egui_virtual_list | **Adopt egui_table** for metadata tables; thumbnail grid on `ScrollArea::show_viewport` (own) |
| Icons | egui-phosphor 0.14 (0.36, font merged as a fallback family); egui_material_icons | **Adopt egui-phosphor**: it fixes the missing glyphs (✓ ▸ ◆ ●) without painting them by hand |
| UI kits, themes | re_ui (Rerun's, too coupled), catppuccin-egui (0.33), egui-aesthetix (0.28) | **Ideas only** from re_ui (tokens, list items, palette); design our tokens to map onto egui 0.37's `StyleProvider` |
| i18n, right-to-left | egui-i18n; egui has no bidi shaping | **Own catalogs** (see [i18n-research.md](i18n-research.md)); right-to-left waits for egui |

## Next steps this asks for

1. Adopt **egui-phosphor** and replace the painted ticks, arrows and marks.
2. Path fields: **rfd** by default, **egui-file-dialog** as the fallback.
3. Draw the menus' submenus and popups with egui's menu primitives
   wherever ours still draw by hand; keep the declared model.
4. Graph editor: own widget, with snarl's wire and pin interaction as the
   reference.
5. Watch **egui 0.37**: system-font glyph fallback, `StyleProvider`,
   `RangeSlider`, `CompletionPopup`, AccessKit trees for popups and menus;
   and `egui_inspection` / `egui_mcp` for agents driving the GUI.

**Avoid** (dead or stuck): egui_node_graph and its forks, egui-modal,
egui_hotkey, egui-keybind as a dependency, catppuccin-egui, egui-aesthetix,
muda on Linux.
