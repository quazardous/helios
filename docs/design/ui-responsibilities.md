# UI responsibilities: helios-ui, helios-rack, egui-pop, egui_hud, egui_fit, egui_stack

Status: study (2026-11-15). It checks the split decided in ROADMAP.md on
2026-11-15 ("No widgets in Helios", "Two worlds") against today's code,
answers where placement belongs, and lists the slices. Nothing here is
built.

The owner's rules this note starts from (decided, not reopened here):

- Helios makes no widgets: every widget lives in egui-pop.
- Two worlds. The **form-like** parts (settings and admin pages, the
  import window's panels, dialogs, lists) go through `helios-ui`: intent
  presets over egui-pop, no drawing code. The **visual module** parts
  (the diagram and its toolboxes, a workshop's on-image controls, the
  controls layer) go through `helios-rack` (name to confirm): a fixed
  frame whose controls come from egui_hud.
- Within Helios, egui_hud renders **nothing** directly: it keeps the HUD
  logic (what to show, where on the image, handles' geometry, toolbox
  contents, states, `when` rules) and every widget, mark and handle is
  drawn by egui-pop, on the same egui-pop version as Helios.
- A visual workshop or tool must be writable almost declaratively by an
  agent: it declares, the frame draws. What cannot be declared yet is a
  gap for the frameworks, never drawing code in a module.
- Clippy's disallowed lists hold each crate to its world.

## In short

- **The split holds.** Every Helios screen falls in one of the two worlds
  or is a composition of both with a clear seam (section 1). No screen
  needs a third kind of framework.
- **Today's state is better than the thread feared on one point and worse
  on another** (section 3). Better: egui_hud 0.3.0 (crates.io) already
  depends on egui-pop 0.10, Helios's lock resolves **one** egui-pop (its
  git pin), and Helios already uses egui_hud through `helios-controls`
  (the darkroom's Parameters panel, the mask handles, the controls
  gallery). Worse: egui_hud's `render` module draws **32 raw egui widget
  calls and 19 painter calls** with colours from egui's visuals (18
  reads) rather than egui-pop's tokens (7 reads), and `helios-gui`
  still has **771 raw egui widget calls in 42 files**.
- **Why egui-pop was ignored** (section 4): not timing (the helpers
  predate most screens) but **no door**. Helios's agent docs never name
  a widget to use, egui-pop's catalogue is a running gallery and a
  survey rather than a "use this" index, agents write the egui they
  know, screens copied their neighbours, and no lint stopped anything.
  The fixes are a generated "use these" index, `helios-ui` and
  `helios-rack` as the only doors, and the Clippy lists.
- **Placement verdict** (section 8): **yes, all placement is egui-pop's
  responsibility, as one family with one door**, but not one crate.
  The layout layer (row, grid, flow, baseline, tracks) is built **inside
  egui-pop-core as an extension of the existing `UiPop` layout-by-role
  helpers**, not beside them. egui_stack (docking) and egui_fit (slot
  packing) stay separate, egui-free solvers, already or newly reached
  only through egui-pop; no Helios crate and no egui_hud renderer
  computes a screen rectangle itself.
- **helios-rack** (section 7): a module or tool declares title, ports,
  controls by kind, handles, toolbox items and states; the frame decides
  placement, sizes, order, theme, focus and agent names. The worked
  example (a radial exposure tool) is about 40 lines of declaration with
  no egui type, against roughly 300 to 500 lines and the egui geometry
  API today.

## 1. Map: screens, their world and their controls today

"Raw" counts raw egui widget calls in the file (labels, buttons, text
fields, combos, tooltips, `RichText`, scroll areas, grids), counted with
the same patterns as visual-identity.md's audit, on `next` of
2026-11-15. All paths under `crates/helios-gui/src/` unless named.

### Panels (docked, declared in `lib.rs`, `helios_panels_with`)

