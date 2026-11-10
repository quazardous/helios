# egui_hud

Controls as data for [egui](https://github.com/emilk/egui) applications:
what a control is (purpose, value, units, ranges, feel, detents,
read-out), the representations it takes on each surface (panel row,
compact field, HUD chip), cards laid out on a slot engine
([egui_gridflow](https://github.com/quazardous/egui_gridflow)) and
resolved in context, and a value flow of gestures (begin, preview,
commit as one undo step through a vetoable change step, or cancel with
no trace).

The crate knows nothing of the application it serves. It asks for:

- a store, through the `ValueSource` and `ValueStore` traits (read
  values and parameter descriptors; begin, preview, commit and cancel
  gestures). `MemoryStore` is a small one in memory, for tests and
  benches;
- parameter descriptors (`ParamDescriptor`), from which a parameter no
  control is patched to gets an implicit control;
- a command registry of its own: the `controls.*` commands are plain
  functions (`commands::COMMANDS`, `commands::run`) the host declares.

The renderers on [egui-pop](https://github.com/quazardous/egui-pop) are
behind the `render` feature; without it the crate has no egui
dependency. The `bench` feature adds the bench: a shelf of a host's
controls, composed onto a card and tried live (context bar, modes,
presets, undo and redo, and why each control takes its form).

## Demo

**[Try it in a browser](https://quazardous.github.io/helios/egui_hud/)**:
a mixer channel and a light, declared as data, on the bench
([source](../egui_hud_demo)).

## Status

Developed inside the Helios workspace while its API settles, under the
workspace's licence (GPL-3.0-or-later). It is meant to move to its own
repository under MIT OR Apache-2.0 once the API is stable.
