# Workshops made on the fly: designed by the AI, refined by hand, run within limits

Status: **decided** (2026-11-11): the owner accepted every
recommendation of section 9 (C1 to C8). A design note: slices 1 to 5
are built (semantic sliders, zone measures, the draft and its check,
`workshop.design` through the user's own agent over MCP, the session
record), the rest is not. It extends [workshops.md](workshops.md) (workshops v1,
built, experimental) and uses decision nodes and AI junctions
([module-contract.md](../module-contract.md)), macros
([macros.md](macros.md)), the undo design ([undo.md](undo.md)), the
workflow level ([workflow-level.md](../workflow-level.md)) and the import
lifecycle ([import-connectors.md](import-connectors.md), section 7).
The choices, now decided, are at the end.

## In short

The owner's idea: the user pilots a workshop on one photo, half by hand
and half with the AI's help; the process then becomes automatable with
a minimum of AI, and runs on a batch.

The **running example** of this note, in the owner's words: *"all my
photos are backlit, with the mountain and the sky too contrasted."*

The pipeline, in the order it is built:

1. **Design** (the near goal). From the intent, the AI **creates the
   workshop**: the real modules chosen and pre-calibrated
   (tone equalizer, a sky mask, local contrast), only the useful
   sliders with narrowed ranges, and composite semantic sliders
   ("Backlight lift", "Sky", "Mountain detail"). The user works the photo
   with that tailored workshop. Valuable on its own, with no automation.
2. **Refine and record.** On a test case (one photo, or two), the user
   refines the workshop half by hand, half with AI proposals. Each step
   is recorded: who made it, what it aimed at, the measures at that
   moment.
3. **Generalise** ("Make it automatic"). Manual values become relative
   targets where a measure explains them, fixed values otherwise; AI
   proposals become deterministic rules where one fits; an AI junction
   stays only where a per-photo judgment is truly needed. The user
   confirms each row. The user also sets the **limits** the AI and the
   solver must stay within.
4. **Run within limits** (the final goal). The workshop runs on a set
   (a selection, a Library query, a series, an import) as a node job:
   deterministic aiming, AI only at the remaining junctions, answers
   recorded in each edit. Whatever breaks a limit is flagged; a
   contact-sheet review fixes those photos one by one. Applying is one
   undo step.

**Minimum AI**, measurably: the number of junctions left in the
workshop (`J`), and the AI calls per photo in a batch (`A`). The
generaliser reports both before the run, the run reports the real
counts after. The goal for the example is `J = 0` (the sky found by a
parametric mask) or `J = 1` (the sky found by an AI object member on
photos where the parametric mask fails), so `A` is 0 for most photos.

## What exists today

Read from `next` on 2026-11-11.

| Piece | Where | What this note takes from it |
|---|---|---|
| Workshops v1 | `helios_template::workshop`; `workshop.apply`, `aim`, `save`, `suggest` | the workshop block (intent, keywords, targets), aiming by bisection on a proxy, "tested at the extremes", the Workshops panel behind `interface.experimental.workshops` |
| Measures | `helios-scopes` | `luma.median`, `luma.high`, `clip.high` on the whole image, nothing per zone |
| Macros | `helios_template::macros`, `macro.*` verbs | the exposed-parameter interface (labels, narrowed and soft ranges, groups), versions, the user library |
| Decision nodes | `helios-decision` (the book), `decision_*` control verbs | three modes (full AI, AI-assisted, human); the answer recorded in the edit with who decided, the model and the inputs' content keys; replayed offline, never recomputed |
| AI object mask | `mask_object` (a decision module), `helios-resolve` | `kind: sky / subject / person / prompt`, a mask with a confidence, answered by a configured resolver; none configured by default |
| Parametric mask | `mask_parametric` | a deterministic mask on luminance, RGB and JzCzhz channels: the deterministic candidate for "the sky" |
| Value store history | `helios-values::Store` | `Step { label, origin, changes: [(path, old, new)], place, group }`; agents' requests are groups attributed to the agent; session only |
| The image's edit | `helios-vault` versions | one content-addressed version per committed step, with its origin (`user`, `agent:<id>`); durable |
| Audit trail | `helios_vault` catalog, `AuditEntry` | library fields only (rating, tags…): at, who, job, image, field, old, new; nothing about edits |
| AI service | `helios-ai` | providers (local Ollama by default, remote only when added, keys in the keyring or the environment, never in a file), versioned prompts with schemas, a fake provider for tests, every call logged in `calls.jsonl` |
| Workflow level | `helios-journal`, `workflow.submit` and the `workflow_*` verbs | a batch as journal rows, leases, retries, cancel, progress, follow-up in the jobs centre |
| Import lifecycle | import-connectors.md, section 7 (designed) | `batch.done` and `session.ended` schedule follow-up jobs; participants answer with data |
| MCP | `crates/helios-node/src/mcp.rs` | `helios_workshop_apply`, `_aim`, `_save`, `_suggest`; `helios_describe`; `helios_decisions`, `helios_decision_*`; `helios_history_*`; `helios_workflow_*`; `helios_ai_*` |

