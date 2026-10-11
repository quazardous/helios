# A separate UI library: research and blueprint

Status: **research** (2026-10-11), for the decision to extract Helios's
widgets into an egui component library published on its own, `egui-pop`.
It gathers what mature UI libraries and design systems do (React Aria and
Radix, shadcn/ui, Material 3, Fluent 2, Spectrum, Carbon, libadwaita,
SwiftUI, Flutter, Compose) and what the egui ecosystem does (Rerun's
`re_ui`, egui's own widgets, egui_kittest, the hello_egui crates), and
turns it into a blueprint. See also [egui-ecosystem.md](egui-ecosystem.md).

## Where egui stands (October 2026)

- The latest release is **0.36.2**; 0.37 is not out. Style **classes**
  shipped in 0.35. **`StyleProvider`** (a per-widget style registry fed by
  classes and state) exists only on egui's main branch, behind its
  `experimental` feature, for buttons, text fields and drag values so far.
  Design for it, depend on it only behind a feature gate.
- **AccessKit is always on** since 0.34, and 0.35 added `egui_inspection`
  and `egui_mcp`, through which agents read and drive a running egui
  application by its accessibility tree. A widget with an accurate role,
  name and value is at once drivable by agents, testable by egui_kittest
  and read by screen readers.
- Every egui minor release breaks the API (0.35 removed every deprecated
  item); MSRV is 1.95.

## Practices

### Architecture
- **Behaviour below style.** React Aria and Radix ship behaviour (state,
  focus, keyboard, accessibility) apart from looks. In egui: *primitives*
  (interaction, state, AccessKit, a `Response`, painting through a style
  struct; e.g. one range-drag core for a slider and a curve point) under
  *components* (primitives plus tokens). egui's own `RangeSlider` shares
  `Slider`'s core the same way.
- **Controlled by default**: a widget bound to a value takes `&mut T` or a
  source trait; only view state (open, scroll, a drag under way) lives in
  egui's memory, keyed by a stable id. Application state never does.
- **Composition over configuration**: closures and slots
  (`show(ui, |ui| …)`) and `impl IntoAtoms` / `WidgetText` content, not
  dozens of flags (the "prop explosion" shadcn and Radix react against).
- Ids come from an id salt and the parent, **never from labels**, which
  change with the language. Results go out as `Response`s and queued
  actions, not callbacks holding the application.
- **Commands are data**: Rerun's `re_ui` has a command type, a palette and
  a snapshot test of command ids, the model for menus built from declared
  commands.

### Design tokens
- **Three tiers**: reference (the palette), system (roles: `surface.raised`,
  `text.muted`, `accent.fg`, `danger.bg`), component (`slider.track.fill`
  pointing at a system token). Named by role, never by hue.
- **DTCG 2025.10** is the first stable version of the format (aliases,
  `$extensions` for tool-specific data, Oklch and P3 colours). The existing
  DTCG files stay the source; egui-only data goes under `$extensions`.
- **A theme** overrides the system tier: mode (dark, light), contrast
  (normal, high), density (compact, comfortable: spacing and size tokens
  only), accent. Compose them, do not multiply files.
- **Mapping to egui**: generate egui's `Visuals` and `Style` from the
  system tier (so egui's own widgets and other crates match), keep a typed
  token set in the context for custom components, and later provide
  `StyleProvider`s resolving component tokens from classes (`primary`,
  `danger`, `small`). Never change the global style in the middle of a
  frame to theme one widget: scoped styles or classes.
- Rerun's `re_ui` proves the tiers work in egui (palette, aliases, hot
  reload of tokens).

### API
- Builders consumed by `impl Widget` for leaves, `show(self, ui, contents)
  -> InnerResponse` for containers, `#[must_use]`, `#[non_exhaustive]`
  where things will grow, the Rust API Guidelines.
- Generics for value types, `dyn` traits for the application's services:
  a parameter source, a command sink, a strings provider, an icon source.
  No application type crosses the boundary.
- **One library minor per egui minor**, released within days of it, egui
  re-exported, a version table in the README; `cargo-semver-checks` in CI;
  additive feature flags (`serde`, `phosphor`, `node-graph`, `testing`,
  `experimental-style-provider`); no font or icon set imposed.

### Accessibility
- Every interactive widget declares its role, a translated name, its value
  with range and step, and its actions (increment, decrement, set, focus,
  click).
- Keyboard patterns from WAI-ARIA's practices: arrows, page keys, Home and
  End on sliders; roving focus in menus and toolbars; a focus trap in
  dialogs with **focus restored on close**; Escape dismisses; a visible
  focus ring from a token.
- Contrast pairs checked by a **unit test over every theme's tokens**
  (text 4.5:1, UI 3:1); a high-contrast theme; a reduced-motion token
  honoured by every animation.
- Tests query by role and label; a generic assertion fails any interactive
  node without a name; a manual pass with Orca, NVDA and VoiceOver per
  release.

### Icons
An `Icon` abstraction (a glyph from an icon font, or an image), sized by
role from tokens (inline, toolbar, panel strip) and coloured from tokens;
Phosphor (MIT) as an optional default set.

### i18n
No user-visible string hard-coded in a component: labels are passed in,
and the few the library owns (reset, cancel, accessibility descriptions)
come from a strings provider with English fallbacks (React Aria ships its
own in 30 languages this way). Number formatting and parsing are hooks.
Layout and tokens speak of start and end, not left and right, ready for
right-to-left when egui supports it.

### Testing
- egui_kittest on the wgpu renderer, the theme installed before the first
  frame; thresholds per OS in `kittest.toml`.
- Rerun's recipe: one harness helper, **CI rendering on lavapipe** for
  stable pixels, `UPDATE_SNAPSHOTS=1` to regenerate, visual diffs on pull
  requests, snapshots stored in Git LFS.
- Per component: interaction tests (by role and label) and snapshots per
  theme, density and state (rest, hover, focus, disabled); `insta`
  snapshots for data (command ids, token resolution, the accessibility
  tree).

### Documentation
A gallery application (native and WebAssembly), one page per component
with variants and code, whose pages are also the snapshot tests; runnable
examples on every public item on docs.rs; guideline pages per component
(when to use it, keyboard map, accessibility notes, token table).

### Governance
Per-crate changelogs from fragments, one minor of deprecation before a
removal (forced egui bumps excepted), contribution guide (one purpose per
pull request, a gallery page and snapshots with each component), issue
templates (egui version, OS, GPU; component proposals with keyboard map
and tokens), complete crates.io metadata and `rust-version`.

### Pitfalls
Over-configurable components; application state leaking into widgets;
theming through globals; labels as ids; tests green locally and red in CI
because of GPUs; fonts or icons shipped unconditionally; experimental egui
APIs without a feature gate.

## Licence

The Rust and egui ecosystem norm is **MIT OR Apache-2.0** (egui,
egui_kittest, egui_tiles, egui-phosphor, egui-snarl, re_ui): a GPL library
would keep most egui applications from adopting it. Helios (GPLv3) can
depend on a permissive library; the reverse is impossible. Code moving
into the library is relicensed by its author when it moves. Two rules:

- **Nothing derived from darktable** goes into the library (its code is
  GPL by its contributors): behaviour is reimplemented from a
  specification, and such commits say so.
- **Decide before the first outside contribution**: contributions come in
  under the library's licence (inbound = outbound, with a DCO sign-off),
  so the licence cannot change later without every contributor.

Dependencies are checked by `cargo-deny` against an allow-list (MIT,
Apache-2.0, BSD, Zlib, ISC, Unicode; OFL for fonts).

## Blueprint

A repository of its own, MIT OR Apache-2.0, `rust-version` 1.95, egui
0.36:

```
egui-pop/
  crates/
    egui-pop-tokens/   no egui dependency: DTCG parser and resolver
                       (reference, system, component), modes, contrast,
                       density, contrast checks
    egui-pop/          the crate users depend on:
      theme/           tokens to egui Visuals and Style; StyleProvider
                       behind a feature
      primitives/      range-drag core, focus helpers, overlay with focus
                       trap, accessibility helpers, Icon, Strings and
                       CommandSink traits
      widgets/         slider (soft and hard range), parameter widget over a
                       ParamSource trait, menus from declared commands,
                       confirm, form, pickers, toasts, panel strip
      graph/           the node graph view (feature "node-graph")
    egui-pop-test/     themed harness, snapshot per theme, "every
                       interactive node is named", accessibility tree dump
    egui-pop-gallery/  eframe application, native and web; its pages are
                       the snapshot fixtures
```

One main crate keeps egui bumps cheap; `egui-pop-tokens` has no egui
dependency so it outlives them.

**Phases**
1. Licence, the audit of what may move (nothing derived from darktable),
   the name, the repository and its CI (fmt, clippy, tests on lavapipe,
   semver checks, cargo-deny, the web build).
2. Tokens and theme, from `helios-theme`: dark, light, high contrast, two
   densities, the contrast tests, the gallery's skeleton. Helios consumes
   it first, which proves the boundary.
3. Primitives: icons (Phosphor as a feature), the strings provider (Helios
   plugs `helios-i18n` in), the command model, the test kit.
4. The slider and the parameter widget over `ParamSource` (Helios's value
   store implements it), then dialogs, forms and toasts; each with its
   interaction tests, snapshots and gallery page.
5. Menus from declared commands, the panel strip, the graph view, pickers.
6. 0.1: guideline pages, the web demo, then `StyleProvider` behind a
   feature once egui 0.37 is out, and a checklist for each egui release.
