# The egui_hud agent bench: three workshops, their presentation decisions counted

Status: **harness built** (2026-11-26): the prompt, the inputs, the
counting rules and their counter (`cargo xtask hud-bench`), with today's
baseline. The agent run itself (three workshops composed) is the next
step.

## Why

egui_hud's design rule (its README, and ROADMAP section 2.1, decided
2026-11-24): a feature must reduce what an agent composing a workshop has
to know (the presentation decisions it writes, the rules for one control,
the words it must learn), never add vocabulary of its own. A presentation
preference goes through what exists: the control (its purpose, feel and
the faces it accepts), the kit and the context (slot, surface), the host
(its drawings, configured kits). A new mechanism, such as a context-wide
presentation preference, needs a measured need. This bench is the
measure.

## The run

An agent, with no knowledge of Helios beyond the inputs below, composes
three workshops, one after the other, in this order:

1. **portrait retouching**: skin, eyes, a soft light;
2. **raw development**: from the sensor data to a balanced image;
3. **old photo restoration**: dust, faded colours, contrast.

Each workshop is a composite file with a `workshop` block
(`templates/schema/composite.schema.json`, `docs/design/workshops.md`),
written under `target/hud-bench/<run>/`, never in `templates/`.

### Inputs (all the agent reads)

- the module declarations: `modules/<module>/src/lib.rs` (the manifest,
  the parameter descriptors, `patches()`), and their `*.bricks.json`;
- the controls: `crates/helios-controls/src/catalogue.rs` and egui_hud's
  README sections "Controls and faces" and "Generic controls first";
- the handles: egui_hud's handles file format (`egui_hud-handles/1`) as
  `crates/helios-rack/src/plugin.rs` describes it;
- the composite schema and one example,
  `templates/composites/helios.lift-midtones@1.helios-composite.json`.

Nothing else: no shell code, no gallery, no snapshot.

### The prompt

> You compose workshops for Helios, a photo editor. A workshop is a
> composite file: a network of processing modules, the few parameters it
> exposes (with a label, a narrowed range, a soft range), and a
> `workshop` block (intent, keywords, relative targets). Read only the
> files listed as inputs. Compose, in this order, a portrait retouching
> workshop, a raw development workshop and an old photo restoration
> workshop, each in its own file under `target/hud-bench/<run>/`. Each
> parameter must be shown right in the workshop's panel. Write a
> presentation setting (a face, a slot size, a kit, an integration
> level, a drawing) only where the default would be wrong, and say in one
> line why. When done, change the style of all three to compact
> everywhere, and list the files you touched for it.

### What is counted

`cargo xtask hud-bench target/hud-bench/<run>/*.helios-composite.json`
prints, per workshop, the counts below; the run's reply gives the style
change's files, compared with the counter's `style files`.

## Counting rules

Each rule is a pattern in `xtask/src/hud_bench.rs`, with a unit test as
its example. A workshop's files are its composite and the source files of
each module its nodes name (`modules/<module>/src/*.rs` and `*.json`, the
`mask_*` modules in `modules/masks`). The host's files (the catalogue, the
rack) are shared by every workshop and counted once, apart. In Rust,
comments and the `#[cfg(test)]` part are left out.

**Presentation decisions**, by kind:

- **face**: in JSON a `face`, `faces` or `widget` key; in Rust a face id
  of egui_hud's library as a string literal, a hand reorder of a control's
  faces (`faces.insert`, `faces.retain`) or a `ParamWidget::` hint. A
  control's own `.face("…")` list is not counted: naming the faces it
  accepts is the control's declaration, not a decision.
- **slot**: in JSON a `slot`, `size`, `orientation`, `representation` or
  `surface` key; in Rust `Slot::`, `.slot(`, `SizePolicy`, `Orientation::`.
- **kit**: in JSON a `kit` or `integration` key; in Rust `Kit::`, `.kit(`,
  `Integration::`.
- **host drawing**: a `fn draw(` in an `impl Drawings for` whose body is
  not a lone `None`.

**Rules for one control only**: in JSON an object naming a `control`
beside a presentation key; in Rust a test on an id (`id == "…"`,
`id == catalogue::…`) or a match arm on a catalogue id
(`catalogue::X =>`).

**Parameters shown right with no intervention**: the composite's exposed
parameters with no presentation key (`face`, `faces`, `slot`, `size`,
`kit`, `integration`, `orientation`, `representation`, `surface`,
`widget`). `label`, `unit`, `min`, `max` and `soft` are content (what the
workshop shows), not presentation. Whether each is shown *right* is
checked by the run's reviewer on the panel's snapshot; the counter gives
the "no intervention" half.

**Files touched for a global style change**: one (the context, where the
density lives) plus each file holding a presentation decision, since a
pinned face or slot ignores the context and must be edited too.

## Pass and fail

- **Pass**: workshop 3 needs fewer presentation decisions than workshop
  1, and a global style change touches one place (`style files` is 1 for
  each workshop, and the run's reply lists one file).
- **Fail**: rules repeated across modules (the same one-control rule in
  several workshops), or a style change spread over several files. A
  failure is what would justify a new mechanism, not before.

## Baseline (2026-11-26, `origin/next`)

`cargo xtask hud-bench` on the composites shipped today:

| composite | workshop | face | slot | kit | host drawing | decisions | one-control rules | exposed unattended | style files |
|---|---|---|---|---|---|---|---|---|---|
| helios.lift-midtones | yes | 0 | 0 | 0 | 0 | 0 | 0 | 2/2 | 1 |
| helios.masked-adjustment | no | 0 | 0 | 0 | 0 | 0 | 0 | 3/3 | 1 |
| helios.raw-front | no | 0 | 0 | 0 | 0 | 0 | 0 | 0/0 | 1 |

Host files, shared by every workshop:

| file | face | slot | kit | host drawing | one-control rules |
|---|---|---|---|---|---|
| crates/helios-controls/src/catalogue.rs | 0 | 0 | 0 | 0 | 0 |
| crates/helios-rack/src/lib.rs | 5 | 0 | 0 | 0 | 0 |
| crates/helios-rack/src/plugin.rs | 0 | 0 | 0 | 0 | 0 |
| crates/helios-rack/src/tools.rs | 0 | 0 | 0 | 0 | 0 |

Reading it: the shipped composites and the modules they name write no
presentation decision, and no rule for one control; every exposed
parameter is left to its control and the context. The host's five face
decisions are all in the rack's `control_of`, one face per value kind
(a choice with previews, a list with read-out lines, a list with no
editor), never per control: the host's configured kit, as the rule
allows. One workshop only (`lift-midtones`) has a `workshop` block, with
two parameters: too small to test the rule, which is why the bench
composes three richer ones.
