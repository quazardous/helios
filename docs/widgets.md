# Widgets: bound to values, driven by humans and agents alike

Status: **accepted** (2026-10-11); the value store is built
(`crates/helios-values`), the widgets bound to it come next. The user's
requirements: every widget can be driven remotely by an agent (MCP) as
easily as by a human, and a widget is bound to the value it controls, so
that a change made elsewhere reaches it. darktable paths are relative to
its source tree.

## What others do

- **Blender's RNA.** Every editable value is a property declared once
  (type, hard and soft range, step, precision, unit, default) and read by
  the UI, Python, drivers, keyframes and undo. A widget is a view of a data
  path (`layout.prop(data, "name")`); "Copy data path" gives the address
  Python and drivers resolve. A change tags the dependency graph and sends
  notifiers; listening regions redraw. One schema feeds every consumer and
  the path is the public identity
  ([API](https://docs.blender.org/api/current/bpy.types.Property.html)).
- **darktable's bauhaus.** Sliders have a hard and a soft range
  (`src/bauhaus/bauhaus.c`), typed entry and fine or coarse steps.
  `dt_bauhaus_slider_from_params` builds a slider from the params' own
  introspection (`src/develop/imageop_gui.c`): our "widgets from the
  manifest". Widget to params goes through `dt_iop_gui_changed`, params to
  widget through `dt_iop_gui_update` behind a guard against loops. Every
  widget is an action (`iop/exposure/exposure`) with elements and effects,
  used by keys, MIDI and Lua. Weak spots: actions target the widget (the
  module's GUI must exist), instances are addressed by position, paths come
  from translated labels, and remote feedback is polled.
- **Qt/QML, React, SwiftUI, Flutter.** QML bindings break silently when
  assigned imperatively; React's controlled components and SwiftUI's
  `@Binding` keep the value outside the widget, which reads it and reports
  changes; Flutter's keys keep identity across rebuilds and its semantics
  tree serves tests and assistive technology.
- **Accessibility trees** (AccessKit, used by egui; UI Automation; AT-SPI)
  give roles, labels, values and actions, and `egui_kittest` queries them.
  They are not enough for an agent: only what is on screen, translated and
  sometimes duplicate labels, no units or soft ranges or defaults, and
  driving them means imitating a human.
- **Control surfaces and remote models.** MIDI2LR maps controllers to
  Lightroom parameters by name, with ranges and soft takeover; OSCQuery
  describes a namespace (type, range, unit, access) with change listening;
  Home Assistant entities have a stable id, typed attributes, writes
  through services and change events carrying who caused them; VS Code
  declares settings by schema and reports which key changed.
- **egui** is immediate mode: a widget reads its value every frame. A
  change made elsewhere has to update the shared state and call
  `request_repaint`; widget ids should come from the value's path, not the
  layout, so focus, drag state and accessibility follow the value.

## Proposal

**One source of truth per value, at a stable path.** A value lives in the
engine's graph (`node/<id>/<param>`, node ids stable, never positional),
in settings (`settings/<key>`) or in the application's state
(`view/zoom`), never in a widget. Each path has a descriptor, generated
from the module manifest or the settings schema: type, hard and soft
range, fine and coarse steps, unit and display scaling, default, label and
translation key, choices.

**Widgets are controlled views.** A widget reads the value from the store
every frame and, on input, sends a command (`param.set` with the path,
the value, its origin and the gesture), never writing the value itself.
The store announces `param.changed` (path, old, new, origin, sequence) on
the bus; the GUI repaints, coalescing changes per frame. Dependent widgets
(a choice that hides a slider) are a function of the values, evaluated
every frame.

**Agents act on values and commands, by path.** Through MCP:
`list_params(prefix)` with descriptors; `get(path)` with the value, its
descriptor, whether it differs from the default and who last changed it;
`set(path, value)` or a relative step; `reset(path)`; `run_command(id)`;
`subscribe(prefix)` for changes. Effects as darktable's: set, up, down,
reset, top, bottom, and `force` past the soft range up to the hard one.
The answer gives the value actually applied and says when it was clamped.
Pixels (`gui.snapshot`) and the accessibility tree show what the user
sees; they are not the way to drive.

**A human and an agent on the same value.**

- Every write carries its origin (user, agent and its session, script,
  MIDI, a remote node) and, for a drag, a gesture.
- During a human's drag the value belongs to that gesture: an agent's
  write is refused with the current value, never fighting the hand.
- Otherwise the last writer wins, ordered by the store's sequence; an
  agent may write only if the value has not changed since it read it.
- One drag is one undo step; one agent call (or an explicit group) is one
  undo step named after it.
- Changes not made by the user are visible: the history names their origin
  and the widget shows briefly that someone else moved it.
- Absolute hardware faders take over softly, without jumps.

## What every widget provides

- Its path and an id derived from it; a label, a translated label, a
  tooltip; "Copy path" in its menu, to hand addresses to agents and
  scripts.
- A typed value read from the store; hard and soft range; unit, display
  scaling and precision; typed entry with units.
- Default and reset (double click, the reset effect) and a "changed from
  default" mark.
- Fine and coarse steps (modifiers, wheel), relative steps for encoders.
- Keyboard use: focus, arrows, page keys, Enter for typed entry, Escape.
- Accessibility generated from the descriptor: role, label, value, range,
  step, increment and set actions.
- States: disabled with a reason, hidden, pending (sent, not applied yet),
  outside the soft range, held by a gesture, last origin.
- Snapshots: rendered headless for image tests; found by path in tests,
  not only by label.

## Pitfalls to avoid

- State held by widgets, or copies that go stale.
- Loops: a change event causing a write causing an event; values are
  compared and a write is never echoed to its origin.
- Addressing by label or position.
- Addressing widgets instead of values.
- Silent clamping, and soft and hard ranges mixed up.
- An undo step per frame of a drag or per agent micro-step.
- Repaint storms from a fast remote source: changes are coalesced per
  frame and subscriptions throttled per path.
- Changes nobody can attribute.
- Stored and displayed values confused (percent against 0 to 1): the
  interface always speaks the stored value and gives the displayed one
  separately.
