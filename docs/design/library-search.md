# Library search: one gallery, one query, saved as virtual folders

Status: **proposed** (2026-11-15). The owner asked for a comparative
study of the kind of search a photo gallery wants, "aesthetic and
compact versus advanced": GitHub-style `key:value`, or a classic, old
school form. Then two additions: a saved search becomes a virtual folder
whose membership is cached like a materialized view, and the search bar
with its grid is one reusable gallery component that every Library-type
panel instantiates. The inventory describes the code on `next` as of
that date.

Related notes: [library.md](library.md) (L4 one query, L6 smart albums,
L12 keyword tree, L13 metadata panel, L16 the Library window),
[thumbnail-badges.md](thumbnail-badges.md) (the badges contract the
gallery draws), [windows.md](windows.md).

## In short

- **Today** the Library panel stacks search, sort, count, view options,
  sources and the current photo's marks in one narrow column, with no
  grouping. Two filter grammars coexist: the search text (egui-pop's
  `Filter` plus Helios's own keys) and the smart album rule tree
  (`helios-vault::rules`); their keys differ (`type:` versus
  `file_type`, `collection:` versus `album`, `rating>=3` versus
  `{rating, >=, 3}`) and the text cannot say ISO, aperture or "not".
- **Recommendation: both, on one query.** A compact single field with
  chips and suggestions (GitHub, Linear, Slack): `key:value` under the
  hood, typed or picked, so a beginner never has to learn the syntax.
  An **Advanced** rule builder with classic rows (Finder smart folders,
  digiKam, Capture One) that edits the **same** query, and saves it as a
  virtual folder. As Jira does with basic and JQL, both views edit one
  model; unlike Jira, the compact view never refuses a query: what it
  cannot draw as chips it shows as one "rules" chip that opens the
  builder.
- **One grammar**: the text is a serialisation of the vault's rule tree,
  parsed and printed by one function in `helios-vault`, shared by the
  search field, smart albums, `library.*` commands and MCP.
- **Saved searches are virtual folders** (the smart albums of L6) in the
  Sources panel. Their membership is **materialized** (ids and count),
  updated incrementally from each vault step's changes, rebuilt only
  when their rules change.
- **One `Gallery` component** (search bar, sort and count, grid, its own
  source, query, sort and selection, the badges contract) instantiated
  by each host: the main window's Library panel, each pane of the
  Library window's commander, a reduced filmstrip.

## 1. Today's zone

### What each control does

From `crates/helios-gui/src/library.rs` (`Library::grid`, `toolbar`,
`facets_bar`, the sections, `marks_bar`), top to bottom:

| Control | What it does | Used | Belongs to |
|---|---|---|---|
| "Import…" button | Starts the workspace's import source (only when it has one) | Rarely | Sources / File menu |
| Search field | egui-pop `SearchField`: words (name, tag), `rating>=N`, `label:`, `flag:`, `tag:`/`#tag`, `date:a..b`, `from:`, `to:`; Helios keys `camera:`, `lens:`, `type:`, `folder:`, `collection:`, `import:`, `presence:`, `cull:`, `backup:`. Criteria become chips on Enter; Backspace in the empty field removes the last chip | Often (browse) | Search bar |
| "+" beside the field | egui-pop's menu adding a rating, a label or a flag criterion. Nothing says so | Rarely (undiscovered) | Search bar, as "Filter" |
| Sort menu and direction arrow | Capture time, import time, file name, rating, last edit, file size, sharpness, content | Sometimes | Search bar (end) |
| Count "438 of 438" | Shown of total | Glanced | Search bar (end) |
| Auto-advance checkbox | After a mark, the cursor moves on (culling) | Per session | View menu |
| Compare button | The selection side by side (C) | Culling | View menu / grid toolbar |
| Facets bar | While a filter is in force: per facet (rating, label, flag, camera, lens, type, presence, backup, culling), each value with its count; a click adds the term | Sometimes | Suggestions, and the expanded bar |
| Folders, Albums, Imports sections | The browse tree (L5, L6, import history); a click scopes the grid with a term (`folder:`, `collection:`, `import:`) | Often | Sources panel |
| Current photo's name, stars, labels, flag, tags field, versions strip | Marks of the cursor's image, applied to the selection | Often (culling) | Metadata panel (L13) and the viewer |
| Empty-library message and import button | — | First run | Grid's empty state |

Recent and pinned filters (`recent_filters.rs`, Ctrl+Shift+K switcher)
and the smart album rule editor (`rule_editor.rs`, a dialog driven by
`library.collection.rules.edit`) exist but are not reachable from the
search zone.

### What moves elsewhere

- **View options** (auto-advance, Compare, thumbnail size, captions,
  badges) go to a **View** menu button at the end of the gallery's bar
  (and the window's View menu); Compare keeps its key.
- **The current photo's marks** go to the metadata panel (L13) and the
  viewer pane of the commander layout; the grid cell keeps its marks as
  badges and its keys (digits, P, X, colour keys) keep working on the
  selection.
- **Sources** (Folders, Albums, Virtual folders, Imports, Keywords from
  L12) go to their own Sources panel beside or above the grid.
- **The facets bar** becomes the suggestions' value lists (with counts)
  and the expanded view's columns; it no longer pushes the grid down.

### Two grammars

| Concept | Search text today | Rule tree today |
|---|---|---|
| Rating | `rating>=3`, `rating:3`, `stars:3` (minimum only) | `rating` with `=`, `!=`, `<`, `<=`, `>`, `>=`, `between` |
| File type | `type:cr3` | `file_type` |
| Album | `collection:<id>` | `album` `in`/`not_in` |
| Capture date | `date:a..b`, `from:`, `to:` (file day, matched in the window) | `captured` with comparisons, `between`, `within` (relative) |
| Exposure | — | `iso`, `aperture`, `shutter`, `focal_length` |
| Text fields | words (name, tag) | `title`, `description` `contains` |
| Negation | — | `none` groups, `is_not`, `lacks`, `not_in` |
| Presence, backup, culling | `presence:`, `backup:`, `cull:` | — (`focus`, `clip_*` numbers only) |

The fix is one grammar (section 4).

## 2. What established applications do

| Application | Search model | Good at | Costs |
|---|---|---|---|
| GitHub issues | One field, `key:value` qualifiers, AND/OR and parentheses; suggests qualifiers and values as you type, warns on errors; a key focuses it | Expert speed, compact, shareable as text, agents read and write it | The raw text is intimidating; chips are absent (text only) |
| GitLab issues | Filtered search: each `key: value` becomes a token (a chip) picked from dropdowns; Ctrl+Backspace removes a token | Discoverable (pick, not type), removable chips, compact | Dropdown steps feel slow to experts; no free grouping |
| Gmail | Operators (`from:`, `has:attachment`, `before:`) typed in the field, plus an advanced form (the field's dropdown) that writes the same operators | Form and text are one query: the form teaches the syntax | The form is a separate panel; few users discover operators |
| Slack | `in:`, `from:`, `-in:` modifiers in the field; filters after searching | Negation with `-`, filters for beginners, modifiers for experts | Two places to filter |
| Linear | Filter bar: F opens a menu, type a key then a value; filters show as chips whose operator is changed with the mouse; save as a view | Keyboard first and still visual; saved views | No free text grammar to paste |
| Jira | Basic (dropdowns) and JQL (text) over one query; a JQL too complex cannot switch back to basic | Beginners and experts, one model | The "cannot convert" wall frustrates users |
| Lightroom Classic | Library Filter bar above the grid: Text / Attribute / Metadata / None tabs; Metadata is columns of values with counts; presets; `\` toggles it | Attribute row is ideal for culling (stars, flags, labels in one row); columns show counts | Takes height; Text, Attribute and Metadata are three UIs; no OR across columns |
| Lightroom (cloud) | One search field (keywords, titles, captions, file names, AI content) plus a filter bar (rating, keywords, camera, location) | Clean, aesthetic, AI content search | No saved searches as smart albums; inconsistent AND/OR, as users report |
| Capture One | Filters tool (categories with counts) in the Library tab; Advanced Search (rows of criteria) opens from the search field; saves as a smart album, album or search preset | The advanced rows save straight to a smart album | Filters can hide images silently; two tools |
| darktable | Collections module (rules on attributes, combined, history of recent collections) and collection filters (some pinned to the top bar, rating range widget) | Powerful, every attribute; history | Confusing: collections and filters overlap, users mix them up |
| Photo Mechanic Plus | A text query language (`city "paris" and (rating = 2 or rating > 4)`), case control, examples menu, filter tool | Very fast for experts; full boolean | No guidance beyond the examples menu |
| digiKam | Search sidebar (quick text) and an Advanced Search form (field rows, groups, Try and OK); recent versions turn a plain-language request into an advanced search | Exhaustive fields, saved searches; natural language as a front end to the form | Heavy dialog, many steps |
| Apple Photos / Google Photos | Natural language search with suggestion chips (people, places, dates, things) | Most aesthetic and approachable | Opaque: no exact criteria, not reproducible as a rule |
| Excire | AI keywords and search by example photo, results as a collection | Content search without keywording | Content search only, separate tool |
| Finder smart folders | Rule rows (attribute, operator, value), "+" adds a row, Option-+ a nested group with any/all/none; saved as a smart folder | The classic form everyone understands; saved as a virtual folder | Tall, slow to edit, mouse heavy |
| Windows Explorer | Search Tools ribbon (date, kind, size) writes Advanced Query Syntax (`kind:picture datemodified:lastweek`) into the field | Picking fills the text: the form teaches the syntax | Syntax is undocumented in the UI |

### Lessons

1. **The winners put one query behind two faces** (Gmail, Jira,
   Windows, Capture One): a quick face for most uses, a rule form for
   the rest, both writing the same query.
2. **Chips plus suggestions** (GitLab, Linear, Google Photos) make
   `key:value` discoverable and good-looking; the syntax is a power-user
   shortcut, never a requirement.
3. **Counts beside values** (Lightroom's Metadata columns, Capture One,
   our facets) are what photographers expect when narrowing.
4. **A saved search is a folder** (Finder, Capture One, digiKam,
   Lightroom smart collections). Lightroom cloud's lack of it is a
   standing user complaint.
5. **Do not split one concept over two tools** (darktable's collections
   versus filters, Capture One's global filters): users lose track of
   what hides their photos.
6. **Natural language** (digiKam, Apple, Google) is a front end that
   writes rules; it fits later as an agent writing the same grammar.

## 3. Recommendation

**A compact search bar with chips and suggestions, and an Advanced rule
builder, both editing one rule tree, saved as virtual folders.** The
old-school form alone is too tall for a gallery pane and slow for
culling; the `key:value` text alone hides the vocabulary from
beginners. Both, on one model, cost one parser and one printer more
than either, and every app studied that does it is the one users
praise.

### The compact bar (default)

```
┌ Gallery ─────────────────────────────────────────────────────────────┐
│ 🔍 [★≥3 ×] [camera: Z 8 ×] [#beach ×] sunset▏  Filter ▾  Capture ▾ ↓ 438/9 120  View ▾ │
└──────────────────────────────────────────────────────────────────────┘
```

- One field; the **Filter ▾** button (a word, replacing the anonymous
  "+") opens the key list; typing a key also opens it.
- Chips: each criterion one chip, drawn in words ("★ 3 or more",
  "camera: Z 8", "not rejected"); a click on a chip edits its value in
  place (a dropdown of values with counts); its × removes it.
- Free words stay text (name, title, tags, keywords).
- At the end: sort menu and direction, the count, the View menu.
- Narrow (a commander pane under ~420 px): chips past the first two
  fold into "+3", the sort and View menus fold into one "⋯" menu, the
  count stays.

### Suggestions while typing

```
 🔍 cam▏
 ┌──────────────────────────────┐
 │ Keys                         │
 │  camera   Camera model       │
 │  captured Capture date       │
 │ Values                       │
 │  camera: Z 8          4 210  │
 │  camera: Z 50         1 980  │
 │  camera: X-T5           640  │
 │ Recent                       │
 │  camera:"Z 8" ★≥4            │
 └──────────────────────────────┘
```

- Empty field focused: recent and pinned filters, virtual folders, then
  the top facets (rating, flag, label) with counts.
- After `key:`: the key's values with counts (the vault's facets),
  operators for numbers and dates (`≥ 3`, `between`, `last 30 days`).
- A word matching a value of a key ("Z 8", "beach") offers the
  qualified chip (`camera: Z 8`, `#beach`) above "search the words".

### The expanded view: the Advanced rule builder

The "Advanced" entry of the filter menu (or Ctrl+Shift+F) opens the
rule rows under the bar, in the pane, not a dialog:

```
 🔍 [★≥3 ×] [camera: Z 8 ×] sunset       Filter ▾  Capture ▾ ↓ 438/9 120
 ┌ Match [all ▾] of the following ───────────────────────────────────┐
 │ [Rating      ▾] [is at least ▾] [★★★☆☆]                     − + │
 │ [Camera      ▾] [is          ▾] [Z 8               ▾]       − + │
 │ [Any of ▾] ┐                                                 − + │
 │   [Label   ▾] [has ▾] [● red]                                − + │
 │   [Keyword ▾] [has ▾] [beach]                                − + │
 │ [Text        ▾] [contains    ▾] [sunset]                     − + │
 └─────────────────────────────── [Clear]  [Save as virtual folder…] ┘
```

- Rows are the existing rule editor (`rule_editor.rs`) drawn inline;
  the bar's chips and the rows are two views of one tree: edit either,
  the other follows on the same frame.
- A tree the chips cannot draw flat (a nested "any" group across keys,
  a "none" group) shows as one chip, "rules (3)", that opens the
  builder. Nothing is ever refused, unlike Jira.
- "Save as virtual folder…" asks a name and makes a smart album with
  the tree; the gallery's source becomes it.
- Quick attribute row (Lightroom's Attribute bar) for culling: an
  optional second line of toggles (stars, flags, labels, edited), a View
  menu option, off by default; each toggle is a chip.

### Mockups: before and after

Before (main window's Library panel, narrow):

```
[Search images        ][+]
Capture time ▾ ↓   438 of 438
[ ] Auto-advance  [Compare]
▸ Folders
▸ Albums
▸ Imports
DSC_0412.NEF ★★★☆☆ ●●●●● ⚑ ✕
Tags [            ]
[grid]
```

After (same panel; Sources collapsed to a header list):

```
Sources ▾ All photos
🔍 [★≥3 ×] Search…  Filter ▾  ⋯  438/438
[grid]
```

After (Library window, commander default):

```
┌ Sources ──────┬ Gallery ───────────────────────────────┬ Viewer ─────────┐
│ All photos    │ 🔍 [★≥3 ×] [#beach ×] Filter▾ Capture▾↓│                 │
│ ▸ Folders     │                         438/9 120 View▾│   (loupe)       │
│ ▾ Virtual     │ ┌───┬───┬───┬───┬───┐                  │                 │
│   ⚙ Best 2025 │ │   │   │   │   │   │                  ├ Metadata ───────┤
│   ⚙ To back up│ └───┴───┴───┴───┴───┘                  │ ★★★☆☆ ● ⚑      │
│ ▸ Albums      │                                        │ Keywords […]    │
│ ▸ Imports     │                                        │ EXIF …          │
│ ▸ Keywords    │                                        │                 │
└───────────────┴────────────────────────────────────────┴─────────────────┘
```

## 4. The grammar

One grammar, defined in `helios-vault::rules`: `rules::parse_text(&str)
-> Result<Rule, ParseError>` and `rules::to_text(&Rule) -> String`
(round trip for every tree; nested groups print with parentheses). The
search field, smart albums, workspace queries (`QueryDesc`),
`library.filter.apply`, `library.collection.rules` and MCP all go
through it. egui-pop's `Filter` keeps its generic keys for other apps;
Helios parses its whole text with the vault grammar and gives egui-pop
chips to draw (the chip contract: a title, a value menu, a remove).

### Terms

```
query     = term ( ws term )*            all terms hold (AND)
term      = [ "-" ] ( qualified | word | "(" query ( "OR" query )* ")" )
qualified = key ":" [ op ] value
op        = ">=" | "<=" | ">" | "<" | "!="       (numbers, dates)
value     = bare | "\"" quoted "\"" | low ".." high
word      = text searched in name, title, description, tags, keywords
```

- The same key repeated means any of them (`label:red label:blue`);
  different keys all hold. `-` negates (`-flag:reject`, Slack's habit).
- `OR` and parentheses for the rare cross-key "any"; the builder is the
  friendly way to them.
- `#beach` is `tag:beach` (kept from egui-pop); `★3` is not typed, it
  is how a chip draws `rating:>=3`.

### Keys (rule field, kind, operators)

| Key (aliases) | Rule field | Values |
|---|---|---|
| `rating` (`stars`) | rating, number | `3`, `>=3`, `1..3`; `rating:3` keeps today's meaning, at least 3 |
| `label` | label, set | red, yellow, green, blue, purple |
| `flag` | flag | pick, reject, none |
| `tag` (`#`), `keyword` (L12) | tag, set | names; keyword paths `a\|b` |
| `camera`, `make`, `lens` | text | exact by default, `~Z` contains |
| `type` (`file_type`) | text | cr3, nef, jpg… |
| `folder` | folder | path, a copy there or below |
| `album` (`collection`) | album | id or name |
| `import`, `imported` | import, imported | session id; time like `captured` |
| `captured` (`date`; `from:`/`to:` kept) | date | `2024-06-01`, `2024-06..2024-08`, `>2024`, `last:30d`, `today`, `this-year` |
| `iso`, `aperture` (`f`), `shutter`, `focal` | number | `iso:<=800`, `f:1.4..2.8`, `shutter:<1/250` |
| `title`, `description` | text | contains |
| `presence` | presence (new rule field) | present, offline, missing |
| `backup` | backup (new rule field) | met, short, unverified, none |
| `cull` | over `focus`, `clip_*` | sharp, soft, clipped, unmeasured |
| `edited`, `has` | (L4 checklist) | `has:versions`, `has:gps`, `edited:yes` |

An unknown key or a bad value stays a word, as today, and the chip
shows the vault's reason in its tooltip (GitHub warns the same way).
The keys and labels come from one table, `RULE_FIELDS`, extended with
aliases and translated titles, so the builder's field menu, the
suggestions and the MCP description list the same keys.

### Agents

`library.filter.apply {query: "rating:>=3 camera:\"Z 8\""}` and
`library.collection.create {kind: "smart", query: "…"}` take the text;
`rules` (the JSON tree) stays accepted. Answers give both `query` and
`rules`. A gallery pane is targeted by `pane` (section 6).

## 5. Keyboard

| Key | Does | Note |
|---|---|---|
| Ctrl+F | Focus the active gallery's search field | `shortcuts-research.md` keeps Ctrl+F as find; the darktable profile maps it to the filmstrip, so there it is `/` only |
| `/` | Same, from the grid | GitHub, Linear; free in the grid keymap (to check against every profile) |
| Ctrl+Shift+F | Toggle the Advanced rows | |
| Ctrl+Shift+K | Recent and pinned filters switcher (exists) | |
| Esc | In the field: close suggestions; again: clear the text; again: back to the grid. In the grid with a filter: nothing (Esc stays deselect) | Esc never clears chips silently |
| Up, Down, Enter, Tab | Move in suggestions, take one, complete the key | |
| Backspace in an empty field | Select the last chip; again removes it | GitLab, egui-pop today |
| Ctrl+Backspace | Remove the last chip | GitLab |
| Left, Right at the field's start | Move between chips; Enter edits one, Delete removes it | |
| `\` (Lightroom profile) | Show or hide the bar | Lightroom's key |

## 6. One gallery component

Today one `Library` struct holds both the shared data (entries, index,
albums, folder counts, thumbnails) and one view's state (search,
filter, order, sort, selection, facets). The main window's Library panel
and the Library window (L16 part 1) draw the same struct: one view in
two windows. The commander layout of the Library window (each gallery pane has its
own source, filter, sort and selection) needs several views.

### The split

- **`LibraryData`** (one per library): entries, index, albums and
  virtual folders with their materialized counts, folder and keyword
  trees, thumbnails cache, backups, known tags. Read by every gallery.
- **`Gallery`** (one per view): its source (all photos, folder, album,
  virtual folder, import, keyword), its rule tree, sort, the order it
  shows, its selection and cursor, its facets, its search bar state,
  its view options (thumbnail size, captions, badges, auto-advance),
  its id (`main`, `pane-1`, `pane-2`, `filmstrip`).
- **`GalleryOptions`** (given by the host, so one component serves a
  more or less complete search depending on where it is hosted; the
  user may change some per host in the View menu, kept per gallery):
  - `search`: `none` (no bar; the gallery follows its link or source),
    `compact` (one field with chips and suggestions, no Advanced rows),
    `full` (chips, suggestions, Advanced rows, Save as virtual folder).
  - `sources`: which sources it offers (`all`, or a subset of folders,
    albums, virtual folders, imports, keywords) and where (`none`,
    `header` menu, `panel` beside the grid).
  - `sort` and `count`: shown, folded into the "⋯" menu, or hidden.
  - `view_menu`: shown or not (thumbnail size, captions, auto-advance,
    Compare).
  - `badges`: the badge set drawn on cells (thumbnail-badges.md: `all`,
    `marks` only, `none`).
  - `selection`: `multi` (rubber band, Ctrl and Shift clicks), `single`
    (cursor only), `follow` (mirrors the linked gallery's).
  - `layout`: `grid` or `strip` (one row, filmstrip).
  - `link`: none, or another gallery whose query and selection it
    shares.
- **`Gallery::show(ui, &LibraryData, &GalleryOptions)`** draws bar, rows
  and grid; marks, Compare and commands act on the **active** gallery's
  selection (the shell keeps which one is active, focus-follows-click).
- The badges contract (thumbnail-badges.md) is called by the gallery's
  grid, so every host draws the same badges.

### Hosts

Defaults per host:

| Option | Main window, Library panel (`main`) | Library window, commander pane (`pane-N`) | Filmstrip (`filmstrip`) |
|---|---|---|---|
| `search` | `compact` (Advanced reachable from Filter ▾, which opens the Library window's full bar) | `full` | `none` |
| `sources` | `all`, as a `header` menu | `all`, as a `panel` | `none` |
| `sort`, `count` | shown; folded into "⋯" when narrow | shown | count only (tooltip) |
| `view_menu` | shown | shown | hidden |
| `badges` | `all` | `all` | `marks` |
| `selection` | `multi` | `multi` | `follow` |
| `layout` | `grid` | `grid` | `strip` |
| `link` | none | `pane-1`: `main`; others: none | `main` |

### Converging the two code paths

1. Extract `Gallery` from `Library` without changing behaviour: one
   gallery, `main`, drawn by both windows (today's shared selection,
   decided 2026-11-14, kept).
2. The Library window's first pane is `main` (linked), so a user who
   opens the window sees the same photos; "Add gallery" makes `pane-2`
   with its own state. Commands carry `pane` (default: the active one).
3. The filmstrip becomes a reduced gallery linked to `main`.

## 7. Virtual folders: saved searches, materialized

### Today

A smart album is a vault document `{kind: "smart", name, rules}`.
`Vault::albums` runs one full `store.select` per smart album and set on
every albums read (`collections.rs`) and keeps only the length as
`count`; opening one runs its query again (`Query::album`). With tens of
smart albums on a 100,000-photo library, each Library refresh runs tens
of full queries.

### Proposed: a materialized set per virtual folder

- **Storage** (SQLite store; the in-memory store keeps a map):
  `smart_members(album_id, image_rowid)` with a primary key on both, and
  `smart_state(album_id, rules_hash, count, step, day)`: the hash of the
  rules it was built from, its count, the vault step it is current
  with, the day its relative dates were resolved for.
- **Count** comes from `smart_state.count`: the Sources panel shows
  every count without a query. Opening a virtual folder joins on
  `smart_members` (indexed), then applies the gallery's own extra
  chips, sort and paging as any query.
- **Full build** when the folder is made, when its rules change
  (`rules_hash` differs), at a schema change, or when `step` is behind
  by more than the journal keeps.
- **Incremental update**: each vault step already lists the images it
  touched with their rows before and after (`undo::Changes`). After a
  step commits, for each virtual folder whose **fields** intersect the
  step's changed fields (a rating change touches folders testing
  `rating`; an import touches `import`, `imported` and every folder,
  since new photos enter the Library), the store re-evaluates the rules
  on the touched ids only (`… WHERE id IN (touched) AND <rules>`),
  inserts and deletes members, adjusts the count, records the step. A
  folder whose fields are untouched is not looked at.
- **Field dependencies** come from the tree: `Rule::fields()` lists the
  fields read; `album` conditions also depend on that album's members;
  `import` on the import history documents; `folder` on copies (file
  moves, L15); `presence` and `backup` on volumes and copies, so a disk
  mounted or a backup verified updates folders that test them.
- **Relative dates** (`within last 30 days`, `today`): `day` records the
  resolution day; the first read on a later day rebuilds those folders
  only (cheap: one query each, once a day).
- **Size**: a member row is two integers; 100,000 members is about
  2 MB with its index. No cap: the worst case (every folder matching
  every photo) is folders × photos rows, bounded by the user's own
  folders. A step touching more than a threshold of images (say 5,000,
  a large import or a batch) rebuilds the affected folders instead of
  testing ids in batches.
- **Consistency**: the update runs in the step's transaction, so undo
  and redo update members too (undo is a step). The health check gains
  "virtual folders up to date" (a rebuild compares the count).
- **Ad-hoc searches** (the bar, not saved) are not materialized: they
  run as today's L4 query, and their facets come with it.

### Migration

Existing smart albums keep their document; the first `albums` read
after the upgrade finds no `smart_state` row and builds each one (one
query each, once). Their rules print as text through `to_text`, so the
editor and the bar open them unchanged. "Smart album" is shown as
"Virtual folder" in the UI (open question 1); the commands keep their
names.

## 8. Accessibility

- The field is a labelled text box ("Search photos"), each chip a
  button whose accessible name says it in words ("Rating at least 3,
  remove"), the suggestions a list box with `aria-activedescendant`
  semantics (accesskit `ListBox` and `ListBoxOption`), the count a live
  status ("438 of 9,120 photos").
- Builder rows: each combo labelled by its column ("Field", "Condition",
  "Value"), the group's match mode a labelled combo, +/− buttons named
  "Add a condition after Camera", "Remove Camera".
- **Every icon-only control has a tooltip and an accessible name**,
  in words: the search icon ("Search photos"), each chip's × ("Remove
  rating at least 3"), the sort direction arrow ("Newest first" /
  "Oldest first"), "⋯" ("More: sort and view"), the Advanced rows' + and
  − ("Add a condition", "Remove this condition"), the quick attribute
  toggles ("Show picked photos"). Where room allows, a word beats an
  icon (Filter ▾, View ▾). A test walks the gallery's accesskit tree and
  fails on any button without a name.
- Targets at least 24 px; chips never rely on colour alone (labels show
  their name).
- Everything reachable by keyboard (section 5); tests drive the bar
  through accesskit, as the Storage page's are.

## 9. Open questions

1. Name: "Virtual folders", "Smart albums" (Lightroom, Capture One,
   today's code) or "Saved searches" (digiKam). Default: **Smart
   albums** in the UI (what photographers know), "virtual folder" in
   the docs' explanation.
2. Does the main window's panel and the Library window's first pane
   stay linked, or does each window get its own gallery? *Decided
   2026-11-15 (owner): apart.* Each Library view keeps its own search,
   filters, restriction and selection; changing one never changes
   another, and each is kept with its own view in the workspace.
3. The quick attribute row (Lightroom's Attribute bar): off by default
   or on in the Lightroom profile? Default: on in the Lightroom
   profile, off otherwise.
4. Words: do they search keywords and EXIF text too (Lightroom's "Any
   searchable field") or names and tags only? Default: name, title,
   description, tags, keywords.
5. Natural language ("sharp photos of the dog last summer") as an agent
   writing the grammar, later: out of scope here.

## 10. Slices

Each slice is one PR with its tests; the order matters.

1. **G1. The `Gallery` component, behaviour unchanged.** Split
   `Library` into `LibraryData` and `Gallery` (+ `GalleryOptions`); the
   main window's panel and the Library window both draw the `main`
   gallery, each host giving its `GalleryOptions` (section 6 defaults).
   Tests: options select what is drawn (a gallery with `search: none`
   has no field, `sources: none` no sources); the existing library tests pass untouched; a test
   that two hosts drawing `main` share selection and filter; snapshot
   of the panel unchanged.

   *Landed (2026-11-15), with the views apart (question 2):*
   `library::Gallery` holds the view state (search, filter,
   restriction, order, selection, facets) inside `Library`, which keeps
   the shared data and one gallery per host by id (`main` for the main
   window's panel, `window` for the Library window): `Library::show`
   puts the host's gallery in force for its drawing, and the gallery of
   the window with the input is the one commands act on. Each gallery's
   vault query runs and is answered on its own. The sort stays one for
   every gallery (the order the entries are read in) until the vault
   sorts per gallery. The workspace keeps the main panel's query,
   folder and collection at the top of `library`, where older
   workspaces have them (they load into the main panel only), and the
   other views' under `library.views.<id>`; `GalleryOptions` (`search`,
   `sources`, `sort_count`, `view_options`, `marks`) with
   `MAIN_PANEL`, `LIBRARY_WINDOW` and `BARE`, each host drawing
   `Library::show(ui, &options)`. The filter keys are declared
   (`library::SearchKeys`, built-in and module keys alike: label,
   values, facet, term); the facets bar lists them. It is a typed
   registry until the hook kernel of [hooks.md](hooks.md) exists, then
   its declarations are collected through `library.search.keys`.
   Left for later slices (parked): `Library` renamed `LibraryData`; a
   sort per gallery; the filmstrip's own gallery; the grid moved onto the import window's
   browser core (view modes, thumbnail size), egui-pop's `ThumbGrid`
   and file browser meeting there.
2. **G2. Hosts switch, the zone is regrouped.** The gallery's bar
   (field, filter button, sort, count, View menu); auto-advance and
   Compare into the View menu; sources into a Sources header (main) or
   panel (window); the marks row out (to L13's metadata panel, kept in
   a "Marks" section of the Library window until L13 lands). Tests:
   snapshots compact and narrow; accesskit clicks on sort, View menu
   and Compare; every button of the gallery has an accessible name and
   a tooltip (a test walking the tree); marks still applied by keys to the selection.

   *Replaced by the owner's design (2026-11-17), landed:* a gallery is
   an **explorer** of two linked parts. Its **Filters** (the search, the
   restrictions, the facets, the sources; the current photo's marks
   until L13) hold no photos; its **Results** show them, as thumbnails
   at a size or as a list (egui-pop's `ThumbGrid` and `ListView`),
   chosen in a "View as" menu as in a file browser's dialog, with the
   sort and count, an auto-advance toggle (the fast-forward icon,
   accessible name and pressed state), Compare and Import in its bar.
   The main window draws them as two panels, `helios.filters` and
   `helios.vault` (`library::EXPLORER`), linked by the gallery id both
   draw; the default layouts keep both in view (the Library place puts
   the Filters at the start side, the darkroom splits the start side,
   Filters above). The Library window draws its gallery's two parts side
   by side. Each view has its own auto-advance and its own results'
   mode and size, kept with it in the workspace (`library.auto_advance`
   and `library.results` for the main view, `library.views.<id>` for
   the others; an older workspace's auto-advance goes to every view).
   `vault.autoAdvance` acts on the view the user works in. Later: a
   details mode (columns, egui-pop's `DataTable`), the filters laid out
   in a bar above the results.
   **The Filters as a bar** (#5078 A2, 2026-11-21; the owner chose the
   panel's shape over an option per panel): a Filters panel docked at
   least twice as wide as tall (above the Results, across the window)
   draws the search, the restrictions and the facets on one line, the
   sources (egui-pop's places panel, as in the column) behind a
   "Sources" drop-down at its start that stays open until a click
   outside it; the current photo's marks stay under the line until L13.
   Docked at a side it stays a column, a short window included.
   **Several views** (2026-11-21): the Results panel opens as many
   times as asked (egui-pop panel instances, `helios.vault#<n>`; View ›
   New Library, `library.view.open` / `library.view.close`). Each new
   one is a view of its own: its gallery is named after it, its Filters
   drawn beside its photos, and its filter, cursor and results' mode
   are its instance's payload, kept in the layout (so with the
   workspace's layout copy and at quit) and dropped when it closes
   (`helios-gui/src/library_views.rs`). The main explorer stays as it
   is. Keys and undo routing scoped to `helios.vault` do not reach an
   instance yet (quazardous/egui-pop#647).
   **Quick-collection panels** (2026-11-21): a view may show a quick
   collection (`library.quick.open`, a quick collection's "Open in a
   panel"): its state names the collection (`quick`) and its filter is
   `collection:<id>`; its grid has no search, and the whole panel takes
   dropped photos. Photos dragged out of one quick collection onto
   another move (`library.quick.move`, one undo step); Ctrl copies;
   from anywhere else they are added. The panel's tab keeps the Results
   instance's title: egui-pop has no way yet to name an instance.
3. **G3. One grammar.** `rules::parse_text` and `rules::to_text` in
   `helios-vault`, aliases, `-`, `OR`, ranges, relative dates;
   `presence`, `backup`, `cull` as rule fields; the search text, the
   workspace query and `library.filter.apply` go through it; egui-pop's
   own keys stay for other apps. Tests: round trip for every field and
   operator (property test on generated trees); today's texts parse to
   the same results (a table of the current terms); the window's
   results equal the vault's on a fixture.
4. **G4. Chips and suggestions.** Chips drawn in words with a value
   menu (counts from the facets), the filter button's key list,
   suggestions (keys, values with counts, recent, pinned, virtual
   folders), the keyboard of section 5; the facets bar goes. Tests:
   kittest typing `cam`, Down, Enter gives the `camera:` chip;
   Backspace and Ctrl+Backspace; Esc sequence; accesskit names;
   snapshots of the open suggestions.
5. **G5. The Advanced rows.** The rule editor drawn inline under the
   bar, synchronised with the chips; the "rules (n)" chip for trees the
   chips cannot draw; Ctrl+Shift+F. Tests: an edit in a row updates the
   chips and the reverse; a nested any-group shows one chip and
   survives a round trip; agent edits via
   `library.collection.rules.edit` on the inline builder.
6. **G6. Save as virtual folder, materialized.** "Save as virtual
   folder…" from bar and rows; `smart_members` and `smart_state`;
   full build, incremental update from `Changes` by field dependency,
   relative-date daily rebuild, threshold rebuild, undo; counts from
   state; migration of existing smart albums. Tests: vault tests per
   field kind (a mark, an import, a file move, a volume event update
   only the dependent folders, verified against a full query); undo
   restores members; 100,000-photo fixture: opening and counting do no
   full query (query counter), an import of 1,000 updates in bounded
   time (box bench job).
7. **G7. Pane-aware commands and the commander.** `pane` on
   `library.filter.*`, `library.show` and the collection commands; the
   active gallery; "Add gallery" (feeds the commander work). Tests: two
   galleries with their own query, sort and selection; a command
   targeted at `pane-2`; the active pane switching on Tab.
8. **G8. Filmstrip as a reduced gallery** and the optional attribute
   row. Tests: filmstrip follows `main`'s query and selection; the
   attribute row's toggles are chips.

## Sources

- GitHub, filtering and searching issues and pull requests:
  <https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/filtering-and-searching-issues-and-pull-requests>
  ("As you type your filter, GitHub will show available qualifiers,
  suggest values, and warn when there is a problem with your filter.")
- GitLab search and filtered search (archived docs):
  <https://archives.docs.gitlab.com/16.0/ee/user/search>
- Gmail search operators (third-party summary):
  <https://www.ionos.com/digitalguide/e-mail/technical-matters/gmail-search-operators>
- Slack, search modifiers: <https://slack.com/help/articles/202528808>
- Linear, filters: <https://linear.app/docs/filters>
- Jira, advanced search and switching from basic:
  <https://support.atlassian.com/jira-software-cloud/docs/what-is-advanced-search-in-jira-cloud/>
  ("a complex query created using advanced JQL criteria may not be
  translated to basic search")
- Lightroom Classic, find photos in the catalog (Library Filter bar):
  <https://helpx.adobe.com/lightroom-classic/desktop/organize-photos-in-lightroom-classic/finding-photos-catalog.html>
- Lightroom (cloud), search by content, ratings and flags:
  <https://helpx.adobe.com/lt/lightroom-cc/how-to/search-find-photos-ratings-flags-lightroom-cc.html>;
  user requests for boolean search and smart albums:
  <https://community.adobe.com/t5/lightroom-ecosystem-cloud-based-ideas/p-please-add-smart-albums/idi-p/12240055/page/2>
- Capture One, Filters tool, Advanced Search, saving results:
  <https://support.captureone.com/hc/en-us/articles/360002526358>,
  <https://support.captureone.com/hc/en-us/articles/360010482817>,
  <https://support.captureone.com/hc/en-us/articles/360002526658>;
  hidden global filters: <https://imagealchemist.net/filter-and-search-in-capture-one/>
- darktable, collections and collection filters:
  <https://docs.darktable.org/usermanual/4.6/en/module-reference/utility-modules/shared/collections>,
  <https://docs.darktable.org/usermanual/5.2/en/module-reference/utility-modules/shared/collection-filters/>;
  users confusing the controls:
  <https://discuss.pixls.us/t/confusable-controls-in-lighttable-ui-can-it-be-improved/54739>
- Photo Mechanic Plus search:
  <https://docs.camerabits.com/support/solutions/articles/48001077445>
- digiKam search view and advanced search:
  <https://docs.digikam.org/en/left_sidebar/search_view.html>;
  natural language to advanced search:
  <https://www.digikam.org/news/2026-08-20-advanced_search_improvements_with_llm/>
- Google Photos, Ask Photos and suggestion chips:
  <https://support.google.com/photos/answer/15318661>;
  Apple Photos suggested searches:
  <https://jarango.com/2019/09/05/suggested-searches-in-apple-photos/>
- Excire, search by example: <https://excire.com/en/tutorials/excire-search/beispielsuche/>
- macOS Finder smart folders: <https://support.apple.com/guide/mac-help/mchlp2804/mac>;
  nested any/all/none groups:
  <https://www.intego.com/mac-security-blog/sort-smarter-with-macos-smart-folders-albums-and-playlists/>
- Windows Explorer search filters writing query text:
  <https://www.tenforums.com/tutorials/94452-search-file-explorer-windows-10-a-post1719454.html>
