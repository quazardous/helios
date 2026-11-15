# Visual identity, the layout layer and helios-ui

Status: study (2026-11-15). Nothing here is built; the slices at the end
say what would be, in which order.

The owner's asks, on the same thread:

- Helios's screens look poor. egui-pop draws widgets well; what is
  missing is how widgets **relate to each other on screen**: a label, a
  value, a button and an icon on one line sitting on the same text
  baseline, columns that line up across rows, consistent heights,
  wrapping into a flow, sizes relative to each other and to their
  container, never pixel math. Forms and CRUD come later, on top.
- A search field should not differ from one module or window to
  another. Each module picks its controls directly in egui-pop (or raw
  egui), so nothing is normalised. Wanted: a thin Helios layer over
  egui-pop that sets the global style in one place, with direct calls
  forbidden by Clippy and justified exceptions only.

## In short

- **The widgets exist; Helios does not use them** (section 1). egui-pop
  already ships a search input, empty states, skeletons, a delayed
  spinner, collapsible sections, toggle chips, fault cards, data tables,
  command buttons, hover cards, tag chips, toolbars and text roles, all
  drawn from the theme's kit. Helios's desktop calls almost none of them:
  `EmptyState`, `Skeleton`, `Sections`, `ToggleChip`, `Fault`,
  `DataTable`, `CommandButton`, `Toolbar` and the `UiPop` text and space
  roles have **zero** uses; the shell draws with **504 raw egui widget
  calls in 39 files** instead. The look varies because every screen
  rebuilds its controls from egui primitives.