**What is missing**, each addressed below: building a workshop from an
intent; semantic sliders; measures inside a zone; a record of a
session that says *why* each value was set and what the image measured;
generalising that record; limits; running a workshop on a set; a review
of the run.

## 1. Design: the AI creates the workshop

This is the first deliverable. The user states an intent, the AI
answers with a **workshop draft**, Helios checks it, and the user gets a
workshop built for that intent.

### The draft

A draft is a composite file with a workshop block (workshops.md, "The
data model"), plus a `rationale` per item so the user can see why each
piece is there. For the example:

- **Modules**: `toneequal` (shadows lifted, highlights held), `shadhi`
  as a lighter alternative when the AI judges it enough, `bilat` (local
  contrast) for the mountain's detail, `colorbalancergb` for the sky's
  saturation; `exposure` only if the whole image is underexposed.
- **Zones**: `sky` (a `mask_parametric` on high luminance and blue hue,
  feathered; or a `mask_object` with `kind: sky` when the AI says the
  parametric mask will not separate it), `foreground` (the sky inverted,
  low luminance), `mountain` (midtones between the two, optional).
- **Exposed sliders**, narrowed: the tone equalizer's lower bands, the
  sky's exposure and saturation, local contrast's detail. Everything
  else stays inside, at the AI's pre-calibrated values.
- **Semantic sliders**: "Backlight lift" (the tone equalizer's -8 to
  -4 EV bands along one curve, plus a little `shadhi` shadows), "Sky"
  (darker and more saturated inside the sky zone, from -1 to +1),
  "Mountain detail" (local contrast's detail in the mountain zone).
- **Targets** where the AI can name one: foreground median at some
  stops, sky highlights under a clip share.

### Semantic sliders

A new optional field of an exposed parameter, `drives`: one slider
drives several inner parameters, each along a piecewise-linear curve
(at most 8 points), so the file stays data and the curve is drawn in
the interface editor. Today one exposed name on several nodes links
them to one value; `drives` generalises that. The slider's own range is
the curve's domain, usually -1 to +1 or 0 to 1.

```json
{ "name": "backlight_lift", "label": "Backlight lift", "min": 0.0, "max": 1.0,
  "drives": [
    { "node": "toneequal", "param": "noise",  "curve": [[0, 0], [1, 1.6]] },
    { "node": "toneequal", "param": "ultra_deep_blacks", "curve": [[0, 0], [1, 1.2]] },
    { "node": "shadhi", "param": "shadows", "curve": [[0, 0], [0.5, 10], [1, 30]] }
  ] }
```

Built (slice 1): an exposed entry with `drives` gives neither `node`
nor `param`. Its range is `min`/`max`, else the curves' domain; its
default is `default`, else 0 within the range. A whole-number inner
parameter gets the curve's value rounded. Refused, the entry named: a
curve of fewer than 2 or more than 8 points, `x` not strictly
increasing, a point outside the inner hard range, a parameter that is
not a number, and one inner parameter set by two exposed names (a drive
and a plain exposure, or two drives: the cycle case). Opening or
dissolving a macro writes the curves' values onto the inner parameters;
folding it back drops them, the slider keeping its value. The interface
editor's "New semantic slider" adds one; its row edits the range and,
per driven parameter, the curve's points, drawn beside them. Agents
author it with `macro.interface`'s `exposed` list; no new verb.

### Helios checks the draft (deterministic, not the AI)

`helios_template::draft::check_draft` takes the draft and the open
image's proxy:

- **schema and catalogue**: every module exists, every parameter is in
  its hard range, every curve point too, the network is a valid
  composite (one input, one output);
- **tested at the extremes**: each exposed or semantic slider is rendered
  at its two ends on the proxy and measured (whole and per zone). A
  slider whose ends measure the same (below a threshold: 0.05 stop) is
  dropped as useless; a slider whose end clips more than a limit is
  narrowed by bisection until it does not. This is "restricted ranges,
  tested at the extremes" from ROADMAP section 2.5, done by Helios;
- **zones**: each zone is rendered once; a zone empty or covering the
  whole image is reported ("sky zone: 0.2 % of the image").

The answer is the checked draft and its notes, so an agent can amend
and re-post. Nothing is written yet.

Built (slice 3). A draft is `{ "intent", "composite", "rationale" }`:
`composite` is a composite file whose `format`, `id`, `version`, `inputs`
and `outputs` may be left out (one RGBA image in and out) and whose
block's intent defaults to the draft's; `rationale` holds a sentence per
piece, under `workshop`, `nodes`, `exposed`, `zones` and `targets`, each
by name. The check answers `{ ok, refused, sliders, zones, notes, draft }`:
a slider's `verdict` is `kept`, `narrowed` (with the range kept),
`dropped` or `untested` (not a number), with its `change` in stops, where
it acts most (`most_in`: a zone or `image`) and the share clipped at each
end; a zone's is `ok`, `empty` (under 0.5 %) or `whole` (over 99.5 %);
`notes` are the same in sentences ("Haze: dropped: from 0 to 1 it changes
the image by 0 stop at most, under 0.05"). An end is narrowed when it
clips more than 1 % of the image beyond what the defaults clip, pulled in
by bisection (10 renders at most). A dropped slider's value at its
default is written into the inner parameters it set, so the workshop
renders as before; its targets and rationale go with it. A rationale for
a piece the draft lacks is said and dropped. Refused, each problem said:
the schema, an unknown module, a value or curve point out of range, other
than one input and one output, a target on an unknown zone or slider, an
empty intent. `State::draft_applied` saves the cleaned composite as
`user.<slug of its name>@1` and inserts it as `workshop.apply` does, one
step; given a node a former design applied, it pins that node to the next
version instead.

### Applying a draft

`workshop.design` with `apply: true` (the default from the panel) inserts
the checked draft as a workshop node, as `workshop.apply` does, as **one
undo step** ("design workshop: backlit, sky too contrasted"). The draft
is saved as the workspace's composite `user.<slug>@1`; it is a workshop
like any other: re-aimed, opened in the graph, its interface edited,
published. Asking again replaces the node with a new draft, one more
undo step.

In the Workshops panel, the Find field gains a "Design a workshop for
this" row under the matches: it sends the typed intent to the design
step (section 8 says who answers).

Built (slice 4, the MCP path of C1). The Workshops panel opens with
"Describe what you want": the words and **Design a workshop** start a
design session (`workshop.describe`), which waits for the user's own
agent; a checkbox switches `workshops.ai.send_proxy` for the session (the
user's alone: an agent asking `workshop.send_proxy` is refused). The
agent calls `helios_workshop_context` (`intent` defaults to the
session's words; the measures of the image on the proxy, whole and in
two candidate zones, `bright` and `dark`, it may copy; the modules with
their parameters; the bricks; the workshops the words find; how to write
the draft; the proxy as a PNG only when allowed, else `proxy_refused`),
then `helios_workshop_design` with its `draft` (`apply` true by default,
`replace`, `after`, `dry_run`). The answer carries the `check`, and once
applied the node, the workshop (`designed`) and the aimed targets; the
panel shows the notes, "Designed: user.<slug>@1", and the applied
workshop's sliders. A refused draft applies nothing; posting again in the
session replaces the former design. Not built yet: the `helios-ai` path
(`workshop.design@1`), and an event telling a waiting agent a session
started (it reads the session from `helios_workshop_context`).

### Zones and their measures (built, slice 2)

A workshop block names its **zones**: `{ "name": "sky", "module":
"mask_parametric", "params": {…}, "port": "mask" }`, a module with an
image input and a one-channel output, drawn once from what enters the
workshop (the proxy the targets are aimed on), never from its sliders. A
pixel is in a zone where the mask is at least one half. A target gains
`zone` (measured inside it; the whole image otherwise) and, for
`zones.ratio`, `against`:

```json
"zones": [
  { "name": "sky", "module": "mask_parametric",
    "params": { "luminance": { "list": [0.4, 4.0, 0.05, 0.0] } } }
],
"targets": [
  { "param": "sky_exposure", "measure": "luma.median", "value": -1.0, "zone": "sky" },
  { "param": "backlight_lift", "measure": "zones.ratio", "value": -2.1,
    "zone": "foreground", "against": "sky" }
]
```

`zone.contrast` is the 95th minus the 5th percentile of luminance, in
stops; `zones.ratio` is the zone's median minus the median of the zone
it is against. A target whose zone is empty on an image keeps its
value and says so. Refused when the file is built: a zone that is not a
name or is named twice, a module the catalogue lacks or that draws no
one-channel output, a target naming no zone, `zones.ratio` without two
distinct zones, `against` on another measure. The save form keeps the
three whole-image measures; agents give zones and zone targets in
`workshop.save`'s `zones` and `targets`.

## 2. Refine on a test case, and record

### Piloting

The user works the photo with the designed workshop: moves sliders,
edits a mask, asks the AI for help ("the climber's face is still too
dark"). An AI answer while piloting is a **proposal**: values (or a
mask, or a module to add) shown as ghost values on the sliders, with
Accept, Override (the user's value wins) and Reject, as an AI-assisted
decision is. A second test photo can be opened in the same session: the
user refines it too, which tells generalisation what changes with the
photo and what does not.

### What the history holds today, and what it lacks

The value store's steps hold the paths, the old and new values, the
origin and the group; the vault's versions hold the values and the
origin; the decision book holds AI answers with their inputs; the audit
trail holds only library fields. None of them says **what a step aimed
at**, **what the image measured** before and after, or **that a user's
value overrode an AI proposal**. And the value store is session only.

### The session record

A **workshop session** is a vault document (namespace
`workshops/sessions`, keyed by session id) started when a workshop is
designed or applied with recording on, and closed by the user or when
the workshop is generalised. It refers to the history; it does not
replace it.

```json
{
  "session": "s-0007", "workshop": "user.backlit-mountain@1",
  "images": ["<image id>", "<image id>"],
  "steps": [
    { "seq": 1, "image": 0, "author": "agent:mcp", "kind": "design",
      "aim": "backlit, sky and mountain too contrasted",
      "version": "<edit version id>", "store_step": 41 },
    { "seq": 2, "image": 0, "author": "user", "kind": "set",
      "param": "backlight_lift", "old": 0.4, "new": 0.65,
      "aim": null,
      "measures": { "before": { "foreground.luma.median": -2.6, "sky.luma.high": 0.4 },
                    "after":  { "foreground.luma.median": -1.4, "sky.luma.high": 0.4 } } },
    { "seq": 3, "image": 0, "author": "agent:mcp", "kind": "propose",
      "aim": "climber's face too dark", "proposal": { "backlight_lift": 0.8 } },
    { "seq": 4, "image": 0, "author": "user", "kind": "override",
      "of": 3, "param": "backlight_lift", "new": 0.72 }
  ]
}
```

- **author**: `user`, `agent:<id>`, or `model:<provider>/<model>@<version>`
  for an answer from `helios-ai`, as the decision book names them.
- **kind**: `design`, `set`, `propose`, `accept`, `override`, `reject`,
  `mask` (a zone edited), `structure` (a module added or removed),
  `undo`.
- **aim**: the words that came with the step (an AI proposal's ask, the
  user's optional note); empty for a plain slider move.
- **measures**: the zone measures before and after, on the proxy,
  computed when the step commits (one proxy render, cached by content
  key; a gesture is one step, measured at its end).
- An undo is recorded as a step of kind `undo`; nothing is deleted.
  Generalisation reads the **net outcome** per parameter and keeps the
  path for the user to read.

The value store's `Step` gains an optional `aim` (the words), and the
version's metadata the session id, so the History panel can show "aim:
climber's face too dark" on a row and the session can be rebuilt from
the versions. No change to version ids (metadata is not hashed).

Built (slice 5). `helios_template::session` holds the record; the vault
keeps it in `workshops/sessions` (`Vault::workshop_session`). Recording
is always on (C6): `workshop.design` and `workshop.apply` start a session
(their step is its first, kind `design` or `apply`, measured before on
what enters the workshop and after on what it gives), ending the one
before. Each value step on the workshop's node is then a step: `set`
(or `override` when the user moves a value an open proposal holds),
`undo`, `redo`. A step lists its `changes` (`param`, `old`, `new`, as
plain JSON) rather than one `param`, since a step may move several; it
names its `store_step`. Measures are keyed `image.<measure>` (median,
highlights, clipping) and `<zone>.<measure>` (median, highlights,
contrast), from one render of the workshop alone on the proxy. The
record is written as it grows with `kept: false`; `workshop.session`
reads it with its `net` outcome, `keep` (the user's alone, the panel's
Keep) or `close` (dropped unless kept); a session a former run left
unkept is dropped when the desktop opens the vault. `Set::aim` and
`Store::aim` (for a group) put the words on the value store's step;
`Version::meta` (a column of its own, not hashed) carries
`{"workshop_session": "<id>"}` on each version saved during a session.
Not built yet: `mask` and `structure` steps, a second test image in the
same session, the History panel's aim column.

## 3. Generalise: "Make it automatic"

The user presses **Make it automatic** in the Workshops panel (or an
agent calls `workshop.generalise`). Helios reads the session's net
outcome on the test images and proposes one **row per decision**, each
with what it found, its evidence and a default. Nothing is written until
the user confirms.

### The rules

For each exposed or semantic slider the user or the AI set:

1. **A relative target where a measure explains the value.** Candidate
   measures: the three of v1, the same per zone (`sky.luma.high`,
   `foreground.luma.median`…), and two new ones, `zone.contrast` (the
   95th minus the 5th percentile of luminance in a zone, in stops) and
   `zones.ratio` (one zone's median minus another's, in stops: the
   backlight gap). A candidate **explains** the value when re-aiming from
   the workshop's neutral value on each test image lands within
   tolerance of the user's value (one slider step, or 0.1 EV) and the
   measure is monotonic over the slider's range. With two test images,
   the target must fit both better than a fixed value would.
2. **Otherwise a fixed value**: the user's value (the mean of the test
   images', with their spread shown).
3. **AI proposals become deterministic rules where one fits.** A zone the
   AI found (a `mask_object` answer) is fitted with a `mask_parametric`
   (luminance and hue bounds searched on the test images) and kept when
   the overlap (intersection over union) is at least 0.9 on every test
   image. A value the AI proposed goes through rule 1 like a user's.
4. **An AI junction is kept only where nothing fits**: the zone stays an
   AI object member, or a value stays a decision node asked per photo.
   The row says why ("sky: the parametric mask overlaps 0.71 on the
   second photo; kept as AI, 1 call per photo").

A row reads: *"Backlight lift: 0.72 and 0.55 on the test photos;
target `zones.ratio(foreground, sky)` at -2.1 stops fits both within
0.03. [Same target] [Same value 0.64] [Keep as AI]"*.

### Limits

Each row also carries its **limits**, the explicit bounds the run must
stay within. They are prefilled from the test cases and the user caps
them; they are saved in the workshop block (`limits`):

| Limit | On | Default | What a breach does |
|---|---|---|---|
| `range` | a slider | the narrowed range, tightened to the test values ± 50 % of their spread plus one step | clamp, flag `at_limit` |
| `max_deviation` | a slider | twice the test cases' spread, at least 10 % of the range | clamp to test value ± deviation, flag `deviation` |
| `tolerance` | a target | one step, or 0.1 stop | flag `target_missed` |
| `input_span` | the workshop | the test images' measures ± 1.5 stops | flag `unlike_test_cases` (the photo is not like what was demonstrated) |
| `min_confidence` | a junction | 0.7 | apply the fallback, flag `low_confidence` |
| `fallback` | a junction | the test case's answer (a mask is re-asked; a value is the test value) | used on low confidence or no AI available, flag `fallback` |
| `max_ai_calls` | the run | `J` × photos | stop asking, use fallbacks, flag `ai_budget` |
| `on_breach` | the run | `flag` | `flag` (apply, flag for review), `hold` (do not apply this photo), `stop` (stop the run) |

A non-monotonic measure on a photo (aiming cannot decide) keeps the
test value and flags `not_monotonic`.

### Minimum AI, measured

- `J`: junctions left in the workshop. Shown on the workshop's row in
  the panel ("AI per photo: none" when `J = 0`).
- `A`: expected AI calls per photo in a batch: the sum over junctions of
  the probability the junction is asked. A junction whose answer the
  cache holds (same input content key, same question) costs nothing; a
  zone kept as AI only when the parametric mask's own check fails
  ("sky zone empty or whole image") is asked only on those photos, and
  the generaliser estimates that share on the test images.
- After a run: the real calls, per photo and in total, from
  `calls.jsonl` and the decision book; a second run of the same set
  makes **zero** calls (answers recorded).
- When `J > 0`, the row shows the provider, an estimated latency per call
  and, for a remote provider with a known price, a cost per 100 photos.

The confirmed rows write the workshop's next version (`workshop.save`'s
step, one undo step); the session is closed and kept, linked from the
version.

## 4. Run within limits

### What runs on what

`workshop.run {workshop, set, mode, limits?, dry_run}` where `set` is one
of:

- `selection`: the Library's or the filmstrip's selected images;
- `query`: a Library query (as a template's library opens on one);
- `series`: the images of a stack, a burst, or a time gap around the
  open image (the series rule of the Library);
- `import`: a saved import names a workshop; its `batch.done` stage
  schedules `workshop.run` over the page's images (a follow-up job, as
  import-connectors.md 7.6 does for stacks and backups; a participant
  never writes edits itself).

### How it runs

A run is a **node job on the workflow level**: one journal row per
(photo, step), with leases, retries, cancel and progress in the jobs
centre and through `helios_workflow_*`. The steps per photo:

1. **resolve zones**: parametric masks rendered (deterministic); AI
   object members asked only when the workshop keeps them, through the
   configured resolver, in full-AI mode for the run, at most
   `limits.calls` at once;
2. **aim**: the targets solved in file order on the photo's proxy (v1's
   bisection, per zone now);
3. **check limits**: clamp, flag, hold, or stop;
4. **write**: the photo's edit gets the workshop node (pinned version),
   its values, the decisions' answers in the book, and the
   **provenance** of each value: `aimed` (target, measured value,
   probes), `fixed`, `fallback`, or `user` after a fix (workshops.md's
   open question 4, answered yes for runs). The edit re-renders
   deterministically and offline from what it holds.

Two runs of the same workshop version on the same photos give the same
edits (same content keys) and the second asks no model.

### Applying is one undo step

The run writes each photo's edit as a version labelled "workshop
<name> (run <n>)", and registers the whole run as **one library-scope
step** (`library_by: workshop-run-<n>`), as an import is one step
(undo.md, "Built: an import as a library step"). Undoing it writes each
photo's previous edit back as a new version; a photo the user edited
after the run is left alone and the undo says so (VS Code's rule, as
for an agent's batch). Photos held by `on_breach: hold` are not written
until fixed.

### The review: a contact sheet

`workshop.review {run}` and a Review view in the Workshops panel (a grid
of the run's photos, after, with before on hover):

- flagged photos first, each with its badges (`target_missed`,
  `at_limit`, `deviation`, `unlike_test_cases`, `low_confidence`,
  `fallback`, `not_monotonic`, `ai_budget`);
- a filter per flag; the counts on top ("212 photos, 9 flagged, AI
  calls: 14");
- **fix one**: opens the photo with the workshop's sliders, the flagged
  one highlighted; the user's change is a normal value step with
  provenance `user`, and clears the flag;
- **decide a junction**: a low-confidence mask shown with its options,
  as an AI-assisted decision;
- **teach it**: a fixed photo can be added to the workshop's test cases,
  which re-opens generalisation with three photos instead of two.

## 5. Series consistency: same look or same targets

Two meanings of "the same" across a series, chosen **per slider** on its
generalisation row:

- **Same look**: the same value on every photo (Lightroom's Sync, a
  style). Right when the light did not change (a studio, a burst): the
  photos stay identical in treatment, differences in the scene show.
- **Same targets**: each photo aimed to the same measure. Right when the
  light changed (backlight that comes and goes, clouds): the output
  measures match, the values differ.
- **Same targets, held together** (proposed third): aimed per photo,
  then the values smoothed over the series in time order and kept
  within `max_deviation` of the series median, so a time-ordered series
  does not flicker (the deflicker idea of time-lapse tools).

The default per slider follows generalisation: a target where a measure
explains the value, a fixed value otherwise; the user flips any row. A
whole-workshop switch "Same look for every slider" exists for a series
shot in constant light.

## 6. The agent contract

### Verbs

All on the node's MCP server, mirrored by commands of the same names,
experimental like v1's, refused with the setting's key when it is off.

| Verb | Arguments | Answer |
|---|---|---|
| `helios_workshop_context` | `node` or `image`, `intent`, `proxy` (asked) | what an agent may read: measures (whole, per candidate zone), the module catalogue with parameter descriptors, the bricks' descriptions, existing workshops matching the intent, the privacy setting; a proxy render only if allowed |
| `helios_workshop_design` | `intent`, `draft`, `apply`, `dry_run` | the checked draft and its notes; applied as one undo step |
| `helios_workshop_session` | `action` (`start`, `read`, `close`), `session` | the record |
| `helios_workshop_propose` | `session`, `aim`, `values` or `mask` or `structure` | a proposal shown to the user; the user's answer in the record |
| `helios_workshop_generalise` | `session`, `rows` (to amend), `confirm`, `dry_run` | the rows with evidence, `J`, `A`; with `confirm` the next version |
| `helios_workshop_run` | `workshop`, `set`, `mode`, `limits`, `dry_run` | the run's id; the dry run says the photo count, `A`, and the estimate |
| `helios_workshop_review` | `run`, `flag`, `fix` | the flagged photos; a fix applied as the user's would be |

An agent **proposes**, it does not decide: `generalise` with `confirm`
and `run` without `dry_run` need the user's confirmation in that
session, as `macro.publish` does. Undo of an agent's design or proposal
is in its lane (undo.md).

### What the agent receives

- the **intent** (the user's words) and the session's steps;
- **measures**: numbers, histograms per zone, the image's exposure
  metadata (no GPS, no names, no file paths);
- the **catalogue**: modules with their parameters, ranges and
  descriptions, the bricks' AI-readable descriptions
  ([process-bricks.md](process-bricks.md), 3.8), the existing workshops;
- **a proxy render only if the user allows it** (a per-session switch,
  `workshops.ai.send_proxy`, off by default), at
  `privacy.remote_image_side` for a remote model, re-encoded without
  metadata ([ai-models.md](../ai-models.md), "Privacy"); never the
  original.

### Where the AI runs

- **The user's own Claude** (or any MCP client) through the node's MCP
  server: the agent reads the context and posts drafts and proposals.
  Helios stores **no API key**: the model and its account are the user's.
  For a batch, Helios cannot call that agent on its own; a junction in a
  run becomes a **pending decision**, and the agent answers it with the
  decision verbs (a pull: the agent waits on `helios_wait_event` for
  `workshop.junction_pending` and answers). With `J = 0` no agent is
  needed during the run.
- **`helios-ai`**: the same steps as decisions with versioned prompts
  (`workshop.design@1`, `workshop.propose@1`) through the configured
  provider (local Ollama by default; a remote one only when the user adds
  it, its key in the keyring or the environment). Junctions in a run are
  asked by Helios directly.
- **A plugin** (out of process): a `workshop.junction` service answering
  a junction's request with the same schema, for a local model shipped
  as a plugin.

All three post the same draft format and go through the same check and
the same refusals.

### Privacy, cost and latency

- **Privacy**: measures leave by default, pixels only by the user's
  switch; every call is in `calls.jsonl` with what left (images, bytes);
  the review shows a run's calls.
- **Cost**: design and piloting are a handful of calls per session; a
  run costs `A` × photos calls, shown before it starts; `max_ai_calls`
  caps it.
- **Latency**: a deterministic photo costs its proxy renders (a few
  hundred milliseconds); a junction adds its call (seconds, remote). A
  run never waits on a junction to aim the others: pending photos are
  finished when their answer arrives.

## 7. Prior art, and what Helios does differently

| Tool | What it does | What it lacks for this |
|---|---|---|
| Lightroom: Auto Sync, Sync Settings | copies the open photo's settings to the selection as it changes | the same values everywhere (same look only); no measure, no target |
| Lightroom: presets, Auto | a saved set of values; Auto sets tone from an analysis | presets are absolute; Auto has no user target |
| Lightroom: AI masks, Adaptive Presets (2022) | Select Sky / Subject / People; presets carrying such masks, recomputed per photo | the AI is asked on every photo; the look inside the mask is still fixed values; not inspectable |
| Capture One: styles, Smart Adjustments (16.0) | styles are value sets; Smart Adjustments set exposure and white balance per photo from a reference image | two measures, fixed by the vendor; no zones, no user-chosen targets |
| darktable: styles, auto-applied presets, Lua | a style is a replayed history; presets auto-apply by Exif rules; Lua can apply styles in batch, and import hooks exist | absolute values; Lua writes outside the history and undo |
| Photoshop: Actions, droplets, Batch | a recorded list of steps, replayed on a folder; "modal" steps stop for a human | absolute values; a modal step is the only junction, and it is a human every time |
| Programming by demonstration | Berthouzoz, Li, Dontcheva and Agrawala (2011) learn from a few demonstrations how each selection and parameter depends on image features; Grabler et al. (2009) record a session into a tutorial | research; parameters learned by regression, no recorded per-photo judgment |
| Adobe's Claude connector (April 2026) | Claude drives Photoshop, Lightroom and other Creative Cloud tools from a chat: adjust colour and tone, apply presets, remove backgrounds | the agent acts per request; nothing is generalised into a reusable, deterministic workshop |

What Helios does differently:

- **The AI builds the tool, not the result**: the first deliverable is a
  workshop (real modules, the right sliders, narrowed and tested), which
  the user drives; the photo is never a black box.
- **Targets, not values**: like Capture One's Smart Adjustments, but the
  measures and zones are the user's, chosen from what explained their own
  demonstration.
- **AI only where needed, and counted**: each AI judgment is a junction,
  replaced by a deterministic rule when one fits, and `J` and `A` are
  shown before a run.
- **Recorded, reproducible**: every per-photo answer is in the edit; a
  run replays offline and a second run asks nothing.
- **Limits and review**: the user caps what the automation may do, and
  sees exactly where it reached a cap.
- **Undoable**: a run is one step; nothing writes outside the history.

## 8. Slices

Ordered as the owner set: the tailored workshop first, then refining
and recording, then generalisation and the batch within limits. Each is
one commit or a few, builds and passes its tests, and is testable
headless. Sizes: S under 300 lines, M 300 to 800, L over 800, tests
included.

1. **Semantic sliders** (M, built 2026-11-11). `drives` in the composite format and schema,
   piecewise-linear curves, the interface editor's curve rows; refusals
   (a point out of the inner range, a cycle of drives). Test: a
   "Backlight lift" slider on a synthetic network moves three inner
   values as its curves say; the flat network renders with the same
   content keys.
2. **Zone measures** (M, built 2026-11-11). The measures per zone (a mask input of the
   proxy render), `zone.contrast` and `zones.ratio`; targets with a
   `zone`; aiming per zone. Test: a synthetic backlit image (a bright
   upper band, a dark lower band) aims "foreground median at -1.5 stops"
   within tolerance.
3. **The draft and its check** (M, built 2026-11-11). The draft format (a composite with a
   workshop block and rationales), `check_draft`: catalogue, ranges,
   extremes tested, useless sliders dropped, clipping narrowed, zones
   reported. Test: a hand-written backlit draft with one useless slider
   and one clipping range is checked: the first dropped, the second
   narrowed, the notes say so.
4. **`workshop.design`** (M, built 2026-11-11 but the `helios-ai`
   path). The verb and command, applying a checked
   draft as one undo step, the panel's "Design a workshop for this" row,
   `helios_workshop_context`; the `helios-ai` path with the
   `workshop.design@1` prompt. Test: the fake provider (and a scripted
   MCP agent) answers the backlit intent with a draft; the workspace
   gains the workshop with exactly its sliders; Ctrl+Z removes it. This
   slice ends the **near goal**.
5. **The session record** (M, built 2026-11-11). The `workshops/sessions` document, step
   records with author, kind, aim and measures, the `aim` on value-store
   steps, the session id in version metadata. Test: a scripted pilot
   session (design, three slider moves, an undo) gives the expected
   record and net outcome.
6. **Proposals while piloting** (S). `workshop.propose`, ghost values,
   Accept, Override, Reject recorded. Test: the fake agent proposes, the
   script overrides; the record holds proposal and override.
7. **Generalisation** (L). The rules (targets that explain a value,
   fixed values, parametric fits of AI zones, junctions kept), the rows
   with evidence, `J` and `A`, `confirm` writing the next version. Test:
   the scripted session on two synthetic test images gives "Backlight
   lift: target `zones.ratio`", "Sky: parametric mask", `J = 0`; a
   variant where the parametric fit fails keeps one junction, `J = 1`.
8. **Limits** (S). The `limits` block, its defaults from the test cases,
   the pure checker (clamp, flag, hold, stop). Test: each flag raised
   by a crafted input.
9. **The run** (L). `workshop.run` on the workflow level over a selection,
   a query and a series; zones, aiming, limits, writing with provenance;
   pending junctions; one library-scope step. Test: a synthetic set of
   24 images of varying backlight with a fake agent at one junction: the
   expected flags, the AI calls equal to the photos where the fit fails,
   a second run with zero calls and the same content keys, undo restores
   every edit and leaves a photo edited after the run alone.
10. **The review** (M). `workshop.review`, the contact sheet, fix one,
    decide a junction, teach it. Test: headless, the flagged photos
    first, a fix clears its flag and records provenance `user`.
11. **From an import** (S, after the import hooks are built). A saved
    import names a workshop; `batch.done` schedules the run. Test: an
    import of the synthetic set ends with each page's run in the jobs
    centre.

Slices 1 and 2 can land in either order; 3 needs both; then in order.
Slice 11 waits for import-connectors.md's lifecycle slice.

## 9. Choices for the owner

Decided on 2026-11-11: the owner accepted every recommendation (C1 C,
C2 C, C3 C, C4 B, C5 B, C6 C, C7 A, C8 C). Each choice keeps its
options in key order and says which was taken.

**C1. Who answers the design and proposal steps.**
- A. The user's own agent through the node's MCP only: no key in Helios,
  no model shipped.
- B. `helios-ai` only (local by default, remote when added).
- C. Both, with one draft format, one check and the same refusals.

Decided (2026-11-11): **C**, as recommended, the MCP path first (slice 4 tests both with fakes):
it is the owner's own setup and it stores no key.

**C2. How a run writes edits.**
- A. Write every photo, review after; undo the run as one step.
- B. Stage every photo as a proposal; nothing is written until the
  review applies them.
- C. Write the unflagged photos, stage the flagged ones until fixed.

Decided (2026-11-11): **C**, as recommended: the bulk lands at once, the doubtful ones wait, and
the run is still one undo step.

**C3. Series consistency default.**
- A. Same look (same values everywhere).
- B. Same targets (each photo aimed).
- C. Per slider, from generalisation: a target where a measure explains
  the value, a fixed value otherwise; "held together" offered for
  time-ordered series.

Decided (2026-11-11): **C**, as recommended.

**C4. A junction with low confidence in a run.**
- A. Hold the photo for review.
- B. Use the fallback (the test case's answer) and flag it.
- C. Skip the photo.

Decided (2026-11-11): **B**, as recommended: the run completes, the review shows each fallback.

**C5. Pixels sent to the AI.**
- A. Never: measures and descriptions only.
- B. A proxy only when the user switches it on for the session.
- C. A proxy always to a local model, by switch to a remote one.

Decided (2026-11-11): **B**, as recommended: to start, as the owner asked ("only if the user
allows"); C once a local vision model is in the recommended setup.

**C6. When a session is recorded.**
- A. Always, while a workshop is designed or applied.
- B. Only after the user presses Record.
- C. Always, and dropped unless the workshop is generalised or the user
   keeps it.

Decided (2026-11-11): **C**, as recommended: no button to forget, nothing kept that is not used.

**C7. Semantic sliders' form.**
- A. Piecewise-linear curves in the composite file.
- B. Expressions (a small formula language).
- C. A brick graph behind the slider.

Decided (2026-11-11): **A**, as recommended: data an AI can write and a user can see as a curve;
B and C can come later for what curves cannot say.

**C8. Defaults for limits.**
- A. Tight: the test values' spread, one step of tolerance.
- B. Loose: the narrowed ranges only.
- C. The table of section 3 (spread-based, at least 10 % of the range).

Decided (2026-11-11): **C**, as recommended, each limit shown and editable on its row.

## Sources

- Adobe Lightroom Adaptive Presets and AI masks (Adobe MAX 2022):
  <https://blog.adobe.com/en/publish/2022/10/18/photography-at-adobe-max-2022-new-features-for-adobe-lightroom-and-more>
- Capture One Smart Adjustments:
  <https://support.captureone.com/hc/en-us/articles/7129920998045-Smart-Adjustments>
- Berthouzoz, Li, Dontcheva, Agrawala, "A Framework for Content-Adaptive
  Photo Manipulation Macros", ACM TOG 30(5), 2011:
  <https://research.adobe.com/publication/a-framework-for-content-adaptive-photo-manipulation-macros-application-to-face-landscape-and-global-manipulations/>
- Grabler, Agrawala, Li, Dontcheva, Igarashi, "Generating Photo
  Manipulation Tutorials by Demonstration", SIGGRAPH 2009:
  <https://history.siggraph.org/?p=108824>
- Adobe for Claude (connector help page):
  <https://helpx.adobe.com/creative-cloud/apps/integration-with-other-apps/adobe-connectors/adobe-for-claude.html>;
  announcement coverage, 2026-04-28:
  <https://petapixel.com/2026/04/28/claude-ai-can-orchestrate-creative-workflows-across-adobe-apps/>
