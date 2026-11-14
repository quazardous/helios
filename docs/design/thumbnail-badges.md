# Thumbnail badges

Status: study (2026-11-14). Nothing here is built yet; the slices at the
end are the proposal.

The owner's ask: in the import window, a photo the Library holds already
carries the words "in your Library" over its thumbnail (beside
"RAW+JPG"). It should be a visible icon instead, the same idea should
hold for every thumbnail in the Library, and modules should be able to
add their own badges through an open contract.

## 1. What is drawn on a thumbnail today

Two painters draw everything, both in egui-pop:

- **The file browser** (`egui_pop::api::files`, `FileBrowser::with_marks`)
  draws the import window's Contents. An entry's `EntryMarks` is a list
  of strings and a `dimmed` flag; `paint_marks` stacks the strings as
  plain text boxes from the image's **bottom left**, upwards. The
  accessible name is the entry's name followed by its badges
  (`EntryMarks::label`: "IMG_0001.CR3 · in your Library").
- **The library views** (`egui_pop::views::library`, `ThumbGrid` for the
  grid and the filmstrip, `Compare` for the compare panel). Marks
  (rating, labels, flag) and the stack chip are drawn as icons; the
  application's `Badge`s (`text` + `tone`) are drawn as tinted pills in
  a row from the **bottom right** leftwards. Below `OVERLAY_MIN` a cell
  draws no overlay. The cell's accessible name is the photo's name only:
  the badges are not said.

### Import window (`crates/helios-gui/src/shell/import_window.rs`, `sync_browser`)

| Overlay | Drawn as | Where | Cost |
|---|---|---|---|
| offline (volume not plugged in) | text | bottom left stack | cell field, from the image map's record |
| in your Library | text, only while "New only" is on (the cell is also dimmed) | bottom left stack | `Cell::known()`; one batched `helios_library::fetch::in_library` per source read, off the UI thread (`main.rs`) |
| RAW+JPG | text | bottom left stack | `Cell::is_pair()`, from the scan's grouping (`import_scan.rs`) |
| not importable | dimmed cell | whole image | `Contents::importable(i)` |
| chosen remote photos (Google Photos) "in your Library" | same text badge | same | `KnownRemote`, one call per batch (`import_chosen.rs`, `import_accounts.rs`) |

The marks are rebuilt only when the contents' revision changes, into a
map by path the browser's closure reads per cell: no read per cell.
There is no "new" badge: "new" is the absence of "in your Library", and
the New only filter hides the rest from the selection.

### Library grid and filmstrip (`crates/helios-gui/src/library.rs`, `Items::badges`)

The grid (`ThumbGrid::new("grid")`) and the filmstrip
(`ThumbGrid::filmstrip("strip")`) share one `Items`, so they show the
same overlays.

| Overlay | Drawn as | Where | Cost |
|---|---|---|---|
| flag (pick / reject) | icon (flag, cross) | top left | `Entry::marks()`, from the windowed library read |
| colour labels | dots | top right | same |
| rating | 5 stars, empty stars on hover, clickable | bottom left | same |
| stack | pill with count and chevron, clickable (expand / collapse) | top left, under the flag | `Entry::stack`, from the read |
| RAW | text pill | bottom right row | file extension (`helios_load::Kind::of`), no I/O |
| offline / missing | text pill, warning / danger | bottom right row | `Entry::presence`, from the read; hover tooltip (where the photo was last seen) by `presence_hover` |
| changed outside | text pill, info | bottom right row | `Entry::changed_outside`, from the read |
| quick (quick collection) | text pill, info | bottom right row | `quick_set`, held in memory |
| N versions | text pill, info | bottom right row | `head_counts`, one scan per refresh (`set_heads`) |
| backed up / backup short / unverified / no backup | text pill, success / warning / info / danger; only when a strategy is set | bottom right row | `backups`, one read per refresh (`set_backups`) |
| soft focus / blown highlights / crushed shadows | text pill, warning | bottom right row | `Entry::focus`, `clip_white`, `clip_black`, from the read |

There is no "edited" (develop history) badge yet, and no keyword, GPS or
album-membership badge.

### Compare (`egui_pop::views::library::compare`)

Each pane draws its label as text at the bottom left and a "best" star
button in the top right corner (a checkbox to assistive technology). No
application badge.

### What follows