- **Divergence counted** (section 1): 3 search fields built 3 ways;
  section headers in 5 styles (`heading`, `strong`, collapsing, a text
  role, the admin manifest's section) with the role helper never used;
  empty states in 4 forms, none with an icon, one with an action;
  close buttons as a `×` glyph in 5 places beside the icon set; 36
  `add_space` calls, 29 of them literal points; 17 fixed widths and 9
  `allocate_exact_size`, 5 of them for placement; 14 literal colours in
  interface code.
- **Identity** (section 2): Helios already has its own themes (Helios
  Dark and Light, Rerun's neutral look over egui-pop's tokens). The
  identity is mostly *written down*, not invented: named tokens for type,
  space, radii, tones, icons and motion, plus photo-first rules (chrome
  recedes, neutral greys around images, accent never next to a photo).
- **The layout layer** (section 3): none of egui::Grid, egui_extras,
  egui_flex, egui_taffy or our egui_fit aligns on the text baseline, and
  none has em units. Recommendation: **own** a small relations layer in
  egui-pop-core (MIT, generic): `row`, `column`, `flow`, `grid` with
  header/value columns, tracks in `Content`, `Fr`, `Percent`, `Em` with
  `min`/`max`, equal-width groups, baseline alignment computed from the
  text metrics, gaps only from the spacing tokens. Measurement uses the
  previous frame's sizes, with one sizing pass on the first frame, as
  egui's own Grid does. egui_fit stays the HUD's slot packer.
- **Placement** (the parent analysis's A to D): **D, split**. Tokens and
  layout relations below, in egui-pop (generic, so egui-pop's own views
  can adopt them); Helios's intent-level controls above, in a new
  `helios-ui` crate.
- **helios-ui** (section 4): one function per Helios control, taking
  intent not style (`ui::search(Search::Library)`, `ui::empty(Empty::
  NoPhotos)`, `ui::section(..)`). Enforcement by `clippy.toml`
  `disallowed-methods` / `disallowed-types`, each with a reason naming
  the replacement; exceptions only through `#[expect(.., reason = "..")]`.
- **First slices** (section 5): the layout layer's `grid` and `row` with
  baseline on two screens; then search fields, section headers and empty
  states through helios-ui, each with its Clippy entry.

## 1. Audit

### Method

Counts are `grep` over `crates/`, `modules/` and `plugins/` at the time
of writing; they are a measure of spread, not exact call graphs.
"Raw egui widget calls" counts `ui.button`, `ui.small_button`,
`ui.label`, `ui.heading`, `ui.text_edit_singleline`, `ui.checkbox`,
`ui.radio_value`, `ui.selectable_label`, `ui.selectable_value`,
`ui.add_space`, `ui.separator`, `ui.spinner` and `egui::{Button,
TextEdit, Label, Checkbox, ComboBox, Slider, DragValue, Grid,
RichText, Frame, Spinner, ProgressBar, CollapsingHeader, Modal}::`.

Snapshots: the existing headless snapshots under
`crates/helios-gui/tests/snapshots/` (pop-dark, 1x and 2x) are enough to
show the variants; no new run was needed for this note. A side-by-side
page of every variant is the gallery's job (slice 1, section 5).

### Where the calls are

| Crate | `egui_pop::` paths | Raw egui widget calls |
|---|---|---|
| helios-gui | 834 | 539 |
| helios-controls-gallery | 13 | 24 |
| helios-i18n-extract | 6 | 0 |
| modules, plugins, other crates | 0 | 0 |

Only the desktop draws. Most of its `egui_pop::` paths are framework
(`AppHandle`, commands, ids, windows, steps), not widgets.

Raw calls by kind in helios-gui: `ui.label` 133, `ui.weak` 116 (counted
apart: muted text), `on_hover_text` 102, `ui.separator` 49,
`ui.add_space` 36, `selectable_label` 36, `egui::Button` 33,
`RichText` 32, `ScrollArea` 26, `egui::Label` 24, `ui.button` 21,
`ui.small_button` 17, `ui.checkbox` 17, `TextEdit` 15, `ComboBox` 15,
`selectable_value` 13, `radio_value` 12, `ui.heading` 12,
`ui.spinner` 9, `CollapsingHeader` 8, `Grid` 6, `DragValue` 6,
`ProgressBar` 5.

Files with the most raw calls (raw / `egui_pop::`):

| File | Raw | egui-pop |
|---|---|---|
| `shell/activity_focus.rs` | 44 | 2 |
| `shell/import_window.rs` | 39 | 14 |
| `shell/import_accounts.rs` | 38 | 1 |
| `shell/calibrate_focus.rs` | 37 | 9 |
| `shell/import_wizard.rs` | 32 | 9 |
| `macro_interface.rs` | 30 | 3 |
| `rule_editor_ui.rs` | 30 | 0 |
| `library.rs` | 26 | 16 |
| `workshops.rs` | 25 | 1 |
| `shell/import_profile_editor.rs` | 24 | 1 |
| `shell/import_profiles.rs` | 23 | 0 |
| `splash.rs` | 22 | 0 |
| `decisions.rs` | 19 | 2 |
| `params.rs` | 17 | 18 |

Two drawing paths coexist. The **declarative** one, egui-pop's admin
manifest (`Manifest::section`, `Field`, `Records::empty`, `Callout`),
draws Settings, the init wizard and parts of the import wizard: those
screens are consistent with each other. The **hand-drawn** one draws
everything else (Activity, the import window and its detail zone,
profile editors, calibration, workshops, macros, decisions).

### What egui-pop offers and what Helios uses

| Family | egui-pop has | Uses in Helios |
|---|---|---|
| Search field | `core::input::Input::search` (placeholder, leading icon, clearable, attached button, states); `views::library::filter::SearchField` (chips) | `SearchField` 1; `Input` 0 |
| Text roles, spacing | `UiPop::{section_title, muted, meta, text_role, space_role}`, `role_text` | `role_text` 7 (4 files); the trait's role helpers 0 |
| Buttons | `UiPop::button_tone`, `api::busy::CommandButton`, `band::button` | 0, 0, 0 |
| Toggles | `widgets::ToggleChip`, `Indicator::toggle` | 0 |
| Lists, tables | `views::list::ListView`, `views::item::ListItem`, `views::table::DataTable`, `PropertyRow`, `SelectableRow` | `ListItem` 13 (2 files); others 0 |
| Section headers | `core::sections::Sections` (collapsible, stacked), the section band | 0 |
| Empty, loading, error | `placeholder::EmptyState` (icon, title, text, action), `Skeleton`, `progress::Spinner` (delayed), `fault::Fault` | 0, 0, 0, 0 |
| Badges, chips | `Indicator::*`, `TagChip`, `MarkChip`, library `Badge` | `Indicator` 7, `MarkChip` 5, `Badge` 28 |
| Toolbars | `api::toolbars::Toolbar` (command-driven, overflow menu) | 0 |
| Dialogs | `api::dialogs::Dialogs` / `DialogSpec`, `overlay::DialogLayout` | `DialogLayout` 6; `egui::Modal` 3, `egui::Window` 2 |
| Toasts | `api::toasts::Toast::{info, success, warning, error}` | 13 (7 files), each called directly |
| Tooltips | `on_hover_text` (themed), `HoverCard` | `on_hover_text` 102; `HoverCard` 0 |

### Control families, variant by variant

**Search fields: 3 in Helios, built 3 ways** (plus 5 more inside
egui-pop's own views, each a `TextEdit` with its own hint).

| Where | Built with | Label | Hint | Icon | Clear | Width |
|---|---|---|---|---|---|---|
| Library panel (`library.rs`) | egui-pop `SearchField` | accessible label | "Search images…" | magnifier | yes | `(available - 72).clamp(80, 220)` |
| Macro interface (`macro_interface.rs`) | `ui.text_edit_singleline` | visible "Filter" label beside | none | none | no | egui default |
| Workshops (`workshops.rs`) | `egui::TextEdit::singleline` | visible "Find" label beside | "what you want: brighter…" | none | no | full width |

None takes Ctrl+F; only the Library's has the theme's search-field kit.

**Section headers: 5 styles.** `ui.heading` 12 (6 files: Activity,
calibration, import window, accounts, source list, rule editor);
`.strong()` 40 (22 files, as section titles in the profile editor,
wizard, workshops, queue, decisions); `CollapsingHeader` 8 (5 files);
`role_text(TextRole::Section)` 7 (Storage, settings); the admin
manifest's own section. `UiPop::section_title`, made for this, has no
use. In `import-window-profiles-pop-dark-1x.png` the left column's
"Import profiles" is a heading while the right column's "Copy",
"Naming", "Filter", "Apply" are strong body text: two levels for one
role, side by side.

**Empty states: 4 forms, no icon, one action.** A bare `ui.weak` line
(Activity's "No background jobs", the queue's "Nothing queued", import
history's "No import yet", the source list's "None yet", "No folder
yet", "No card inserted"); `horizontal_wrapped` with an Import button
and a weak line (the Library's "No images"); `vertical_centered` blocks
(accounts, 2); the admin manifest's `Records::empty(text, action)` (6,
the only ones with an action). `import-window-empty-pop-dark-1x.png`
shows the first form four times in one column and a fifth phrasing
("No photos in the sources ticked") in the middle.

**Loading and errors.** `ui.spinner` 9 and `egui::Spinner` 2, none
delayed (egui-pop's `Spinner::delayed` waits 300 ms and shows at least
500 ms, so quick loads do not flicker); `ProgressBar` 5 with egui's
default track, which reads as a black bar on a selected row
(`activity-running-pop-dark-1x.png`); errors as `colored_label` with
`warn_fg_color` 6, `error_fg_color` 4, a custom colour 9, and a "⚠"
glyph 2, beside 18 uses of egui-pop's `Indicator` for the same idea.

**Buttons and icon buttons.** `ui.button` 21, `ui.small_button` 17,
`egui::Button::new` 29 (14 of them `.small()`), `button_tone` 0. Five
remove or close buttons are a `×` text glyph (`macro_interface.rs` 3,
`pointer_choice.rs`, `rule_editor_ui.rs`) while the icon set has
`Icon::Close`. Primary actions are not marked: the wizard's "Add the
source" is drawn like "Cancel" and "Back"
(`import-window-wizard-name-pop-dark-1x.png`).

**Toggles and choices.** `ui.checkbox` 17, `radio_value` 12,
`selectable_value` 13, `selectable_label` 36. The same kind of choice
is drawn three ways: one-of-four naming schemes as selectable labels
(a segmented look), duplicates policy as radio buttons, a file kind as
a combo box, all in the same profile editor.

**Lists and tables.** Activity's jobs list is hand-painted rows
(`allocate_exact_size` with a 50 pt `ROW` constant); its detail is an
`egui::Grid`; the queue, calibration patches and account form are
`Grid`s too; `ListView` and `DataTable` are not used.

**Dialogs and toasts.** `DialogLayout` 6, `egui::Modal` 3,
`egui::Window` 2. Toasts are posted straight to `TOASTS` from 7 files;
the log and alerts note already moves them behind one function.

### Hand-placed pixels

| What | Count | Examples |
|---|---|---|
| `add_space` | 36 | literal 8.0 ×10, 4.0 ×8, 6.0 ×7, 12.0 ×2, 16.0 ×1; 4 computed centrings |
| Fixed widths and heights | 17 | `TextEdit` at 84, 140, 240, 320 pt; `set_width(400)`, `set_width(360)`, `set_min_width(560)`, `set_max_width(440)`; `max_height(200)` ×4 |
| `allocate_exact_size` | 9 | 5 for placement (Activity rows, source list rows and labels, import window, wizard, profile editor), 4 for painting |
| Layout constants in points | 9 | `SOURCES_WIDTH 220`, `CHOICES_WIDTH 300`, `SEARCH_BESIDE 72`, Activity `ROW 50`, the graph's node and rank steps |
| Literal colours in interface code | 14 | calibration's status colours (4), a scan placeholder blue, `WHITE`/`GRAY`/`YELLOW` strokes; scopes and navigator are pixel code and fine |

### Misalignment seen in the snapshots

- **Baseline of label and field.** In the wizard's "Source name" row the
  label's text sits about 3 pt below the field's text: egui centres both
  boxes, and the field's box is taller than the label's
  (`import-window-wizard-name`).
- **Headers across columns.** "Sources" shares its row with the "Add…"
  button and sits about 3 pt lower than "Contents" and "Choices" beside
  it (`import-window-empty`).
- **Mixed fonts on one line.** Activity's log lines put a proportional
  time beside monospace text; their baselines differ
  (`activity-running`).
- **Ragged columns.** The import window's three zones have fixed widths
  (220, 300 pt) that do not follow the font size; a key/value block in
  Activity lines up only because it is a `Grid`.
- **Footer.** Every import dialog keeps an empty dark strip under its
  buttons, and the buttons float right with nothing marking the primary.

## 2. Identity

### What already exists

Helios has its own themes (`crates/helios-gui/themes/*.tokens.json`,
`crates/helios-gui/src/theme.rs`): Helios Dark and Helios Light extend
egui-pop's `pop-dark`/`pop-light` with the Rerun viewer's neutral greys
and blue accent, plus Helios component tokens (`viewport.surround`,
`state.*`, `mask.*`). egui-pop resolves them into typed tokens: surfaces,
text, borders, accent, status, a spacing scale with intent aliases, a
type scale with text roles, radii, control and icon sizes, motion, two
densities, and a kit of about a hundred component files. Icons are one
set, Phosphor Regular, through egui-pop's `Icon` roles and
`phosphor:<name>`.

So the identity is not missing; it is **not applied**, because screens
bypass the roles. The tokens below are the names screens may use; each
maps onto an existing egui-pop token unless marked *new*.

### Tokens

**Type** (egui-pop `TextRole`, `font.size.*`):

| Token | Use | Helios Dark |
|---|---|---|
| `text.title` | a page or window title | xl, strong, 600 |
| `text.section` | a section header | m, strong, 600 |
| `text.body` | everything else | m, default |
| `text.muted` | descriptions, empty states, secondary values | m, muted |
| `text.meta` | captions, counts, timestamps | xs, muted |
| `text.mono` | paths, file names, log lines | mono at body size *(new rule: same size as body, so baselines meet)* |
| `text.numeric` | values in tables and grids | tabular figures |

Two levels of header only: title and section. No `heading` inside a
page.

**Space** (egui-pop `space.*`, scaled by density): `inline` between items
on a line, `group` between related groups, `section` before a section,
`page` around a page. Screens name the intent, never points. *New*:
`em`, one line of body text (font size × line height), the unit of
relative sizes in section 3.

**Size**: `control.height` (one height for every single-line control
on a line), `icon.inline` / `icon.toolbar` / `icon.panel`,
`dialog.width`, `toast.width`; *new*: `field.width.short` (8 em),
`field.width.medium` (16 em), `field.width.long` (fill), so the four
field widths of today (84, 140, 240, 320 pt) become three named ones
that follow the font.

**Radius**: `radius.widget` for controls and chips, `radius.window` for
windows and cards. Thumbnails: no radius (a photo keeps its corners).

**Tones** (egui-pop `kit::Tone`): `neutral`, `accent`, `info`,
`success`, `warning`, `danger`, `muted`. Every status colour on screen is
a tone; calibration's red, green and amber become `danger`, `success`,
`warning`.

**Icons**: Phosphor Regular only, at the three sizes; an action shown
as an icon always has a tooltip and an accessible name; no text glyph
(`×`, `⚠`, `▲`) stands for an icon.

**Motion** (egui-pop `motion.*`): spinners delayed 300 ms and shown at
least 500 ms; no animation over a photo.

**Dark and light**: both themes carry every token; light follows the
system preference. High contrast stays a full theme.

### Photo-first rules

Borrowed where photographers expect them: darktable's and Lightroom's
neutral grey surround, Lightroom's and macOS Photos' receding chrome.

1. **Neutral around images.** `viewport.surround` and every surface that
   touches a photo are pure neutral greys (equal R, G, B); no tinted
   panel next to a photo. Helios Dark's greys already are.
2. **The accent never borders a photo.** Selection on thumbnails is a
   neutral light outline, not an accent fill; the accent fills controls
   and selected list rows away from images. (Today the library selection
   and the Activity selected row use the saturated blue.)
3. **Chrome recedes.** Panels at rest show no lines (Helios Dark: no
   dock outlines, scroll bars on hover); controls over a photo fade
   when the pointer leaves (the HUD already does).
4. **No colour cast in what judges colour.** Badges over thumbnails are
   small, in neutral pills, toned only for warnings (offline, missing).
5. **Density follows the task.** Compact density in panels next to the
   photo (darktable's module panel), comfortable in settings and windows.
6. **A middle-grey option.** A "neutral 18 %" surround for colour
   judgement, as darktable and Lightroom offer: a theme token, not a new
   theme. *Not decided.*

## 3. The layout layer

### Survey

| | egui::Grid | egui_extras StripBuilder | egui_extras TableBuilder | egui_flex 0.9 | egui_taffy 0.16 | egui_fit 0.1 (ours) |
|---|---|---|---|---|---|---|
| Licence | MIT/Apache | MIT/Apache | MIT/Apache | MIT | MIT | MIT/Apache |
| Purpose | aligned rows and columns | split space along an axis | large tables | flexbox subset | CSS block, flex, grid (taffy) | dense slot packing in twelfths (HUD cards) |
| Nesting | any `Ui` in a cell | `cell.strip` | `Ui` in a cell | through `add_flex` | full tree | groups only |
| Alignment | left-centre only | per cell `Layout` | per cell `Layout` | start, centre, end, stretch, justify | full CSS set | none (rects) |
| Text baseline | no | no | no | no | no (its measure function returns no baseline) | no |
| Fractions, weights | no | `relative`, `remainder` | `remainder` | `grow` | `fr`, grow, shrink | slot steps |
| % of parent | no | `relative` | no | `Percent` | yes | twelfths |
| Min / max | min column, max as wrap width | `at_least`, `at_most` | `range` | min only | min, max, `minmax` | max growth |
| Content-sized | yes (previous frame) | no | `auto` (sizing pass) | yes (previous frame) | min- and max-content | fixed forms |
| Equal widths | no | `remainder` | `remainder` | equal `grow` | `fr` | no |
| em units | no | no | no | no | no | base unit = a row |
| Passes | sizing pass and discard on its first frame | none | sizing pass for `auto` | discard on change | sizing pass for new nodes, discard on recompute | none (no egui) |
| Cost per frame | very low | very low | low | low | moderate (tree, mutex, full relayout on change) | low |

Findings:

- **Nobody aligns on the baseline**, and **nobody has em units**: both
  are ours to write whatever we pick.
- **egui_taffy** has the richest model but is the heaviest, announces
  breaking changes per release, and its tree-builder API does not match
  egui-pop's builders; adding baseline means patching its measure
  function. **egui_flex** lacks max sizes and baseline, and its author
  asks to use it sparingly. Neither is worth a dependency for what we
  need.
- **egui_extras' `Size`** (`exact`, `relative`, `remainder`,
  `at_least`, `at_most`) is the right vocabulary for tracks; its strips
  never measure content, so we borrow the model, not the crate.
- **egui::Grid** shows the measuring technique that works in immediate
  mode: column widths from the previous frame, one invisible sizing pass
  plus `request_discard` on the first frame.
- **egui_fit** packs HUD cards into slots; it is not a general layout and
  stays where it is. Its "base unit = one row" idea is the em unit below.
- **egui_stack** is docking and the window-level zone solver (panels,
  min/wanted/max, fold); a widget-level layout does not belong there.
- Large virtualised tables stay on egui-pop's `DataTable` (egui_table).

**Decision proposed: own** a small relations layer, in egui-pop-core
(generic, MIT), borrowing egui_extras' size model and Grid's measuring.

### Placement

The parent analysis's options: A below egui-pop, B a sibling layer in
egui-pop, C in Helios only, D split. **D**:

- **Tokens and layout relations in egui-pop-core**: one source of truth
  for spacing and sizes; egui-pop's own views (file browser, dialogs,
  settings) can adopt the layout without a cycle; other egui-pop users
  get it; additive, no API break.
- **Intent controls in Helios (`helios-ui`)**: what a "library search"
  or "no photos yet" is belongs to Helios; egui-pop stays generic.
- The admin manifest (declarative pages) keeps rendering through
  egui-pop and moves onto the same layout, so declarative and hand-drawn
  screens line up.

The two throwaway spikes the parent analysis planned are replaced by the
first slice itself: D is additive, so its first slice (two real screens
on the new layout) is the comparison, and it is kept if it holds.

### API sketch

```rust
use egui_pop::layout::{Align, Gap, Track, grid, row, flow};

// Header/value grid: headers as wide as the widest header, values take
// the rest; every row on one baseline.
grid(ui, "activity.detail")
    .tracks([Track::Content, Track::Fr(1.0)])
    .align(Align::Baseline)
    .gap(Gap::Group)
    .show(|g| {
        g.header(t!("Kind"));
        g.value(|ui| ui.label(kind));
        g.header(t!("Started"));
        g.value(|ui| ui.label(started));
    });

// A line: a search field taking what is left, between 8 and 24 em, an
// icon button sized to content, all on one baseline.
row(ui)
    .gap(Gap::Inline)
    .align(Align::Baseline)
    .item(Track::Fr(1.0).min(Track::Em(8.0)).max(Track::Em(24.0)), |ui| search(ui))
    .item(Track::Content, |ui| add_button(ui))
    .show();

// A button bar: every button as wide as the widest, at the end.
row(ui).justify(Align::End).equal_widths().show_each(buttons, |ui, b| b.show(ui));

// Chips that wrap like text.
flow(ui).gap(Gap::Inline).align(Align::Baseline).show_each(chips, |ui, c| c.show(ui));
```

- **Tracks**: `Content` (intrinsic), `Fr(w)` (a weight of what is
  left), `Percent(p)` (of the container), `Em(n)` (lines of body text,
  so sizes follow the font, the text scale and the density), each with
  `.min(..)` and `.max(..)`. `Track::Points` exists for painting code
  only and is on the disallowed list outside it.
- **Equal widths**: `.equal_widths()` on a row, or a named group shared
  across rows (`Track::Same("dialog.buttons")`) for buttons in several
  places.
- **Alignment**: `Start`, `Center`, `End`, `Stretch`, `Baseline`, on
  both axes for rows and grids; `Baseline` is the default for rows and
  grids of text and controls.
- **Gaps**: only `Gap::{Inline, Group, Section, Page}`, the spacing
  intents; no points.
- **Nesting**: any cell holds a `Ui`, so rows, grids and flows nest.
- **Responsive**: `grid(..).fold_below(Track::Em(30.0))` turns a
  header/value grid into stacked header-over-value when narrow (the
  "two columns to one" the parent asked for).

### How baseline works

egui places each widget once and centres single-line widgets in their
box; no `Response` carries a baseline. The layer computes it:

1. Each cell's **first baseline** is recorded during the frame: a
   helios-ui or egui-pop control reports it exactly (it knows its galley
   and padding); for a raw egui widget the layer estimates it from the
   cell's first text row: `top + (height − row_height) / 2 + ascent`,
   exact for egui's single-line widgets with body text.
2. The row's baseline is the largest first baseline; each cell is
   offset by the difference, using the sizes stored from the previous
   frame (cell sizes and baselines in temp memory, keyed by the layout's
   id).
3. On the first frame, or when a cell's size changes, one invisible
   sizing pass and `request_discard`, as egui's Grid and egui_taffy do;
   the app's `max_passes` is already 2.

Cost: a small vector per layout, no allocation at steady state.

## 4. helios-ui

### The crate

`crates/helios-ui` (GPLv3, Helios only), depending on egui-pop. One
function per Helios control, each taking **intent**: what the control is
for, never a width, a colour or a hint string. It reads tokens from the
theme and lays out with egui-pop's layout layer. Modules and plugins
that draw (today only helios-gui and the controls gallery) go through it.

| Family | helios-ui | Built on |
|---|---|---|
| Page and section | `ui::page(title, ..)`, `ui::section(Section::new(title).description(..).collapsible())` | text roles, `Sections` |
| Search | `ui::search(state, Search::Library \| Search::Find(scope))`: placeholder from the scope, clearable, Ctrl+F when focused panel | `Input::search`, `SearchField` for the Library |
| Text fields | `ui::text(value, Field::Name \| Field::Path \| Field::Pattern)`: width from the field kind | `Input::text`, `Input::path` |
| Buttons | `ui::action(Action::Primary \| Secondary \| Danger, label)`, `ui::icon_action(Icon, tooltip)`, `ui::close()` | `button_tone`, `CommandButton`, icons |
| Choices | `ui::toggle(&mut bool, label)`, `ui::choice(&mut T, options)`: segmented up to four short options, a menu beyond | checkbox kit, `ToggleChip`, `combo_box` |
| Lists, tables | `ui::list(..)`, `ui::table(..)`, `ui::details(..)` (header/value grid) | `ListView`, `DataTable`, layout `grid` |
| States | `ui::empty(Empty::..)`, `ui::loading(..)`, `ui::failed(err)`, `ui::value(Value<T>)` | `EmptyState`, `Skeleton`, delayed `Spinner`, `Fault` |
| Badges | `ui::badge(Tone, label)`, `ui::chip(..)` | `Indicator`, `TagChip`, `MarkChip` |
| Toolbars | `ui::toolbar(menu_id)` | `Toolbar` |
| Dialogs | `ui::dialog(..)` with its button bar (primary last, equal widths) | `Dialogs`, `DialogLayout` |
| Notifications | `ui::notify(record)` (the log and alerts note's one function) | the log sink, then toasts |
| Paths and names | `ui::path(..)`: mono, truncated in the middle, full text in the tooltip | text roles |

`Empty` names the cases (`NoPhotos`, `NoJobs`, `NoSource(kind)`,
`NothingQueued(node)`, …), each with its icon, its words and, when there
is one, its action; screens pick a case, they do not write the sentence.

### Generic in egui-pop, Helios in helios-ui

| egui-pop (generic, MIT) | helios-ui (Helios, GPLv3) |
|---|---|
| tokens, kit, text roles, spacing intents | which token a Helios control uses |
| the layout layer (rows, grids, flow, tracks, baseline) | the screens' standard compositions (details grid, dialog button bar) |
| widgets: `Input`, `EmptyState`, `Spinner`, `Sections`, `DataTable`, `Indicator`… | the intents: library search, "no photos yet", a job's state |
| the admin manifest renderer | Helios's manifests |

A widget missing in egui-pop is added there first (generic), then
wrapped here; helios-ui never paints.

### Enforcement

`clippy.toml` at the workspace root lists what screens may not call,
each with a reason naming the replacement. The box's check job runs
Clippy with warnings denied, so a direct call blocks the merge.

```toml
disallowed-methods = [
  { path = "egui::Ui::heading", reason = "use helios_ui::section or helios_ui::page" },
  { path = "egui::Ui::text_edit_singleline", reason = "use helios_ui::text or helios_ui::search" },
  { path = "egui::Ui::small_button", reason = "use helios_ui::action or helios_ui::icon_action" },
  { path = "egui::Ui::add_space", reason = "use a layout gap (egui_pop::layout::Gap)" },
  { path = "egui::Ui::colored_label", reason = "use helios_ui::badge or a text role in a tone" },
  { path = "egui::Ui::spinner", reason = "use helios_ui::loading (delayed spinner)" },
  { path = "egui::Ui::radio_value", reason = "use helios_ui::choice" },
  { path = "egui::Ui::selectable_value", reason = "use helios_ui::choice" },
  { path = "egui::Ui::set_width", reason = "use a layout track (Fr, Em, Content)" },
  { path = "egui::Ui::set_min_width", reason = "use a layout track with .min()" },
  { path = "egui::Ui::set_max_width", reason = "use a layout track with .max()" },
  { path = "egui::Color32::from_rgb", reason = "use a theme token or a tone" },
  { path = "egui_pop::api::toasts::Toasts::notify", reason = "use helios_ui::notify, which writes the record first" },
]
disallowed-types = [
  { path = "egui::TextEdit", reason = "use helios_ui::text or helios_ui::search" },
  { path = "egui::CollapsingHeader", reason = "use helios_ui::section(..).collapsible()" },
  { path = "egui::Grid", reason = "use egui_pop::layout::grid or helios_ui::details" },
  { path = "egui::Spinner", reason = "use helios_ui::loading" },
  { path = "egui::ProgressBar", reason = "use helios_ui::progress" },
  { path = "egui::Modal", reason = "use helios_ui::dialog" },
  { path = "egui_pop::views::library::filter::SearchField", reason = "use helios_ui::search(Search::Library)" },
]
```

- **Exceptions**: `#[expect(clippy::disallowed_methods, reason = "..")]`
  on the one item that needs it: helios-ui itself at its crate root;
  painting code (scopes, navigator, the graph, mask display) for colours
  and points; a justified one-off elsewhere. `expect` rather than
  `allow`, so an exception that is no longer needed fails too.
- **Reasons everywhere**: workspace lints
  `clippy::allow_attributes_without_reason = "deny"` and
  `clippy::allow_attributes = "deny"`. The workspace has 97 `#[allow]`
  today, 93 without a reason; they move to `#[expect(.., reason)]` in
  the enforcement slice (mechanical), before the lints are denied.
- **Associated constants** (`Color32::WHITE`) cannot be listed; review
  and the gallery catch them.
- **Too coarse?** If a path is too broad (forbid a builder only with some
  options), a dylint custom lint is the next step; not needed to start.
- The entries land **one per migration slice**: a family is forbidden
  in the slice that moves its last caller, so `next` stays green.

## 5. Migration slices

Each slice: one PR, before/after snapshots in its tests, the gallery
page updated, its Clippy entries added.

0. **The gallery page of controls.** A "Helios controls" page in the
   gallery showing every helios-ui control and state side by side, dark
   and light, with today's variants beside them while they exist. It is
   where UI bugs get reported, and its snapshots are the before/after
   of every slice.
1. **Layout layer, `grid` and `row` with baseline** (egui-pop). Shown on
   Activity's job detail (a header/value grid) and the wizard's name row
   (label + field on one baseline). After: the "Source name" label and
   its field share a baseline; the detail grid's headers size to the
   widest.
2. **helios-ui crate, search fields.** `ui::search` replaces the three
   search fields. Before: a "Filter" label + bare field, a "Find" label
   + hinted field, the Library's field. After: one field, magnifier,
   placeholder from the scope, clear button, Ctrl+F; `TextEdit` and
   `SearchField` forbidden outside helios-ui.

   ```rust
   // before (macro interface)
   ui.horizontal(|ui| {
       let l = ui.label(tr(ui, Message::new("gui-macro-iface-filter", "Filter")));
       ui.text_edit_singleline(&mut self.filter).labelled_by(l.id);
   });
   // after
   helios_ui::search(ui, &mut self.filter, Search::Find(Scope::MacroParameters));
   ```
3. **Section headers.** `ui::section` and `ui::page` replace `heading`,
   strong titles and `CollapsingHeader`. After: the profile editor's
   "Import profiles" and "Copy", "Naming" are one level; the import
   window's three column headers on one baseline. `heading` and
   `CollapsingHeader` forbidden.
4. **Empty, loading and error states.** `ui::empty`, `ui::loading`,
   `ui::failed`. Before: "None yet", "No folder yet", "No card
   inserted" as weak lines. After: one empty state per case, with its
   icon and its action ("Add a folder…", "Connect an account…").
   `spinner`, `Spinner`, `colored_label` forbidden.
5. **Buttons and dialogs.** `ui::action`, `ui::icon_action`,
   `ui::close`, the dialog button bar (primary last, equal widths, no
   empty footer). The `×` glyphs go.
6. **Choices and toggles**, then **lists and tables** (Activity's jobs
   on `ListView`, the queue on `ui::details`).
7. **Pixels out.** The remaining `add_space`, fixed widths and layout
   constants become gaps and tracks; `add_space`, `set_*width` and
   `Color32::from_rgb` forbidden outside painting code. The
   `allow_attributes` lints are denied once the `#[allow]`s are moved.
8. **Forms and CRUD**, later, as compositions of the above, where a
   screen needs them (the parent analysis's form layer).

### How the neighbouring work fits

- **Per-value spinners** (the Storage page's values that arrive late,
  served by the read kernel): `ui::value(Value<T>)` draws a value in its
  state: fresh (the value), loading (a delayed inline spinner the size of
  the text, the label already shown), stale (the old value, muted, with
  a refresh mark), failed (a warning badge with the reason in the
  tooltip). The labels never wait for the data. The admin manifest's
  `Status` and `Records` use the same states.
- **Notifications** (log and alerts note): `ui::notify` is that note's
  single function; toasts, the notification area's badges and the Log &
  Alerts window draw with `ui::badge`, `ui::list` and the tones here.
- **The gallery**: slice 0 makes it the reference page for every control
  and state; the owner's goal of reporting UI bugs from the gallery
  works because each control there is the one screens use.

## Open questions (not decided)

- The neutral 18 % surround as an option (section 2, rule 6).
- Thumbnail selection as a neutral outline instead of the accent fill.
- Whether helios-ui's `Empty` cases live in Helios's i18n or are
  declared by each module (modules adding their own cases).
- Whether the controls gallery and egui-pop's gallery merge into one
  page for Helios.
