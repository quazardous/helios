# egui_hud demo

A web demo of [egui_hud](../egui_hud), the controls framework, laid out by
[egui_gridflow](https://github.com/quazardous/egui_gridflow) and drawn
with [egui-pop](https://github.com/quazardous/egui-pop).

**Live:** <https://quazardous.github.io/helios/egui_hud/>

## What it shows

A neutral catalogue declared as data in [`kit.json`](kit.json), with no
application behind it:

- **a mixer channel**: a gain fader, a pan knob, a mute toggle, a level
  meter (an indicator, following a simulated signal), a colour tag, and an
  equaliser case under its legend: an EQ curve drawn by egui-pop's curve
  editor where it has room, as rows of points where it has not, and a low
  cut toggle;
- **a light**: power, intensity, colour temperature, a colour wheel (its
  swatch and channels where the wheel does not fit) and an effect.

They sit on egui_hud's **bench** (`egui_hud::bench`, feature `bench`):

- the shelf, searchable; drag a control onto the bench, or click it;
- **Arrange**: move, select, group into a case under a legend, remove;
- the handle on the bench's right edge sets its width; every control takes
  the form that fits (full, compact, simplified), its orientation and its
  slot, with hysteresis;
- the values live in a `MemoryStore`, through the value flow: each gesture
  is one undo step, with undo and redo;
- the context bar: task, detail, image flags, tool, input device,
  density;
- the modes: panel, dock, toolbar, HUD (chips), float;
- the Placements pane says, per control, its slot, form and orientation,
  and why: the detail level against its tier, a rule of the card, the
  slot engine, a fallback;
- presets that show the resizing: the gain is a fader two columns wide, a
  knob under one column, a scrub field under three quarters of one; at the
  essential detail the other parts fold under "More (n)";
- the bench saved and loaded as text (and downloaded as a file in a
  browser).

## Running it

In a browser, with [Trunk](https://trunkrs.dev):

```bash
cd crates/egui_hud_demo
trunk serve        # or: trunk build --release
```

Natively: `cargo run -p egui_hud_demo`.

The model's tests run headless: `cargo test -p egui_hud_demo`.

## The catalogue

`kit.json` holds:

- `controls`: egui_hud controls (`egui_hud::model::Control`) that add
  something beyond their value type (the gain in decibels as a fader or
  a knob, the level meter, pan, the light's intensity and temperature), each with its
  value, connector, variants and their slot forms per representation,
  orientations and integration (who draws it on which surface, and its
  fallback), feel, detents and read-out;
- `groups`: each control's shelf group;
- `parameters`: domain uses of the generic controls, from their
  descriptors (mute, power, the low cut: toggles; the colour tag and the
  effect: choices; the EQ curve, the colour wheel, a text field);
- `implicit`: whether the shelf lists the generic controls, one per
  value type, under "Miscellaneous";
- `presets`: benches as data (composition, rules, context, mode, width);
- `tools`: the tools the context bar offers.

## Licence

GPL-3.0-or-later, as the Helios workspace it lives in.