- Every badge already comes from data cached per refresh or per source
  read; nothing reads the vault per cell. That property is the one to
  keep (`library_scale`'s read counts, `docs/performance.md`).
- Status is told in words of varying length, so three badges overflow a
  small cell, and their order is the code's order, not a priority.
- The two painters disagree on the corner (bottom left vs bottom right),
  on the look (plain box vs toned pill) and on accessibility (said with
  the name vs not said).

## 2. What established applications do

From general knowledge of their current versions; menu names drift
between releases.

| Application | Badges | Corners | Always or hover | User choice |
|---|---|---|---|---|
| Lightroom Classic | flag, stack count (top left); unsaved metadata, missing file "!" (top right); thumbnail badges: keywords, map pin, in a collection, cropped, develop adjustments, comments, virtual copy page curl (bottom right / left) | flag and stack top left, status top right, content badges bottom right | rotate buttons and flag "on mouse over only" is an option; badges always | View Options: compact / expanded cells, "Thumbnail badges", "Show clickable items on mouse over only" |
| Lightroom import | photos already imported are dimmed and unticked ("Don't import suspected duplicates") | — | always | the option itself |
| Capture One | rating and colour tag under the image, adjusted / cropped / offline / variant icons, stack count | status icons in the top corners, labels in the caption | always; quick buttons on hover | Browser label options |
| darktable lighttable | stars and reject (bottom), colour label dots, "altered" icon (top right; its tooltip lists the history), group "G" (top right), file extension text (top left) | as listed | overlay modes: none, on hover, extended on hover, always, always extended, on hover with timeout | the star button in the top panel picks the mode; extended overlays take a text pattern |
| Photo Mechanic | tag checkbox, colour class, rating, rotate, edit pencil, R+J for paired files, small file-state icons | tag bottom right, the rest along the cell's bottom bar | most always; rotate on hover | contact sheet preferences |
| digiKam | rating, pick label, colour label, rotate, group, geolocation | rating on the bottom edge, rotate and select on hover in the corners | rating and labels always; actions on hover | icon view settings (which overlays show) |

Common patterns:

1. **Icons for status, short text for file type.** Everyone shows
   "RAW", "R+J", "CR2" as text and states (edited, missing, offline,
   in collection) as icons.
2. **Fixed corners by role.** Selection marks (flag) top left, status
   top right, content badges bottom right, rating along the bottom.
3. **Hover shows actions, not facts.** Facts stay visible; buttons
   (rotate, rate, flag) appear on hover.
4. **Badges are clickable shortcuts** (Lightroom's keyword badge opens
   keywording, the develop badge opens Develop).
5. **The user picks** which overlays show, in the view options.
6. **Already imported is dimmed** in an import grid; an icon on top
   tells why.

## 3. One decorator contract

A badge is declared once and computed many times:

```rust
/// Declared once (a registry entry, or a plugin manifest's [[badges]]).
pub struct BadgeDecl {
    pub id: String,            // "helios.in-library", "<plugin>.ai-pending"
    pub icon: String,          // egui-pop's icon registry: "phosphor:check-circle"
    pub text: Option<String>,  // a short text instead of / beside the icon ("RAW", "{versions}")
    pub name: Message,         // accessible name and tooltip ("in your Library")
    pub detail: Option<Message>, // a longer tooltip, with the facts as arguments
    pub tone: Tone,            // egui-pop's tone: neutral, info, success, warning, danger
    pub slot: Slot,            // TopLeft | TopRight | BottomLeft | BottomRight
    pub priority: i16,         // higher stays when a slot overflows
    pub visibility: Visibility, // Always | Hover | Never (the default the user may change)
    pub surfaces: Surfaces,    // import, library (grid + filmstrip), compare
    pub when: Predicate,       // over the cell's facts (below)
    pub command: Option<(CommandId, Value)>, // run on click, with the cell's key
}
```

**Facts, not callbacks.** A cell carries `Facts`: a flat map of typed
values built from what the surface already caches. The Library's come
from `Entry` (presence, kind, changed outside, marks, culling measures),
the quick set, the head counts and the backups, all read per refresh.
The import window's come from the cell (offline, pair, known) and from
the connector's item. A predicate is data:

```toml
when = { fact = "presence", is = "offline" }
when = { fact = "versions", gt = 1 }        # text = "{versions}"
when = { all = [{ fact = "kind", is = "raw" }, { not = { fact = "edited" } }] }
```

It is evaluated per visible cell, in memory, against those facts. A
predicate never reads the vault, a file or a node: so the library read
stays one query per window and `library_scale`'s counts do not move.
A fact that is not cached yet is added to the windowed read (a column
or a join in the same query), never to a per-cell read; the
`library_scale` test is the guard.

**Painting** moves into egui-pop, one painter for both widgets:
`Badge` (library views) and `EntryMarks` (file browser) gain `icon`,
`name`, `slot`, `priority`, `hover_only` and an optional click id, with
`Badge::new(text)` kept as today's text pill. Status badges are icons
(16 px, on the kit's `thumb_overlay_badge` box, tinted by tone); file
type stays short text, as everyone does. Helios builds the `Badge`s
from the declarations and the facts; egui-pop knows no Helios id.

**Corners** (Lightroom and darktable agree): top left selection (flag,
stack, as today), top right status (offline, missing, changed outside,
backup state, in your Library), bottom left rating (as today), bottom
right content (versions, quick, culling, file type, module badges).

**The import window knows only connector contracts.** It hard-codes no
badge. Its facts are the ones every source gives through the connector
contract (`in_library` from the prefilter, `pair` from the item's
`group`, `offline` from the volume, `kind` from `ItemKind`, the item's
`metadata` keys), and a connector adds badges by declaring them over
those facts in its manifest (a service's "shared album", "favourite").

First icons proposed (all in egui-phosphor 0.14):

| Badge | Icon | Slot | Tone |
|---|---|---|---|
| in your Library | `phosphor:check-circle` | top right | success |
| RAW+JPG | text "R+J" | bottom right | neutral |
| offline | `phosphor:plugs` | top right | warning |
| missing | `phosphor:link-break` | top right | danger |
| changed outside | `phosphor:arrows-clockwise` | top right | info |
| quick | `phosphor:lightning` | bottom right | info |
| N versions | `phosphor:copy` + count | bottom right | info |
| backed up / short / unverified / none | `phosphor:shield-check` / `shield-warning` / `seal-question` / `shield-slash` | top right | success / warning / info / danger |
| soft / blown / crushed | `phosphor:drop-half` / `sun` / `moon` | bottom right | warning |

## 4. Open to modules

- **Declared, like commands.** An in-repo crate registers `BadgeDecl`s
  in the desktop's badge registry (as panels declare `PANEL_ICONS` and
  commands their ids), so `helios-declarations` extracts their names
  for translation. An out-of-process plugin declares `[[badges]]` in
  `helios-plugin.toml`, beside `[[commands]]`, with its names in its
  `i18n/` folder. The icon must resolve in egui-pop's registry
  (`phosphor:*` or a built-in name); an unknown icon is refused when
  the manifest is checked.
- **Facts a module owns.** A module may only read facts. It adds its
  own by declaring keys it writes (per-image fields, an extractor's
  output with declared keys, e.g. the image-to-text work's
  "suggestions pending"); the library read carries the declared keys in
  its windowed query, so a badge like "AI suggestions pending"
  (`phosphor:sparkle`) costs no read per cell. A plugin is never called
  per cell.
- **Theming.** Badges take the kit's badge box and tone colours; a
  theme overrides tones, not single badges. Icon size follows the cell
  size band.
- **Overflow.** Each slot fits what its size band allows (proposed:
  one icon below 120 px cells, three above, none below `OVERLAY_MIN`;
  the bands are tuned on snapshots), sorted by
  priority; the rest fold into a "+k" chip whose tooltip and accessible
  name list them. Hovering the cell shows the slot's hover badges.
- **The user chooses.** The grid's view options get a **Badges** menu:
  one check per badge declared, grouped by origin (Helios, each plugin),
  remembered per surface (import, Library). Today's backup badge switch
  becomes the backup badges' entries there (still shown only with a
  strategy, by predicate).

## 5. Accessibility and agents

- Every badge has a name. A cell's accessible name is the photo's name
  followed by the names of its badges shown (as the file browser does
  already, now for the library grid too). A clickable badge is its own
  accesskit button, named. A tooltip gives the name, then the detail.
- Agents read the same facts: `library.badges.list` answers the
  declarations; the per-image reads answer each image's badge ids
  (`badges: ["helios.offline", ...]`), computed by the same predicate
  over the same facts, so an agent sees what the user sees. The import
  window's contents read gives each cell's badge ids. Command names are
  not decided.
- Tests check accessible names through egui-pop's accessibility tree,
  as `badges_of` checks the texts today.

## 6. Slices

Each slice is one small pull request (an egui-pop change ships in an
egui-pop release first), with its tests and snapshots.

1. **"in your Library" as an icon in the import grid.** egui-pop:
   `EntryMarks` badges gain an optional icon, a slot and a name (text
   kept for the accessible name and tooltip). Helios: the badge becomes
   `phosphor:check-circle` top right, the accessible name unchanged.
   Tests: the accessible label still says "in your Library"; a snapshot
   of an import cell with and without it.
2. **The import grid's other marks.** RAW+JPG as a short "R+J" chip,
   offline as an icon; one painter and corner rules shared with the
   library grid. Snapshot of a cell with all three.
3. **Icon badges in the Library grid and filmstrip.** egui-pop `Badge`
   gains icon, name, slot, priority; Helios turns presence, changed
   outside, quick, versions, backup and culling badges into icons; the
   cell's accessible name lists its badges. Tests: `badges_of` keeps
   working on names; accessible names; grid and filmstrip snapshots.
4. **The contract.** `BadgeDecl`, `Facts`, predicates, the registry;
   the built-in badges rewritten as declarations; overflow "+k". Tests:
   predicate and overflow unit tests; `library_scale` read counts
   unchanged; snapshot of an overflowing small cell.
5. **View options.** The Badges menu per surface, remembered; the
   backup badge switch folded in. Tests: toggling hides the badge and
   persists; snapshot of the menu.
6. **Agents.** `library.badges.list` and badge ids in the image and
   import-contents reads. Tests: the answers match what the grid shows.
7. **Modules and connectors.** `[[badges]]` in plugin manifests and the
   connector's import badges, checked at load; an example plugin badge.
   Tests: manifest validation, a plugin badge shown from a declared fact.
8. **Module facts.** Declared fact keys carried by the windowed read
   (the image-to-text extractor's keys first). Tests: `library_scale`
   counts unchanged with a declared key.
9. **Click commands.** Versions opens the versions, offline and backup
   open Storage, changed outside opens its review. Tests: a click runs
   the command with the cell's key.