| Panel | World | Today's source of controls | Raw |
|---|---|---|---|
| Image (`helios.viewport`) | visual | egui-pop's viewport (tiles, display transform, navigation) in `viewport.rs`; mask outlines and handles on egui-pop's overlay kit in `masks.rs`, their behaviour declared in `helios-controls/src/gizmo.rs` | 1 + 0 |
| Graph (`helios.graph`) | visual | egui-pop's graph view (`egui_pop::viewport::graph`) in `graph.rs` | 6 |
| Library (`helios.vault`) | mixed | grid, marks, tags, compare from `egui_pop::views::library`; filters and search hand-drawn around it in `library.rs`, `library/keywords.rs`, `recent_filters.rs` | 40 + 6 |
| Modules (`helios.modules`) | forms | hand-drawn list, `modules.rs` | 17 |
| Masks (`helios.masks`) | forms | egui-pop's `TreeView`, `mask_tree.rs` | 15 |
| Navigator | visual | egui-pop's imaging navigator | 0 |
| Parameters (`helios.params`) | visual | egui_hud's `render::card` through `helios-controls`, Helios as the `render::Drawings` host (`params.rs`, `impl Drawings for Host`), plus egui-pop's curve editor and wheels | 24 |
| Scopes | visual | egui-pop's histogram (`egui_pop::viewport::histogram`, `imaging::histogram`), `scopes.rs` | 4 |
| Filmstrip | visual | egui-pop's library views (`shell.rs`) | (in shell.rs) |
| Queue, Log | forms | hand-drawn lists, `queue.rs`, `log_panel.rs` | 18, 4 |
| Compare, Stacks, Duplicates, Decisions | mixed | egui-pop's compare view; hand-drawn lists and decision rows (`comparison.rs`, `stack_review.rs`, `duplicates.rs`, `decisions.rs`) | 0, 11, 13, 28 |
| Workshops (experimental) | forms (list), visual (run) | hand-drawn panel `workshops.rs`, `workshops/review.rs` | 47, 18 |

### Windows and focus screens (`shell/`)

| Window | World | Today's source of controls | Raw |
|---|---|---|---|
| Settings, Storage, Health, Accounts admin, init wizard | forms | egui-pop's admin manifest (`egui_pop::api::admin`) from `helios-declarations` and `shell/storage.rs`, `health.rs`, `accounts_admin.rs`, `init_wizard.rs` | 0 to a few |
| Import window | mixed | frame and sources column hand-drawn (`shell/import_window.rs`, `import_window/source_list.rs`); the middle column is egui-pop's file browser over the photos; the detail zone partly the admin manifest, partly hand-drawn (`import_choices.rs`, `import_detail.rs`) | 48 + 25 + 11 |
| Import wizard, profiles, profile editor, accounts, history | forms | hand-drawn (`import_wizard.rs` partly manifest) | 41, 26, 34, 55, 7 |
| Activity (focus) and its detail | forms (a list and a detail), with live progress | hand-drawn `shell/activity_focus.rs`, egui-pop indicators in `activity.rs` | 60, 6 |
| Calibrate (focus) | mixed | hand-drawn steps around an image (`shell/calibrate_focus.rs`, `calibrate.rs`) | 48, 22 |
| Macro interface, rule editor | forms | hand-drawn (`macro_interface.rs`, `rule_editor_ui.rs`) | 44, 31 |
| Workshop design, edit, run, session, generalise | forms today; visual when a run shows on-image controls | `shell/workshop_*.rs`; no on-image drawing yet (0 painter calls) | small |
| Splash, pointer choice, promise line | forms | hand-drawn | 16, 5, 11 |
| Controls gallery (`crates/helios-controls-gallery`) | visual (a bench) | egui_hud's bench and renderers | 29 |

### The mixed cases, and their seam

