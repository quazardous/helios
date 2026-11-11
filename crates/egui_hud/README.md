# egui_hud

Controls as data for [egui](https://github.com/emilk/egui) applications:
what a control is (purpose, value, units, ranges, feel, detents,
read-out), the faces that draw it and the representations they take on
each surface (panel row, compact field, HUD chip), cards laid out on a slot engine
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

## Controls and faces

A **control** is a value (its type, range, unit, default) and a
behaviour (its connector: jacks, gestures, intents, signals). It says
nothing about drawing. A **face** is a way to draw a value type, with
its slot sizes per representation; `egui_hud::face` holds the library
every control shares:

| value | faces |
|---|---|
| bool | checkbox, switch, push button, lamp (read-only) |
| number | slider, fader, knob, endless knob, scrub field, meter (read-only), read-out (read-only) |
| one-of | menu, segmented buttons, radio list |
| points, colour, text | curve and point list, colour wheel, text field |

A face fits values, not only a type: the slider, fader and knob need a
bounded number; the endless knob needs one that `wraps` (an angle, a
hue) or is `open`, and moves it by relative nudges. A control may name
the faces it accepts, in order (a gain: `["fader", "knob"]`); otherwise
it accepts every face that fits its value. A kit names faces by id, or
by an object for what one control changes
(`{"face": "fader", "integration": ...}`). Which face shows is
presentation: the first accepted by default, another when the context's
slot is too small for it, or the one the person picks on the bench.

The switch and the endless knob are drawn for now by the nearest
existing renderer (the checkbox, the knob); the model already names
them.

## Generic controls first

A checkbox stays a checkbox whatever its domain. Register a control of
its own only when it adds something beyond its value type: a unit and a
range (a gain in decibels), a choice of faces of its own (a fader and a
knob). Otherwise use the generic control of the value type: a
domain use of it is a parameter (its label, description, default and
choices in a `ParamDescriptor`), and the bench's shelf lists the generic
controls, one per value type, under "Miscellaneous".

## Demo

**[Try it in a browser](https://quazardous.github.io/helios/egui_hud/)**:
a mixer channel and a light, declared as data, on the bench
([source](../egui_hud_demo)).

## Status

Developed inside the Helios workspace while its API settles, under the
workspace's licence (GPL-3.0-or-later). It is meant to move to its own
repository under MIT OR Apache-2.0 once the API is stable.
