# Extractors: typed data about images, and the nodes that use it

Status: **design** (2026-11-14). The plan was accepted by the owner on
that date, and the design below follows the owner's refinements of the
same day (no runtime installation; tasks are diagram nodes; extractors
paired with filters, routers and alters; extractors are not predefined
but a prompt plus declared keys; modules may ship ready-made ones and the
LLM core module has an editor). The slices at the end wait for the
owner's go.

**Owner amendments of the same evening** (the latest word wins over
the text below where they differ): extractors are **blocks, not
nodes**. They have no ports; they snap onto **carrier nodes** (router,
filter, update-metadata alter), which read their declared keys while
the diagram is designed and hand them each image at run time. A
reusable **definition** (prompt or built-in, keys, capability, version)
is made in the extractor editor; a diagram holds **uses** of it
attached to carriers, each pinned to a version with optional
parameters, all sharing one cache per image. Keys may also be
**semi-finite**: a declared list plus whatever values runs see.

### Slice 1 built (2026-11-14)

The `helios-extract` crate holds the contract, with no computation and
no model call. It depends on no other Helios crate, so carriers, the
editor, module manifests and helios-ai's call path can all read it
without pulling an HTTP client.

- `decl`: `KeyDecl` (name, type `text | label | number | integer |
  bool`, values `Finite | SemiFinite | Open`, multi, confidence for
  labels, `other` for finite keys, bounds and unit for numbers),
  `ExtractorDef` (id, version, title, keep, confidence floor, kind
  `Llm { capability, prompt or prompt_id, context, provider, model }`
  or `Builtin { compute }`, keys), `ExtractorUse` (definition id,
  pinned version, parameters), `Manifest` (`[[extractors]]`, TOML or
  JSON) and `Catalog`. Unsound declarations are refused with their
  reasons (reserved values `none` and `other`, duplicates, a prompt
  variable not in its context...).
- `schema`: the answer schema derived from the keys (table in section
  2; a single non-finite key may be `null`; semi-finite lists go as
  `examples`), checked to stay in helios-ai's subset, and one sentence
  per key for the prompt.
- `answer`: lenient parsing, validation into typed values; out-of-list
  values dropped or mapped to `other`, confidences below the floor
  dropped, new semi-finite values kept, each counted; errors named by
  path, and the note for the one corrective retry.
- `cache`: the key of section 7, BLAKE3 over canonical JSON of the
  image and its source, `id@version` and the hash of what the
  definition computes (not its title), the model, the use's parameters
  and the context inputs; pinned by a golden test.
- Built-ins declared only: `exif`, `cull`, and the `animals` LLM preset.

### Slice 2 built (2026-11-14)