- **Library**: the photo grid is a visual surface but a *generic* one
  (egui-pop's library views): it stays egui-pop's, framed by
  `helios-ui` as one region. The filter zone is forms (`helios-ui`:
  search, chips, choices). Seam: `helios-ui::library_view(..)` hands a
  region to egui-pop's grid; Helios never draws a thumbnail cell.
- **Import window middle grid**: egui-pop's file browser, same rule as
  the Library grid; the three zones are a `helios-ui` page layout.
- **Workshop side panel with sliders**: the sliders are a module's
  controls, so the panel is a `helios-rack` toolbox (egui_hud cards),
  even when it sits in a side panel; the list of workshops above it is
  forms. Seam: a panel may host one rack region and form regions, each
  drawn by its own framework.
- **Activity detail**: forms (a details grid, a log list, actions),
  with live values shown by `helios-ui::value` / `progress`. No visual
  world here.
- **Parameters panel**: visual (rack), although it looks like a form:
  its rows are a node's controls, resolved by egui_hud, so the same
  control appears on the image, in the HUD and in the panel.

Rule of thumb: **a control bound to a node parameter or drawn over the
image is the rack's; everything else is a form.**

## 2. Boundaries

| Crate | Owns | Must never own | Depends on |
|---|---|---|---|
| **egui-pop** (MIT) | every widget and every pixel: tokens, themes, kit, icons; the layout-by-role helpers and the layout layer built on them; all placement (section 8): windows, docking (via egui_stack), focus, floating placement, slot packing (via egui_fit), anchored HUD elements; the viewport and its overlay and handle kit; the admin/settings manifest renderer; the read and hook kernels; the agent bridge; history contract; i18n plugin | Helios vocabulary (photos, modules, nodes as business), business controls, HUD logic (what to show when) | egui, egui_stack, egui_fit (to add), egui_table |
| **egui_hud** (MIT/Apache) | HUD and controls *logic*: the control model, cards and cases, context and `resolve`, the registry, value flow, `controls.*` commands, handles' geometry and behaviour, toolbox contents, `when` rules; the *choice* of which egui-pop widget shows a part (a renderer table) | raw egui widgets, `Painter` calls, its own colours, strokes or sizes, its own placement maths beyond asking egui_fit / egui-pop; any Helios dependency | egui-pop-model, egui_fit (types), egui-pop behind `render` (always on within Helios) |
| **egui_fit** (MIT/Apache) | the slot solver: twelfths grid, size policies, orientation, dense packing, hysteresis; pure, no egui | drawing, controls, values | serde only |
| **egui_stack** (MIT) | docking: tabs, splits, the window-level zone solver | widgets, application vocabulary | egui |
| **helios-ui** (GPLv3, to create) | Helios's form intents (`ui::search(Search::Library)`, `ui::empty(Empty::NoPhotos)`, sections, actions, details grid, dialogs, notify); the standard page compositions (three zones, dialog bar); hosting egui-pop's generic visual views (library grid, file browser) as regions | painting, colours, literal sizes, any control bound to a node parameter | egui-pop only (never egui directly, never egui_hud) |
| **helios-rack** (GPLv3, to create; the rack frame) | the frame of a visual module or tool: header, ports, toolbox placement, which surface (panel, HUD, on-image) a declared control goes to, the module's states (bypassed, loading, error); the binding of declarations to Helios's store, commands and undo | widgets, painting, placement maths (it states rules, egui-pop computes) | egui_hud (logic), egui-pop (through egui_hud's renderers and its overlay kit), `helios-controls` |
| **helios-controls** (GPLv3, exists) | the business catalogue (exposure…), patches, gizmos, adapters to Helios's store and commands | any egui (it already holds none) | egui_hud model, helios-values |
| **module and plugin crates** | declarations: patches, gizmos, toolbox items, settings contracts, manifests | egui, egui-pop widgets, egui_hud renderers | `helios-controls` model, `helios-rack` / `helios-ui` declaration types only |
| **helios-gui** | the application shell: mounting panels and windows, the kernel wiring | widgets and drawing (it shrinks to hosting as screens move to the frameworks) | helios-ui, helios-rack, egui-pop's app API |

`helios-ui` and `helios-rack` stay two crates (the owner's shape): both
thin, neither draws, and the Clippy lists differ (section 9). They share
nothing but egui-pop's tokens, reached the same way.

## 3. Leaks today

### Corrections to what the thread assumed

Read on 2026-11-15 from Helios's `Cargo.lock` and egui_hud's published
0.3.0 sources:

- egui_hud **0.3.0** (crates.io, its release "egui-pop 0.10") depends on
  `egui-pop = "0.10"`. Helios patches egui-pop to its git pin
  (0.10.2), so the lock holds **one** `egui-pop` and one theme. The
  0.9 pin is egui_hud 0.2.x (and a stale local checkout), no longer what
  Helios builds. The constraint stands for every future release:
  egui_hud's egui-pop requirement must stay semver-compatible with
  Helios's pin, else two versions load.
- Helios **is** wired to egui_hud: `helios-controls` (model always,
  `render` feature), `helios-gui` (`helios-controls` with `render`:
  `params.rs`, `masks.rs`, `controls.rs`, `viewport.rs`), and the
  controls gallery (`render`, `bench`). What is missing is not the
  wiring but the **frame**: there is no `helios-rack`, so each host
  (`params.rs`, `masks.rs`) wires egui_hud by hand.

### egui_hud's direct rendering (0.3.0, `src/render.rs`, 2,070 lines)

| Kind | Count | Examples |
|---|---|---|
| Raw egui widgets | 32 | `egui::TextEdit::singleline` at line 1199 (a text part), `egui::Button` at line 1728 (the "More" row's entries), `egui::ComboBox` at 1160, `egui::Label`/`RichText` at 843 and 1276, `egui::ScrollArea` at 1300, `ui.label`/`ui.weak` (9), `on_hover_text` (11) |
| Painter calls | 19 | the scrub field drawn by hand at lines 283 to 320 (frame, label, value, layout of galleys), dots and markers at 776, 796, 813 (`convex_polygon`), a value bar at 1565 to 1578, swatches at 1345, read-outs at 1531 and 1686, a separator `hline` at 847 |
| Own colours and strokes | 18 reads of `ui.visuals()` colours, 1 `Stroke::new` (1236), a colour built from `hsva` (1348) | against 7 reads of egui-pop's `Kit`/`Tokens` |
| Already on egui-pop | `Kit`, `Glyph`, `Knob`, `Slider`, `sections`, `widgets::checkbox` | the right path, to extend |

The bench (`src/bench/ui.rs`, feature `bench`) has 107 raw widget calls
and 32 painter calls; it is a tool, not a Helios surface, so it gets a
looser list (section 9) but the same direction.

What egui-pop must provide so `render.rs` drops all of it:

1. a **scrub field** (a value whose label drags; planned in controls.md
   section 4, item 1; the knob is done);
2. **text and choice fields in a compact cell** (the kit's `Input` and
   `combo_box` at a given rect, no extra frame);
3. **value marks**: the "changed from default" dot, the "held by" dot,
   the "moved by" marker, as an `Indicator` variant (egui-pop has
   `Indicator`; these three are missing);
4. a **value bar** (a horizontal level read-out), a **read-out** text
   role for numbers, and the **swatch** from egui-pop's indicators;
5. a **menu row** for the "More (n)" entries (egui-pop's menu item);
6. **anchored HUD elements** (controls.md item 2) and the **scalar
   handle** and **mapping hook** (items 3 and 4) for on-image parts.

### Helios's raw egui (next, 2026-11-15)

| Crate | Raw widget calls | Files | Painter calls |
|---|---|---|---|
| helios-gui | 771 | 42 of 141 | 17 |
| helios-controls-gallery | 29 | 1 | (bench) |
| helios-controls, helios-declarations, modules, plugins | 0 | 0 | 0 |

(visual-identity.md counted 539 a few days earlier with a narrower
pattern; the increase is new screens plus `strong`, `small`,
`monospace`, `RichText` counted here.) 14 literal colours
(`Color32::from_rgb` and friends) remain in helios-gui. The worst files
are the forms screens of section 1 (Activity 60, import accounts 55,
import window 48, calibrate 48, workshops 47).

Duplicated tokens or widgets between egui-pop and egui_hud: none as
code (egui_hud has no token set of its own), but its 18 `visuals()`
reads are a second, implicit palette: they follow egui's style, which
egui-pop's theme sets, so they *look* right in the default theme and
drift in any kit that styles a component differently.

## 4. Why egui-pop's parts were ignored

The facts, then the causes, then the fixes.

**Not timing.** `Input::search` dates from 2026-10-11,
`UiPop::section_title` and the text roles from 2026-10-12,
`EmptyState` from 2026-10-13, the `UiPop` layout-by-role helpers as a
trait from 2026-10-20 (with the gallery's "Layout by role" page the same
day). Most Helios screens came after: the import window on 2026-11-07,
the Workshops panel on 2026-11-05, import accounts on 2026-11-12, the
Activity focus and the saved-source filters on 2026-11-14.

**What `UiPop` already covers.** `egui_pop::core::ui::UiPop`, in
egui-pop's prelude: text roles (`text_role`, `section_title`, `muted`,
`meta`), spacing by intent (`space_role(Space::..)`), cards and
buttons by tone (`card`, `button_tone`), kit-styled scopes for egui's
own widgets (`scoped`, `combo_box`, `scroll_area`), shown on the
gallery's `roles` page (test
`the_layout_by_role_page_shows_roles_spacing_and_tones`), with icons by
role on the `icons` page. It names *what a piece of text is* and *why
there is space*; it does not relate widgets to each other (no row with a
shared baseline, no column tracks, no flow, no relative sizes). That is
exactly what the layout layer adds (section 8), as new methods and
types in the same module, reading the same `Space` and `TextRole`.

**Helios's use of it.** Eight helios-gui files import `UiPop as _`, and
all of them for `scroll_area` (10 calls) and `card` (1). The role
helpers have **zero** calls: `text_role`, `section_title`, `muted`,
`meta`, `space_role`, `button_tone`, `scoped`, `combo_box` all 0,
against 299 `ui.label` / `ui.weak` / `ui.add_space` / `ui.heading`
calls. So the trait was found, for one method, and the rest never
read.

**Causes, with evidence.**

1. **No index on Helios's side.** Helios's `AGENTS.md` (35 lines),
   `CLAUDE.md` and `docs/next.md` say how a change reaches `next`,
   never which egui-pop widget to use. `docs/gui.md` names egui-pop
   130 times, all for the workbench, commands, context and windows.
   Across the last 60 merged pull requests into `next` (about 107 kB of
   bodies), egui-pop is named on 22 lines and `UiPop`, `EmptyState`,
   `Input::search`, `Skeleton`, `ToggleChip`, `text_role`,
   `space_role` or `section_title` **0 times**: briefs say "on egui-pop"
   for framework pieces, never "this widget".
2. **egui-pop's catalogue is not a "use this" index.** Its catalogue is
   the gallery (a running app; `docs/gallery.md` lists page names in one
   long line), and `docs/widgets.md` is a survey of other libraries
   (shadcn, Ant, Blueprint…) that says what egui-pop *should* have.
   Grepping egui-pop's docs for "empty state" finds the survey before
   the widget; nothing maps a need ("a search field", "nothing here
   yet", "a section title") to one call. An agent working in Helios's
   tree sees egui-pop only as a dependency in Cargo's cache.
3. **egui is what agents already know.** `ui.label`, `ui.weak`,
   `TextEdit::singleline(..).hint_text(..)` and `ui.small_button("×")`
   are egui's documented idioms; an extension trait imported as
   `UiPop as _` gives no hint at the call site that `ui.muted(..)`
   exists.
4. **Screens copied their neighbours.** The same raw idioms recur
   across screens written by different slices: a `"×"`
   `small_button` as a close or remove mark in 5 places in 3 files
   (`macro_interface.rs`, `pointer_choice.rs`, `rule_editor_ui.rs`)
   beside egui-pop's icon set; `TextEdit` with a literal `hint_text`
   in 5 places in 3 files (`import_choices.rs`, `import_profiles.rs`,
   `workshops.rs`) beside `Input`'s placeholder. Once one raw pattern
   is in the tree, the next screen starts from it.
5. **Nothing stopped it.** `clippy.toml` holds only `doc-valid-idents`;
   no disallowed method or type exists. Tests drive screens by
   accessibility label, which a raw button satisfies as well as a kit
   one, and snapshot changes are accepted as the screen's new look.

**Fixes** (each is a slice in section 10):

- **One door per world**: `helios-ui` for forms, `helios-rack` for
  visual modules; screens import those, not egui.
- **Clippy lists** naming the replacement in each reason (section 9):
  the reason is the index at the moment it matters, in the compiler's
  output.
- **A "use these" index in Helios's agent docs**, generated, not
  written by hand: a `cargo xtask ui-index` that lists `helios-ui`'s
  and `helios-rack`'s public functions with their first doc line (need
  → call), checked in as `docs/ui-index.md` and linked from
  `AGENTS.md`. Until helios-ui exists, the same tool lists egui-pop's
  `UiPop` methods and widget constructors, so the index is never
  empty.
- **egui-pop side**: a short "by need" page (`docs/by-need.md`: need →
  widget → gallery page), generated from the gallery's page list, and
  the gallery's page names in its test names (already so for `roles`).

## 5. Shared pieces, one owner each

| Piece | Owner | Both worlds reach it through |
|---|---|---|
| Tooltips and hover cards | egui-pop (`on_hover_text` themed, `HoverCard`) | helios-ui (`.help(..)` on an intent), egui_hud's part help, rendered by egui-pop |
| Icons | egui-pop's icon set and registry (`phosphor:`), icons by role | helios-ui by intent (`Icon::Close`), rack declarations by name |
| Keyboard shortcuts | egui-pop's keymap and commands | helios-ui actions and rack controls declare commands; keys bound in the keymap (controls.md slice 15) |
| Accessibility names | egui-pop (every widget sets its `WidgetInfo`) | the label given in the declaration; never set by Helios code |
| Agent control | egui-pop-agent (the bridge), commands | forms: commands and labels; rack: `controls.*` commands and named controls, the same bridge |
| Undo and history | egui-pop's history contract | helios-ui actions run commands (one step each); rack gestures are store gestures, one step per drag (as `gizmo.rs` does) |
| i18n | egui-pop-i18n; Helios's catalogues in helios-i18n | declarations carry message ids; frameworks resolve them |
| Notifications | the log sink (log-alerts.md) | `helios_ui::notify`; the rack reports a module's state to the same sink |
| Tokens and theme | egui-pop | read by egui-pop's widgets only; neither helios-ui, helios-rack nor egui_hud reads a colour |

## 6. Plugins

Plugins are out-of-process binaries with a manifest
(`helios-plugin.toml`, `crates/helios-plugin/src/manifest.rs`). None
links egui, and none can, so the rule is enforced by the process
boundary; what matters is that the manifest can say everything a
plugin's UI needs:

- **Forms**: `[settings.<key>]`, `[account.settings.<key>]`,
  `[source.settings.<key>]` already declare a configuration contract,
  rendered by egui-pop's admin manifest. The connect kinds (`oauth`,
  `form`, `device`, `trust`, `code`) are flows Helios draws. New needs
  (a plugin's own page, a list it fills) are added as declaration kinds
  rendered by `helios-ui`, never as plugin-supplied drawing.
- **Visual**: plugin-declared controls and composites go over the
  plugin protocol as the same data as a module's patch (controls.md
  slice 21), rendered by `helios-rack`. A plugin's on-image tool is a
  rack declaration (handles by kind, toolbox items), its values flowing
  through the protocol.
- **Hook points** (hooks.md): a plugin contributes to a point (a
  health finding, a badge, a search key) with data; where the result
  shows is the owning screen's choice, drawn by its framework.
  Contributions carry text, icons by name and tones, never sizes or
  colours.
- **In-process modules** (`modules/<name>`) follow the same rule by
  dependency: they depend on `helios-controls`' model (exposure already
  does: `modules/exposure` has no egui) and on declaration types, which
  the dependency check of section 9 holds.

## 7. helios-rack

### What a module or tool declares, what the frame decides

| Declared by the module / tool | Decided by the frame (helios-rack + egui_hud + egui-pop) |
|---|---|
| title, icon, ports (in the diagram) | header layout, port positions, sizes |
| its controls by kind (patched or implicit), with tiers | which surface each lands on (panel, HUD, on image) by context; order; slot sizes and orientation (egui_hud `resolve` through egui_fit) |
| handles: name, kind (point, radius, feather, angle, line), parameters written, Shift / wheel / reset | how a handle looks, its hit size, cursor, drag feedback, keyboard nudge (egui-pop's overlay kit) |
| toolbox items: commands with an icon and a label, toggles bound to values | toolbox placement (side, HUD, toolbar), overflow into "More (n)" |
| states: bypassed, loading, failed, needs a mask | how each state shows (fault card, skeleton, dimmed) |
| `when` rules over context keys | evaluation and anti-jump |
| nothing | theme, spacing, focus order, accessibility names (from labels), agent names (`controls.*` ids), undo grouping |

The frame binds declarations to Helios: values to `helios-values`'
store, toolbox items to commands, gestures to undo steps, states to the
log sink. Agents drive a rack exactly as they drive forms: by command
and by name, through egui-pop-agent.

### Worked example: a radial exposure tool

A visual tool, the owner's first case: a circle on the image (centre,
radius, feather) and a side toolbox (exposure, invert, reset).

```rust
// modules/radial_exposure/src/ui.rs: the whole UI of the tool.
use helios_rack::{Handle, Rack, Tool, Toolbox};

pub fn tool() -> Tool {
    Tool::new("radial-exposure", tr!("radial-exposure.title"))
        .icon("phosphor:sun-dim")
        // On the image: three handles, geometry from the parameters.
        .handle(Handle::point("centre").writes(["x", "y"]).shift_axes())
        .handle(Handle::radius("radius").around("centre").writes(["radius"]).wheel_scale())
        .handle(
            Handle::feather("feather")
                .outside("radius")
                .writes(["feather"])
                .reset(0.0)
                .wheel_scale(),
        )
        // Beside it: the controls its parameters are patched to.
        .toolbox(
            Toolbox::new()
                .control("exposure")         // the catalogue's exposure control
                .control("feather")
                .toggle("invert")
                .command("radial-exposure.reset", "phosphor:arrow-counter-clockwise"),
        )
        .state_when("!image.loaded", Rack::WAITING)
}
```

About 25 lines, no egui type, no coordinate, no colour, no size. Each
line names a parameter or a catalogue entry; the frame and egui_hud do
the rest. What the same tool needs today, by analogy with the circle
mask:

| Today | Lines | egui knowledge |
|---|---|---|
| the gizmo declaration (`helios-controls/src/gizmo.rs`, `circle_mask`) | about 45 | none (already declarative) |
| the patch to the catalogue (as `modules/exposure`' `patches`) | about 20 | none |
| hosting in the viewport: outlines, handle positions, signals, drags, Escape, wheel (`helios-gui/src/masks.rs` is 1,509 lines for all shapes; a new tool's share) | 200 to 400 | `Rect`, `Pos2`, `Response`, overlay `Signal`s, the viewport's hooks |
| a side toolbox, hand-drawn | 50 to 100 | `ui.horizontal`, buttons, tooltips |
| **total** | **about 300 to 550** | the egui geometry and response API |

With helios-rack: **about 25 lines, none of egui**. The gap is the
hosting that `masks.rs` does by hand; it moves into the frame once,
for every tool.

**Gaps found** (to absorb in the frameworks, not in modules): an
`apply` function per handle is code today (`gizmo.rs`' `Apply`); the
frame needs handle *kinds* that know their own maths (point, radius
around a point, feather outside a radius, angle, line), with a code
hook only for unusual shapes. A per-patch range and default (controls.md
slice 12, open) is needed for `feather`. The scalar handle and mapping
hook (controls.md items 3 and 4) are needed for oriented spaces.

### A settings page through helios-ui

```rust
// A source's options page.
ui::page(tr!("source.options"), |p| {
    p.section(Section::new(tr!("source.files")), |s| {
        s.field(Field::pattern("names"));     // a pattern field, its width from the kind
        s.field(Field::days("days"));
        s.toggle("new_only");
    });
    p.empty_when(Empty::NoSource(kind));
    p.actions([Action::primary("source.save"), Action::secondary("source.cancel")]);
});
```

About 10 lines, against `shell/import_profiles.rs` today (400 lines,
37 references to egui types or `ui.` calls, 26 raw widget calls). The
admin manifest already shows the gain: `shell/storage.rs` is a full
page with **0** egui references.

## 8. Placement: does it all go into egui-pop?

**Verdict: yes as a responsibility, with one door, not as one crate.**
Every rectangle on screen is computed by egui-pop or by a solver only
egui-pop calls. Concretely:

| Scale | Today | Verdict |
|---|---|---|
| Text and spacing roles | `UiPop` in egui-pop-core | stays; the base of the layout layer |
| Widget relations: row, column, grid with header/value columns, flow, baseline, tracks (`Content`, `Fr`, `Percent`, `Em`, min/max), equal-width groups | missing | **built in egui-pop-core's `ui` module as an extension of `UiPop`** (e.g. `ui.row(..)`, `ui.grid(..)`, gaps as `Space` roles, baseline from `TextRole` metrics), not a parallel `layout` system; visual-identity.md section 3's API sketch is re-homed accordingly |
| Floating placement (popovers, menus) | egui-pop-core `placement` | stays |
| Docking, tabs, window zones | egui_stack (continues egui_dock), a dependency of egui-pop-core; Helios declares `PanelSpec`s and layouts through egui-pop | **stays a separate crate** (about 20,000 lines, its own users, MIT); the rule already holds: no Helios crate depends on egui_stack; written down and checked |
| Slot packing (cards in panels, toolbars, HUD) | egui_fit, used by egui_hud and `helios-controls` directly | **stays a separate, egui-free crate** but joins egui-pop's family: egui-pop depends on it and offers slot regions (`ui.slots(..)` in the layout layer), egui_hud keeps its slot *policies* (logic: what size a control asks for) as egui_fit types, and asks egui-pop to lay the card out; Helios crates reach egui_fit only through egui_hud's or egui-pop's re-export |
| Image-anchored elements (HUD chips near a point, handles) | egui-pop viewport overlay kit; anchored HUD element planned | egui-pop (viewport) |

Why not merge egui_fit and egui_stack into egui-pop's repository: both
are egui-free (egui_fit) or a continuation of an upstream crate
(egui_stack) with users outside egui-pop; merging them buys no
behaviour, costs their independent release, and the "one door" rule
gets the same constraint with a dependency check. If their versions
ever drift from egui-pop's, moving them into egui-pop's workspace is
the fallback.

**Consequences.**

- egui_hud: no placement of its own. `render::card` stops computing
  rects from placements; it hands egui-pop a card (parts with slot
  policies) and gets drawn parts back. Its 19 painter calls go with it.
- helios-rack: declares placement *rules* (which surface, which side,
  anchored to which handle, pinned or packed), never sizes; egui-pop
  computes.
- helios-ui: page compositions (three zones, details grid, dialog bar)
  are calls to the layout layer, no literal widths.
- egui-pop: gains the layout layer (planned), slot regions over egui_fit
  (new), the anchored HUD element (planned), and the gaps of section 3.
- The 36 `add_space`, 17 fixed widths and 9 `allocate_exact_size` that
  visual-identity.md counted all have a replacement to name in Clippy.

## 9. Enforcement

### Crate worlds

| World | Crates | Clippy list |
|---|---|---|
| egui-pop internals | egui-pop's crates | none from Helios (egui-pop's own rules) |
| HUD logic | egui_hud (`render` included) | raw egui widgets and `Painter` forbidden: `egui::TextEdit`, `egui::Button`, `egui::Label`, `egui::ComboBox`, `egui::ScrollArea`, `egui::Ui::label`, `egui::Ui::painter`, `egui::Ui::painter_at`, `egui::Painter::*`, `egui::Stroke::new`, `egui::Style::visuals` reads; the bench (feature `bench`) with `#[expect(.., reason)]` until it moves too |
| Forms framework | helios-ui | everything raw *except* at its crate root, under one `#[expect]` with a reason (it wraps egui-pop's widgets, never egui's) |
| Visual framework | helios-rack | same as egui_hud: no widgets, no painter, no colours |
| Shell | helios-gui | visual-identity.md's list (raw widgets, `add_space`, widths, literal colours, `egui::Modal`, `egui::Grid`…), plus `egui_hud::render::*` (go through helios-rack) |
| Declarations | helios-controls, modules, plugins, helios-declarations | must not depend on egui at all (dependency rule below) |

### Dependency rules

Checked by a `cargo xtask deps-worlds` test over `cargo metadata`
(fails the check job):

- `modules/*`, `plugins/*`, `helios-controls`, `helios-declarations`:
  no `egui`, no `egui-pop` (except `egui-pop-model`), no `egui_hud`
  `render` feature;
- `helios-ui`: `egui-pop` only among UI crates;
- `helios-rack`: `egui_hud` (with `render`), `egui-pop`;
- no Helios crate depends on `egui_stack`, and only `egui_hud`,
  `helios-controls` and egui-pop on `egui_fit`;
- `helios-gui` depends on `egui` only for the eframe entry point (one
  documented exception);
- the lock holds exactly one `egui-pop` and one `egui` (a test reads
  `Cargo.lock`).

## 10. Verdict and slices

**Verdict.** The split is confirmed, with three adjustments:

1. egui_hud is a logic crate that *chooses* egui-pop widgets; it draws
   nothing (owner's rule, made concrete in section 3's list of what
   egui-pop must add).
2. All placement is egui-pop's, the layout layer grows out of the
   existing `UiPop` layout-by-role helpers, and egui_fit and egui_stack
   stay egui-pop's solvers behind one door (section 8).
3. The missing piece is the doors and the index: `helios-ui`,
   `helios-rack`, the Clippy lists, the dependency test and a generated
   "use these" index (section 4).

**Slices**, each one small PR:

| # | Slice | Repository | Depends on |
|---|---|---|---|
| 1 | The "use these" index: `cargo xtask ui-index` listing egui-pop's `UiPop` helpers and widget constructors (later helios-ui's), checked in as `docs/ui-index.md`, linked from `AGENTS.md` | Helios | none |
| 2 | Dependency-worlds test (`xtask deps-worlds`), one `egui-pop` in the lock | Helios | none |
| 3 | Layout layer v0 inside `UiPop`: `row` and `grid` with baseline, gaps as `Space`; the gallery's roles page extended | egui-pop | none |
| 4 | egui-pop additions for egui_hud: scrub field, compact text and choice cells, value marks in `Indicator`, value bar, menu row | egui-pop | none |
| 5 | egui-pop release carrying 3 and 4; egui_hud moves to it (requirement compatible with Helios's pin) | egui-pop, egui_hud | 3, 4 |
| 6 | egui_hud: replace the raw `TextEdit` (render.rs:1199) and `Button` (render.rs:1728) and the other 30 raw calls with egui-pop widgets | egui_hud | 5 |
| 7 | egui_hud: painting into egui-pop primitives (scrub, marks, bar, swatches, read-outs), `visuals()` reads gone; its Clippy list on | egui_hud | 6 |
| 8 | `helios-ui` v0 (search, section, empty, action, details) with its Clippy entries, first two screens (visual-identity.md section 5) | Helios | 3 |
| 9 | `helios-rack` v0: the Parameters panel through it (`params.rs`' `Drawings` host moves into the frame) | Helios | 7 |
| 10 | Handle kinds in the frame; the mask handles (`masks.rs`) through helios-rack; the radial example as a test module | Helios, egui-pop (anchored HUD element) | 9 |
| 11 | Slot regions in egui-pop over egui_fit; egui_hud's card layout through them | egui-pop, egui_hud | 5 |
| 12 | Screens migrate family by family, each slice adding its Clippy entry (visual-identity.md section 5) | Helios | 8 |
| 13 | Plugin declarations for pages and controls over the protocol (controls.md slice 21) | Helios | 8, 9 |

## Decisions (2026-11-15)

- The name `helios-rack` is confirmed.
- egui_fit merges into egui-pop's workspace now, not only if versions
  drift; egui-pop then offers slot regions over it.
- Where the generated "use these" index lives is still open: options on
  the index slice.