helios-ai now depends on helios-extract (not the reverse; the test that
derived schemas stay in helios-ai's subset moved to helios-ai).

- `routing::Overrides` (run, node, diagram, each a `RouteOverride`:
  provider and/or model) and `Routing::resolve`, field by field, highest
  level first, then the map ([ai-models.md](../ai-models.md),
  "Overrides, the route that answered, one retry"). A capability-level
  map as an override is not built: one route per level so far.
- `Ai::run_prompt_with(PromptCall)` returns `PromptRun` (route, where it
  came from, the answering model, prompt `id@version` and hash,
  attempts, the extractor's typed answer and `counts` of adjusted
  values) or `PromptFailure` (each try's raw reply and errors). One
  corrective retry with `retry_note`; the call log gains `route_from`,
  `attempt` and `answered`.
- `Ai::check_needs` is the core of the pre-run check of slice 3: given
  (capability, prompt, overrides), the resolved route and why it cannot
  serve.

### Slice 3 built (2026-11-14)

- **The check before a run** (`Ai::check_run`, `check_submission`):
  each model node (`Need` with its `node` name) resolved and checked,
  the prompt included; a node nothing serves refuses the submission
  with its name, the reason and a pointer to the AI settings, before
  anything runs. `leaves` states which nodes send images off the
  machine, to whom (provider, URL, model), how many and at what side;
  `remote_calls` against the ceiling. Agents call it as
  `helios_ai_check` (`ai.check`); the runs layer (slice 5) calls it on
  submission.
- **Limits per provider** (`helios_ai::limits`): `parallel` and
  `per_minute` under `[limits.providers.<id>]`, a call over them waiting
  (cancellable, its job timeout paused), local vision one at a time by
  default; `limits.calls` stays as the default `parallel`;
  `limits.run_remote_calls` (500) refuses a run's remote calls past it,
  `PromptCall::run` naming the run. "Stops and asks" is the runs
  layer's, on that refusal.

### Slice 4 built (2026-11-15)

- **Results** (`helios_extract::result`): `Extracted` is one
  extractor's result for one image (cache key, image, `id@version`,
  definition hash, the `provider/model` the key was made with, the
  model as it answered, typed values, adjustments, when).
  `ResultCache` is the seam the call path reads before a model call and
  writes after; `MemoryCache` is the reference in memory. A use may
  override where its results go (`ExtractorUse::keep`), outside the
  cache key.
- **In the vault** (`helios_vault::extract`): kept results in
  `extract/<id>`, one document per image, the latest version winning (a
  result of an older version goes with its run instead); run-only ones
  in `workflow/<run>/extract` by cache key, removed with the run. A
  lookup reads the image's kept document, then the run's results, then
  those of the other runs still kept. `Vault::extract_cache(run)` is the
  vault as a `ResultCache`; the contract checks it on both stores.
- **Through the cache** (`Ai::run_extract`): the key is made from the
  route the call resolves to (overrides applied), so another model is a
  miss; a hit calls no model, a miss runs the prompt (one corrective
  retry) and stores the answer; a failed call stores nothing. The
  model's own digest, known only once it answered, is recorded in the
  result, not in the key.

In a manifest the kind is a table (`[extractors.llm]` or `builtin = {
compute = "capture" }`), not a `kind = "llm"` field as the sketch in
section 2 shows.

### Slice 5, first part built (2026-11-15)

The run engine for extractor nodes (`helios_node::workflow::extract`),
beside the export runs, not yet behind a node verb:

- **Library images as items:** `image:` and the content hash
  (`image_item`), so a submission reads no image.
- **Extractor nodes** (`ExtractNode`: a use of an LLM extractor, its
  prompt's inputs, its route overrides, the nodes it waits for) are
  queueing nodes of a journal; each (image, node) row concludes once.
  A run opened again over its rows gives back the running ones and
  goes on; with the cache, nothing concluded is called again.
- **Submission** checks every node with `Ai::check_run` first: a node
  nothing serves refuses it, naming the node, before anything is
  entered.
- **Each row** calls `Ai::run_extract` with the run's cache (for a
  vault, `vault.extract_cache(Some(run))`); the caller's `ImageSource`
  chooses the image sent (embedded preview, or a head version as
  `image_version`). A reply that never fits fails for good; another
  failure is retried twice (2 s, then 4 s).
- **Facts on the bus:** `extract.run` (`submitted` with what leaves
  the machine, `refused`, `progress` with the counts, `finished`,
  `stopped`) and `extract.item` (one per concluded row: `done` with
  `hit`, or `failed` with the error).
- Cancel turns the rows not concluded into `cancelled`; what concluded
  stays. Removing the run drops its run-only results.

Left for the second part: the node verbs (`workflow.extract` or
`diagrams.submit`) and the runner thread, the run as a job and an
`extract` promise in the Activity focus, input nodes (selection, album,
smart album, import session, query, whole library) resolved to image
ids, reading the embedded preview from the library, the alter node, and
the tests through the batch endpoint.

It builds on [ai-models.md](../ai-models.md) (providers, routing,
versioned prompts, the call log), [workflow-level.md](../workflow-level.md)
(runs, journal rows per item and graph node, choices),
[protocol.md](../protocol.md) ("Promises"), [library.md](library.md)
(fields, rules, keywords, the metadata panel, writing back) and
[thumbnail-badges.md](thumbnail-badges.md) (facts and badges).

## In short

- A **diagram** that works on a set of images has three roles of nodes:
  **inputs** choose the images (all or part of the library), **extractors**
  compute typed data about each image and change nothing, **consumers**
  use that data: a **filter** keeps or drops, a **switch/case router**
  sends each image to one output per case, an **alter** changes
  something (update metadata, rating, album…).
- Every extractor declares its output as **keys**: a name, a type, finite
  values (an enum) or open ones, one value or several, and an optional
  confidence. Consumers see keys, never who produced them: an LLM, a
  classifier, EXIF or keywords already on the photo are interchangeable.
- **LLM extractors are not predefined.** One generic node takes a prompt
  written in plain words and the keys the user declares; Helios derives a
  JSON schema, asks for structured output, enforces the enum, retries a
  bad answer once and reports it. The LLM core module's **extractor
  editor** writes, tries and saves them (name + version). Modules may
  ship ready-made extractors with the same contract.
- **Two layers for models:** an extractor declares a capability and a
  prompt; helios-ai's providers and routing choose who answers,
  overridable per diagram, per node and per run. The same diagram runs on
  a local runtime or a remote API without being edited. Helios installs
  nothing: it uses the providers the user has configured.
- Results are **cached** per image, extractor version and model. They are
  either **kept on the photo** (searchable, usable by smart albums and
  badges) or **run-only**. Written metadata is **suggested** until
  accepted, one undo step per run.
- A diagram run is **one run and one promise** in the Activity focus:
  resumable per image and node, cancellable, limited by the provider.

## 1. The three roles

### Where the diagram lives

The workflow level already runs a network over many items: a run pins a
workflow derived from the network, keeps one journal row per (item,
queueing graph node), and a node of kind `decision` with branches becomes
a journal `choice` whose untaken branches are `omitted`
([workflow-level.md](../workflow-level.md), sections 1 and 2). Extractors
and consumers are **queueing graph nodes** of that level: each is once
per item, may fail, wait or be rate-limited on its own, and never renders
pixels. They are declared and drawn in the same graph as the rest (the
Graph tab), so an extractor diagram can sit before an export, and a
batch export can be routed by extracted data.

| Role | Ports | Journal shape | Examples |
|---|---|---|---|
| input | images out | the run's `submit` items | whole library, selection, album, smart album, import session, filter/query, folder |
| extractor | images in, images + data out | one row per item | LLM extractor, EXIF facts, culling measures, perceptual hash |
| filter | images + data in; `kept`, `dropped` out | a `choice` with two branches | `animal contains dog`, `focus > 0.6` |
| router | images + data in; one output per case, `other`, `none` | a `choice`, several branches allowed (below) | switch on `animal`, `camera`, `place.country` |
| alter | images + data in, images out | one row per item, writes | update metadata, set rating or label, add to album |
| gather | all items of its input in, one value out | a barrier (new, below) | the context of an import or folder |

Data travels with the image along the edges: a consumer reads the keys of
the extractors upstream of it by name (`animals.animal`), checked when the
diagram is built.

### What the workflow level needs for this

1. **Items from the library.** Runs today take files (`path:` ids); image
   ids from the library waited for a library-driven batch. Inputs need
   it.
2. **Multi-branch choices.** A router in "all matches" mode sends one
   image down several branches. The journal's `choice` concludes with one
   branch; it gains a set of branches (the others `omitted`).
3. **A gather node.** A set-level step (a folder's or an import's
   context, taken from the captions of all its images) waits for every
   item of its input group, computes one value per group, and hands it to
   each item downstream. It is a journal node whose rows are claimed per
   group once their parents concluded for the whole group.
4. **Nodes without a render.** An extractor reads what it needs (a
   preview, a small decode, the head version rendered small, the vault's
   facts); it never sits in the pixel pipeline.

## 2. The extractor contract

One contract for every extractor, user-made or shipped:

```toml
id = "animals"            # dotted lowercase, unique in its namespace
version = 3               # any change of prompt, keys or settings is a new version
title = "Animals in the photo"
kind = "llm"              # llm | builtin | plugin
keep = true               # data kept on the photo (section 7) or run-only

[[keys]]
name = "animal"
type = "label"            # text | label | number | integer | bool
values = ["dog", "cat", "bird", "horse"]   # finite: an enum; absent: open
multi = true              # one value or several
confidence = true         # each value carries p in [0, 1]
other = true              # keep "outside the list" as `other` instead of dropping it
```

**Keys.** A key has a name (lowercase, unique in the extractor), a type,
finite values or open ones, single or multi, and an optional confidence.
A number may carry a range (`min`, `max`, a unit). The full name seen by
consumers is `<extractor>.<key>`.

**The schema derived from the keys** (helios-ai's JSON Schema subset:
`type`, `properties`, `required`, `items`, `enum`, bounds):

| Declared | Answer schema |
|---|---|
| finite, single | `{"enum": [values…, "none"]}` |
| finite, multi | `{"type": "array", "items": {"enum": [values…]}}` (empty: none) |
| finite, multi, confidence | `{"type": "array", "items": {"type": "object", "required": ["value", "p"], "properties": {"value": {"enum": […]}, "p": {"type": "number", "minimum": 0, "maximum": 1}}}}` |
| open text, single | `{"type": "string", "maxLength": 300}` |
| open label, multi | `{"type": "array", "items": {"type": "string", "minLength": 1}, "maxItems": 30}` |
| number / integer / bool | that type, with its bounds |

The keys together are one object, every key required, no other property.
`other = true` adds `"other"` to the enum, so a model that sees a goat
answers `other` instead of forcing `dog`.

**How it is enforced:**

1. The schema goes to the provider as structured output where it takes
   one (Ollama's `format`, OpenAI-compatible `json_schema`); Anthropic
   gets it in words with the prompt, as helios-ai does today.
2. The reply is parsed leniently (JSON found in the text, code fences
   stripped) and validated against the schema.
3. Values outside a finite list are dropped (or mapped to `other`), and
   the drop is counted, never silent. A confidence below the extractor's
   floor (default 0) is dropped the same way.
4. A reply that still does not fit is **retried once**, with the
   validation errors appended to the prompt ("Your answer did not fit:
   `animal[0]`: `goat` is not one of …").
5. A second failure concludes the item `failed` at that node, with the
   raw reply and the errors in the run's item detail; downstream, a
   consumer sees no data, so the router sends it to `none`, the filter
   to `dropped` (a parameter can hold it instead).

**Non-LLM extractors** declare the same keys with fixed values and
compute them in code; nothing about them reaches a model. Consumers treat
both kinds the same.

## 3. The LLM extractor and its editor

### One generic node

An LLM extractor is the generic node of the LLM core module, configured
by:

- **a prompt** in plain words ("Which animals are in this photo?"),
  with optional **context variables** filled per image: `{{keywords}}`
  (existing), `{{caption}}` (accepted or suggested), `{{exif}}` (camera,
  lens, date, settings), `{{folder}}`, `{{import}}` (the
  import session's name and tags), `{{context}}` (a gather node's
  value), `{{language}}`;
- **the declared keys** (section 2);
- **a capability**: `vision` (the image is sent) or `chat` (texts only,
  far cheaper, enough for "is it about animals?" when captions exist);
- **an optional provider override** (section 6);
- **the image it sends**: the embedded preview (default), or the head
  version rendered small; the longest side follows helios-ai's privacy
  settings.

Helios turns this into a helios-ai prompt (`extract.<id>@<version>`):
a fixed system text ("You extract facts from photographs; answer with
JSON only"), the user's text, a generated sentence per key ("`animal`:
any of dog, cat, bird, horse, or other; several allowed; give each a
confidence between 0 and 1"), and the derived output schema. It then
goes through the same call path as every prompt: scheduler job, call log
with the prompt's hash, downscaled images.

### The editor

The LLM core module's front end to the contract, a form (an admin form,
not hand-drawn widgets):

1. the prompt, with the context variables offered as chips;
2. the keys: a row each with name, type, "finite" with its list of
   values or "open", "one / several", "with confidence";
3. the capability, and optionally a provider and model;
4. **Try on samples:** runs on the selection's first few images (or
   chosen ones) and shows each image with its answer, the dropped values,
   the time per image and the model; nothing is written. A "values seen"
   list from an open key can be turned into a finite list in one click;
5. **Save** under a name: a new version each time the prompt, the keys
   or the settings change. Saved extractors show in the diagram's node
   palette next to the modules' ones, grouped by origin.

Saved extractors are files beside the user prompts (one per version),
exportable and shareable as such. A module's preset (an "Animals" or
"Scene" extractor) is read-only: **Duplicate** gives an editable user
copy.

## 4. Ready-made extractors, and what Helios already has

Any module or plugin may ship extractors with fixed keys, declared in its
manifest (`[[extractors]]` beside `[[commands]]` and `[[badges]]`); an
out-of-process plugin computes them over its usual protocol, per image,
never per thumbnail.

What exists today and could be declared with the contract, without new
computation:

| Today | Where | Keys it would declare |
|---|---|---|
| Capture metadata (EXIF) | `helios_vault::capture`, `helios_library::capture` | `exif.camera`, `make`, `lens` (label, open); `iso`, `aperture`, `shutter`, `focal_length` (number); `captured` (time); `width`, `height`, `orientation`, `file_type` |
| Near-duplicate signals | `helios_library::signals` | `signals.phash` (text), `taken`, `frame` |
| Culling measures | `helios_library::cull` | `cull.focus`, `focus_at`, `clip_white`, `clip_black` (number), exposure mean/median/spread |
| Sharpness | `helios_library::signals` | `signals.sharpness` (number) |
| Library fields | `helios_vault::fields` | `library.rating`, `labels` (finite multi), `tags` (open multi), `title`, `description`, `flag`, `album`, `folder` |
| Keywords | library (L12) | `library.keywords` (open multi, `|` paths) |
| Technical assessment prompt | `image.assess@1` | `assess.focus`, `exposure`, `composition` (integer 1–5), `defects` (finite multi) |
| Caption prompt | `image.describe@1` | `describe.caption` (text), `keywords` (open multi) |
| Stack suggestions | `stacks.suggest` prompt | not an extractor (set-level, writes stacks): a gather plus an alter |
| Zone measures of workshops | `workshops` (`luma.median`, `zone.contrast`, `zones.ratio`) | `zones.*` (number), per declared zone |
| Histogram and scopes | `helios-scopes`, the GUI's scopes | not stored; an extractor would measure on the preview |

Not present and worth a module later: faces (count, boxes; opt-in, never
by default), place names from GPS (reverse geocoding needs a source; GPS
itself is not in the capture fields yet), colour palette, OCR, embeddings
(the `embeddings` capability exists, no store for vectors does).

Most of these are already read by smart-album rules (`rules.rs`:
`camera`, `lens`, `iso`, `focus`, `clip_white`…): declaring them as
extractors makes the same facts reachable by diagrams, and later lets
rules read any kept extractor key the same way.

## 5. Consumers

All consumers are generic over keys: they take a key name and work from
its declaration (type, finite values, multi, confidence).

- **Filter.** A condition over keys, in the rule language of smart
  albums (`contains`, `is`, `>`, `between`, `all`/`any` groups), plus
  `p >= x` for confident values. Outputs `kept` and `dropped`. **Preview:**
  run on the first N images and show kept and dropped side by side before
  the whole set runs.
- **Switch/case router.** One output per case, plus `other` (a value no
  case matched) and `none` (the key is empty or failed). Cases are typed
  by hand, filled from a finite key's values, or from the values seen in
  a preview run. On a multi-valued key a parameter chooses:
  - **all matches**: an image with a dog and a cat goes out on both;
  - **first match** in case order;
  - **best match**: the highest confidence.

  A confidence threshold applies before the rule. An open key routes by
  condition (contains, matches) per case.
- **Alters.** Write, never compute:
  - **update metadata**: maps keys to fields: to keywords under a branch
    (`Animals|{value}`, the branch proposed from a finite key's values),
    to title, description, labels or rating; mode `suggest` (default) or
    `accept`; `merge` (default) or `replace`; never replaces a value the
    user entered without the run saying so first;
  - **add to album**, **set rating or label**, **rename** (with the
    import's patterns), **stack** (from a gather);
  - later, **apply a workshop or template** chosen by the data.

  An alter's writes for a run are **one undo step** of the library
  history (a `command.batch` step), whatever the number of images.

## 6. Providers: the two layers against helios-ai

**Task layer.** A node declares a capability (`vision`, `chat`,
`embeddings`…), a prompt `id@version` with input and output schemas, and
limits. It never names a provider or a model.

**Provider layer.** helios-ai's providers (Ollama, OpenAI-compatible,
Anthropic, a local command; plugins later) declare capabilities; routing
picks the provider and model.

What helios-ai has today: providers with capabilities (narrowed per
entry), routing by capability and by prompt id (prompt first, then
capability, `chat` and `structured` standing in for each other), prompts
with `tuned_for`, input/output validation, `temperature` and `seed`,
image downscaling by local/remote, `limits.calls` (two calls at once by
default), a call log with the prompt's hash, and `check_route` that says
why a route cannot serve a capability.

What is missing:

1. **Overrides per diagram, node and run.** `run_prompt` always routes
   from the global map; `try_prompt` takes an explicit route. Add a
   resolution order: run, then node, then diagram, then prompt route,
   then capability route, each override a `Route` or a capability-level
   map, recorded with the run.
2. **The model in cache keys and results.** The call log records the
   model; `run_prompt` returns only the value. It must return the route
   that answered (provider, model, the model's own version or digest when
   the runtime gives it) so the cache and the item detail can keep it.
3. **A pre-run check.** Before a run starts, every model node's route is
   resolved and checked (`check_route`: provider reachable, model
   present, capability served); a node nothing serves fails the
   submission with the reason and a pointer to the AI settings, never
   halfway through.
4. **One retry with the errors**, and counts of dropped enum values, in
   the call path (section 2), not in each caller.
5. **Rate and cost limits per provider.** `limits.calls` is global;
   add per-provider concurrency, calls per minute, and for remote ones a
   per-run ceiling on calls (a run stops at the ceiling and asks).
6. **What leaves the machine, stated** before a run: for each model node,
   local or remote, the image side sent, how many images.

## 7. Cache, and where data lives

**Cache key:** the image's id (content hash), the extractor's
`id@version` and its content hash, the route that answered (provider,
model and model version), the input variables' hash (context, language),
and the image source (preview, or the head version's id when rendered).
A re-run, or the same extractor in another diagram, reads the cache and
calls no model; the router and the alter downstream of one extractor read
the same cached result. Changing the model is a miss, never a stale hit.

**Kept or run-only** (`keep` on the extractor, overridable per node):

- **run-only:** the data lives in the run's item documents and goes with
  the run (its retention);
- **kept:** written to a vault namespace per extractor
  (`extract/<id>`, one document per image: the values, the version, the
  route, when), the latest version winning per image. Kept keys become
  facts for badges ("AI suggestions pending"), rule fields for smart
  albums (`extract.animals.animal contains dog`, read from what is kept,
  never a model call), and searchable text in the Library filter.

Suggested metadata is not extractor data: it is what an update-metadata
alter wrote in `suggest` mode (section 9).

## 8. Running

- **One run, one promise.** Submitting a diagram creates a workflow run
  (its id from the canonical diagram, its settings and resolved routes)
  and a promise of kind `extract` owned by the run's job: the Activity
  focus shows it like an import, with progress (`done` of `total` items
  through the last node), the per-item outcome (kept, dropped, routed to
  which output, failed with the raw reply and errors), Cancel and Retry.
- **Resumable.** Each (item, node) row concludes once; after a stop or a
  crash the run resumes where it was (the journal's recovery), and cached
  results make a re-submission cheap. Retries with backoff for transient
  provider errors (timeout, 429, 5xx); a schema failure is not retried
  past the one corrective retry.
- **Cancellable.** Cancel stops claiming, cancels in-flight calls (their
  scheduler jobs), and leaves what concluded; alters already applied stay
  as one undo step.
- **Limits.** Per-provider concurrency and rate (section 6), a remote
  call ceiling per run, and the local runtime's own limit (one vision
  model at a time on most machines: concurrency 1 by default for local
  vision).
- **Automatic after import** is an option of an import profile: "run this
  diagram on what was imported", submitted when the import's promise
  settles. Off by default.

## 9. Suggested and accepted metadata, review

- **Suggested** values are kept apart from the photo's own (title,
  description, keywords), with their origin (run, extractor, model).
  They are not exported, written to XMP or matched by rules on the
  accepted fields until accepted.
- **Review in the metadata panel** (L13): suggested caption and keywords
  shown under the real ones, each with Accept, Reject and Edit; Accept
  all and Reject all for a selection, in bulk. Each review action is one
  undo step.
- **A badge** (`phosphor:sparkle`, bottom right, info) shows "suggestions
  pending", declared over the kept fact as
  [thumbnail-badges.md](thumbnail-badges.md) section 4 foresees; a Library
  filter "has suggestions" lists them.
- **Accepted** values are ordinary metadata: searchable, written back to
  sidecars by "Save metadata to files" (L14), in smart albums.
- **Search.** The Library filter's text search covers title,
  description and keywords, and kept extractor text keys; agents reach it
  through the same query.

Whether the LLM filter shows up directly in the Library's filter bar and
as a smart-album rule: **not at first**. Diagrams first; once kept
extractor keys are rule fields, a smart album filters on kept data with
no model call, which covers the need without a model in the filter bar.

## 10. Agents over MCP

Agents use the same objects, with no separate path:

- `extractors.list`, `.get`, `.set` (a new version), `.try` (on given
  images, applies nothing), mirroring `ai.prompts.*`;
- `diagrams.submit` with an input (a selection, an album, an import
  session, a query) and route overrides, answering a promise;
  `promises.wait` to follow it;
- `extract.get` for an image's kept data, and the review verbs
  (`library.suggestions.accept`, `.reject`) for suggested metadata.

Extractors are pinned `id@version` for reproducible work; each item's
detail ties a value to its extractor version, prompt hash and model, as
the call log does. "From this import, the photos with animals, then tag
them" is: input import session, the Animals extractor, a filter
`animal is not empty`, update metadata.

## 11. Reference example: animals

```
[Input: import session] -> [LLM extractor "animals"@1] -> [Switch on animals.animal]
                                                            |- dog   -> [Add to album "Dogs"] -> [Update metadata]
                                                            |- cat   -> [Update metadata]
                                                            |- bird  -> [Update metadata]
                                                            |- other -> [Update metadata]
                                                            '- none  -> (nothing)
```

- **Extractor** (user-made in the editor): prompt "Which animals are in
  this photo?", capability `vision`, key `animal`: label, finite
  `[dog, cat, bird, horse]`, multi, with confidence, `other` allowed.
  Schema: an object with `animal`, an array of `{value: enum, p: 0..1}`.
  An answer `[{value: "dog", p: 0.94}, {value: "cat", p: 0.71}]`.
- **Router:** cases filled from the finite values; rule **all matches**,
  threshold 0.5: the photo goes to `dog` and to `cat`. With **best**,
  only `dog`; with **first**, the first case in order. A goat gives
  `other`; an empty list `none`.
- **Update metadata:** `animal` to keywords under `Animals|{value}`
  (`Animals|Dog`, `Animals|Cat`, capitalised from the value), mode
  suggest, merge. One undo step for the run.
- **Chaining:** the `dog` branch adds to an album before writing
  keywords; the extractor ran once per image, its cached result feeds
  every node downstream.
- The same router routes on `exif.camera` or `assess.defects` by
  changing only its key.

## 12. Ideas borrowed

From a photo indexer with three LLM passes (a caption per image, a
folder "universe" from the captions and the folder's text files, then
per-image keywords combining both), ideas only:

- the three passes are a diagram here: an LLM extractor (caption), a
  gather per folder or import (context from captions, file names and
  sidecar text, capped in length; a parent folder's context passed down),
  then an LLM extractor with `{{caption}}` and `{{context}}`, then update
  metadata;
- a **stage per image** for resuming: here the journal row per (item,
  node) and the cache;
- **noise filtering** of keywords: a stop list (generic phrases such as
  "a picture", "stock photo"), as an option of the update-metadata alter;
- **language** forced or detected (from folder and file names), as the
  `{{language}}` variable with a library default;
- lenient JSON parsing of replies.

## 13. Measurement plan

Against a provider already running (nothing installed by the bench), on
the bench box:

- **Throughput:** images per minute for a vision LLM extractor (one key,
  finite; one key, open; three keys) and a text-only one, on a 7B-class
  local vision model, CPU and GPU, at concurrency 1 and 2; median and
  95th percentile latency per call.
- **Fit:** share of replies valid on the first try, after the retry,
  failed; dropped out-of-enum values; with and without structured output
  (`format` on and off).
- **Cache:** a re-run of the same diagram makes zero model calls; a run
  with a router and an alter after one extractor makes one call per
  image.
- **Overhead:** the run machinery per image with a fake provider (no
  model): journal, cache and writes, so the model time is the only
  cost that matters.

The fixture set is the bench's synthetic and corpus images with a small
labelled subset (animals, none, other) to count right and wrong
answers. Results go in a table in this note.

## 14. Slices

Each slice is one branch, building and passing its tests; code checked
locally, finalised on the box.

1. **Extractor contract and schema derivation** (a new `helios-extract`
   crate or a module of helios-ai). Keys, the derived schema, enum
   enforcement, `other`, confidence floor, the prompt rendering for an
   LLM extractor, validation of declarations. *Tests:* unit tests per
   key shape (finite single and multi, with confidence, open, number,
   bool) against `schema::validate`; out-of-enum values dropped or
   mapped and counted; a bad declaration refused with its reason.
2. **helios-ai: route overrides, answering route, retry once.**
   Resolution order run > node > diagram > prompt > capability;
   `run_prompt` returns the route and model version; one corrective
   retry with the errors; dropped-value counts. *Tests:* with the fake
   provider: each override wins in order; a bad then good reply passes
   on the retry, two bad replies fail with both errors; the answering
   model is returned.
3. **Pre-run check and provider limits.** Every model node's route
   checked before a run; per-provider concurrency and rate; remote
   call ceiling. *Tests:* an unrouted capability refuses the submission
   naming the node; a stopped provider is reported before the run; the
   rate limit spaces calls (fake clock).
4. **Cache and kept data.** The cache key, the `extract/<id>` vault
   namespace, run-only versus kept. *Tests:* a second run makes zero
   calls (fake provider counts); a new extractor version or model misses;
   kept data survives the run's removal, run-only does not.
5. **Diagram runs over library images.** Library image ids as items,
   input nodes (selection, album, smart album, import session, query,
   whole library), extractor and alter nodes as queueing graph nodes,
   the promise in the Activity focus. *Tests:* through the node's batch
   endpoint with deadlines: a run of synthetic images with a fake
   provider concludes each item once; kill and restart resumes without
   re-calling; cancel stops and keeps what concluded.
6. **Filter and switch/case router.** Multi-branch choices in the
   journal; all/first/best; threshold; `other` and `none`. *Tests:*
   journal contract scenarios for a multi-branch choice; the animals
   router on fixed extractor data sends each image where the table says.
7. **Update-metadata alter and suggested metadata.** Suggest/accept,
   merge/replace, keyword branch mapping, noise stop list, one undo step
   per run. *Tests:* a run writes suggestions only; undo removes them in
   one step; user values are never replaced in merge mode.
8. **Review UX.** Suggestions in the metadata panel with accept, reject,
   edit, in bulk; the "suggestions pending" badge and filter. *Tests:*
   GUI tests (kittest) through accesskit: accept all on a selection,
   one undo; the badge appears and goes; `library_scale` unchanged.
9. **The extractor editor.** The form, try on samples, save as a new
   version, the palette, duplicate a preset. *Tests:* GUI test: declare
   a key, try on two images with the fake provider, save, version 2 on
   change; the saved extractor appears in the palette.
10. **Built-in extractors declared.** EXIF, signals, culling measures,
    library fields and keywords, `image.assess` and `image.describe` as
    presets, with the module manifest's `[[extractors]]`. *Tests:* a
    router on `exif.camera` and a filter on `cull.focus` over fixtures;
    a plugin manifest's extractor is listed and refused when malformed.
11. **Gather node and context variables.** Per folder or import group;
    the three-pass caption, context, keywords diagram as a shipped
    template. *Tests:* the gather waits for the whole group; its value
    reaches each item's prompt; the template runs end to end on the fake
    provider.
12. **MCP verbs.** `extractors.*`, `diagrams.submit`, `extract.get`,
    suggestion review. *Tests:* protocol tests per verb; an agent-style
    scenario (import session, animals, filter, tag) through the batch
    endpoint.
13. **Kept keys as rule fields and search.** Smart-album rules and the
    Library's text search over kept extractor data. *Tests:* a smart
    album on `extract.animals.animal contains dog` with no model call;
    search finds a kept caption.
14. **Measurements on the box** (section 13), results written here.

Slices 1–4 need no GUI; 5–7 give a working diagram for agents; 8–9 give
it to the photographer.

## Open questions (not decided)

- Where suggested values are stored: beside each field in the field
  documents, or one suggestions document per image.
- Whether kept extractor data is written to XMP (a Helios namespace) or
  stays in the catalogue only.
- The default image an extractor sees: the embedded preview (fast, may
  differ from the edit) or the head version rendered small.
- Embeddings and similarity search: a vector store is out of this note.
